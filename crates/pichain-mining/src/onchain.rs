//! On-chain mining proof verification and reward distribution.
//!
//! This module handles the full lifecycle of a mining proof:
//! 1. Miner submits proof as a MiningProof transaction
//! 2. Validators verify the proof via spot-checking
//! 3. Verified proofs are registered in the digit registry
//! 4. Mining rewards are distributed from the community pool (supply-capped)
//! 5. Digit data is committed to on-chain storage
//!
//! The verification is designed to be cheap enough to run on-chain
//! while making fraud economically irrational.

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::difficulty::{self, DifficultyAdjuster};
use crate::proof::{MiningProof, ProofVerifier};
use crate::registry::{DigitRange, DigitRegistry};
use crate::reward::{self, RewardCalculator};

/// Maximum percentage of epoch emission any single miner can earn (basis points).
/// 500 bps = 5%, meaning at least 20 distinct miners needed to consume full epoch emission.
pub const MAX_MINER_REWARD_PCT_BPS: u32 = 500;

/// Blocks per mining epoch for reward cap tracking (~24 hours at 314ms/block).
pub const BLOCKS_PER_MINING_EPOCH: u64 = 275_159;

/// Result of on-chain mining proof verification.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerificationResult {
    /// Whether the proof was valid.
    pub valid: bool,
    /// Number of spot checks performed.
    pub spot_checks: u32,
    /// All spot checks passed.
    pub all_checks_passed: bool,
    /// Reward amount (0 if invalid or pool exhausted).
    pub reward_amount: u64,
    /// Digit range that was verified.
    pub start_position: u64,
    pub digit_count: u32,
    /// Error message if invalid.
    pub error: Option<String>,
    /// Miner's remaining reward budget in current epoch (after this proof).
    #[serde(default)]
    pub epoch_remaining_budget: Option<u64>,
}

/// On-chain mining proof processor.
/// Integrates proof verification, registry tracking, reward distribution,
/// and difficulty recording.
///
/// Mining uses a fixed minimum PoW (8 bits) for anti-spam. Rewards are purely
/// proportional to PI digits computed, making mining accessible to all hardware.
/// Supply tracking ensures total rewards never exceed the mining pool
/// (85% of total supply + accumulated fee income).
#[derive(Clone)]
pub struct MiningProcessor {
    verifier: ProofVerifier,
    registry: DigitRegistry,
    reward_calc: RewardCalculator,
    difficulty: DifficultyAdjuster,
    /// Maximum digits per proof submission (prevents DoS).
    max_digits_per_proof: u32,
    /// Minimum digits per proof (prevents dust submissions).
    min_digits_per_proof: u32,
    /// Maximum digit position (prevents DoS via astronomical BBP computation).
    max_digit_position: u64,
    /// Current block height (for registry tracking).
    current_height: u64,
    /// Block timestamp for deterministic state recording and year calculation.
    block_timestamp_ms: u64,
    /// Per-miner reward accumulator for the current mining epoch.
    /// Reconstructed on replay via register_historical().
    epoch_miner_rewards: BTreeMap<pichain_crypto::ed25519::Address, u64>,
    /// Current mining epoch number (current_height / BLOCKS_PER_MINING_EPOCH).
    current_mining_epoch: u64,
}

impl MiningProcessor {
    /// Create a new mining processor.
    pub fn new() -> Self {
        Self {
            verifier: ProofVerifier::new(),
            registry: DigitRegistry::new(),
            reward_calc: RewardCalculator::new(),
            difficulty: DifficultyAdjuster::new(),
            max_digits_per_proof: 1_000_000, // 1M hex digits max
            min_digits_per_proof: 10,         // 10 hex digits min (browser-friendly)
            max_digit_position: 10_000_000_000, // 10B max position (prevents BBP DoS)
            current_height: 0,
            block_timestamp_ms: 0,
            epoch_miner_rewards: BTreeMap::new(),
            current_mining_epoch: 0,
        }
    }

    /// Set the current block height.
    /// AUDIT-FIX L-1: Upgraded from debug_assert to hard assert. Height regression
    /// would corrupt epoch tracking (resetting epoch_miner_rewards to a stale epoch),
    /// potentially allowing miners to exceed per-epoch caps. This is a consensus
    /// invariant that must hold in release builds.
    pub fn set_height(&mut self, height: u64) {
        assert!(
            height >= self.current_height || self.current_height == 0,
            "MiningProcessor::set_height regression: {} -> {}",
            self.current_height, height
        );
        self.current_height = height;
    }

    /// Set the block timestamp for deterministic state recording and year calculation.
    pub fn set_block_timestamp(&mut self, ts: u64) {
        self.block_timestamp_ms = ts;
    }

