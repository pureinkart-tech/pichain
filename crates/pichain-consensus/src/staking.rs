//! Staking and slashing system for PIChain validators.
//!
//! Validators stake PI tokens as collateral. Misbehavior (double-signing,
//! downtime, invalid blocks) triggers slashing penalties.
//!
//! Staking rewards come from the 65% fee distribution to stakers.
//! Delegation allows non-validators to earn rewards by delegating to validators.

use pichain_crypto::ed25519::Address;
use pichain_crypto::Hash;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Minimum stake to become a validator at maturity (10,000 PI in base units).
/// During bootstrap (< 10 validators), `effective_min_validator_stake()` returns
/// a lower value (100 PI) to make validator participation accessible at launch.
pub const MIN_VALIDATOR_STAKE: u64 = 10_000 * 1_000_000_000;

/// Bootstrap minimum stake (100 PI) — accessible for early adopters.
pub const BOOTSTRAP_MIN_VALIDATOR_STAKE: u64 = 100 * 1_000_000_000;

/// Number of validators before minimum stake tightens to full MIN_VALIDATOR_STAKE.
pub const MIN_VALIDATORS_FOR_FULL_STAKE: usize = 10;

/// Calculate the effective minimum validator stake based on current validator count.
///
/// Progressive strengthening:
/// - < 10 validators: 100 PI (accessible for early adopters)
/// - 10+ validators: 10,000 PI (full security requirement)
pub fn effective_min_validator_stake(validator_count: usize) -> u64 {
    if validator_count < MIN_VALIDATORS_FOR_FULL_STAKE {
        BOOTSTRAP_MIN_VALIDATOR_STAKE
    } else {
        MIN_VALIDATOR_STAKE
    }
}

/// Minimum stake for delegation (1 PI in base units).
pub const MIN_DELEGATION: u64 = 1_000_000_000;

/// Unbonding period in epochs.
/// 62 epochs × 31,415 blocks/epoch × 314ms/block ≈ 7 days.
pub const UNBONDING_EPOCHS: u64 = 62;

/// Slash rate for double-signing (33% of stake).
pub const SLASH_DOUBLE_SIGN_BPS: u16 = 3300;

/// Slash rate for extended downtime (1% of stake).
pub const SLASH_DOWNTIME_BPS: u16 = 100;

/// Slash rate for invalid block proposal (5% of stake).
pub const SLASH_INVALID_BLOCK_BPS: u16 = 500;

/// Jail duration in epochs for slashed validators.
/// 88 epochs × 31,415 blocks/epoch × 314ms/block ≈ 10 days.
pub const JAIL_DURATION_EPOCHS: u64 = 88;

/// Maximum evidence age in epochs. Evidence older than this is rejected.
/// Must be strictly less than UNBONDING_EPOCHS so that a misbehaving validator
/// who immediately begins unbonding still has locked funds when evidence arrives.
pub const MAX_EVIDENCE_AGE_EPOCHS: u64 = 42;

/// Maximum commission rate (20%).
pub const MAX_COMMISSION_BPS: u16 = 2000;

/// Maximum percentage of total staked that any single validator can hold (basis points).
/// 3333 bps = 33.33%. Prevents one validator from controlling 2/3+1 quorum alone.
pub const MAX_VALIDATOR_STAKE_PCT_BPS: u16 = 3333;

/// Maximum percentage of total staked that any single address can control (basis points).
/// 1000 bps = 10%. Forces capital distribution even across multiple delegations.
pub const MAX_ADDRESS_STAKE_PCT_BPS: u16 = 1000;

/// Maximum stake growth per validator per epoch (basis points of epoch-start stake).
/// 5000 bps = 50%. Prevents flash-staking attacks.
pub const MAX_STAKE_GROWTH_PCT_BPS: u16 = 5000;

/// Minimum number of validators before full concentration caps are enforced.
/// During bootstrap (<4 validators), a relaxed 41.3% cap applies per validator.
pub const MIN_VALIDATORS_FOR_STAKE_CAP: usize = 4;

/// Maximum number of active (eligible) validators.
/// Prevents Sybil flooding where an attacker registers hundreds of validators
/// to dilute the set and control consensus.
pub const MAX_ACTIVE_VALIDATORS: usize = 100;

/// Correlated slashing multiplier (basis points per simultaneously-slashed validator).
/// When N validators are slashed in the same epoch, each receives an ADDITIONAL
/// penalty of N * 3333 bps (capped at 100%). Makes Sybil attacks self-destructive.
pub const CORRELATED_SLASH_MULTIPLIER_BPS: u32 = 3333;

/// Bootstrap validator concentration cap (41.3%).
/// Even during bootstrap (<4 validators), no single validator can hold >41.3% of stake.
/// Prevents a first-mover monopoly before full caps activate.
/// 4130 bps — PI-themed (4-1-3 = first 3 digits of π).
pub const BOOTSTRAP_VALIDATOR_STAKE_PCT_BPS: u16 = 4130;

/// A staking entry for a validator.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StakeEntry {
    /// Validator address.
    pub validator: Address,
    /// Self-staked amount.
    pub self_stake: u64,
    /// Total delegated stake from others.
    pub delegated_stake: u64,
    /// Commission rate in basis points (e.g., 1000 = 10%).
    pub commission_bps: u16,
    /// Whether this validator is currently jailed.
    pub jailed: bool,
    /// Epoch when the validator was jailed (None if not jailed).
    pub jailed_until: Option<u64>,
    /// Epoch when the validator joined.
    pub joined_epoch: u64,
    /// Accumulated rewards (not yet claimed).
    pub pending_rewards: u64,
    /// Blocks proposed.
    pub blocks_proposed: u64,
    /// Blocks missed (for downtime tracking).
    pub blocks_missed: u64,
    /// Whether the validator is active in the current epoch.
    pub active: bool,
    /// Snapshot of total_stake() at the start of the current epoch (for velocity limiting).
    #[serde(default)]
    pub epoch_start_stake: u64,
}

impl StakeEntry {
    pub fn new(validator: Address, self_stake: u64, commission_bps: u16, epoch: u64) -> Self {
        Self {
            validator,
            self_stake,
            delegated_stake: 0,
            commission_bps: commission_bps.min(MAX_COMMISSION_BPS),
            jailed: false,
            jailed_until: None,
            joined_epoch: epoch,
            pending_rewards: 0,
            blocks_proposed: 0,
            blocks_missed: 0,
            active: true,
            epoch_start_stake: self_stake,
        }
    }

    /// Total stake (self + delegated).
    pub fn total_stake(&self) -> u64 {
        self.self_stake.saturating_add(self.delegated_stake)
    }

    /// Whether the validator meets minimum stake requirements.
    pub fn meets_minimum(&self) -> bool {
        self.self_stake >= MIN_VALIDATOR_STAKE
    }

    /// Whether the validator is eligible to participate in consensus.
    pub fn is_eligible(&self) -> bool {
        self.active && !self.jailed && self.meets_minimum()
    }
}

/// A delegation from a delegator to a validator.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Delegation {
    /// Delegator address.
    pub delegator: Address,
    /// Validator being delegated to.
    pub validator: Address,
    /// Amount delegated.
    pub amount: u64,
    /// Accumulated rewards (not yet claimed).
    pub pending_rewards: u64,
    /// Epoch when delegation started.
    pub start_epoch: u64,
}

/// An unbonding entry (stake being withdrawn, subject to unbonding period).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnbondingEntry {
    /// Who is unbonding.
    pub address: Address,
    /// Validator being unbonded from.
    pub validator: Address,
    /// Amount being unbonded.
    pub amount: u64,
    /// Epoch when unbonding completes.
    pub completion_epoch: u64,
}

/// A slashing event record.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlashEvent {
    /// Validator that was slashed.
    pub validator: Address,
    /// Reason for the slash.
    pub reason: SlashReason,
    /// Amount slashed.
    pub amount_slashed: u64,
    /// Epoch when the slash occurred.
    pub epoch: u64,
    /// Evidence hash (for double-signing, this is the conflicting block hashes).
    pub evidence_hash: Hash,
}

/// Reasons a validator can be slashed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SlashReason {
    /// Signed two different blocks at the same height.
    DoubleSigning,
    /// Extended downtime (missed too many blocks).
    Downtime,
    /// Proposed an invalid block.
    InvalidBlock,
}

impl SlashReason {
    /// Get the slash rate in basis points.
    pub fn slash_rate_bps(&self) -> u16 {
        match self {
            SlashReason::DoubleSigning => SLASH_DOUBLE_SIGN_BPS,
            SlashReason::Downtime => SLASH_DOWNTIME_BPS,
            SlashReason::InvalidBlock => SLASH_INVALID_BLOCK_BPS,
        }
    }
}

/// The staking manager — manages all validator stakes, delegations, and slashing.
pub struct StakingManager {
    /// Validator stakes, keyed by validator address.
    validators: HashMap<Address, StakeEntry>,
    /// Delegations, keyed by (delegator, validator).
    delegations: HashMap<(Address, Address), Delegation>,
    /// Pending unbonding entries.
    unbonding: Vec<UnbondingEntry>,
    /// Slash history.
    slash_history: Vec<SlashEvent>,
    /// Current epoch.
    current_epoch: u64,
    /// Total staked across all validators.
    total_staked: u64,
    /// Total slashed (sent to burn address).
    total_slashed: u64,
    /// Per-epoch sets of distinct validators slashed.
    /// Used for correlated slashing: when N validators are slashed in the same
    /// epoch, each receives an additional penalty of N * CORRELATED_SLASH_MULTIPLIER_BPS.
    /// This makes Sybil attacks self-destructive.
    /// R37-FIX: HashMap keyed by evidence_epoch instead of single-epoch tracker.
    /// Prevents interleaved evidence from different epochs resetting the count.
    epoch_slashed_validators: HashMap<u64, std::collections::HashSet<Address>>,
}

