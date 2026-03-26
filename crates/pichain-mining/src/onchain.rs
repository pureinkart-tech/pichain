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

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tracing::warn;

use crate::difficulty::{self, DifficultyAdjuster};
use crate::proof::{MiningProof, ProofVerifier};
use crate::registry::{DigitRange, DigitRegistry};
use crate::reward::{self, RewardCalculator};

/// Maximum percentage of epoch emission any single miner can earn at maturity.
/// 314 bps = 3.14% (π × 100). At least ~32 miners needed to consume full emission.
///
/// During bootstrap, this is relaxed via `effective_miner_cap_bps()` to allow
/// small networks to function (a solo miner needs more to earn anything meaningful).
pub const MAX_MINER_REWARD_PCT_BPS: u32 = 314;

/// Bootstrap miner cap (used when fewer than π² unique miners exist).
/// 3141 bps = 31.41% (π × 1000), allowing solo/small-team mining during launch.
pub const BOOTSTRAP_MINER_REWARD_PCT_BPS: u32 = 3141;

/// Minimum unique miners before the cap tightens from bootstrap to mature level.
/// π² ≈ 9.87, rounded to 9.
pub const MIN_MINERS_FOR_TIGHT_CAP: usize = 9;

/// Blocks per mining epoch for reward cap tracking (~24 hours at 314ms/block).
pub const BLOCKS_PER_MINING_EPOCH: u64 = 275_159;

/// Percentage of unmined epoch emission recycled to staking rewards (basis points).
pub const UNMINED_TO_STAKING_BPS: u32 = 8000; // 80% → staking

/// Size of each miner's assigned slot range (in digits).
/// Each miner gets a non-overlapping range of this size ahead of the frontier.
/// 10K keeps miners close to the frontier so they advance it together,
/// while still preventing overlap between concurrent miners.
pub const SLOT_RANGE_SIZE: u64 = 10_000;

/// Maximum distance ahead of the frontier that a proof can be submitted.
///
/// Prevents miners from skipping far ahead to easy positions, forcing them to
/// work near the frontier where PI is actually being solved.
///
/// Early chain (frontier < 100K): 10x lookahead for parallelism
/// Mature chain: 2x lookahead
pub fn max_frontier_distance(frontier: u64) -> u64 {
    if frontier < 100_000 {
        // Early: generous lookahead (min 100K, or 10x frontier)
        frontier.max(10_000).saturating_mul(10)
    } else {
        // Mature: 2x lookahead
        frontier.saturating_mul(2)
    }
}

/// Minimum digits per proof, scaling with frontier position.
///
/// As the frontier advances deeper into PI, each proof must contain more digits,
/// ensuring that each proof represents serious computation at the current depth.
/// Uses integer log2 of (frontier / 1000) as a scaling factor.
pub fn min_batch_size(frontier: u64) -> u32 {
    let factor = if frontier <= 1000 {
        1u64
    } else {
        let scaled = frontier / 1000;
        // Integer log2 via leading_zeros
        let log2 = (u64::BITS - scaled.leading_zeros()).saturating_sub(1);
        (log2 as u64).max(1)
    };
    // Cap at 500 (not 10,000) to keep browser/mobile mining viable at all frontier depths.
    // A browser miner computing 500 digits takes ~2-5 seconds — accessible to everyone.
    (10u64.saturating_mul(factor)).clamp(10, 500) as u32
}

/// Integer square root via Newton's method. Deterministic across all platforms.
fn isqrt(x: u32) -> u32 {
    if x <= 1 {
        return x;
    }
    let shift = (32 - x.leading_zeros()).div_ceil(2);
    let mut r = 1u32 << shift;
    loop {
        let r1 = (r + x / r) / 2;
        if r1 >= r {
            break;
        }
        r = r1;
    }
    r
}

/// Reward scaling with **progressive strengthening**.
///
/// Converts raw digit count to "effective digits" using a scaling curve
/// that starts LINEAR (user-friendly for bootstrapping) and transitions
/// to SQRT (quantum-resistant) as the chain matures.
///
/// # Progressive schedule
///
/// The scaling curve is controlled by `frontier`:
///
/// | Phase | Frontier | Scaling | Effect |
/// |-------|----------|---------|--------|
/// | Genesis | 0-10K | Linear | 1000 digits → 1000 effective |
/// | Bootstrap | 10K-100K | 3/4 power | 1000 digits → ~562 effective |
/// | Early | 100K-1M | 2/3 power | 1000 digits → ~464 effective |
/// | Mature | 1M+ | sqrt | 1000 digits → 316 effective |
///
/// This ensures:
/// - At launch, casual laptop miners earn proportional to their work (no penalty)
/// - As the network grows and quantum threats approach, diminishing returns kick in
/// - At maturity, hardware advantages are fully dampened
///
/// Uses integer-only arithmetic for consensus determinism.
pub fn sqrt_effective_digits(digit_count: u32) -> u32 {
    // Default to sqrt scaling (mature chain)
    sqrt_effective_digits_at_frontier(digit_count, u64::MAX)
}