    /// Set the genesis timestamp for emission year calculation.
    pub fn set_genesis_timestamp(&mut self, ts_ms: u64) {
        self.reward_calc.set_genesis_timestamp(ts_ms);
    }

    /// Set total mined amount (for replay/restore from persisted state).
    pub fn set_total_mined(&mut self, amount: u64) {
        self.reward_calc.set_total_mined(amount);
    }

    /// Get total rewards distributed so far.
    pub fn total_mined(&self) -> u64 {
        self.reward_calc.total_mined()
    }

    /// Check whether genesis timestamp has been configured.
    /// Returns `true` if `set_genesis_timestamp()` has been called with a non-zero value.
    pub fn is_genesis_configured(&self) -> bool {
        self.reward_calc.genesis_timestamp_ms() != 0
    }

    /// Compute the mining epoch number from a block height.
    fn mining_epoch_for_height(height: u64) -> u64 {
        height / BLOCKS_PER_MINING_EPOCH
    }

    /// Calculate the maximum reward any single miner can earn in the current mining epoch.
    fn epoch_emission_cap(&self) -> u64 {
        let year = self.reward_calc.year_from_timestamp(self.block_timestamp_ms);
        let annual = self.reward_calc.annual_emission(year) as u128;
        let epochs_per_year = (reward::BLOCKS_PER_YEAR / BLOCKS_PER_MINING_EPOCH).max(1) as u128;
        let epoch_emission = annual / epochs_per_year;
        let cap = epoch_emission * MAX_MINER_REWARD_PCT_BPS as u128 / 10_000;
        cap.min(u64::MAX as u128) as u64
    }

    /// Advance the mining epoch if the current block height has crossed a boundary.
    fn advance_mining_epoch_if_needed(&mut self) {
        let new_epoch = Self::mining_epoch_for_height(self.current_height);
        if new_epoch != self.current_mining_epoch {
            self.epoch_miner_rewards.clear();
            self.current_mining_epoch = new_epoch;
        }
    }

    /// Check whether a miner has remaining budget in this epoch.
    /// Returns the capped reward amount (may be 0 if cap exceeded).
    fn check_miner_cap(&self, miner: &pichain_crypto::ed25519::Address, proposed_reward: u64) -> u64 {
        let cap = self.epoch_emission_cap();
        if cap == 0 {
            return 0; // zero emission means no mining rewards available
        }
        let already_earned = self.epoch_miner_rewards.get(miner).copied().unwrap_or(0);
        let remaining = cap.saturating_sub(already_earned);
        proposed_reward.min(remaining)
    }

    /// Get (earned_this_epoch, cap_this_epoch) for a given miner.
    pub fn miner_epoch_budget(&self, miner: &pichain_crypto::ed25519::Address) -> (u64, u64) {
        let earned = self.epoch_miner_rewards.get(miner).copied().unwrap_or(0);
        let cap = self.epoch_emission_cap();
        (earned, cap)
    }

    /// Process a mining proof submission.
    /// This is called during block execution when a MiningProof transaction is encountered.
    pub fn process_proof(&mut self, proof: &MiningProof, anchor_block_hash: &[u8; 32]) -> VerificationResult {
        // 0. Ensure genesis timestamp has been set — without it, year_from_timestamp()
        // always returns year 1, giving perpetual year-1 rewards regardless of chain age.
        if self.reward_calc.genesis_timestamp_ms() == 0 {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some("genesis timestamp not set — cannot compute emission year".to_string()),
                epoch_remaining_budget: None,
            };
        }