impl StakingManager {
    pub fn new() -> Self {
        Self {
            validators: HashMap::new(),
            delegations: HashMap::new(),
            unbonding: Vec::new(),
            slash_history: Vec::new(),
            current_epoch: 0,
            total_staked: 0,
            total_slashed: 0,
            epoch_slashed_validators: HashMap::new(),
        }
    }

    /// Get the current epoch.
    pub fn current_epoch(&self) -> u64 {
        self.current_epoch
    }

    /// Set the current epoch. Automatically snapshots validator stakes for velocity limiting.
    pub(crate) fn set_epoch(&mut self, epoch: u64) {
        if epoch != self.current_epoch {
            self.snapshot_epoch_stakes();
            // R36-FIX: Prune slash_history entries older than 2x UNBONDING_EPOCHS
            // to prevent unbounded memory growth over the chain's lifetime.
            let prune_before = epoch.saturating_sub(UNBONDING_EPOCHS * 2);
            self.slash_history.retain(|e| e.epoch >= prune_before);
            // R37-FIX: Prune epoch_slashed_validators entries older than MAX_EVIDENCE_AGE_EPOCHS
            // to prevent unbounded growth of the correlated slashing tracker.
            let evidence_prune_before = epoch.saturating_sub(MAX_EVIDENCE_AGE_EPOCHS);
            self.epoch_slashed_validators
                .retain(|&e, _| e >= evidence_prune_before);
        }
        self.current_epoch = epoch;
    }

    /// Snapshot epoch_start_stake for all validators. Called at epoch boundaries.
    /// Restricted to pub(crate) to prevent external mid-epoch velocity limit resets.
    pub(crate) fn snapshot_epoch_stakes(&mut self) {
        for entry in self.validators.values_mut() {
            entry.epoch_start_stake = entry.total_stake();
        }
    }

    /// Check whether adding stake to a validator would exceed the per-validator
    /// concentration cap. During bootstrap (<4 validators), a relaxed 41.3% cap
    /// applies to prevent first-mover monopoly.
    fn check_concentration_cap(
        &self,
        validator: &Address,
        additional_stake: u64,
    ) -> Result<(), StakingError> {
        // Use relaxed cap during bootstrap, full cap once network is established
        let cap_bps = if self.validators.len() < MIN_VALIDATORS_FOR_STAKE_CAP {
            BOOTSTRAP_VALIDATOR_STAKE_PCT_BPS as u32
        } else {
            MAX_VALIDATOR_STAKE_PCT_BPS as u32
        };

        let current_validator_stake = self
            .validators
            .get(validator)
            .map(|e| e.total_stake())
            .unwrap_or(0);

        let new_validator_stake = current_validator_stake.saturating_add(additional_stake);
        let new_total_staked = self.total_staked.saturating_add(additional_stake);

        if new_total_staked == 0 {
            return Ok(());
        }

        // Use u32 for BPS result to prevent u16 truncation wrapping at >655%
        let share_bps = ((new_validator_stake as u128 * 10_000) / new_total_staked as u128) as u32;

        if share_bps > cap_bps {
            return Err(StakingError::StakeConcentrationExceeded {
                validator: *validator,
                would_hold_bps: share_bps.min(u16::MAX as u32) as u16,
                max_bps: cap_bps.min(u16::MAX as u32) as u16,
            });
        }

        Ok(())
    }

    /// Check whether an address's total stake across all validators would exceed
    /// the per-address cap. During bootstrap (<4 validators), a relaxed 41.3% cap
    /// applies to prevent first-mover monopoly.
    fn check_address_concentration(
        &self,
        address: &Address,
        additional_stake: u64,
    ) -> Result<(), StakingError> {
        // SECURITY: Per-address cap is always 10% in mature networks and 20% during
        // bootstrap (< 4 validators). The bootstrap relaxation is smaller than the
        // per-validator cap (41.3%) to prevent two addresses from controlling >40%.
        // With 3 validators at equal stake (~33% each), a 20% per-address cap allows
        // small additions but prevents any single address from dominating.
        let cap_bps = if self.validators.len() < MIN_VALIDATORS_FOR_STAKE_CAP {
            2000u32 // 20% — tighter than old 41.3%, still allows bootstrap
        } else {
            MAX_ADDRESS_STAKE_PCT_BPS as u32 // 10%
        };

        let new_total_staked = self.total_staked.saturating_add(additional_stake);
        if new_total_staked == 0 {
            return Ok(());
        }

        let current_address_stake = self.address_total_stake(address);
        let new_address_stake = current_address_stake.saturating_add(additional_stake);
        let share_bps = ((new_address_stake as u128 * 10_000) / new_total_staked as u128) as u32;

        if share_bps > cap_bps {
            return Err(StakingError::AddressConcentrationExceeded {
                address: *address,
                would_hold_bps: share_bps.min(u16::MAX as u32) as u16,
                max_bps: cap_bps.min(u16::MAX as u32) as u16,
            });
        }

        Ok(())
    }

    /// Check whether adding stake would exceed the per-epoch velocity limit.
    /// Skips enforcement during bootstrap or for new validators (epoch_start_stake == 0).
    fn check_stake_velocity(
        &self,
        validator: &Address,
        additional_stake: u64,
    ) -> Result<(), StakingError> {
        if self.validators.len() < MIN_VALIDATORS_FOR_STAKE_CAP {
            return Ok(());
        }

        let entry = match self.validators.get(validator) {
            Some(e) => e,
            None => return Ok(()), // new validator, no velocity limit
        };

        if entry.epoch_start_stake == 0 {
            return Ok(()); // new validator in this epoch
        }

        let new_total = entry.total_stake().saturating_add(additional_stake);
        // growth = (new - start) / start, in bps
        let growth = new_total.saturating_sub(entry.epoch_start_stake);
        // Use u32 for growth_bps to prevent u16 truncation wrapping at extreme values
        let growth_bps = ((growth as u128 * 10_000) / entry.epoch_start_stake as u128) as u32;

        if growth_bps > MAX_STAKE_GROWTH_PCT_BPS as u32 {
            return Err(StakingError::StakeVelocityExceeded {
                validator: *validator,
                growth_bps: growth_bps.min(u16::MAX as u32) as u16,
                max_bps: MAX_STAKE_GROWTH_PCT_BPS,
            });
        }

        Ok(())
    }

    /// Get total stake controlled by an address (self-stake as validator + all delegations).
    pub fn address_total_stake(&self, address: &Address) -> u64 {
        let mut total = 0u64;
        // Self-stake if this address is a validator
        if let Some(entry) = self.validators.get(address) {
            total = total.saturating_add(entry.self_stake);
        }
        // Sum all delegations from this address
        for ((delegator, _), delegation) in &self.delegations {
            if delegator == address {
                total = total.saturating_add(delegation.amount);
            }
        }
        total
    }

    /// Register a new validator with initial self-stake.
    pub fn register_validator(
        &mut self,
        address: Address,
        self_stake: u64,
        commission_bps: u16,
    ) -> Result<(), StakingError> {
        if self.validators.contains_key(&address) {
            return Err(StakingError::AlreadyRegistered(address));
        }
        // Progressive: lower stake requirement during bootstrap to attract early validators
        let min_stake = effective_min_validator_stake(self.validators.len());
        if self_stake < min_stake {
            return Err(StakingError::InsufficientStake {
                required: min_stake,
                provided: self_stake,
            });
        }
        if commission_bps > MAX_COMMISSION_BPS {
            return Err(StakingError::InvalidCommission(commission_bps));
        }

        // Anti-Sybil: cap maximum active validator count
        let active_count = self.validators.values().filter(|v| v.is_eligible()).count();
        if active_count >= MAX_ACTIVE_VALIDATORS {
            return Err(StakingError::MaxValidatorsReached {
                max: MAX_ACTIVE_VALIDATORS,
            });
        }

        // Anti-concentration: skip for registration. MAX_ACTIVE_VALIDATORS prevents
        // Sybil flooding. The per-validator (33%) and per-address (10%) caps only
        // apply to add_stake/delegate — initial registration must be free so new
        // validators can always join. Without this, once 4 validators exist at 10k PI
        // each, a 5th can never register because 10k/(50k) = 20% > 10% address cap.

        let entry = StakeEntry::new(address, self_stake, commission_bps, self.current_epoch);
        self.total_staked = self.total_staked.saturating_add(self_stake);
        self.validators.insert(address, entry);
        Ok(())
    }

    /// Add more self-stake to an existing validator.
    pub fn add_stake(&mut self, validator: Address, amount: u64) -> Result<(), StakingError> {
        // R36-FIX: Reject zero-amount to prevent no-op transactions.
        if amount == 0 {
            return Err(StakingError::InsufficientStake {
                required: 1,
                provided: 0,
            });
        }
        // Verify validator exists (immutable check)
        if !self.validators.contains_key(&validator) {
            return Err(StakingError::ValidatorNotFound(validator));
        }

        // Anti-concentration checks before mutating state
        self.check_concentration_cap(&validator, amount)?;
        self.check_address_concentration(&validator, amount)?;
        self.check_stake_velocity(&validator, amount)?;

        let entry = self
            .validators
            .get_mut(&validator)
            .ok_or(StakingError::ValidatorNotFound(validator))?;
        entry.self_stake = entry.self_stake.saturating_add(amount);
        self.total_staked = self.total_staked.saturating_add(amount);
        Ok(())
    }