/// Compute effective digits with frontier-aware progressive scaling.
pub fn sqrt_effective_digits_at_frontier(digit_count: u32, frontier: u64) -> u32 {
    if digit_count == 0 {
        return 0;
    }

    if frontier < 10_000 {
        // Genesis/Bootstrap: LINEAR scaling — no dampening.
        // New chain needs to be accessible and rewarding for early miners.
        digit_count
    } else if frontier < 100_000 {
        // Early chain: gentle dampening — cube-root-ish (approximate 3/4 power).
        // sqrt(sqrt(x)) * sqrt(sqrt(x)) * sqrt(sqrt(x)) ≈ x^(3/4)
        // Simpler approximation: (x + sqrt(x)*10) / 2 — halfway between linear and sqrt
        let sqrt_scaled = isqrt(digit_count).saturating_mul(10);
        (digit_count.saturating_add(sqrt_scaled)) / 2
    } else if frontier < 1_000_000 {
        // Growing chain: moderate dampening — between linear and sqrt.
        // (x + 2*sqrt(x)*10) / 3 — weighted toward sqrt
        let sqrt_scaled = isqrt(digit_count).saturating_mul(10);
        (digit_count.saturating_add(sqrt_scaled.saturating_mul(2))) / 3
    } else {
        // Mature chain: full sqrt dampening — quantum-resistant.
        // sqrt(digit_count) * 10, normalized so 100 digits → 100 effective
        isqrt(digit_count).saturating_mul(10)
    }
}

/// Frontier bonus: reward multiplier based on proof proximity to the frontier.
///
/// Returns (numerator, denominator) for integer multiplication.
/// - At or behind frontier (filling gaps): 3x bonus
/// - At max_frontier_distance ahead: 1x (no bonus)
/// - Linear interpolation in between
///
/// This strongly incentivizes miners to work at the hard frontier edge
/// where PI is actually being advanced, rather than parallel positions ahead.
pub fn frontier_bonus(proof_start: u64, frontier: u64) -> (u64, u64) {
    if proof_start <= frontier {
        // At or filling gaps behind the frontier — maximum bonus
        (3, 1)
    } else {
        let distance = proof_start - frontier;
        let max_dist = max_frontier_distance(frontier);
        if max_dist == 0 {
            return (3, 1);
        }
        // Linear: 3x at frontier → 1x at max_distance
        // multiplier = 3 - 2 * (distance / max_distance)
        // Integer form: (3 * max_dist - 2 * distance) / max_dist
        // Floor at 1x (i.e. num >= max_dist)
        let num = (3u128 * max_dist as u128).saturating_sub(2u128 * distance.min(max_dist) as u128);
        let den = max_dist as u128;
        // Ensure floor of 1x and fit in u64
        let num = num.max(den).min(u64::MAX as u128) as u64;
        let den = den.min(u64::MAX as u128) as u64;
        (num, den)
    }
}

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
    /// Maximum digit position (prevents DoS via astronomical BBP computation).
    max_digit_position: u64,
    /// Current block height (for registry tracking).
    current_height: u64,
    /// Block timestamp for deterministic state recording and year calculation.
    block_timestamp_ms: u64,
    /// Per-miner reward accumulator for the current mining epoch.
    /// Reconstructed on replay via register_historical().
    epoch_miner_rewards: BTreeMap<pichain_crypto::keys::Address, u64>,
    /// Current mining epoch number (current_height / BLOCKS_PER_MINING_EPOCH).
    current_mining_epoch: u64,
    /// Total mining rewards actually minted in the current epoch.
    epoch_actual_minted: u64,
    /// Accumulated staking reward pool from unmined emission recycling.
    /// Drips to the block proposer (validators/stakers) over the next epoch.
    staking_reward_pool: u64,
    /// Per-block staking drip calculated at epoch boundary.
    staking_drip_per_block: u64,
    /// Active miner registry: address → (slot_index, last_proof_height).
    /// Slot indices are assigned sequentially; stale miners (no proof in 1 epoch)
    /// get their slots released for reuse.
    active_miners: BTreeMap<pichain_crypto::keys::Address, (u32, u64)>,
    /// Next slot index to assign (monotonically increasing, wraps via reuse).
    next_slot_index: u32,
}