        // 1. Check digit count bounds (zero check first as defense-in-depth)
        if proof.digit_count == 0 {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some("digit count must be > 0".to_string()),
                epoch_remaining_budget: None,
            };
        }

        if proof.digit_count < self.min_digits_per_proof {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(format!(
                    "too few digits: {} (minimum {})",
                    proof.digit_count, self.min_digits_per_proof
                )),
                epoch_remaining_budget: None,
            };
        }

        if proof.digit_count > self.max_digits_per_proof {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(format!(
                    "too many digits: {} (maximum {})",
                    proof.digit_count, self.max_digits_per_proof
                )),
                epoch_remaining_budget: None,
            };
        }

        // 1b. Reject proofs at positions beyond the max to prevent BBP DoS
        if proof.start_position.saturating_add(proof.digit_count as u64) > self.max_digit_position {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(format!(
                    "digit position exceeds maximum: {} + {} > {}",
                    proof.start_position, proof.digit_count, self.max_digit_position
                )),
                epoch_remaining_budget: None,
            };
        }

        // 2. Check if this range has already been computed (full range pre-check)
        // Reject early if the entire range is already covered by registered ranges,
        // avoiding expensive spot-check verification for duplicate submissions.
        if self.registry.is_range_fully_computed(proof.start_position, proof.digit_count) {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(format!(
                    "range {}..{} already fully computed",
                    proof.start_position,
                    proof.start_position.saturating_add(proof.digit_count as u64)
                )),
                epoch_remaining_budget: None,
            };
        }
        // Also check individual boundary positions for partial overlaps
        let end = match proof.start_position.checked_add(proof.digit_count as u64) {
            Some(e) => e,
            None => return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some("start_position + digit_count overflows u64".to_string()),
                epoch_remaining_budget: None,
            },
        };
        for pos in [proof.start_position, end.saturating_sub(1)] {
            if self.registry.is_computed(pos) {
                return VerificationResult {
                    valid: false,
                    spot_checks: 0,
                    all_checks_passed: false,
                    reward_amount: 0,
                    start_position: proof.start_position,
                    digit_count: proof.digit_count,
                    error: Some(format!("position {} already computed", pos)),
                    epoch_remaining_budget: None,
                };
            }
        }

        // 2b. Pre-check overlap via registry before expensive spot-check verification
        if self.registry.has_overlap(proof.start_position, end) {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some("digit range overlaps existing computed range".to_string()),
                epoch_remaining_budget: None,
            };
        }

        // 3. Verify the proof (spot-check digits)
        let spot_checks = self.verifier.effective_check_count(proof.digit_count) as u32;
        if let Err(e) = self.verifier.verify(proof, anchor_block_hash) {
            return VerificationResult {
                valid: false,
                spot_checks,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(format!("verification failed: {e}")),
                epoch_remaining_budget: None,
            };
        }

        // Proof is valid! Now check caps BEFORE registering in the registry,
        // so the digit range remains available for another miner if this one
        // has exceeded their per-epoch cap.

        // 4. Calculate reward (pure computation, no side effects)
        let raw_reward = self
            .reward_calc
            .reward_for_digits_at_time(proof.digit_count, self.block_timestamp_ms);

        // 5. Check per-miner epoch cap
        self.advance_mining_epoch_if_needed();
        let reward = self.check_miner_cap(&proof.miner, raw_reward);

        if reward == 0 && raw_reward > 0 {
            // Miner has hit their epoch cap — reject BEFORE registering range
            let already_earned = self.epoch_miner_rewards.get(&proof.miner).copied().unwrap_or(0);
            return VerificationResult {
                valid: false,
                spot_checks,
                all_checks_passed: true,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(format!(
                    "miner epoch cap reached: earned {} of {} cap in epoch {}",
                    already_earned, self.epoch_emission_cap(), self.current_mining_epoch,
                )),
                epoch_remaining_budget: Some(0),
            };
        }

        // 6. Register in the digit registry — after cap check passes.
        // If registration fails (duplicate range, overlap), we must NOT consume
        // mining supply.
        let range = DigitRange {
            start: proof.start_position,
            count: proof.digit_count,
            commitment: proof.commitment,
            miner: proof.miner,
            committed_at_height: self.current_height,
            committed_at_ms: self.block_timestamp_ms,
        };

        if let Err(e) = self.registry.register(range) {
            return VerificationResult {
                valid: false,
                spot_checks,
                all_checks_passed: true,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(format!("registry error: {e}")),
                epoch_remaining_budget: None,
            };
        }

        // 7. Record the reward distribution (updates total_mined)
        // AUDIT-FIX M-2: If record_reward fails, unregister the digit range to
        // prevent permanently consuming digits without issuing a reward.
        if reward > 0 {
            if let Err(e) = self.reward_calc.record_reward(reward) {
                // Rollback: unregister the range so another miner can claim it
                self.registry.unregister(proof.start_position);
                return VerificationResult {
                    valid: false,
                    spot_checks,
                    all_checks_passed: true,
                    reward_amount: 0,
                    start_position: proof.start_position,
                    digit_count: proof.digit_count,
                    error: Some(format!("reward tracking error: {e}")),
                    epoch_remaining_budget: None,
                };
            }
        }

        // 8. Track per-miner epoch reward (saturating to prevent theoretical wrap)
        let miner_entry = self.epoch_miner_rewards.entry(proof.miner).or_insert(0);
        *miner_entry = miner_entry.saturating_add(reward);

        // 9. Record proof timestamp for difficulty adjuster
        self.difficulty.record_proof(self.block_timestamp_ms);

        if reward == 0 {
            warn!("mining pool exhausted — no reward for valid proof");
        }

        let remaining_budget = self.epoch_emission_cap()
            .saturating_sub(self.epoch_miner_rewards.get(&proof.miner).copied().unwrap_or(0));

        VerificationResult {
            valid: true,
            spot_checks,
            all_checks_passed: true,
            reward_amount: reward,
            start_position: proof.start_position,
            digit_count: proof.digit_count,
            error: None,
            epoch_remaining_budget: Some(remaining_budget),
        }
    }

    /// Process a mining proof with PoW verification.
    /// This is the primary entry point called by the executor.
    ///
    /// Uses a fixed minimum PoW difficulty (8 bits / ~256 nonce attempts) so that
    /// ANY hardware — from a laptop to a 224-core server — can mine successfully.
    /// Mining rewards scale with the number of PI digits computed, not PoW difficulty.
    /// More powerful hardware computes more digits per unit time → more rewards.
    pub fn process_proof_with_pow(
        &mut self,
        proof: &MiningProof,
        pow_nonce: u64,
        anchor_block_hash: &[u8; 32],
    ) -> VerificationResult {
        // Always verify PoW — no bypass.
        // Fixed minimum difficulty: 8 bits (INITIAL_DIFFICULTY).
        // This prevents spam without blocking any hardware.
        // The miner address is included in the hash to bind the PoW
        // to the submitting miner, preventing proof-of-work theft.
        if !difficulty::check_proof_difficulty(
            &proof.digits,
            pow_nonce,
            anchor_block_hash,
            &difficulty::INITIAL_DIFFICULTY,
            &proof.miner.0,
        ) {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some("PoW difficulty not met (minimum: 8 bits)".to_string()),
                epoch_remaining_budget: None,
            };
        }

        // Run the standard proof verification (spot-checks, registry, rewards)
        self.process_proof(proof, anchor_block_hash)
    }

    /// Get the digit registry.
    pub fn registry(&self) -> &DigitRegistry {
        &self.registry
    }

    /// Get the difficulty adjuster.
    pub fn difficulty(&self) -> &DifficultyAdjuster {
        &self.difficulty
    }

    /// Get the next position where a miner should start computing.
    pub fn next_mining_position(&self) -> u64 {
        self.registry.next_uncomputed_position()
    }

    /// Get the current frontier (highest consecutive verified position).
    pub fn frontier(&self) -> u64 {
        self.registry.frontier()
    }

    /// Get the difficulty target (fixed minimum: 8 bits).
    pub fn difficulty_target(&self) -> &[u8; 32] {
        &difficulty::INITIAL_DIFFICULTY
    }

    /// Get difficulty as approximate leading zero bits (fixed: 8).
    pub fn difficulty_bits(&self) -> u32 {
        difficulty::MIN_DIFFICULTY_BITS
    }

    /// Add transaction fee income to the mining pool.
    pub fn add_fee_income(&mut self, amount: u64) {
        self.reward_calc.add_fee_income(amount);
    }

    /// Set fee income (for replay/restore from persisted state).
    pub fn set_fee_income(&mut self, amount: u64) {
        self.reward_calc.set_fee_income(amount);
    }

    /// Get cumulative fee income added to the mining pool.
    pub fn fee_income(&self) -> u64 {
        self.reward_calc.fee_income()
    }

    /// Get mining statistics.
    pub fn stats(&self) -> MiningStats {
        let reg_stats = self.registry.stats();
        let year = self.reward_calc.year_from_timestamp(self.block_timestamp_ms);
        MiningStats {
            total_digits_verified: reg_stats.total_digits_verified,
            frontier_position: reg_stats.frontier_position,
            total_ranges: reg_stats.total_ranges,
            unique_miners: reg_stats.unique_miners,
            remaining_pool: self.reward_calc.remaining_pool(),
            total_mined: self.reward_calc.total_mined(),
            fee_income: self.reward_calc.fee_income(),
            next_position: self.next_mining_position(),
            reward_per_digit: self.reward_calc.reward_per_digit(year),
            emission_year: year,
            // Fixed minimum difficulty — accessible to all hardware
            difficulty_bits: difficulty::MIN_DIFFICULTY_BITS,
            difficulty_target_hex: hex::encode(difficulty::INITIAL_DIFFICULTY),
            mining_epoch: self.current_mining_epoch,
            epoch_miner_cap: self.epoch_emission_cap(),
        }
    }

    /// Register a historical mining range from block replay on startup.
    /// Bypasses spot-check verification since the proof was already verified
    /// when the block was originally finalized.
    /// Also records the historical reward to maintain accurate supply tracking.
    ///
    /// The emission year is derived from `block_timestamp_ms` (same method as
    /// live `process_proof()`). Both live and replay paths now use
    /// `reward_for_digits_at_time()` so that year boundaries are evaluated
    /// identically. The block timestamp is stored in every block header and
    /// is therefore always available during replay.
    pub fn register_historical(
        &mut self,
        start_position: u64,
        digit_count: u32,
        digits: &[u8],
        miner: pichain_crypto::ed25519::Address,
        block_height: u64,
        block_timestamp_ms: u64,
    ) -> Result<(), String> {
        if self.reward_calc.genesis_timestamp_ms() == 0 {
            return Err("genesis timestamp not set -- cannot replay reward".to_string());
        }
        let commitment = pichain_crypto::hash(digits);
        let range = DigitRange {
            start: start_position,
            count: digit_count,
            commitment,
            miner,
            committed_at_height: block_height,
            committed_at_ms: block_timestamp_ms,
        };

        self.registry
            .register(range)
            .map_err(|e| e.to_string())?;

        // Replay the reward using timestamp-based year calculation — the same
        // method used by live process_proof(). This eliminates the year
        // divergence that occurred when live processing used timestamp-based
        // years but replay used height-based years: at year boundaries the
        // two could disagree, causing different total_mined values between a
        // node that processed blocks live and one that replayed them.
        let reward = self.reward_calc.reward_for_digits_at_time(digit_count, block_timestamp_ms);

        // Track per-miner epoch reward during replay for accurate state after restart.
        // Apply the same per-miner epoch cap during replay that process_proof uses
        // during live processing. Record the CAPPED reward to total_mined to match
        // what live processing records.
        let replay_epoch = Self::mining_epoch_for_height(block_height);
        if replay_epoch != self.current_mining_epoch {
            self.epoch_miner_rewards.clear();
            self.current_mining_epoch = replay_epoch;
        }
        let saved_ts = self.block_timestamp_ms;
        self.block_timestamp_ms = block_timestamp_ms;
        let capped_reward = self.check_miner_cap(&miner, reward);
        self.block_timestamp_ms = saved_ts;

        if capped_reward > 0 {
            self.reward_calc.record_reward(capped_reward)
                .map_err(|e| format!("replay reward tracking failed: {e}"))?;
        }

        let miner_entry = self.epoch_miner_rewards.entry(miner).or_insert(0);
        *miner_entry = miner_entry.saturating_add(capped_reward);

        Ok(())
    }
}