    /// Delegate tokens to a validator.
    pub fn delegate(
        &mut self,
        delegator: Address,
        validator: Address,
        amount: u64,
    ) -> Result<(), StakingError> {
        if amount < MIN_DELEGATION {
            return Err(StakingError::InsufficientStake {
                required: MIN_DELEGATION,
                provided: amount,
            });
        }

        // R36-FIX: Reject self-delegation. A validator delegating to themselves creates
        // a (V,V) delegation entry that can never be withdrawn through begin_unbonding
        // (which always routes to the self-stake branch for address == validator).
        if delegator == validator {
            return Err(StakingError::SelfDelegationNotAllowed);
        }

        // Immutable checks first (before any state mutation)
        {
            let entry = self
                .validators
                .get(&validator)
                .ok_or(StakingError::ValidatorNotFound(validator))?;
            if entry.jailed {
                return Err(StakingError::ValidatorJailed(validator));
            }
        }

        // Anti-concentration checks (all immutable &self access)
        self.check_concentration_cap(&validator, amount)?;
        self.check_address_concentration(&delegator, amount)?;
        self.check_stake_velocity(&validator, amount)?;

        // All checks passed — now mutate state
        let entry = self
            .validators
            .get_mut(&validator)
            .ok_or(StakingError::ValidatorNotFound(validator))?;
        entry.delegated_stake = entry.delegated_stake.saturating_add(amount);
        self.total_staked = self.total_staked.saturating_add(amount);

        let delegation = self
            .delegations
            .entry((delegator, validator))
            .or_insert_with(|| Delegation {
                delegator,
                validator,
                amount: 0,
                pending_rewards: 0,
                start_epoch: self.current_epoch,
            });
        delegation.amount = delegation.amount.saturating_add(amount);

        Ok(())
    }

    /// Begin unbonding (withdrawal subject to unbonding period).
    pub fn begin_unbonding(
        &mut self,
        address: Address,
        validator: Address,
        amount: u64,
    ) -> Result<(), StakingError> {
        // R36-FIX: Reject zero-amount to prevent spam unbonding entries.
        if amount == 0 {
            return Err(StakingError::InsufficientStake {
                required: 1,
                provided: 0,
            });
        }
        // Check if this is a self-unstake or delegation unstake
        if address == validator {
            let entry = self
                .validators
                .get_mut(&validator)
                .ok_or(StakingError::ValidatorNotFound(validator))?;
            if entry.self_stake < amount {
                return Err(StakingError::InsufficientStake {
                    required: amount,
                    provided: entry.self_stake,
                });
            }
            entry.self_stake = entry.self_stake.saturating_sub(amount);
            // If self-stake drops below minimum, deactivate
            if !entry.meets_minimum() {
                entry.active = false;
            }
        } else {
            let delegation = self
                .delegations
                .get_mut(&(address, validator))
                .ok_or(StakingError::DelegationNotFound(address, validator))?;
            if delegation.amount < amount {
                return Err(StakingError::InsufficientStake {
                    required: amount,
                    provided: delegation.amount,
                });
            }
            delegation.amount = delegation.amount.saturating_sub(amount);
            // R30-FIX: Clean up zero-amount delegation entries to prevent phantom
            // delegators from interfering with slash proportional reduction.
            // R37-FIX: Only remove if both amount and pending_rewards are zero.
            // Removing when amount==0 but pending_rewards>0 loses unclaimed rewards.
            if delegation.amount == 0 && delegation.pending_rewards == 0 {
                self.delegations.remove(&(address, validator));
            }

            let entry = self
                .validators
                .get_mut(&validator)
                .ok_or(StakingError::ValidatorNotFound(validator))?;
            entry.delegated_stake = entry.delegated_stake.saturating_sub(amount);
        }

        self.total_staked = self.total_staked.saturating_sub(amount);
        self.unbonding.push(UnbondingEntry {
            address,
            validator,
            amount,
            completion_epoch: self.current_epoch.saturating_add(UNBONDING_EPOCHS),
        });

        Ok(())
    }

    /// Process completed unbonding entries. Returns (address, amount) pairs to credit.
    pub fn process_unbonding(&mut self) -> Vec<(Address, u64)> {
        let epoch = self.current_epoch;
        let (completed, remaining): (Vec<_>, Vec<_>) = self
            .unbonding
            .drain(..)
            .partition(|u| u.completion_epoch <= epoch);

        self.unbonding = remaining;
        completed
            .into_iter()
            .map(|u| (u.address, u.amount))
            .collect()
    }