impl MiningProcessor {
    /// Create a new mining processor.
    pub fn new() -> Self {
        Self {
            verifier: ProofVerifier::new(),
            registry: DigitRegistry::new(),
            reward_calc: RewardCalculator::new(),
            difficulty: DifficultyAdjuster::new(),
            max_digits_per_proof: 1_000_000,    // 1M hex digits max
            max_digit_position: 10_000_000_000, // 10B max position (prevents BBP DoS)
            current_height: 0,
            block_timestamp_ms: 0,
            epoch_miner_rewards: BTreeMap::new(),
            current_mining_epoch: 0,
            epoch_actual_minted: 0,
            staking_reward_pool: 0,
            staking_drip_per_block: 0,
            active_miners: BTreeMap::new(),
            next_slot_index: 0,
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
            self.current_height,
            height
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
    ///
    /// Uses **progressive strengthening**: starts with a generous cap during bootstrap
    /// (allowing solo miners to earn meaningful rewards), then tightens as more miners
    /// join the network.
    ///
    /// - < 5 unique miners: 20% cap (allows bootstrapping)
    /// - 5-19 miners: 10% cap (early network)
    /// - 20+ miners: 2% cap (mature network, quantum-resistant)
    fn epoch_emission_cap(&self) -> u64 {
        let cap_bps = self.effective_miner_cap_bps();
        let year = self
            .reward_calc
            .year_from_timestamp(self.block_timestamp_ms);
        let annual = self.reward_calc.annual_emission(year) as u128;
        let epochs_per_year = (reward::BLOCKS_PER_YEAR / BLOCKS_PER_MINING_EPOCH).max(1) as u128;
        let epoch_emission = annual / epochs_per_year;
        let cap = epoch_emission * cap_bps as u128 / 10_000;
        cap.min(u64::MAX as u128) as u64
    }

    /// Calculate the effective per-miner cap.
    ///
    /// Uses the STRICTER of two caps:
    /// 1. Miner count cap: fewer miners → looser cap (bootstrap)
    /// 2. Chain age cap: 31.41% ÷ year, floor 3.14% (tightens every year)
    ///
    /// Formula: effective = min(count_cap, max(314, 3141 / chain_age_years))
    /// All constants derived from π.
    fn effective_miner_cap_bps(&self) -> u32 {
        // Count-based cap
        let unique_miners = self.registry.stats().unique_miners as usize;
        let count_cap = if unique_miners < MIN_MINERS_FOR_TIGHT_CAP {
            BOOTSTRAP_MINER_REWARD_PCT_BPS // 31.41%
        } else if unique_miners < 31 {
            628 // 6.28% (2π)
        } else {
            MAX_MINER_REWARD_PCT_BPS // 3.14%
        };

        // Age-based cap: 3141 / chain_age_years, floor at 314 (3.14%)
        let chain_age_years = (self.current_height / reward::BLOCKS_PER_YEAR).max(1) as u32;
        let age_cap = (BOOTSTRAP_MINER_REWARD_PCT_BPS / chain_age_years).max(MAX_MINER_REWARD_PCT_BPS);

        // Use the STRICTER (lower) of the two
        count_cap.min(age_cap)
    }

    /// Advance the mining epoch if the current block height has crossed a boundary.
    /// At epoch transitions, recycle unmined emission to the staking reward pool.
    fn advance_mining_epoch_if_needed(&mut self) {
        let new_epoch = Self::mining_epoch_for_height(self.current_height);
        if new_epoch != self.current_mining_epoch {
            // Recycle unmined emission from the ending epoch to staking
            self.recycle_unmined_emission();

            self.epoch_miner_rewards.clear();
            self.epoch_actual_minted = 0;
            self.current_mining_epoch = new_epoch;
        }
    }

    /// Calculate and recycle unmined emission from the current epoch to staking rewards.
    /// Called at epoch boundaries.
    fn recycle_unmined_emission(&mut self) {
        let year = self
            .reward_calc
            .year_from_timestamp(self.block_timestamp_ms);
        let expected = self.expected_epoch_emission(year);
        if expected == 0 {
            return;
        }
        let unmined = expected.saturating_sub(self.epoch_actual_minted);
        if unmined == 0 {
            return;
        }
        // 80% to staking, 20% stays in pool (implicitly — we just don't touch it)
        let to_staking = (unmined as u128 * UNMINED_TO_STAKING_BPS as u128 / 10_000) as u64;
        self.staking_reward_pool = self.staking_reward_pool.saturating_add(to_staking);
        // Calculate per-block drip for the next epoch
        self.staking_drip_per_block = self.staking_reward_pool / BLOCKS_PER_MINING_EPOCH.max(1);
    }

    /// Expected emission for one mining epoch in a given year.
    fn expected_epoch_emission(&self, year: u32) -> u64 {
        let annual = self.reward_calc.annual_emission(year) as u128;
        let epochs_per_year = (reward::BLOCKS_PER_YEAR / BLOCKS_PER_MINING_EPOCH).max(1) as u128;
        (annual / epochs_per_year).min(u64::MAX as u128) as u64
    }

    /// Check whether a miner has remaining budget in this epoch.
    /// Returns the capped reward amount (may be 0 if cap exceeded).
    fn check_miner_cap(
        &self,
        miner: &pichain_crypto::keys::Address,
        proposed_reward: u64,
    ) -> u64 {
        let cap = self.epoch_emission_cap();
        if cap == 0 {
            return 0; // zero emission means no mining rewards available
        }
        let already_earned = self.epoch_miner_rewards.get(miner).copied().unwrap_or(0);
        let remaining = cap.saturating_sub(already_earned);
        proposed_reward.min(remaining)
    }

    /// Get (earned_this_epoch, cap_this_epoch) for a given miner.
    pub fn miner_epoch_budget(&self, miner: &pichain_crypto::keys::Address) -> (u64, u64) {
        let earned = self.epoch_miner_rewards.get(miner).copied().unwrap_or(0);
        let cap = self.epoch_emission_cap();
        (earned, cap)
    }

    /// Process a mining proof submission.
    /// This is called during block execution when a MiningProof transaction is encountered.
    pub fn process_proof(
        &mut self,
        proof: &MiningProof,
        anchor_block_hash: &[u8; 32],
    ) -> VerificationResult {
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

        let current_min_batch = min_batch_size(self.registry.frontier());
        if proof.digit_count < current_min_batch {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(format!(
                    "too few digits: {} (minimum {} at frontier {})",
                    proof.digit_count,
                    current_min_batch,
                    self.registry.frontier()
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
        if proof
            .start_position
            .saturating_add(proof.digit_count as u64)
            > self.max_digit_position
        {
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

        // 1c. Reject proofs too far ahead of the frontier
        let frontier = self.registry.frontier();
        let max_dist = max_frontier_distance(frontier);
        if proof.start_position > frontier.saturating_add(max_dist) {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(format!(
                    "proof position {} too far ahead of frontier {} (max distance: {})",
                    proof.start_position, frontier, max_dist
                )),
                epoch_remaining_budget: None,
            };
        }

        // 2. Check if this range has already been computed (full range pre-check)
        // Reject early if the entire range is already covered by registered ranges,
        // avoiding expensive spot-check verification for duplicate submissions.
        if self
            .registry
            .is_range_fully_computed(proof.start_position, proof.digit_count)
        {
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
                    proof
                        .start_position
                        .saturating_add(proof.digit_count as u64)
                )),
                epoch_remaining_budget: None,
            };
        }
        // Also check individual boundary positions for partial overlaps
        let end = match proof.start_position.checked_add(proof.digit_count as u64) {
            Some(e) => e,
            None => {
                return VerificationResult {
                    valid: false,
                    spot_checks: 0,
                    all_checks_passed: false,
                    reward_amount: 0,
                    start_position: proof.start_position,
                    digit_count: proof.digit_count,
                    error: Some("start_position + digit_count overflows u64".to_string()),
                    epoch_remaining_budget: None,
                }
            }
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

        // 4. Calculate reward with progressive scaling + frontier bonus (pure computation, no side effects)
        // Progressive: linear at genesis → sqrt at maturity (dampens hardware advantages)
        let effective_digits = sqrt_effective_digits_at_frontier(proof.digit_count, frontier);
        let base_reward = self
            .reward_calc
            .reward_for_digits_at_time(effective_digits, self.block_timestamp_ms);
        let (bonus_num, bonus_den) = frontier_bonus(proof.start_position, self.registry.frontier());
        let bonused = if bonus_den > 0 {
            ((base_reward as u128) * bonus_num as u128 / bonus_den as u128).min(u64::MAX as u128)
                as u64
        } else {
            base_reward
        };
        // Cap to remaining pool (frontier bonus cannot exceed what's available)
        let raw_reward = bonused.min(self.reward_calc.remaining_pool());

        // 5. Check per-miner epoch cap
        self.advance_mining_epoch_if_needed();
        let reward = self.check_miner_cap(&proof.miner, raw_reward);

        if reward == 0 && raw_reward > 0 {
            // Miner has hit their epoch cap — reject BEFORE registering range
            let already_earned = self
                .epoch_miner_rewards
                .get(&proof.miner)
                .copied()
                .unwrap_or(0);
            return VerificationResult {
                valid: false,
                spot_checks,
                all_checks_passed: true,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(format!(
                    "miner epoch cap reached: earned {} of {} cap in epoch {}",
                    already_earned,
                    self.epoch_emission_cap(),
                    self.current_mining_epoch,
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

        // 8. Track per-miner epoch reward and total epoch minted
        let miner_entry = self.epoch_miner_rewards.entry(proof.miner).or_insert(0);
        *miner_entry = miner_entry.saturating_add(reward);
        self.epoch_actual_minted = self.epoch_actual_minted.saturating_add(reward);

        // 8b. Update active miner registry
        self.touch_miner(&proof.miner);

        // 9. Record proof timestamp for difficulty adjuster
        self.difficulty.record_proof(self.block_timestamp_ms);

        if reward == 0 {
            warn!("mining pool exhausted — no reward for valid proof");
        }

        let remaining_budget = self.epoch_emission_cap().saturating_sub(
            self.epoch_miner_rewards
                .get(&proof.miner)
                .copied()
                .unwrap_or(0),
        );

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
    /// PoW difficulty scales with frontier position and chain age:
    /// - Frontier component: +1 bit per ~2x frontier growth (deeper PI = harder PoW)
    /// - Moore's Law component: +1 bit every 2 years (outpaces hardware improvements)
    /// - Combined with BBP's natural O(position) per-digit cost, mining becomes
    ///   exponentially harder over time.
    ///
    /// All difficulty computation is off-chain (in the miner). The node only checks
    /// `hash <= target` — one comparison, nanoseconds. Regular transactions are unaffected.
    pub fn process_proof_with_pow(
        &mut self,
        proof: &MiningProof,
        pow_nonce: u64,
        anchor_block_hash: &[u8; 32],
    ) -> VerificationResult {
        // Compute frontier-scaled PoW difficulty
        let frontier = self.registry.frontier();
        let year = self
            .reward_calc
            .year_from_timestamp(self.block_timestamp_ms);
        let required_bits = difficulty::frontier_pow_bits(frontier, year);
        let frontier_target = difficulty::frontier_difficulty_target(frontier, year);

        // Combine frontier-scaled and rate-based targets: use the HARDER (smaller) of the two.
        // This ensures that if proofs are arriving too fast, the rate-based adjuster
        // raises difficulty above the frontier floor.
        let effective_target =
            difficulty::harder_target(&frontier_target, &self.difficulty.current_target);

        if !difficulty::check_proof_difficulty(
            &proof.digits,
            pow_nonce,
            anchor_block_hash,
            &effective_target,
            &proof.miner.0,
        ) {
            let effective_bits = self.difficulty.difficulty_bits().max(required_bits);
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(format!(
                    "PoW difficulty not met (required: {} bits at frontier {}, year {})",
                    effective_bits, frontier, year
                )),
                epoch_remaining_budget: None,
            };
        }

        // Run the standard proof verification (spot-checks, registry, rewards)
        self.process_proof(proof, anchor_block_hash)
    }

    /// Process a mining proof with PoW + VDF verification (quantum-resistant).
    ///
    /// This extends `process_proof_with_pow` to also verify a VDF proof,
    /// ensuring that miners cannot submit proofs faster than the VDF time floor
    /// regardless of their computational advantage (quantum or classical).
    ///
    /// The VDF proof must satisfy:
    /// 1. Input seed = vdf_seed(digits, anchor_block_hash, miner)
    /// 2. iterations >= required_iterations(frontier)
    /// 3. output = Blake3^iterations(input) — re-verified by the node
    pub fn process_proof_with_vdf(
        &mut self,
        proof: &MiningProof,
        pow_nonce: u64,
        anchor_block_hash: &[u8; 32],
        vdf_proof: &crate::vdf::VdfProof,
    ) -> VerificationResult {
        // 0. Reject VDF proofs with excessive iterations (DoS prevention)
        if vdf_proof.iterations > crate::vdf::MAX_VDF_ITERATIONS {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(format!(
                    "VDF iterations exceed maximum: {} > {}",
                    vdf_proof.iterations,
                    crate::vdf::MAX_VDF_ITERATIONS
                )),
                epoch_remaining_budget: None,
            };
        }

        // 1. Verify VDF iterations meet minimum requirement
        // Uses progressive strengthening: considers both frontier depth and chain age
        let frontier = self.registry.frontier();
        let required = crate::vdf::required_iterations_with_age(frontier, self.current_height);
        if vdf_proof.iterations < required {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(format!(
                    "VDF iterations too low: {} < {} required at frontier {}",
                    vdf_proof.iterations, required, frontier
                )),
                epoch_remaining_budget: None,
            };
        }

        // 2. Verify VDF input seed matches the proof components
        let expected_seed = crate::vdf::vdf_seed(&proof.digits, anchor_block_hash, &proof.miner.0);
        if vdf_proof.input != expected_seed {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some("VDF input seed does not match proof components".to_string()),
                epoch_remaining_budget: None,
            };
        }

        // 3. Verify the VDF hash chain
        if !crate::vdf::vdf_verify(vdf_proof) {
            return VerificationResult {
                valid: false,
                spot_checks: 0,
                all_checks_passed: false,
                reward_amount: 0,
                start_position: proof.start_position,
                digit_count: proof.digit_count,
                error: Some(
                    "VDF verification failed: output does not match hash chain".to_string(),
                ),
                epoch_remaining_budget: None,
            };
        }

        // 4. VDF passed — proceed with standard PoW + proof verification
        self.process_proof_with_pow(proof, pow_nonce, anchor_block_hash)
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

    /// Get the current frontier-scaled difficulty target.
    pub fn difficulty_target(&self) -> [u8; 32] {
        let year = self
            .reward_calc
            .year_from_timestamp(self.block_timestamp_ms);
        difficulty::frontier_difficulty_target(self.registry.frontier(), year)
    }

    /// Get current difficulty as approximate leading zero bits (frontier-scaled).
    pub fn difficulty_bits(&self) -> u32 {
        let year = self
            .reward_calc
            .year_from_timestamp(self.block_timestamp_ms);
        difficulty::frontier_pow_bits(self.registry.frontier(), year)
    }

    /// Get the per-block staking drip from unmined emission recycling.
    pub fn staking_drip_per_block(&self) -> u64 {
        self.staking_drip_per_block
    }

    /// Get the accumulated staking reward pool.
    pub fn staking_reward_pool(&self) -> u64 {
        self.staking_reward_pool
    }

    /// Drain one block's worth of staking drip. Returns the amount drained.
    /// Called by the executor after processing all transactions in a block.
    pub fn drain_staking_drip(&mut self) -> u64 {
        let drip = self.staking_drip_per_block.min(self.staking_reward_pool);
        self.staking_reward_pool = self.staking_reward_pool.saturating_sub(drip);
        drip
    }

    /// Get or assign a mining slot for the given address.
    /// Returns `(recommended_start_position, slot_index, total_active_miners)`.
    ///
    /// Slot assignment is advisory — the real overlap enforcer is the digit registry.
    /// Slots spread miners across non-overlapping ranges ahead of the frontier
    /// to reduce wasted computation from collisions.
    pub fn get_or_assign_slot(
        &mut self,
        miner: &pichain_crypto::keys::Address,
    ) -> (u64, u32, usize) {
        // Cleanup stale miners first
        self.cleanup_stale_miners();

        let slot_index = if let Some(&(idx, _)) = self.active_miners.get(miner) {
            // Already has a slot — update last_proof_height
            self.active_miners
                .insert(*miner, (idx, self.current_height));
            idx
        } else {
            // Try to reuse a freed slot index
            let used_slots: std::collections::BTreeSet<u32> =
                self.active_miners.values().map(|(idx, _)| *idx).collect();
            let idx = (0..self.next_slot_index)
                .find(|i| !used_slots.contains(i))
                .unwrap_or_else(|| {
                    let i = self.next_slot_index;
                    self.next_slot_index = self.next_slot_index.saturating_add(1);
                    i
                });
            self.active_miners
                .insert(*miner, (idx, self.current_height));
            idx
        };

        // Use the higher of frontier or total_verified as the base position.
        // Frontier can lag behind when there are gaps in contiguous ranges,
        // but total_verified reflects how far miners have actually computed.
        // Using frontier alone causes miners to get positions that are already computed.
        let frontier = self.registry.frontier();
        let total_verified = self.registry.stats().total_digits_verified;
        let base = frontier.max(total_verified);
        let position = base.saturating_add((slot_index as u64).saturating_mul(SLOT_RANGE_SIZE));
        let total = self.active_miners.len();
        (position, slot_index, total)
    }

    /// Remove miners that haven't submitted a proof within one mining epoch.
    fn cleanup_stale_miners(&mut self) {
        let cutoff = self.current_height.saturating_sub(BLOCKS_PER_MINING_EPOCH);
        self.active_miners
            .retain(|_, (_, last_height)| *last_height >= cutoff);
    }

    /// Record that a miner submitted a proof (updates their last_proof_height).
    fn touch_miner(&mut self, miner: &pichain_crypto::keys::Address) {
        if let Some(entry) = self.active_miners.get_mut(miner) {
            entry.1 = self.current_height;
        } else {
            // Auto-assign slot if miner isn't registered yet
            self.get_or_assign_slot(miner);
        }
    }

    /// Get slot info for a specific miner without modifying state.
    pub fn miner_slot_info(&self, miner: &pichain_crypto::keys::Address) -> Option<(u64, u32)> {
        self.active_miners.get(miner).map(|&(idx, _)| {
            let frontier = self.registry.frontier();
            let position = frontier.saturating_add((idx as u64).saturating_mul(SLOT_RANGE_SIZE));
            (position, idx)
        })
    }

    /// Number of currently active miners (with non-stale slots).
    pub fn active_miner_count(&self) -> usize {
        self.active_miners.len()
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
        let year = self
            .reward_calc
            .year_from_timestamp(self.block_timestamp_ms);
        let frontier = reg_stats.frontier_position;
        let min_batch = min_batch_size(frontier);
        let (next_pos, gap_size) = self.registry.find_mineable_gap(min_batch);
        let pow_bits = difficulty::frontier_pow_bits(frontier, year);
        let frontier_target = difficulty::frontier_difficulty_target(frontier, year);
        // Effective target is the harder (smaller) of frontier-scaled and rate-based targets
        let effective_target =
            difficulty::harder_target(&frontier_target, &self.difficulty.current_target);
        let effective_bits = pow_bits.max(self.difficulty.difficulty_bits());
        MiningStats {
            total_digits_verified: reg_stats.total_digits_verified,
            frontier_position: frontier,
            total_ranges: reg_stats.total_ranges,
            unique_miners: reg_stats.unique_miners,
            remaining_pool: self.reward_calc.remaining_pool(),
            total_mined: self.reward_calc.total_mined(),
            fee_income: self.reward_calc.fee_income(),
            next_position: next_pos,
            max_batch_at_position: gap_size,
            reward_per_digit: self.reward_calc.reward_per_digit(year),
            emission_year: year,
            difficulty_bits: effective_bits,
            difficulty_target_hex: hex::encode(effective_target),
            mining_epoch: self.current_mining_epoch,
            epoch_miner_cap: self.epoch_emission_cap(),
            // New frontier mining fields
            min_batch_size: min_batch_size(frontier),
            max_allowed_position: frontier.saturating_add(max_frontier_distance(frontier)),
            frontier_bonus_at_next: {
                let (n, d) = frontier_bonus(next_pos, frontier);
                if d == 0 {
                    "3.0x".to_string()
                } else {
                    format!("{:.1}x", n as f64 / d as f64)
                }
            },
            staking_reward_pool: self.staking_reward_pool,
            epoch_actual_minted: self.epoch_actual_minted,
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
        miner: pichain_crypto::keys::Address,
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

        self.registry.register(range).map_err(|e| e.to_string())?;

        // Replay the reward using timestamp-based year calculation — the same
        // method used by live process_proof(). Apply frontier bonus just like
        // live processing does.
        let base_reward = self
            .reward_calc
            .reward_for_digits_at_time(digit_count, block_timestamp_ms);
        let (bonus_num, bonus_den) = frontier_bonus(start_position, self.registry.frontier());
        let bonused = if bonus_den > 0 {
            ((base_reward as u128) * bonus_num as u128 / bonus_den as u128).min(u64::MAX as u128)
                as u64
        } else {
            base_reward
        };
        let reward = bonused.min(self.reward_calc.remaining_pool());

        // Track per-miner epoch reward during replay for accurate state after restart.
        // Apply the same per-miner epoch cap during replay that process_proof uses
        // during live processing. Record the CAPPED reward to total_mined to match
        // what live processing records.
        let replay_epoch = Self::mining_epoch_for_height(block_height);
        if replay_epoch != self.current_mining_epoch {
            // Recycle unmined emission during replay too
            self.block_timestamp_ms = block_timestamp_ms;
            self.recycle_unmined_emission();
            self.epoch_miner_rewards.clear();
            self.epoch_actual_minted = 0;
            self.current_mining_epoch = replay_epoch;
        }
        let saved_ts = self.block_timestamp_ms;
        self.block_timestamp_ms = block_timestamp_ms;
        let capped_reward = self.check_miner_cap(&miner, reward);
        self.block_timestamp_ms = saved_ts;

        if capped_reward > 0 {
            self.reward_calc
                .record_reward(capped_reward)
                .map_err(|e| format!("replay reward tracking failed: {e}"))?;
        }

        let miner_entry = self.epoch_miner_rewards.entry(miner).or_insert(0);
        *miner_entry = miner_entry.saturating_add(capped_reward);
        self.epoch_actual_minted = self.epoch_actual_minted.saturating_add(capped_reward);

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
    /// Maximum contiguous digits that can be mined at `next_position` before
    /// hitting an existing range. Miners should cap their batch size to this value.
    /// u64::MAX means unlimited (no ranges ahead).
    #[serde(default = "default_max_batch")]
    pub max_batch_at_position: u64,
    pub reward_per_digit: u64,
    pub emission_year: u32,
    /// Current PoW difficulty in leading zero bits (frontier-scaled).
    pub difficulty_bits: u32,
    /// Current difficulty target as hex string (frontier-scaled).
    pub difficulty_target_hex: String,
    /// Current mining epoch number.
    pub mining_epoch: u64,
    /// Per-miner reward cap for current epoch (in base units).
    pub epoch_miner_cap: u64,
    /// Minimum digits per proof at current frontier.
    #[serde(default)]
    pub min_batch_size: u32,
    /// Maximum position allowed (frontier + max_frontier_distance).
    #[serde(default)]
    pub max_allowed_position: u64,
    /// Current frontier bonus multiplier at next_position (display string, e.g. "2.5x").
    #[serde(default)]
    pub frontier_bonus_at_next: String,
    /// Accumulated staking reward pool from unmined emission recycling.
    #[serde(default)]
    pub staking_reward_pool: u64,
    /// Total mining rewards actually minted in the current epoch.
    #[serde(default)]
    pub epoch_actual_minted: u64,
}

fn default_max_batch() -> u64 {
    u64::MAX
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bbp::BbpComputer;
    use pichain_crypto::keys::Address;

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
        assert!(
            !result.valid,
            "PoW should be enforced even with zero anchor"
        );
        assert!(result.error.unwrap().contains("PoW difficulty not met"));
    }

    #[test]
    fn pow_with_valid_nonce() {
        let mut processor = configured_processor();

        let miner_addr = Address([1; 20]);
        let digits = BbpComputer::compute_hex_digits(0, 200);
        let anchor = [1u8; 32];
        // Find a valid nonce bound to the miner address
        let nonce = difficulty::find_nonce(
            &digits,
            &anchor,
            &difficulty::INITIAL_DIFFICULTY,
            10_000,
            &miner_addr.0,
        )
        .expect("should find nonce with easy difficulty");

        let proof = MiningProof::new(0, digits, miner_addr, 42);
        let result = processor.process_proof_with_pow(&proof, nonce, &anchor);
        assert!(
            result.valid,
            "proof with valid PoW should pass: {:?}",
            result.error
        );
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
        assert_eq!(
            processor.difficulty().recent_proof_times[0],
            TEST_GENESIS_TS + 1000
        );
    }

    #[test]
    fn historical_replay_tracks_supply() {
        let mut processor = MiningProcessor::new();
        processor.set_genesis_timestamp(TEST_GENESIS_TS);

        let digits = BbpComputer::compute_hex_digits(0, 200);
        processor
            .register_historical(
                0,
                200,
                &digits,
                Address([1; 20]),
                1,
                TEST_GENESIS_TS + 1_000,
            )
            .unwrap();

        assert!(
            processor.total_mined() > 0,
            "historical replay should track supply"
        );
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
        assert!(
            mined_y1 > mined_y2_increment,
            "year 1 reward ({}) should exceed year 2 reward ({})",
            mined_y1,
            mined_y2_increment
        );
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
        assert_eq!(
            r2.spot_checks, 0,
            "rejection should happen before spot-checking"
        );
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

        assert!(
            !result.valid,
            "should reject when genesis timestamp not set"
        );
        assert!(
            result
                .error
                .as_ref()
                .unwrap()
                .contains("genesis timestamp not set"),
            "error should mention genesis timestamp: {:?}",
            result.error
        );
    }

    #[test]
    fn register_historical_rejects_without_genesis_timestamp() {
        let mut processor = MiningProcessor::new();
        // Deliberately do NOT call set_genesis_timestamp()

        let digits = BbpComputer::compute_hex_digits(0, 200);
        let result =
            processor.register_historical(0, 200, &digits, Address([1; 20]), 1, 1_000_000_001_000);
        assert!(
            result.is_err(),
            "should reject when genesis timestamp not set"
        );
        assert!(
            result.unwrap_err().contains("genesis timestamp not set"),
            "error should mention genesis timestamp"
        );
    }

    #[test]
    fn is_genesis_configured_reflects_state() {
        let mut processor = MiningProcessor::new();
        assert!(
            !processor.is_genesis_configured(),
            "should be false before set"
        );

        processor.set_genesis_timestamp(TEST_GENESIS_TS);
        assert!(
            processor.is_genesis_configured(),
            "should be true after set"
        );
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
        assert!(
            r2.valid,
            "should still succeed with partial reward: {:?}",
            r2.error
        );
        assert!(
            r2.reward_amount <= 1,
            "reward should be capped to remaining budget (1)"
        );

        // Now the miner is at or over the cap. Next proof should be rejected.
        let digits3 = BbpComputer::compute_hex_digits(400, 200);
        let proof3 = MiningProof::new(400, digits3, miner, 3);
        let r3 = processor.process_proof(&proof3, &[0u8; 32]);
        assert!(!r3.valid, "should be rejected after hitting cap");
        assert!(
            r3.error.as_ref().unwrap().contains("epoch cap"),
            "expected epoch cap error, got: {:?}",
            r3.error
        );
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
        assert!(
            result_b.valid,
            "miner B should succeed: {:?}",
            result_b.error
        );

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

    // ── Quantum Resistance Tests ──────────────────────────────────────────

    #[test]
    fn sqrt_effective_digits_mature_baseline() {
        // At mature frontier (1M+): 100 digits → sqrt(100) * 10 = 100 effective
        assert_eq!(sqrt_effective_digits_at_frontier(100, 1_000_000), 100);
    }

    #[test]
    fn sqrt_effective_digits_mature_diminishing_returns() {
        let f = 1_000_000; // mature frontier
        let eff_100 = sqrt_effective_digits_at_frontier(100, f);
        let eff_1000 = sqrt_effective_digits_at_frontier(1000, f);
        let eff_10000 = sqrt_effective_digits_at_frontier(10000, f);

        // 10x more compute → ~3.16x more effective digits (not 10x)
        assert!(eff_1000 < eff_100 * 5);
        assert!(eff_1000 > eff_100 * 2);
        // 100x more compute → ~10x more effective digits (not 100x)
        assert!(eff_10000 < eff_100 * 15);
        assert!(eff_10000 > eff_100 * 5);
    }

    #[test]
    fn sqrt_effective_digits_genesis_is_linear() {
        // At genesis (frontier < 10K): scaling is LINEAR — no dampening
        assert_eq!(sqrt_effective_digits_at_frontier(100, 0), 100);
        assert_eq!(sqrt_effective_digits_at_frontier(1000, 0), 1000);
        assert_eq!(sqrt_effective_digits_at_frontier(5000, 0), 5000);
    }

    #[test]
    fn sqrt_effective_digits_progressive_tightens() {
        let digit_count = 10_000u32;
        let genesis = sqrt_effective_digits_at_frontier(digit_count, 0);
        let early = sqrt_effective_digits_at_frontier(digit_count, 50_000);
        let growing = sqrt_effective_digits_at_frontier(digit_count, 500_000);
        let mature = sqrt_effective_digits_at_frontier(digit_count, 5_000_000);

        // Each phase should give fewer effective digits for the same raw count
        assert!(genesis >= early, "genesis should be >= early");
        assert!(early >= growing, "early should be >= growing");
        assert!(growing >= mature, "growing should be >= mature");
        // Genesis (linear) should be much more generous than mature (sqrt)
        assert!(genesis > mature * 2, "genesis should be >2x mature");
    }

    #[test]
    fn sqrt_effective_digits_zero() {
        assert_eq!(sqrt_effective_digits(0), 0);
        assert_eq!(sqrt_effective_digits_at_frontier(0, 0), 0);
    }

    #[test]
    fn sqrt_effective_digits_one() {
        // At mature frontier: sqrt(1) = 1 * 10 = 10 effective digits
        assert_eq!(sqrt_effective_digits(1), 10);
        // At genesis: linear, so 1 → 1
        assert_eq!(sqrt_effective_digits_at_frontier(1, 0), 1);
    }

    #[test]
    fn miner_cap_is_pi_percent_at_maturity() {
        assert_eq!(MAX_MINER_REWARD_PCT_BPS, 314);
    }

    #[test]
    fn miner_cap_progressive() {
        // Bootstrap (< 9 miners): 31.41% cap
        assert_eq!(BOOTSTRAP_MINER_REWARD_PCT_BPS, 3141);
        // Mature (31+ miners): 3.14% cap
        assert_eq!(MAX_MINER_REWARD_PCT_BPS, 314);
    }

    #[test]
    fn process_proof_at_genesis_is_linear() {
        let mut processor = configured_processor();
        processor.set_block_timestamp(TEST_GENESIS_TS);
        processor.set_height(1);

        // At genesis (frontier=0), scaling is linear
        let digits_100 = BbpComputer::compute_hex_digits(0, 100);
        let proof_100 = MiningProof::new(0, digits_100, Address([1u8; 20]), 42);
        let result_100 = processor.process_proof(&proof_100, &[0u8; 32]);
        assert!(
            result_100.valid,
            "proof should be valid: {:?}",
            result_100.error
        );

        let digits_200 = BbpComputer::compute_hex_digits(100, 200);
        let proof_200 = MiningProof::new(100, digits_200, Address([2u8; 20]), 42);
        let result_200 = processor.process_proof(&proof_200, &[0u8; 32]);
        assert!(
            result_200.valid,
            "proof should be valid: {:?}",
            result_200.error
        );

        // At genesis with linear scaling, 2x digits ≈ 2x reward
        // (frontier bonus may vary slightly, but should be close)
        if result_100.reward_amount > 0 {
            let ratio = result_200.reward_amount as f64 / result_100.reward_amount as f64;
            assert!(
                ratio > 1.5 && ratio < 2.5,
                "at genesis, 2x digits should give ~2x reward (linear), got {:.1}x",
                ratio
            );
        }
    }

    #[test]
    fn vdf_verification_rejects_wrong_seed() {
        let mut processor = configured_processor();
        processor.set_block_timestamp(TEST_GENESIS_TS);
        processor.set_height(1);

        let digits = BbpComputer::compute_hex_digits(0, 100);
        let miner = Address([1u8; 20]);
        let proof = MiningProof::new(0, digits.clone(), miner, 42);
        let anchor = [0u8; 32];

        // Use required iterations so we pass the iteration check
        let required = crate::vdf::required_iterations(0);

        // Create VDF with wrong seed (wrong miner address) but correct iterations
        let wrong_seed = crate::vdf::vdf_seed(&digits, &anchor, &[99u8; 20]);
        let vdf = crate::vdf::vdf_compute(&wrong_seed, required);

        let result = processor.process_proof_with_vdf(&proof, 0, &anchor, &vdf);
        assert!(!result.valid);
        assert!(
            result.error.as_ref().unwrap().contains("VDF input seed"),
            "expected VDF seed error, got: {:?}",
            result.error
        );
    }

    #[test]
    fn vdf_verification_rejects_insufficient_iterations() {
        let mut processor = configured_processor();
        processor.set_block_timestamp(TEST_GENESIS_TS);
        processor.set_height(1);

        let digits = BbpComputer::compute_hex_digits(0, 100);
        let miner = Address([1u8; 20]);
        let proof = MiningProof::new(0, digits.clone(), miner, 42);
        let anchor = [0u8; 32];

        // Create VDF with too few iterations (less than required for frontier 0)
        let seed = crate::vdf::vdf_seed(&digits, &anchor, &miner.0);
        let vdf = crate::vdf::vdf_compute(&seed, 100); // way too few

        let result = processor.process_proof_with_vdf(&proof, 0, &anchor, &vdf);
        assert!(!result.valid);
        assert!(
            result
                .error
                .as_ref()
                .unwrap()
                .contains("VDF iterations too low"),
            "expected VDF iterations error, got: {:?}",
            result.error
        );
    }
}