impl Default for MiningProcessor {
    fn default() -> Self {
        Self::new()
    }
}

/// Mining statistics for the RPC API.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MiningStats {
    pub total_digits_verified: u64,
    pub frontier_position: u64,
    pub total_ranges: u64,
    pub unique_miners: u64,
    pub remaining_pool: u64,
    pub total_mined: u64,
    /// Cumulative transaction fee income added to mining pool.
    pub fee_income: u64,
    pub next_position: u64,
    pub reward_per_digit: u64,
    pub emission_year: u32,
    /// Current PoW difficulty in leading zero bits.
    pub difficulty_bits: u32,
    /// Current difficulty target as hex string.
    pub difficulty_target_hex: String,
    /// Current mining epoch number.
    pub mining_epoch: u64,
    /// Per-miner reward cap for current epoch (in base units).
    pub epoch_miner_cap: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bbp::BbpComputer;
    use pichain_crypto::ed25519::Address;

    /// Test genesis timestamp — year 1 starts here.
    const TEST_GENESIS_TS: u64 = 1_000_000_000_000;

    /// Create a MiningProcessor with genesis timestamp configured so that
    /// process_proof() doesn't reject with "genesis timestamp not set".
    fn configured_processor() -> MiningProcessor {
        let mut p = MiningProcessor::new();
        p.set_genesis_timestamp(TEST_GENESIS_TS);
        // Set block timestamp to genesis so we are in year 1.
        p.set_block_timestamp(TEST_GENESIS_TS);
        p
    }

    #[test]
    fn process_valid_proof() {
        let mut processor = configured_processor();
        processor.set_height(1);

        // Compute real PI digits
        let digits = BbpComputer::compute_hex_digits(0, 200);
        let proof = MiningProof::new(0, digits, Address([1; 20]), 42);

        let result = processor.process_proof(&proof, &[0u8; 32]);
        assert!(result.valid, "proof should be valid: {:?}", result.error);
        assert!(result.reward_amount > 0);
        assert_eq!(result.digit_count, 200);

        // Registry should be updated
        assert_eq!(processor.frontier(), 200);

        // Total mined should reflect the reward
        assert_eq!(processor.total_mined(), result.reward_amount);
    }

    #[test]
    fn reject_duplicate_range() {
        let mut processor = configured_processor();

        // Submit first proof
        let digits = BbpComputer::compute_hex_digits(0, 200);
        let proof = MiningProof::new(0, digits, Address([1; 20]), 42);
        let r1 = processor.process_proof(&proof, &[0u8; 32]);
        assert!(r1.valid);

        // Submit same range again — rejected by the full-range pre-check
        let digits2 = BbpComputer::compute_hex_digits(0, 200);
        let proof2 = MiningProof::new(0, digits2, Address([2; 20]), 43);
        let r2 = processor.process_proof(&proof2, &[0u8; 32]);
        assert!(!r2.valid);
        let err = r2.error.unwrap();
        assert!(
            err.contains("already"),
            "error should indicate range already computed: {err}"
        );
    }

    #[test]
    fn reject_too_few_digits() {
        let mut processor = configured_processor();

        let digits = BbpComputer::compute_hex_digits(0, 5);
        let proof = MiningProof::new(0, digits, Address([1; 20]), 0);

        let result = processor.process_proof(&proof, &[0u8; 32]);
        assert!(!result.valid);
        assert!(result.error.unwrap().contains("too few"));
    }

    #[test]
    fn sequential_ranges_advance_frontier() {
        let mut processor = configured_processor();

        // First range: 0..200
        let d1 = BbpComputer::compute_hex_digits(0, 200);
        let p1 = MiningProof::new(0, d1, Address([1; 20]), 0);
        processor.process_proof(&p1, &[0u8; 32]);
        assert_eq!(processor.frontier(), 200);

        // Second range: 200..400
        let d2 = BbpComputer::compute_hex_digits(200, 200);
        let p2 = MiningProof::new(200, d2, Address([2; 20]), 0);
        processor.process_proof(&p2, &[0u8; 32]);
        assert_eq!(processor.frontier(), 400);

        assert_eq!(processor.next_mining_position(), 400);
    }

    #[test]
    fn reject_invalid_digits() {
        let mut processor = configured_processor();

        // Create proof with wrong digits — corrupt 50%+ to guarantee spot-check detection
        let mut digits = BbpComputer::compute_hex_digits(0, 200);
        for i in 0..120 {
            digits[i] = (digits[i] + 1) % 16;
        }
        // Recreate the proof with the corrupted digits (fresh commitment)
        let proof = MiningProof::new(0, digits, Address([1; 20]), 42);

        let result = processor.process_proof(&proof, &[0u8; 32]);
        // Should fail because spot checks will catch the corrupted digits
        assert!(!result.valid);
    }

    #[test]
    fn stats() {
        let mut processor = configured_processor();

        let digits = BbpComputer::compute_hex_digits(0, 200);
        let proof = MiningProof::new(0, digits, Address([1; 20]), 0);
        processor.process_proof(&proof, &[0u8; 32]);

        let stats = processor.stats();
        assert_eq!(stats.total_digits_verified, 200);
        assert_eq!(stats.frontier_position, 200);
        assert_eq!(stats.unique_miners, 1);
        assert_eq!(stats.next_position, 200);
        assert!(stats.total_mined > 0);
        assert_eq!(stats.emission_year, 1);
    }

    #[test]
    fn supply_cap_prevents_infinite_mining() {
        let mut processor = configured_processor();

        // Simulate near-exhaustion of mining pool (85% of TOTAL_SUPPLY)
        let cap = pichain_types::TOTAL_SUPPLY * 85 / 100;
        processor.set_total_mined((cap - 100) as u64);

        let digits = BbpComputer::compute_hex_digits(0, 200);
        let proof = MiningProof::new(0, digits, Address([1; 20]), 42);
        let result = processor.process_proof(&proof, &[0u8; 32]);

        // Should succeed but reward is capped at remaining pool (100 base units)
        assert!(result.valid);
        assert_eq!(result.reward_amount, 100);
    }

    #[test]
    fn pow_always_required() {
        let mut processor = configured_processor();

        let digits = BbpComputer::compute_hex_digits(0, 200);
        let proof = MiningProof::new(0, digits, Address([1; 20]), 42);

        // Submitting with zero anchor and zero nonce should still require PoW
        let result = processor.process_proof_with_pow(&proof, 0, &[0u8; 32]);
        assert!(!result.valid, "PoW should be enforced even with zero anchor");
        assert!(result.error.unwrap().contains("PoW difficulty not met"));
    }

    #[test]
    fn pow_with_valid_nonce() {
        let mut processor = configured_processor();

        let miner_addr = Address([1; 20]);
        let digits = BbpComputer::compute_hex_digits(0, 200);
        let anchor = [1u8; 32];
        // Find a valid nonce bound to the miner address
        let nonce = difficulty::find_nonce(&digits, &anchor, &difficulty::INITIAL_DIFFICULTY, 10_000, &miner_addr.0)
            .expect("should find nonce with easy difficulty");

        let proof = MiningProof::new(0, digits, miner_addr, 42);
        let result = processor.process_proof_with_pow(&proof, nonce, &anchor);
        assert!(result.valid, "proof with valid PoW should pass: {:?}", result.error);
        assert!(result.reward_amount > 0);
    }

    #[test]
    fn difficulty_records_proofs() {
        let mut processor = configured_processor();
        processor.set_block_timestamp(TEST_GENESIS_TS + 1000);

        let digits = BbpComputer::compute_hex_digits(0, 200);
        let proof = MiningProof::new(0, digits, Address([1; 20]), 42);
        processor.process_proof(&proof, &[0u8; 32]);

        // Difficulty adjuster should have recorded the proof timestamp
        assert_eq!(processor.difficulty().recent_proof_times.len(), 1);
        assert_eq!(processor.difficulty().recent_proof_times[0], TEST_GENESIS_TS + 1000);
    }

    #[test]
    fn historical_replay_tracks_supply() {
        let mut processor = MiningProcessor::new();
        processor.set_genesis_timestamp(TEST_GENESIS_TS);

        let digits = BbpComputer::compute_hex_digits(0, 200);
        processor
            .register_historical(0, 200, &digits, Address([1; 20]), 1, TEST_GENESIS_TS + 1_000)
            .unwrap();

        assert!(processor.total_mined() > 0, "historical replay should track supply");
        assert_eq!(processor.frontier(), 200);
    }

    #[test]
    fn historical_replay_uses_timestamp_based_year() {
        // Milliseconds per year (matching reward.rs MS_PER_YEAR: 365.25 days).
        let ms_per_year: u64 = 365 * 24 * 3600 * 1000 + 6 * 3600 * 1000;

        let mut processor = MiningProcessor::new();
        processor.set_genesis_timestamp(TEST_GENESIS_TS);

        // Replay a block from year 1 (timestamp within first year of genesis)
        let digits1 = BbpComputer::compute_hex_digits(0, 200);
        let year1_ts = TEST_GENESIS_TS + 1_000; // 1 second after genesis
        processor
            .register_historical(0, 200, &digits1, Address([1; 20]), 100, year1_ts)
            .unwrap();
        let mined_y1 = processor.total_mined();

        // Replay a block from year 2 (timestamp > genesis + 1 year)
        // The year-2 reward per digit should be lower (20% vs 25%)
        let year2_ts = TEST_GENESIS_TS + ms_per_year + 1_000;
        let digits2 = BbpComputer::compute_hex_digits(200, 200);
        processor
            .register_historical(200, 200, &digits2, Address([2; 20]), 200, year2_ts)
            .unwrap();
        let mined_y2_increment = processor.total_mined() - mined_y1;

        // Year 1 reward > Year 2 reward for same digit count
        assert!(mined_y1 > mined_y2_increment,
            "year 1 reward ({}) should exceed year 2 reward ({})",
            mined_y1, mined_y2_increment);
    }

    #[test]
    fn duplicate_range_rejected_before_spot_check() {
        let mut processor = configured_processor();

        // Submit first proof covering 0..200
        let digits1 = BbpComputer::compute_hex_digits(0, 200);
        let proof1 = MiningProof::new(0, digits1, Address([1; 20]), 42);
        let r1 = processor.process_proof(&proof1, &[0u8; 32]);
        assert!(r1.valid, "first proof should be valid");

        // Submit exact same range again — should be rejected at pre-check
        // (0 spot checks) because the full range is already computed.
        let digits2 = BbpComputer::compute_hex_digits(0, 200);
        let proof2 = MiningProof::new(0, digits2, Address([2; 20]), 43);
        let r2 = processor.process_proof(&proof2, &[0u8; 32]);
        assert!(!r2.valid, "duplicate range should be rejected");
        assert_eq!(r2.spot_checks, 0, "rejection should happen before spot-checking");
        assert!(
            r2.error.as_ref().unwrap().contains("already"),
            "error should mention range already computed: {:?}",
            r2.error
        );
    }

    #[test]
    fn process_proof_rejects_without_genesis_timestamp() {
        // Verify that process_proof() rejects proofs when genesis timestamp
        // has not been configured (defaults to 0).
        let mut processor = MiningProcessor::new();
        // Deliberately do NOT call set_genesis_timestamp()

        let digits = BbpComputer::compute_hex_digits(0, 200);
        let proof = MiningProof::new(0, digits, Address([1; 20]), 42);
        let result = processor.process_proof(&proof, &[0u8; 32]);

        assert!(!result.valid, "should reject when genesis timestamp not set");
        assert!(
            result.error.as_ref().unwrap().contains("genesis timestamp not set"),
            "error should mention genesis timestamp: {:?}",
            result.error
        );
    }

    #[test]
    fn register_historical_rejects_without_genesis_timestamp() {
        let mut processor = MiningProcessor::new();
        // Deliberately do NOT call set_genesis_timestamp()

        let digits = BbpComputer::compute_hex_digits(0, 200);
        let result = processor.register_historical(
            0, 200, &digits, Address([1; 20]), 1, 1_000_000_001_000,
        );
        assert!(result.is_err(), "should reject when genesis timestamp not set");
        assert!(
            result.unwrap_err().contains("genesis timestamp not set"),
            "error should mention genesis timestamp"
        );
    }

    #[test]
    fn is_genesis_configured_reflects_state() {
        let mut processor = MiningProcessor::new();
        assert!(!processor.is_genesis_configured(), "should be false before set");

        processor.set_genesis_timestamp(TEST_GENESIS_TS);
        assert!(processor.is_genesis_configured(), "should be true after set");
    }

    // ==================== Per-Address Mining Cap Tests ====================

    #[test]
    fn miner_cap_epoch_emission_cap_is_nonzero() {
        let processor = configured_processor();
        let cap = processor.epoch_emission_cap();
        assert!(cap > 0, "epoch emission cap should be > 0, got {cap}");
    }

    #[test]
    fn miner_cap_enforced() {
        let mut processor = configured_processor();
        processor.set_height(1);
        let miner = Address([1; 20]);
        let cap = processor.epoch_emission_cap();
        assert!(cap > 0, "cap must be non-zero");

        // Submit one valid proof first to verify the flow works
        let digits1 = BbpComputer::compute_hex_digits(0, 200);
        let proof1 = MiningProof::new(0, digits1, miner, 1);
        let r1 = processor.process_proof(&proof1, &[0u8; 32]);
        assert!(r1.valid, "first proof should succeed: {:?}", r1.error);
        assert!(r1.reward_amount > 0);

        // Simulate that the miner has already earned cap - 1 in this epoch
        // by directly setting the epoch_miner_rewards
        processor.epoch_miner_rewards.insert(miner, cap - 1);

        // Next proof's reward should be capped to 1 base unit at most
        let digits2 = BbpComputer::compute_hex_digits(200, 200);
        let proof2 = MiningProof::new(200, digits2, miner, 2);
        let r2 = processor.process_proof(&proof2, &[0u8; 32]);
        // Reward per proof of 200 digits >> 1, so cap should reduce it
        assert!(r2.valid, "should still succeed with partial reward: {:?}", r2.error);
        assert!(r2.reward_amount <= 1, "reward should be capped to remaining budget (1)");

        // Now the miner is at or over the cap. Next proof should be rejected.
        let digits3 = BbpComputer::compute_hex_digits(400, 200);
        let proof3 = MiningProof::new(400, digits3, miner, 3);
        let r3 = processor.process_proof(&proof3, &[0u8; 32]);
        assert!(!r3.valid, "should be rejected after hitting cap");
        assert!(r3.error.as_ref().unwrap().contains("epoch cap"),
            "expected epoch cap error, got: {:?}", r3.error);
        assert_eq!(r3.epoch_remaining_budget, Some(0));
    }

    #[test]
    fn miner_cap_different_miners_independent() {
        let mut processor = configured_processor();
        processor.set_height(1);
        let miner_a = Address([1; 20]);
        let miner_b = Address([2; 20]);

        // Miner A submits
        let digits_a = BbpComputer::compute_hex_digits(0, 500);
        let proof_a = MiningProof::new(0, digits_a, miner_a, 1);
        let result_a = processor.process_proof(&proof_a, &[0u8; 32]);
        assert!(result_a.valid);

        // Miner B submits different range — should succeed independently
        let digits_b = BbpComputer::compute_hex_digits(500, 500);
        let proof_b = MiningProof::new(500, digits_b, miner_b, 2);
        let result_b = processor.process_proof(&proof_b, &[0u8; 32]);
        assert!(result_b.valid, "miner B should succeed: {:?}", result_b.error);

        // Check budgets are independent
        let (earned_a, _) = processor.miner_epoch_budget(&miner_a);
        let (earned_b, _) = processor.miner_epoch_budget(&miner_b);
        assert!(earned_a > 0);
        assert!(earned_b > 0);
    }

    #[test]
    fn miner_cap_epoch_boundary_resets() {
        let mut processor = configured_processor();
        processor.set_height(1);
        let miner = Address([1; 20]);

        // Submit a proof in epoch 0
        let digits = BbpComputer::compute_hex_digits(0, 500);
        let proof = MiningProof::new(0, digits, miner, 1);
        let result = processor.process_proof(&proof, &[0u8; 32]);
        assert!(result.valid);
        let (earned_e0, _) = processor.miner_epoch_budget(&miner);
        assert!(earned_e0 > 0);

        // Advance to epoch 1
        processor.set_height(BLOCKS_PER_MINING_EPOCH + 1);
        processor.advance_mining_epoch_if_needed();
        let (earned_e1, _) = processor.miner_epoch_budget(&miner);
        assert_eq!(earned_e1, 0, "epoch reset should clear miner earnings");
    }

    #[test]
    fn miner_cap_stats_include_epoch_info() {
        let processor = configured_processor();
        let stats = processor.stats();
        assert_eq!(stats.mining_epoch, 0);
        assert!(stats.epoch_miner_cap > 0);
    }
}