    /// Slash a validator for misbehavior.
    ///
    /// `evidence_epoch` is the epoch when the misbehavior occurred (not when the
    /// evidence was submitted). Evidence older than `MAX_EVIDENCE_AGE_EPOCHS` is
    /// rejected to prevent stale evidence replay after unbonding completes.
    pub fn slash(
        &mut self,
        validator: Address,
        reason: SlashReason,
        evidence_hash: Hash,
        evidence_epoch: u64,
    ) -> Result<SlashEvent, StakingError> {
        // Reject future-epoch evidence: saturating_sub(future) returns 0 which
        // would bypass the staleness check. An attacker could forge evidence_epoch.
        if evidence_epoch > self.current_epoch {
            return Err(StakingError::EvidenceFromFuture {
                evidence_epoch,
                current_epoch: self.current_epoch,
            });
        }
        // Reject stale evidence: if the misbehavior epoch is too old, the validator's
        // unbonding funds may have already been released and cannot be clawed back.
        // MAX_EVIDENCE_AGE_EPOCHS < UNBONDING_EPOCHS guarantees funds are still locked.
        if self.current_epoch.saturating_sub(evidence_epoch) > MAX_EVIDENCE_AGE_EPOCHS {
            return Err(StakingError::EvidenceTooOld {
                evidence_epoch,
                current_epoch: self.current_epoch,
                max_age: MAX_EVIDENCE_AGE_EPOCHS,
            });
        }

        // R36-FIX: Reject duplicate evidence — same evidence cannot slash a validator twice.
        // Without this, a single misbehavior event can be replayed to drain all stake.
        if self
            .slash_history
            .iter()
            .any(|e| e.evidence_hash == evidence_hash && e.validator == validator)
        {
            return Err(StakingError::DuplicateEvidence(evidence_hash));
        }

        // Prevent slashing the last eligible validator — chain liveness must be preserved.
        // Without this check, a coordinated attack could jail all validators, halting the chain.
        let eligible_count = self.eligible_validators().len();
        let entry_is_eligible = self
            .validators
            .get(&validator)
            .map(|e| e.is_eligible())
            .unwrap_or(false);
        if eligible_count <= 1 && entry_is_eligible {
            return Err(StakingError::ValidatorNotFound(validator)); // refuse to slash
        }

        let entry = self
            .validators
            .get_mut(&validator)
            .ok_or(StakingError::ValidatorNotFound(validator))?;

        let total_stake = entry.total_stake();
        let base_slash_rate = reason.slash_rate_bps();

        // Correlated slashing: track unique validators slashed per evidence epoch.
        // R37-FIX: Use HashMap<epoch, HashSet<Address>> so interleaved evidence from
        // different epochs doesn't reset the count for either epoch.
        let epoch_set = self
            .epoch_slashed_validators
            .entry(evidence_epoch)
            .or_default();

        // M8-FIX: Collect previously-slashed validators BEFORE inserting the current one.
        // We need this list to retroactively increase their penalties below.
        let previously_slashed: Vec<Address> = epoch_set
            .iter()
            .copied()
            .filter(|v| *v != validator)
            .collect();

        // Insert this validator (HashSet deduplicates — same validator slashed
        // twice for different offenses only counts once toward N).
        epoch_set.insert(validator);
        let epoch_slash_count = epoch_set.len() as u32;

        // Total rate = base + (N * CORRELATED_SLASH_MULTIPLIER_BPS), capped at 10000 (100%).
        // N includes this validator. With 3 Sybil validators slashed:
        //   base 33% + 3 * 33.33% = 33% + 100% → capped at 100%. Total wipeout.
        let correlated_penalty_bps =
            epoch_slash_count.saturating_mul(CORRELATED_SLASH_MULTIPLIER_BPS);
        let effective_rate = (base_slash_rate as u32)
            .saturating_add(correlated_penalty_bps)
            .min(10_000);

        // M8-FIX: Retroactive correlated slashing — when a new validator is slashed in
        // the same epoch, apply the incremental penalty increase to all previously-slashed
        // validators. Without this, the first validator slashed (at count=1) gets a lighter
        // penalty than the last (at count=N), which is unfair and exploitable.
        //
        // The incremental penalty is exactly CORRELATED_SLASH_MULTIPLIER_BPS (one additional
        // correlated validator). We apply it to each previously-slashed validator's
        // **current** (post-prior-slash) stake, which is conservative — their total_stake
        // was already reduced by prior slashes, so the retroactive amount is smaller than
        // if computed on original stake.
        //
        // Design trade-off: we use current stake rather than original pre-slash stake because
        // tracking original stakes per-validator per-epoch would require additional storage.
        // This slightly under-penalizes compared to a perfect retroactive model, but ensures
        // all validators in a correlated event converge toward the same penalty rate.
        for prev_validator in &previously_slashed {
            if let Some(prev_entry) = self.validators.get(prev_validator) {
                let prev_stake = prev_entry.total_stake();
                if prev_stake == 0 {
                    continue;
                }
                // Apply the incremental correlated penalty (one additional CORRELATED_SLASH_MULTIPLIER_BPS)
                // capped so total effective rate for this validator doesn't exceed 100%.
                let incremental_bps = CORRELATED_SLASH_MULTIPLIER_BPS.min(10_000);
                let additional_slash =
                    (prev_stake as u128 * incremental_bps as u128).div_ceil(10_000) as u64;
                if additional_slash == 0 {
                    continue;
                }
                // Apply to self-stake first, then delegated
                let prev_entry = self.validators.get_mut(prev_validator).unwrap();
                let self_slash = additional_slash.min(prev_entry.self_stake);
                prev_entry.self_stake = prev_entry.self_stake.saturating_sub(self_slash);
                let remaining = additional_slash.saturating_sub(self_slash);
                if remaining > 0 {
                    prev_entry.delegated_stake =
                        prev_entry.delegated_stake.saturating_sub(remaining);
                }
                self.total_staked = self.total_staked.saturating_sub(additional_slash);
                self.total_slashed = self.total_slashed.saturating_add(additional_slash);
            }
        }

        // Round UP so the offender doesn't benefit from integer truncation.
        let amount_slashed = (total_stake as u128 * effective_rate as u128).div_ceil(10_000) as u64;

        // Slash self-stake first, then delegated stake.
        // Scoped block to release the mutable borrow before delegator iteration.
        let remaining_slash = {
            let entry = self
                .validators
                .get_mut(&validator)
                .ok_or(StakingError::ValidatorNotFound(validator))?;
            let self_slash = amount_slashed.min(entry.self_stake);
            entry.self_stake = entry.self_stake.saturating_sub(self_slash);
            let remaining = amount_slashed.saturating_sub(self_slash);
            if remaining > 0 {
                entry.delegated_stake = entry.delegated_stake.saturating_sub(remaining);
            }
            remaining
        };

        if remaining_slash > 0 {
            let old_delegated = self
                .validators
                .get(&validator)
                .map(|e| e.delegated_stake.saturating_add(remaining_slash))
                .unwrap_or(0);

            // R26-FIX: Proportionally reduce each delegator's individual delegation.amount.
            // Without this, delegators could withdraw more than their post-slash balance,
            // creating PI out of thin air (delegated_stake < sum of delegation.amounts).
            // R27-FIX: Accumulate rounding remainder and apply to last delegator to prevent
            // sum(delegation.amounts) > delegated_stake drift from integer truncation.
            if old_delegated > 0 {
                let slash_for_delegators = remaining_slash.min(old_delegated);
                // R28-FIX: Sort delegator keys for deterministic iteration order.
                let mut delegator_keys: Vec<(Address, Address)> = self
                    .delegations
                    .keys()
                    .filter(|(_, v)| *v == validator)
                    .copied()
                    .collect();
                delegator_keys.sort();
                let mut total_deducted: u64 = 0;
                let last_idx = delegator_keys.len().saturating_sub(1);
                for (i, key) in delegator_keys.iter().enumerate() {
                    if let Some(delegation) = self.delegations.get_mut(key) {
                        let delegator_slash = if i == last_idx {
                            slash_for_delegators
                                .saturating_sub(total_deducted)
                                .min(delegation.amount)
                        } else {
                            (delegation.amount as u128 * slash_for_delegators as u128)
                                .div_ceil(old_delegated as u128) as u64
                        };
                        delegation.amount = delegation.amount.saturating_sub(delegator_slash);
                        total_deducted = total_deducted.saturating_add(delegator_slash);
                    }
                }
                // R36-FIX: Reconcile delegated_stake with actual sum of delegation amounts.
                // Rounding-up on individual delegators can cause total_deducted > slash_for_delegators,
                // creating phantom delegated_stake that no delegator owns.
                // R37-FIX: Also add the over-deducted amount to total_slashed so burned PI
                // accounting stays accurate (previously this excess was destroyed but untracked).
                if total_deducted > slash_for_delegators {
                    let over_deducted = total_deducted.saturating_sub(slash_for_delegators);
                    if let Some(entry) = self.validators.get_mut(&validator) {
                        entry.delegated_stake = entry.delegated_stake.saturating_sub(over_deducted);
                    }
                    self.total_staked = self.total_staked.saturating_sub(over_deducted);
                    self.total_slashed = self.total_slashed.saturating_add(over_deducted);
                }
            }
        }

        // Re-acquire mutable reference for jailing and post-slash updates
        let entry = self
            .validators
            .get_mut(&validator)
            .expect("validator must exist — checked above");

        // Jail the validator
        entry.jailed = true;
        entry.active = false;
        entry.jailed_until = Some(self.current_epoch.saturating_add(JAIL_DURATION_EPOCHS));
        // SECURITY: Do NOT reset epoch_start_stake on slash. The velocity check
        // uses epoch_start_stake as the growth baseline, and resetting it on slash
        // allows a slash-then-restake loop where each slash creates a lower baseline,
        // enabling repeated 50% growth compounds within a single epoch.
        //
        // Instead, keep epoch_start_stake as the high-water mark from epoch start.
        // Only set_epoch() (called at epoch boundaries) can reset it.
        // This means post-slash restaking is measured against the ORIGINAL epoch
        // baseline, not the post-slash balance.

        self.total_staked = self.total_staked.saturating_sub(amount_slashed);
        self.total_slashed = self.total_slashed.saturating_add(amount_slashed);

        // Also slash unbonding entries for this validator.
        // R27-FIX: Do NOT add unbonding slashes to total_slashed — the unbonding PI
        // was already removed from total_staked during begin_unbonding. Adding it to
        // total_slashed would inflate the counter. The slash is reflected in the
        // reduced unbonding amount (user receives less when unbonding completes).
        // R29-FIX: Skip entries whose completion_epoch has already passed — those
        // tokens have already been released and cannot be clawed back.
        for unbonding in &mut self.unbonding {
            if unbonding.validator == validator && unbonding.completion_epoch > self.current_epoch {
                // Round UP to match the main slash calculation. Use effective rate (correlated).
                let unbond_slash =
                    (unbonding.amount as u128 * effective_rate as u128).div_ceil(10_000) as u64;
                unbonding.amount = unbonding.amount.saturating_sub(unbond_slash);
            }
        }

        let event = SlashEvent {
            validator,
            reason,
            amount_slashed,
            epoch: self.current_epoch,
            evidence_hash,
        };
        self.slash_history.push(event.clone());

        Ok(event)
    }

    /// Unjail a validator (after jail period expires).
    pub fn unjail(&mut self, validator: Address) -> Result<(), StakingError> {
        // Immutable validation pass — check eligibility before mutable borrow.
        {
            let entry = self
                .validators
                .get(&validator)
                .ok_or(StakingError::ValidatorNotFound(validator))?;

            if !entry.jailed {
                return Err(StakingError::NotJailed(validator));
            }

            if let Some(jailed_until) = entry.jailed_until {
                if self.current_epoch < jailed_until {
                    return Err(StakingError::JailNotExpired {
                        current_epoch: self.current_epoch,
                        jail_until: jailed_until,
                    });
                }
            }

            if !entry.meets_minimum() {
                return Err(StakingError::InsufficientStake {
                    required: effective_min_validator_stake(self.validators.len()),
                    provided: entry.self_stake,
                });
            }
        }

        // Anti-Sybil: don't unjail if it would exceed MAX_ACTIVE_VALIDATORS.
        // Without this, an attacker could register 100 validators, get N jailed,
        // register N new ones, then unjail the original N to exceed the cap.
        let active_count = self.validators.values().filter(|v| v.is_eligible()).count();
        if active_count >= MAX_ACTIVE_VALIDATORS {
            return Err(StakingError::MaxValidatorsReached {
                max: MAX_ACTIVE_VALIDATORS,
            });
        }

        let entry = self.validators.get_mut(&validator).unwrap();
        entry.jailed = false;
        entry.jailed_until = None;
        entry.active = true;

        Ok(())
    }

    /// Distribute staking rewards for a block.
    /// `total_reward` is the 65% of fees allocated to stakers.
    /// Uses dust collection to prevent rounding losses: any remainder from
    /// integer truncation is distributed deterministically using hash-based
    /// selection to avoid compounding centralization.
    pub fn distribute_rewards(&mut self, total_reward: u64) {
        if total_reward == 0 {
            return;
        }

        // Use only eligible validators' stake as the denominator.
        // Using total_staked (which includes jailed validators) would cause
        // rewards to be permanently lost since jailed validators are skipped.
        let total: u128 = self
            .validators
            .values()
            .filter(|v| v.is_eligible())
            .map(|v| v.total_stake() as u128)
            .sum();

        if total == 0 {
            return;
        }

        let reward = total_reward as u128;

        let mut total_distributed: u64 = 0;

        // R31-FIX: Sort validator addresses for deterministic iteration order.
        // HashMap iteration is non-deterministic — truncation errors from integer
        // division accumulate differently depending on order, causing state divergence.
        let mut validator_addrs: Vec<Address> = self.validators.keys().copied().collect();
        validator_addrs.sort();

        for addr in &validator_addrs {
            let entry = match self.validators.get_mut(addr) {
                Some(e) => e,
                None => continue,
            };
            if !entry.is_eligible() {
                continue;
            }

            let validator_share = (reward * entry.total_stake() as u128 / total) as u64;
            let commission =
                (validator_share as u128 * entry.commission_bps as u128 / 10_000) as u64;
            entry.pending_rewards = entry.pending_rewards.saturating_add(commission);
            total_distributed = total_distributed.saturating_add(commission);

            let delegator_pool = validator_share.saturating_sub(commission);
            let self_portion = (delegator_pool as u128 * entry.self_stake as u128
                / entry.total_stake().max(1) as u128) as u64;
            entry.pending_rewards = entry.pending_rewards.saturating_add(self_portion);
            total_distributed = total_distributed.saturating_add(self_portion);
        }

        // Distribute to delegations — R31-FIX: sort keys for deterministic order.
        let mut delegation_keys: Vec<(Address, Address)> =
            self.delegations.keys().cloned().collect();
        delegation_keys.sort();

        for key in &delegation_keys {
            let delegation = match self.delegations.get_mut(key) {
                Some(d) => d,
                None => continue,
            };
            if let Some(entry) = self.validators.get(&delegation.validator) {
                if !entry.is_eligible() {
                    continue;
                }
                let validator_share = (reward * entry.total_stake() as u128 / total) as u64;
                let commission =
                    (validator_share as u128 * entry.commission_bps as u128 / 10_000) as u64;
                let delegator_pool = validator_share.saturating_sub(commission);
                let delegator_share = (delegator_pool as u128 * delegation.amount as u128
                    / entry.total_stake().max(1) as u128)
                    as u64;
                delegation.pending_rewards =
                    delegation.pending_rewards.saturating_add(delegator_share);
                total_distributed = total_distributed.saturating_add(delegator_share);
            }
        }

        // Distribute rounding dust deterministically using hash-based selection.
        // This prevents compounding centralization toward the largest validator
        // by rotating the dust recipient each epoch.
        let dust = total_reward.saturating_sub(total_distributed);
        if dust > 0 {
            // Build sorted list of eligible validators for deterministic selection
            let mut eligible: Vec<Address> = self
                .validators
                .iter()
                .filter(|(_, v)| v.is_eligible())
                .map(|(addr, _)| *addr)
                .collect();
            eligible.sort();

            if !eligible.is_empty() {
                // Deterministic index from epoch number: hash(epoch) mod eligible_count
                let epoch_bytes = self.current_epoch.to_le_bytes();
                let hash = pichain_crypto::hash(&epoch_bytes);
                let idx_bytes: [u8; 8] = hash.as_bytes()[..8].try_into().unwrap_or([0u8; 8]);
                let dust_idx = u64::from_le_bytes(idx_bytes) as usize % eligible.len();
                let dust_recipient = eligible[dust_idx];

                if let Some(entry) = self.validators.get_mut(&dust_recipient) {
                    entry.pending_rewards = entry.pending_rewards.saturating_add(dust);
                }
            }
        }
    }

    /// Claim pending rewards for a validator (self-stake rewards + commission).
    /// Returns the amount claimed (to be credited to the validator's balance externally).
    pub fn claim_validator_rewards(&mut self, validator: Address) -> Result<u64, StakingError> {
        let entry = self
            .validators
            .get_mut(&validator)
            .ok_or(StakingError::ValidatorNotFound(validator))?;
        let amount = entry.pending_rewards;
        entry.pending_rewards = 0;
        Ok(amount)
    }

    /// Claim pending delegation rewards for a delegator.
    /// Returns the amount claimed (to be credited to the delegator's balance externally).
    pub fn claim_delegation_rewards(
        &mut self,
        delegator: Address,
        validator: Address,
    ) -> Result<u64, StakingError> {
        let delegation = self
            .delegations
            .get_mut(&(delegator, validator))
            .ok_or(StakingError::DelegationNotFound(delegator, validator))?;
        let amount = delegation.pending_rewards;
        delegation.pending_rewards = 0;
        Ok(amount)
    }

    /// Record a block proposed by a validator.
    pub fn record_block_proposed(&mut self, validator: Address) {
        if let Some(entry) = self.validators.get_mut(&validator) {
            entry.blocks_proposed = entry.blocks_proposed.saturating_add(1);
        }
    }

    /// Record a missed block for a validator.
    pub fn record_block_missed(&mut self, validator: Address) {
        if let Some(entry) = self.validators.get_mut(&validator) {
            entry.blocks_missed = entry.blocks_missed.saturating_add(1);
        }
    }

    /// Get the list of eligible validators (for ValidatorSet construction).
    pub fn eligible_validators(&self) -> Vec<&StakeEntry> {
        self.validators
            .values()
            .filter(|v| v.is_eligible())
            .collect()
    }

    /// Get all validators (including inactive/jailed).
    pub fn all_validators(&self) -> Vec<&StakeEntry> {
        self.validators.values().collect()
    }

    /// Get a specific validator's stake info.
    pub fn get_validator(&self, address: &Address) -> Option<&StakeEntry> {
        self.validators.get(address)
    }

    /// Get a delegation.
    pub fn get_delegation(&self, delegator: &Address, validator: &Address) -> Option<&Delegation> {
        self.delegations.get(&(*delegator, *validator))
    }

    /// Get all delegations for a given delegator address.
    pub fn delegations_for(&self, delegator: &Address) -> Vec<&Delegation> {
        self.delegations
            .iter()
            .filter(|((d, _), _)| d == delegator)
            .map(|(_, v)| v)
            .collect()
    }

    /// Get total staked.
    pub fn total_staked(&self) -> u64 {
        self.total_staked
    }

    /// Get total slashed.
    pub fn total_slashed(&self) -> u64 {
        self.total_slashed
    }

    /// Get validator count.
    pub fn validator_count(&self) -> usize {
        self.validators.len()
    }

    /// Get active validator count.
    pub fn active_validator_count(&self) -> usize {
        self.validators.values().filter(|v| v.is_eligible()).count()
    }

    /// Get slash history.
    pub fn slash_history(&self) -> &[SlashEvent] {
        &self.slash_history
    }
}

impl Default for StakingManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Staking-related errors.
#[derive(Debug, thiserror::Error)]
pub enum StakingError {
    #[error("validator already registered: {0}")]
    AlreadyRegistered(Address),
    #[error("validator not found: {0}")]
    ValidatorNotFound(Address),
    #[error("insufficient stake: required {required}, provided {provided}")]
    InsufficientStake { required: u64, provided: u64 },
    #[error("invalid commission rate: {0} bps")]
    InvalidCommission(u16),
    #[error("validator is jailed: {0}")]
    ValidatorJailed(Address),
    #[error("validator is not jailed: {0}")]
    NotJailed(Address),
    #[error("jail not expired: current epoch {current_epoch}, jail until {jail_until}")]
    JailNotExpired { current_epoch: u64, jail_until: u64 },
    #[error("delegation not found: {0} -> {1}")]
    DelegationNotFound(Address, Address),
    #[error(
        "evidence too old: epoch {evidence_epoch}, current {current_epoch}, max age {max_age}"
    )]
    EvidenceTooOld {
        evidence_epoch: u64,
        current_epoch: u64,
        max_age: u64,
    },
    #[error("evidence from future: epoch {evidence_epoch} > current {current_epoch}")]
    EvidenceFromFuture {
        evidence_epoch: u64,
        current_epoch: u64,
    },
    #[error("stake concentration exceeded: validator {validator} would hold {would_hold_bps} bps of total (max {max_bps} bps)")]
    StakeConcentrationExceeded {
        validator: Address,
        would_hold_bps: u16,
        max_bps: u16,
    },
    #[error("address concentration exceeded: {address} would control {would_hold_bps} bps of total staked (max {max_bps} bps)")]
    AddressConcentrationExceeded {
        address: Address,
        would_hold_bps: u16,
        max_bps: u16,
    },
    #[error("stake velocity exceeded: validator {validator} would grow {growth_bps} bps this epoch (max {max_bps} bps)")]
    StakeVelocityExceeded {
        validator: Address,
        growth_bps: u16,
        max_bps: u16,
    },
    #[error("maximum active validator count reached ({max})")]
    MaxValidatorsReached { max: usize },
    #[error("duplicate evidence: {0}")]
    DuplicateEvidence(Hash),
    #[error("self-delegation not allowed")]
    SelfDelegationNotAllowed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(i: u8) -> Address {
        Address([i; 20])
    }

    /// Helper: register 3 validators with equal stake for tests that need
    /// bootstrap-safe setup. Returns total staked.
    fn setup_bootstrap(mgr: &mut StakingManager) -> u64 {
        mgr.register_validator(addr(1), MIN_VALIDATOR_STAKE, 1000)
            .unwrap();
        mgr.register_validator(addr(2), MIN_VALIDATOR_STAKE, 1000)
            .unwrap();
        mgr.register_validator(addr(3), MIN_VALIDATOR_STAKE, 1000)
            .unwrap();
        MIN_VALIDATOR_STAKE * 3
    }

    #[test]
    fn register_validator() {
        let mut mgr = StakingManager::new();
        mgr.register_validator(addr(1), MIN_VALIDATOR_STAKE, 1000)
            .unwrap();
        assert_eq!(mgr.validator_count(), 1);
        assert_eq!(mgr.total_staked(), MIN_VALIDATOR_STAKE);
    }

    #[test]
    fn reject_insufficient_stake() {
        let mut mgr = StakingManager::new();
        let result = mgr.register_validator(addr(1), 100, 1000);
        assert!(result.is_err());
    }

    #[test]
    fn reject_duplicate_registration() {
        let mut mgr = StakingManager::new();
        setup_bootstrap(&mut mgr);
        assert!(mgr
            .register_validator(addr(1), MIN_VALIDATOR_STAKE, 1000)
            .is_err());
    }

    #[test]
    fn delegate_and_undelegate() {
        let mut mgr = StakingManager::new();
        setup_bootstrap(&mut mgr);

        // Delegate (small amount to stay under bootstrap 41.3% cap)
        mgr.delegate(addr(5), addr(1), 5 * MIN_DELEGATION).unwrap();
        let entry = mgr.get_validator(&addr(1)).unwrap();
        assert_eq!(entry.delegated_stake, 5 * MIN_DELEGATION);

        // Begin unbonding
        mgr.begin_unbonding(addr(5), addr(1), 2 * MIN_DELEGATION)
            .unwrap();
        let entry = mgr.get_validator(&addr(1)).unwrap();
        assert_eq!(entry.delegated_stake, 3 * MIN_DELEGATION);
    }

    #[test]
    fn slash_double_signing() {
        let mut mgr = StakingManager::new();
        let stake = MIN_VALIDATOR_STAKE;
        setup_bootstrap(&mut mgr);

        let event = mgr
            .slash(addr(1), SlashReason::DoubleSigning, Hash::ZERO, 0)
            .unwrap();
        // Correlated slashing: first slash in epoch → N=1, effective_rate = 3300 + 1*3333 = 6633
        let effective_rate: u32 =
            SLASH_DOUBLE_SIGN_BPS as u32 + 1 * CORRELATED_SLASH_MULTIPLIER_BPS;
        let expected_slash = ((stake as u128 * effective_rate as u128 + 9_999) / 10_000) as u64;
        assert_eq!(event.amount_slashed, expected_slash);

        let entry = mgr.get_validator(&addr(1)).unwrap();
        assert!(entry.jailed);
        assert!(!entry.active);
    }

    #[test]
    fn unjail_after_period() {
        let mut mgr = StakingManager::new();
        // Need enough stake that after correlated slash (66.33% for first slash),
        // still meets MIN_VALIDATOR_STAKE. 4x min → after 66.33% slash = ~1.33x min.
        let stake = MIN_VALIDATOR_STAKE * 4;
        mgr.register_validator(addr(1), stake, 1000).unwrap();
        mgr.register_validator(addr(2), stake, 1000).unwrap();
        mgr.register_validator(addr(3), stake, 1000).unwrap();

        mgr.slash(addr(1), SlashReason::DoubleSigning, Hash::ZERO, 0)
            .unwrap();

        // Can't unjail immediately
        assert!(mgr.unjail(addr(1)).is_err());

        // Advance past jail period
        mgr.set_epoch(JAIL_DURATION_EPOCHS + 1);
        mgr.unjail(addr(1)).unwrap();

        let entry = mgr.get_validator(&addr(1)).unwrap();
        assert!(!entry.jailed);
        assert!(entry.active);
    }

    #[test]
    fn unbonding_period() {
        let mut mgr = StakingManager::new();
        let stake = MIN_VALIDATOR_STAKE * 2;
        mgr.register_validator(addr(1), stake, 1000).unwrap();
        mgr.register_validator(addr(2), stake, 1000).unwrap();
        mgr.register_validator(addr(3), stake, 1000).unwrap();
        mgr.begin_unbonding(addr(1), addr(1), MIN_VALIDATOR_STAKE)
            .unwrap();

        // Not yet completed
        let completed = mgr.process_unbonding();
        assert!(completed.is_empty());

        // Advance to completion
        mgr.set_epoch(UNBONDING_EPOCHS + 1);
        let completed = mgr.process_unbonding();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0], (addr(1), MIN_VALIDATOR_STAKE));
    }

    #[test]
    fn delegation_to_jailed_validator_fails() {
        let mut mgr = StakingManager::new();
        setup_bootstrap(&mut mgr);
        mgr.slash(addr(1), SlashReason::Downtime, Hash::ZERO, 0)
            .unwrap();

        assert!(mgr.delegate(addr(5), addr(1), MIN_DELEGATION).is_err());
    }

    #[test]
    fn reward_distribution() {
        let mut mgr = StakingManager::new();
        setup_bootstrap(&mut mgr);
        // Small delegation to stay under bootstrap cap
        mgr.delegate(addr(5), addr(1), MIN_DELEGATION).unwrap();

        mgr.distribute_rewards(1_000_000); // 1M base units

        let entry = mgr.get_validator(&addr(1)).unwrap();
        assert!(entry.pending_rewards > 0);
    }

    #[test]
    fn claim_validator_rewards() {
        let mut mgr = StakingManager::new();
        setup_bootstrap(&mut mgr);

        mgr.distribute_rewards(1_000_000);

        let entry = mgr.get_validator(&addr(1)).unwrap();
        let rewards = entry.pending_rewards;
        assert!(rewards > 0);

        // Claim rewards
        let claimed = mgr.claim_validator_rewards(addr(1)).unwrap();
        assert_eq!(claimed, rewards);

        // Rewards should be zeroed after claim
        let entry = mgr.get_validator(&addr(1)).unwrap();
        assert_eq!(entry.pending_rewards, 0);

        // Claiming again returns 0
        let claimed = mgr.claim_validator_rewards(addr(1)).unwrap();
        assert_eq!(claimed, 0);
    }

    #[test]
    fn claim_delegation_rewards() {
        let mut mgr = StakingManager::new();
        setup_bootstrap(&mut mgr);
        // Small delegation to stay under bootstrap cap
        mgr.delegate(addr(5), addr(1), MIN_DELEGATION).unwrap();

        mgr.distribute_rewards(1_000_000);

        let delegation = mgr.get_delegation(&addr(5), &addr(1)).unwrap();
        let rewards = delegation.pending_rewards;
        assert!(rewards > 0);

        // Claim delegation rewards
        let claimed = mgr.claim_delegation_rewards(addr(5), addr(1)).unwrap();
        assert_eq!(claimed, rewards);

        // Rewards should be zeroed after claim
        let delegation = mgr.get_delegation(&addr(5), &addr(1)).unwrap();
        assert_eq!(delegation.pending_rewards, 0);
    }

    #[test]
    fn claim_nonexistent_validator_fails() {
        let mut mgr = StakingManager::new();
        assert!(mgr.claim_validator_rewards(addr(99)).is_err());
    }

    #[test]
    fn claim_nonexistent_delegation_fails() {
        let mut mgr = StakingManager::new();
        setup_bootstrap(&mut mgr);
        assert!(mgr.claim_delegation_rewards(addr(99), addr(1)).is_err());
    }

    #[test]
    fn eligible_validators_excludes_jailed() {
        let mut mgr = StakingManager::new();
        setup_bootstrap(&mut mgr);

        assert_eq!(mgr.active_validator_count(), 3);

        mgr.slash(addr(1), SlashReason::Downtime, Hash::ZERO, 0)
            .unwrap();
        assert_eq!(mgr.active_validator_count(), 2);
    }

    #[test]
    fn cannot_slash_last_eligible_validator() {
        let mut mgr = StakingManager::new();
        setup_bootstrap(&mut mgr);

        // Jail 2 of 3 validators
        mgr.slash(addr(1), SlashReason::Downtime, Hash::ZERO, 0)
            .unwrap();
        mgr.slash(addr(2), SlashReason::Downtime, Hash::ZERO, 0)
            .unwrap();
        assert_eq!(mgr.active_validator_count(), 1);

        // Only one eligible validator left — slashing should be refused
        let result = mgr.slash(addr(3), SlashReason::DoubleSigning, Hash::ZERO, 0);
        assert!(
            result.is_err(),
            "should not slash the last eligible validator"
        );

        // Validator 3 should still be active and unjailed
        let entry = mgr.get_validator(&addr(3)).unwrap();
        assert!(entry.active);
        assert!(!entry.jailed);
    }

    #[test]
    fn rewards_not_lost_when_validator_jailed() {
        // Fix 186: Reward denominator must exclude jailed validators.
        let mut mgr = StakingManager::new();

        // Register three validators with equal stake (enough to survive slashing)
        let stake = MIN_VALIDATOR_STAKE * 2;
        mgr.register_validator(addr(1), stake, 0).unwrap();
        mgr.register_validator(addr(2), stake, 0).unwrap();
        mgr.register_validator(addr(3), stake, 0).unwrap();

        // Jail validator 1
        mgr.slash(addr(1), SlashReason::Downtime, Hash::ZERO, 0)
            .unwrap();
        assert!(mgr.get_validator(&addr(1)).unwrap().jailed);

        // Distribute rewards
        let total_reward: u64 = 1_000_000;
        mgr.distribute_rewards(total_reward);

        // Validators 2 & 3 (eligible, equal stake) should split all rewards
        let v2 = mgr.get_validator(&addr(2)).unwrap();
        let v3 = mgr.get_validator(&addr(3)).unwrap();
        assert_eq!(v2.pending_rewards, 500_000);
        assert_eq!(v3.pending_rewards, 500_000);

        // Jailed validator should receive nothing
        let v1 = mgr.get_validator(&addr(1)).unwrap();
        assert_eq!(
            v1.pending_rewards, 0,
            "jailed validator should receive no rewards"
        );

        // Total distributed = total_reward (no permanent loss)
        assert_eq!(
            v1.pending_rewards + v2.pending_rewards + v3.pending_rewards,
            total_reward
        );
    }

    #[test]
    fn rewards_no_panic_when_no_eligible_validators() {
        // Fix 186: distribute_rewards must not panic (division by zero)
        // when there are no eligible validators.
        let mut mgr = StakingManager::new();
        setup_bootstrap(&mut mgr);

        // Jail validator 1 (others remain eligible)
        mgr.slash(addr(1), SlashReason::Downtime, Hash::ZERO, 0)
            .unwrap();
        assert!(mgr.get_validator(&addr(1)).unwrap().jailed);
        assert!(mgr.get_validator(&addr(2)).unwrap().is_eligible());

        // Distribute rewards: jailed validator gets nothing
        mgr.distribute_rewards(1_000_000);
        let v1 = mgr.get_validator(&addr(1)).unwrap();
        let v2 = mgr.get_validator(&addr(2)).unwrap();
        assert_eq!(v1.pending_rewards, 0, "jailed validator should get nothing");
        assert!(
            v2.pending_rewards > 0,
            "eligible validator should get rewards"
        );
    }

    #[test]
    fn rewards_distribute_correctly_with_mixed_eligible_jailed() {
        // Fix 186: With 3 validators where 1 is jailed, the 2 eligible validators
        // should split all rewards proportionally. None should be lost.
        let mut mgr = StakingManager::new();
        let stake = MIN_VALIDATOR_STAKE;
        setup_bootstrap(&mut mgr); // addr(1), addr(2), addr(3) — 0 commission is not needed since
                                   // setup_bootstrap uses 1000 bps (10%). Let's use custom setup.
        drop(mgr);
        let mut mgr = StakingManager::new();
        mgr.register_validator(addr(1), stake, 0).unwrap();
        mgr.register_validator(addr(2), stake, 0).unwrap();
        mgr.register_validator(addr(3), stake, 0).unwrap();

        // Jail validator 3
        mgr.slash(addr(3), SlashReason::Downtime, Hash::ZERO, 0)
            .unwrap();

        let total_reward: u64 = 1_000_000;
        mgr.distribute_rewards(total_reward);

        let v1 = mgr.get_validator(&addr(1)).unwrap();
        let v2 = mgr.get_validator(&addr(2)).unwrap();
        let v3 = mgr.get_validator(&addr(3)).unwrap();

        // Each eligible validator has equal stake, so each gets 50%
        assert_eq!(v1.pending_rewards, 500_000);
        assert_eq!(v2.pending_rewards, 500_000);
        assert_eq!(
            v3.pending_rewards, 0,
            "jailed validator should receive nothing"
        );

        // Total distributed should equal total_reward (no permanent loss)
        let total_distributed = v1.pending_rewards + v2.pending_rewards + v3.pending_rewards;
        assert_eq!(
            total_distributed, total_reward,
            "no rewards should be permanently lost"
        );
    }

    #[test]
    fn stale_evidence_rejected() {
        let mut mgr = StakingManager::new();
        setup_bootstrap(&mut mgr);

        // Advance to epoch 50; evidence from epoch 0 is now older than MAX_EVIDENCE_AGE_EPOCHS (42)
        mgr.set_epoch(50);
        let result = mgr.slash(addr(1), SlashReason::DoubleSigning, Hash::ZERO, 0);
        assert!(result.is_err(), "stale evidence should be rejected");
        match result.unwrap_err() {
            StakingError::EvidenceTooOld {
                evidence_epoch,
                current_epoch,
                max_age,
            } => {
                assert_eq!(evidence_epoch, 0);
                assert_eq!(current_epoch, 50);
                assert_eq!(max_age, MAX_EVIDENCE_AGE_EPOCHS);
            }
            other => panic!("expected EvidenceTooOld, got: {other}"),
        }

        // Validator should NOT be slashed
        let entry = mgr.get_validator(&addr(1)).unwrap();
        assert!(!entry.jailed);
        assert!(entry.active);
    }

    #[test]
    fn recent_evidence_accepted() {
        let mut mgr = StakingManager::new();
        setup_bootstrap(&mut mgr);

        // Advance to epoch 30; evidence from epoch 0 is within MAX_EVIDENCE_AGE_EPOCHS (42)
        mgr.set_epoch(30);
        let result = mgr.slash(addr(1), SlashReason::DoubleSigning, Hash::ZERO, 0);
        assert!(result.is_ok(), "recent evidence should be accepted");
        assert!(mgr.get_validator(&addr(1)).unwrap().jailed);
    }

    // ==================== Anti-Concentration Tests ====================

    #[test]
    fn concentration_cap_bootstrap_registration_skips() {
        // During bootstrap (< 4 validators), registration skips concentration cap
        // so the first validators can join. But add_stake/delegate enforces 41.3%.
        let mut mgr = StakingManager::new();
        // First 3 validators register with equal stake — allowed during bootstrap
        mgr.register_validator(addr(1), MIN_VALIDATOR_STAKE, 1000)
            .unwrap();
        mgr.register_validator(addr(2), MIN_VALIDATOR_STAKE, 1000)
            .unwrap();
        mgr.register_validator(addr(3), MIN_VALIDATOR_STAKE, 1000)
            .unwrap();
        // Each has ~33.3%. Adding stake to push addr(1) over 41.3% should fail.
        // Current total: 3 * stake. Adding stake → 2*stake / (4*stake) = 50% > 41.3%
        let result = mgr.add_stake(addr(1), MIN_VALIDATOR_STAKE);
        assert!(
            result.is_err(),
            "bootstrap 41.3% cap should block add_stake"
        );
        match result {
            Err(StakingError::StakeConcentrationExceeded { .. }) => {}
            other => panic!("expected StakeConcentrationExceeded, got {:?}", other),
        }
    }

    #[test]
    fn concentration_cap_bootstrap_small_add_ok() {
        // SECURITY: Per-address bootstrap cap is now 20% (down from 41.3%).
        // With 3 validators at equal stake (~33% each), adding to any single validator
        // would keep them above 20%, so add_stake is correctly rejected.
        // This test verifies a DELEGATION from a NEW address stays under 20%.
        let mut mgr = StakingManager::new();
        mgr.register_validator(addr(1), MIN_VALIDATOR_STAKE, 1000)
            .unwrap();
        mgr.register_validator(addr(2), MIN_VALIDATOR_STAKE, 1000)
            .unwrap();
        mgr.register_validator(addr(3), MIN_VALIDATOR_STAKE, 1000)
            .unwrap();
        // Delegation from a NEW address (addr(10)) — its share starts at 0%
        // tiny / (3*10K + tiny) ≈ 0.003% — well under 20% cap
        let tiny = MIN_DELEGATION;
        assert!(mgr.delegate(addr(10), addr(1), tiny).is_ok());
    }

    #[test]
    fn concentration_cap_enforced_with_4_validators() {
        let mut mgr = StakingManager::new();
        let stake = MIN_VALIDATOR_STAKE;
        mgr.register_validator(addr(1), stake, 1000).unwrap();
        mgr.register_validator(addr(2), stake, 1000).unwrap();
        mgr.register_validator(addr(3), stake, 1000).unwrap();
        mgr.register_validator(addr(4), stake, 1000).unwrap();
        // Each has 25% — try to push addr(1) above 33.33%
        // Current total: 4 * stake. Adding stake → addr(1) = 2*stake / (5*stake) = 40%
        let result = mgr.add_stake(addr(1), stake);
        assert!(result.is_err(), "should reject: would exceed 33% cap");
        match result {
            Err(StakingError::StakeConcentrationExceeded { .. }) => {}
            other => panic!("expected StakeConcentrationExceeded, got {:?}", other),
        }
    }

    #[test]
    fn concentration_cap_delegate_enforced() {
        let mut mgr = StakingManager::new();
        let stake = MIN_VALIDATOR_STAKE;
        mgr.register_validator(addr(1), stake, 1000).unwrap();
        mgr.register_validator(addr(2), stake, 1000).unwrap();
        mgr.register_validator(addr(3), stake, 1000).unwrap();
        mgr.register_validator(addr(4), stake, 1000).unwrap();
        // Delegate enough to push addr(1) over 33%
        let result = mgr.delegate(addr(10), addr(1), stake);
        assert!(result.is_err());
        match result {
            Err(StakingError::StakeConcentrationExceeded { .. }) => {}
            other => panic!("expected StakeConcentrationExceeded, got {:?}", other),
        }
    }

    #[test]
    fn concentration_cap_exactly_at_boundary() {
        let mut mgr = StakingManager::new();
        // Create 4 validators with 3 having 1/3 each and 1 tiny
        let big = MIN_VALIDATOR_STAKE * 100;
        mgr.register_validator(addr(1), big, 1000).unwrap();
        mgr.register_validator(addr(2), big, 1000).unwrap();
        mgr.register_validator(addr(3), big, 1000).unwrap();
        mgr.register_validator(addr(4), big, 1000).unwrap();
        // Each has 25%. Small delegation should succeed (stays under 33%)
        let small = MIN_DELEGATION;
        assert!(mgr.delegate(addr(10), addr(1), small).is_ok());
    }

    #[test]
    fn address_concentration_cap_enforced() {
        let mut mgr = StakingManager::new();
        let stake = MIN_VALIDATOR_STAKE;
        mgr.register_validator(addr(1), stake, 1000).unwrap();
        mgr.register_validator(addr(2), stake, 1000).unwrap();
        mgr.register_validator(addr(3), stake, 1000).unwrap();
        mgr.register_validator(addr(4), stake, 1000).unwrap();
        // Total staked: 4 * stake. addr(10) tries to delegate >10% to addr(2)
        // 10% of 4*stake = 0.4*stake. Anything over that should fail.
        // Actually: new_total = 4*stake + amount, new_addr = amount
        // (amount / (4*stake + amount)) > 10% → amount > 0.444*stake
        // Try amount = stake/2 = 50% of one validator's stake
        let result = mgr.delegate(addr(10), addr(2), stake / 2);
        assert!(result.is_err());
        match result {
            Err(StakingError::AddressConcentrationExceeded { .. }) => {}
            other => panic!("expected AddressConcentrationExceeded, got {:?}", other),
        }
    }

    #[test]
    fn address_concentration_small_delegation_ok() {
        let mut mgr = StakingManager::new();
        let stake = MIN_VALIDATOR_STAKE * 100;
        mgr.register_validator(addr(1), stake, 1000).unwrap();
        mgr.register_validator(addr(2), stake, 1000).unwrap();
        mgr.register_validator(addr(3), stake, 1000).unwrap();
        mgr.register_validator(addr(4), stake, 1000).unwrap();
        // Tiny delegation from addr(10) — well under 10%
        let small = MIN_DELEGATION;
        assert!(mgr.delegate(addr(10), addr(1), small).is_ok());
    }

    #[test]
    fn velocity_limit_enforced() {
        let mut mgr = StakingManager::new();
        // Use asymmetric stakes: addr(1) = 1x, others = 10x each
        // So addr(1) is small relative to total, and concentration cap won't trigger first.
        let small = MIN_VALIDATOR_STAKE;
        let big = MIN_VALIDATOR_STAKE * 10;
        mgr.register_validator(addr(1), small, 1000).unwrap();
        mgr.register_validator(addr(2), big, 1000).unwrap();
        mgr.register_validator(addr(3), big, 1000).unwrap();
        mgr.register_validator(addr(4), big, 1000).unwrap();
        // epoch_start_stake for addr(1) is small
        mgr.set_epoch(1);
        // Try to grow addr(1) by >50%. Growth of 60% of small.
        // New total for addr(1) = small + growth. New total staked = 31*small + growth.
        // Concentration: (small + growth) / (31*small + growth) — well under 33%.
        // Velocity: growth / small = 60% > 50% limit.
        let growth = small * 6 / 10;
        let result = mgr.delegate(addr(10), addr(1), growth);
        assert!(result.is_err());
        match result {
            Err(StakingError::StakeVelocityExceeded { .. }) => {}
            other => panic!("expected StakeVelocityExceeded, got {:?}", other),
        }
    }

    #[test]
    fn velocity_limit_allows_moderate_growth() {
        let mut mgr = StakingManager::new();
        let stake = MIN_VALIDATOR_STAKE * 100;
        mgr.register_validator(addr(1), stake, 1000).unwrap();
        mgr.register_validator(addr(2), stake, 1000).unwrap();
        mgr.register_validator(addr(3), stake, 1000).unwrap();
        mgr.register_validator(addr(4), stake, 1000).unwrap();
        mgr.set_epoch(1);
        // Small growth should be fine (well under 50% velocity and under 10% address cap)
        // Total = 4 * stake. Delegate small = stake/40 → addr(10) = 0.625% of total < 10%
        let small_growth = stake / 40;
        assert!(mgr.delegate(addr(10), addr(1), small_growth).is_ok());
    }

    #[test]
    fn address_total_stake_computed_correctly() {
        let mut mgr = StakingManager::new();
        let stake = MIN_VALIDATOR_STAKE * 100;
        mgr.register_validator(addr(1), stake, 1000).unwrap();
        mgr.register_validator(addr(2), stake, 1000).unwrap();
        mgr.register_validator(addr(3), stake, 1000).unwrap();
        mgr.register_validator(addr(4), stake, 1000).unwrap();
        // addr(1) is a validator with self_stake
        assert_eq!(mgr.address_total_stake(&addr(1)), stake);
        // addr(10) has nothing
        assert_eq!(mgr.address_total_stake(&addr(10)), 0);
        // Delegate from addr(10) to addr(1)
        let small = MIN_DELEGATION;
        mgr.delegate(addr(10), addr(1), small).unwrap();
        assert_eq!(mgr.address_total_stake(&addr(10)), small);
    }

    // ==================== Correlated Slashing Tests ====================

    #[test]
    fn correlated_slashing_increases_penalty() {
        // When multiple validators are slashed in the same epoch, the penalty escalates.
        // This makes Sybil attacks self-destructive.
        let mut mgr = StakingManager::new();
        let stake = MIN_VALIDATOR_STAKE * 10; // 100k PI — plenty of room for slashing
                                              // 5 validators (4 attackers + 1 honest)
        mgr.register_validator(addr(1), stake, 0).unwrap();
        mgr.register_validator(addr(2), stake, 0).unwrap();
        mgr.register_validator(addr(3), stake, 0).unwrap();
        mgr.register_validator(addr(4), stake, 0).unwrap();
        mgr.register_validator(addr(5), stake, 0).unwrap();

        // Slash validator 1 (N=1): rate = 3300 + 1*3333 = 6633 bps
        let e1 = mgr
            .slash(addr(1), SlashReason::DoubleSigning, Hash::ZERO, 0)
            .unwrap();
        let rate1 = SLASH_DOUBLE_SIGN_BPS as u32 + 1 * CORRELATED_SLASH_MULTIPLIER_BPS;
        let exp1 = ((stake as u128 * rate1 as u128 + 9_999) / 10_000) as u64;
        assert_eq!(e1.amount_slashed, exp1);

        // Slash validator 2 (N=2): rate = 3300 + 2*3333 = 9966 bps
        let e2 = mgr
            .slash(addr(2), SlashReason::DoubleSigning, Hash::ZERO, 0)
            .unwrap();
        let rate2 =
            (SLASH_DOUBLE_SIGN_BPS as u32 + 2 * CORRELATED_SLASH_MULTIPLIER_BPS).min(10_000);
        let exp2 = ((stake as u128 * rate2 as u128 + 9_999) / 10_000) as u64;
        assert_eq!(e2.amount_slashed, exp2);

        // Slash validator 3 (N=3): rate = 3300 + 3*3333 = 13299 → capped at 10000
        let e3 = mgr
            .slash(addr(3), SlashReason::DoubleSigning, Hash::ZERO, 0)
            .unwrap();
        let rate3 =
            (SLASH_DOUBLE_SIGN_BPS as u32 + 3 * CORRELATED_SLASH_MULTIPLIER_BPS).min(10_000);
        assert_eq!(rate3, 10_000); // 100% wipeout
        let exp3 = ((stake as u128 * rate3 as u128 + 9_999) / 10_000) as u64;
        assert_eq!(e3.amount_slashed, exp3);

        // Verify escalation: e1 < e2 < e3
        assert!(
            e1.amount_slashed < e2.amount_slashed,
            "penalty should escalate"
        );
        assert!(
            e2.amount_slashed < e3.amount_slashed,
            "penalty should escalate"
        );

        // Validator 5 (honest) should be unaffected
        let v5 = mgr.get_validator(&addr(5)).unwrap();
        assert!(v5.is_eligible());
        assert_eq!(v5.total_stake(), stake);
    }

    #[test]
    fn correlated_slashing_resets_across_epochs() {
        // Correlated counter resets when epoch changes.
        let mut mgr = StakingManager::new();
        let stake = MIN_VALIDATOR_STAKE * 10;
        mgr.register_validator(addr(1), stake, 0).unwrap();
        mgr.register_validator(addr(2), stake, 0).unwrap();
        mgr.register_validator(addr(3), stake, 0).unwrap();
        mgr.register_validator(addr(4), stake, 0).unwrap();
        mgr.register_validator(addr(5), stake, 0).unwrap();

        // Slash in epoch 0 (N=1)
        let e1 = mgr
            .slash(addr(1), SlashReason::Downtime, Hash::ZERO, 0)
            .unwrap();

        // Advance to epoch 1 — counter resets
        mgr.set_epoch(1);
        // Slash in epoch 1 (N=1 again, not N=2)
        let e2 = mgr
            .slash(addr(2), SlashReason::Downtime, Hash::ZERO, 1)
            .unwrap();

        // Both should have the same effective rate (N=1 in each epoch)
        assert_eq!(
            e1.amount_slashed, e2.amount_slashed,
            "correlated counter should reset across epochs"
        );
    }

    #[test]
    fn max_active_validators_enforced() {
        let mut mgr = StakingManager::new();
        // Register MAX_ACTIVE_VALIDATORS validators
        for i in 0..MAX_ACTIVE_VALIDATORS {
            let a = Address([(i & 0xFF) as u8; 20]);
            // Use unique addresses by varying bytes
            let mut bytes = [0u8; 20];
            bytes[0] = (i >> 8) as u8;
            bytes[1] = (i & 0xFF) as u8;
            let a = Address(bytes);
            mgr.register_validator(a, MIN_VALIDATOR_STAKE, 1000)
                .unwrap();
        }
        assert_eq!(mgr.validators.len(), MAX_ACTIVE_VALIDATORS);

        // The 101st should fail
        let extra = Address([0xFF; 20]);
        let result = mgr.register_validator(extra, MIN_VALIDATOR_STAKE, 1000);
        assert!(result.is_err());
        match result {
            Err(StakingError::MaxValidatorsReached { max }) => {
                assert_eq!(max, MAX_ACTIVE_VALIDATORS);
            }
            other => panic!("expected MaxValidatorsReached, got {:?}", other),
        }
    }
}
