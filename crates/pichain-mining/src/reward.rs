//! Mining reward calculation using 2π% geometric decay.
//!
//! Each year, 6.28% (2π%) of the remaining mining pool is emitted.
//! This creates a smooth exponential decay that never fully drains the pool.
//! Transaction fee income (25% of base fees) flows back into the pool,
//! ensuring mining remains viable forever.
//!
//! Supply tracking ensures total rewards never exceed the mining pool cap.
//! Year is determined from genesis timestamp + current block timestamp.

use pichain_types::{PiAmount, BASE_UNITS_PER_PI, TOTAL_SUPPLY};

/// Total mining pool in base units: exactly 85% of TOTAL_SUPPLY.
///
/// The Satoshi model allocates 85% of total supply to mining, with no
/// team/treasury/foundation allocations. Miners earn this pool over time
/// via 2π% geometric decay, supplemented by transaction fee income.
const MINING_POOL_BASE: u128 = TOTAL_SUPPLY * 85 / 100;

/// Compile-time assertion: MINING_POOL_BASE must fit in u64.
/// Multiple hot paths cast this to u64 for arithmetic with u64 fields.
/// If TOTAL_SUPPLY ever grows beyond ~21.7B PI, this assertion will fail at compile time.
const _: () = assert!(
    MINING_POOL_BASE <= u64::MAX as u128,
    "MINING_POOL_BASE exceeds u64::MAX"
);

/// MINING_POOL_BASE as u64 — safe because of the compile-time assertion above.
const MINING_POOL_U64: u64 = MINING_POOL_BASE as u64;

/// Total mining pool in whole PI (for display / documentation only).
/// 85% of 3,141,592,653 PI ≈ 2,670,353,755 PI.
#[allow(dead_code)]
const MINING_POOL_PI: u64 = (MINING_POOL_BASE / BASE_UNITS_PER_PI as u128) as u64;

/// 2π% emission rate in basis points (628 bps = 6.28%).
const EMISSION_RATE_BPS: u128 = 628;

/// Milliseconds per year (365.25 days for leap year averaging).
const MS_PER_YEAR: u64 = 365 * 24 * 3600 * 1000 + 6 * 3600 * 1000;

/// Blocks per year (assuming 314ms block time, 365-day year).
/// Uses a flat 365-day year (not 365.25) because block production doesn't
/// observe leap years. MS_PER_YEAR uses 365.25 days for timestamp math,
/// but height-based year calculations use this constant instead.
pub(crate) const BLOCKS_PER_YEAR: u64 = 365 * 24 * 3600 * 1000 / 314;

/// Reward calculator for PI digit mining with supply tracking.
#[derive(Clone)]
pub struct RewardCalculator {
    /// Blocks per year (assuming 314ms block time).
    blocks_per_year: u64,
    /// Genesis timestamp in milliseconds (for year calculation).
    genesis_timestamp_ms: u64,
    /// Total rewards distributed so far (in base units).
    total_mined: u64,
    /// Cumulative transaction fee income added to the mining pool.
    /// 25% of base fees flow back into the pool, extending its lifetime.
    fee_income: u64,
}

impl RewardCalculator {
    pub fn new() -> Self {
        Self {
            blocks_per_year: BLOCKS_PER_YEAR,
            genesis_timestamp_ms: 0,
            total_mined: 0,
            fee_income: 0,
        }
    }

    /// Set the genesis timestamp for year calculation.
    pub fn set_genesis_timestamp(&mut self, ts_ms: u64) {
        self.genesis_timestamp_ms = ts_ms;
    }

    /// Get the genesis timestamp (0 means not yet configured).
    pub fn genesis_timestamp_ms(&self) -> u64 {
        self.genesis_timestamp_ms
    }

    /// Get total rewards distributed so far.
    pub fn total_mined(&self) -> u64 {
        self.total_mined
    }

    /// Set total mined (for replay/restore from persisted state).
    pub fn set_total_mined(&mut self, amount: u64) {
        self.total_mined = amount;
    }

    /// Add transaction fee income to the mining pool.
    /// This extends the pool's effective lifetime, making mining sustainable forever.
    pub fn add_fee_income(&mut self, amount: u64) {
        self.fee_income = self.fee_income.saturating_add(amount);
    }

    /// Set fee income (for replay/restore from persisted state).
    pub fn set_fee_income(&mut self, amount: u64) {
        self.fee_income = amount;
    }

    /// Get cumulative fee income added to the mining pool.
    pub fn fee_income(&self) -> u64 {
        self.fee_income
    }

    /// Determine the emission year from a block timestamp.
    /// Year 1 starts at genesis, year 2 after one year, etc.
    /// Returns at least 1 (never year 0).
    pub fn year_from_timestamp(&self, block_timestamp_ms: u64) -> u32 {
        if self.genesis_timestamp_ms == 0 || block_timestamp_ms <= self.genesis_timestamp_ms {
            return 1;
        }
        let elapsed = block_timestamp_ms - self.genesis_timestamp_ms;

        (elapsed / MS_PER_YEAR) as u32 + 1
    }

    /// Determine the emission year from a block height.
    ///
    /// This is the deterministic, replay-safe alternative to `year_from_timestamp`.
    /// During historical block replay the original wall-clock timestamp may not be
    /// available (or may be zero). Using `block_height / BLOCKS_PER_YEAR` produces
    /// the same emission year that would have applied when the block was first
    /// produced, because block height is a monotonic proxy for elapsed time.
    ///
    /// Returns at least 1 (never year 0).
    pub fn year_from_height(&self, block_height: u64) -> u32 {
        (block_height / BLOCKS_PER_YEAR) as u32 + 1
    }

    /// Calculate the mining reward per digit for a given year.
    ///
    /// The reward is proportional to the number of digits computed,
    /// capped by the annual emission for that year.
    ///
    /// NOTE: This uses a fixed assumption of 1000 digits/block. For quantum-safe
    /// proportional rewards, use [`calculate_proportional_reward`] instead, which
    /// divides fixed epoch emission among all miners proportionally.
    pub fn reward_per_digit(&self, year: u32) -> PiAmount {
        let annual_emission = self.annual_emission(year);
        // Assume ~1000 digits computed per block on average across all miners
        let expected_digits_per_year = self.blocks_per_year * 1000;
        if expected_digits_per_year == 0 {
            return 0;
        }
        annual_emission / expected_digits_per_year
    }

    /// Calculate the annual emission for a given year (in base PI units).
    ///
    /// Uses 2π% (6.28%) geometric decay: each year emits 6.28% of the
    /// remaining theoretical pool. The pool never fully drains.
    ///
    /// Year 1: ~167.6M PI (5.3% of total supply)
    /// Year 10: ~91.5M PI
    /// Year 31 (π decades): pool ~82% distributed
    pub fn annual_emission(&self, year: u32) -> PiAmount {
        // Cap year to prevent O(year) DoS loop. After year 200, the
        // remaining pool is negligible.
        if year > 200 {
            return 0;
        }

        // Compute remaining pool after all prior years' emissions
        let remaining = self.remaining_pool_at_year(year) as u128;

        // This year's emission: 2π% (628 bps) of remaining
        (remaining * EMISSION_RATE_BPS / 10_000) as PiAmount
    }

    /// Calculate the total reward for a mining proof.
    pub fn calculate_reward(&self, year: u32, digit_count: u32) -> PiAmount {
        self.reward_per_digit(year)
            .saturating_mul(digit_count as u64)
    }

    /// Calculate reward for a given number of digits, using the block timestamp
    /// to determine the emission year. Enforces the mining pool supply cap.
    ///
    /// Returns 0 if the mining pool is exhausted.
    pub fn reward_for_digits_at_time(&self, digit_count: u32, block_timestamp_ms: u64) -> PiAmount {
        let year = self.year_from_timestamp(block_timestamp_ms);
        let reward = self.calculate_reward(year, digit_count);

        // Enforce supply cap: never exceed the effective mining pool (base + fee income)
        let remaining = self.remaining_pool();
        if remaining == 0 {
            return 0;
        }
        reward.min(remaining)
    }

    /// Record a reward distribution. Call this after a proof is accepted.
    /// Returns Err if the reward would exceed the effective pool cap (base + fee income).
    pub fn record_reward(&mut self, amount: u64) -> Result<(), String> {
        let effective_cap = MINING_POOL_U64.saturating_add(self.fee_income);
        let new_total = (self.total_mined as u128) + (amount as u128);
        if new_total > effective_cap as u128 {
            return Err(format!(
                "mining reward would exceed pool cap: mined={}, reward={}, cap={}",
                self.total_mined, amount, effective_cap
            ));
        }
        self.total_mined = new_total as u64;
        Ok(())
    }

    /// Get the total remaining mining pool based on actual distributions.
    /// Includes fee income that has been added back to the pool.
    pub fn remaining_pool(&self) -> PiAmount {
        let effective_pool = MINING_POOL_U64.saturating_add(self.fee_income);
        effective_pool.saturating_sub(self.total_mined)
    }

    /// Get the total remaining mining pool for a given year (theoretical, based on schedule).
    /// Uses only the base pool (no fee income) for deterministic emission calculation.
    pub fn remaining_pool_at_year(&self, year: u32) -> PiAmount {
        let mut remaining: u128 = MINING_POOL_BASE;
        for _y in 1..year {
            let emission = remaining * EMISSION_RATE_BPS / 10_000;
            remaining = remaining.saturating_sub(emission);
        }
        remaining as PiAmount
    }

    /// Calculate reward for a given number of digits, using the block height
    /// to determine the emission year. Enforces the mining pool supply cap.
    ///
    /// This is the replay-safe alternative to `reward_for_digits_at_time()`.
    /// During historical block replay, the original timestamp may not be available
    /// so the emission year is derived deterministically from block height.
    ///
    /// Returns 0 if the mining pool is exhausted.
    pub fn reward_for_digits_at_height(&self, digit_count: u32, block_height: u64) -> PiAmount {
        let year = self.year_from_height(block_height);
        let reward = self.calculate_reward(year, digit_count);

        let remaining = self.remaining_pool();
        if remaining == 0 {
            return 0;
        }
        reward.min(remaining)
    }

    /// Legacy: calculate reward for a given number of digits (year 1 default).
    /// Used by process_proof() which doesn't have timestamp context.
    /// Prefer reward_for_digits_at_time() or reward_for_digits_at_height().
    pub fn reward_for_digits(&self, digit_count: u32) -> PiAmount {
        let reward = self.calculate_reward(1, digit_count);
        let remaining = self.remaining_pool();
        if remaining == 0 {
            return 0;
        }
        reward.min(remaining)
    }

    /// Calculate the epoch emission for the given year and epoch duration.
    ///
    /// This is the FIXED amount of PI available for mining in one epoch,
    /// regardless of how many digits are computed. Used for proportional rewards.
    pub fn epoch_emission(&self, year: u32, blocks_per_epoch: u64) -> PiAmount {
        let annual = self.annual_emission(year) as u128;
        let epochs_per_year = (self.blocks_per_year / blocks_per_epoch.max(1)).max(1) as u128;
        (annual / epochs_per_year).min(u64::MAX as u128) as PiAmount
    }
}

impl Default for RewardCalculator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn year_1_emission() {
        let calc = RewardCalculator::new();
        let emission = calc.annual_emission(1);
        // 2π% (628 bps) of MINING_POOL_BASE (85% of TOTAL_SUPPLY)
        let expected = (MINING_POOL_BASE * EMISSION_RATE_BPS / 10_000) as u64;
        assert_eq!(emission, expected);
    }

    #[test]
    fn mining_pool_consistent_with_total_supply() {
        let node_pool = pichain_types::TOTAL_SUPPLY * 85 / 100;
        assert_eq!(
            MINING_POOL_BASE, node_pool,
            "mining pool cap must equal 85% of TOTAL_SUPPLY"
        );
    }

    #[test]
    fn emission_decreases_over_time() {
        let calc = RewardCalculator::new();
        let y1 = calc.annual_emission(1);
        let y2 = calc.annual_emission(2);
        let y3 = calc.annual_emission(3);
        assert!(y1 > y2);
        assert!(y2 > y3);
    }

    #[test]
    fn reward_per_digit_is_nonzero() {
        let calc = RewardCalculator::new();
        let reward = calc.reward_per_digit(1);
        assert!(reward > 0, "reward per digit should be positive");
    }

    #[test]
    fn more_digits_more_reward() {
        let calc = RewardCalculator::new();
        let r1 = calc.calculate_reward(1, 100);
        let r2 = calc.calculate_reward(1, 1000);
        assert!(r2 > r1);
        assert_eq!(r2, r1 * 10);
    }

    #[test]
    fn remaining_pool_decreases_with_mining() {
        let mut calc = RewardCalculator::new();
        let pool_before = calc.remaining_pool();
        let reward = calc.reward_for_digits(1000);
        calc.record_reward(reward).unwrap();
        let pool_after = calc.remaining_pool();
        assert!(pool_after < pool_before);
        assert_eq!(pool_before - pool_after, reward);
    }

    #[test]
    fn supply_cap_enforced() {
        let mut calc = RewardCalculator::new();
        let cap = MINING_POOL_U64;
        calc.set_total_mined(cap - 100);

        let reward = calc.reward_for_digits(1000);
        assert_eq!(reward, 100);

        calc.record_reward(reward).unwrap();
        assert_eq!(calc.remaining_pool(), 0);

        let reward2 = calc.reward_for_digits(1000);
        assert_eq!(reward2, 0);
    }

    #[test]
    fn year_from_timestamp() {
        let mut calc = RewardCalculator::new();
        calc.set_genesis_timestamp(1_000_000_000_000);

        assert_eq!(calc.year_from_timestamp(1_000_000_000_000), 1);
        assert_eq!(
            calc.year_from_timestamp(1_000_000_000_000 + MS_PER_YEAR / 2),
            1
        );
        assert_eq!(calc.year_from_timestamp(1_000_000_000_000 + MS_PER_YEAR), 2);
        assert_eq!(
            calc.year_from_timestamp(1_000_000_000_000 + MS_PER_YEAR * 3),
            4
        );
    }

    #[test]
    fn reward_for_digits_at_time_uses_year() {
        let mut calc = RewardCalculator::new();
        calc.set_genesis_timestamp(1_000_000_000_000);

        let r_y1 = calc.reward_for_digits_at_time(1000, 1_000_000_000_000);
        let r_y2 = calc.reward_for_digits_at_time(1000, 1_000_000_000_000 + MS_PER_YEAR);

        // Year 2 reward should be less than year 1 (smaller remaining pool)
        assert!(r_y1 > r_y2, "year 1 reward should exceed year 2");
    }

    #[test]
    fn record_reward_rejects_overflow() {
        let mut calc = RewardCalculator::new();
        let cap = MINING_POOL_U64;
        calc.set_total_mined(cap);

        let result = calc.record_reward(1);
        assert!(result.is_err());
    }

    #[test]
    fn remaining_pool_at_year_theoretical() {
        let calc = RewardCalculator::new();
        let pool_y1 = calc.remaining_pool_at_year(1);
        let pool_y2 = calc.remaining_pool_at_year(2);
        assert!(pool_y1 > pool_y2);
    }

    #[test]
    fn year_from_height_deterministic() {
        let calc = RewardCalculator::new();

        assert_eq!(calc.year_from_height(0), 1);
        assert_eq!(calc.year_from_height(BLOCKS_PER_YEAR / 2), 1);
        assert_eq!(calc.year_from_height(BLOCKS_PER_YEAR), 2);
        assert_eq!(calc.year_from_height(BLOCKS_PER_YEAR * 3), 4);
    }

    #[test]
    fn reward_for_digits_at_height_uses_year() {
        let calc = RewardCalculator::new();

        let r_y1 = calc.reward_for_digits_at_height(1000, 0);
        let r_y2 = calc.reward_for_digits_at_height(1000, BLOCKS_PER_YEAR);

        assert!(r_y1 > r_y2, "year 1 reward should exceed year 2");
    }

    #[test]
    fn height_and_timestamp_year_approximate_agreement() {
        let mut calc = RewardCalculator::new();
        let genesis_ts: u64 = 1_000_000_000_000;
        calc.set_genesis_timestamp(genesis_ts);

        let mid_y1_height = BLOCKS_PER_YEAR / 2;
        let mid_y1_ts = genesis_ts + mid_y1_height * 314;
        assert_eq!(calc.year_from_height(mid_y1_height), 1);
        assert_eq!(calc.year_from_timestamp(mid_y1_ts), 1);

        let deep_y3_height = BLOCKS_PER_YEAR * 2 + BLOCKS_PER_YEAR / 2;
        let deep_y3_ts = genesis_ts + deep_y3_height * 314;
        assert_eq!(calc.year_from_height(deep_y3_height), 3);
        assert_eq!(calc.year_from_timestamp(deep_y3_ts), 3);

        let boundary_height = BLOCKS_PER_YEAR;
        let boundary_ts = genesis_ts + boundary_height * 314;
        let year_h = calc.year_from_height(boundary_height);
        let year_t = calc.year_from_timestamp(boundary_ts);
        let diff = (year_h as i32 - year_t as i32).unsigned_abs();
        assert!(
            diff <= 1,
            "height and timestamp year should differ by at most 1 at boundary, got h={} t={}",
            year_h,
            year_t
        );
    }

    #[test]
    fn geometric_decay_consistent() {
        let calc = RewardCalculator::new();

        // Every year uses the same 2π% formula — no special cases
        for year in 1..=20 {
            let remaining = calc.remaining_pool_at_year(year) as u128;
            let expected = (remaining * EMISSION_RATE_BPS / 10_000) as u64;
            assert_eq!(
                calc.annual_emission(year),
                expected,
                "year {} emission should be 2π% of remaining pool",
                year
            );
        }

        // Later years emit strictly less
        let y10 = calc.annual_emission(10);
        let y20 = calc.annual_emission(20);
        assert!(y10 > y20);
    }

    #[test]
    fn mining_pool_never_fully_drained() {
        let calc = RewardCalculator::new();

        let mut total_emitted: u128 = 0;
        for year in 1..=100 {
            total_emitted += calc.annual_emission(year) as u128;
        }

        assert!(
            total_emitted < MINING_POOL_BASE,
            "total emitted over 100 years ({}) must be less than pool ({})",
            total_emitted,
            MINING_POOL_BASE
        );

        let y100 = calc.annual_emission(100);
        assert!(y100 > 0, "year 100 emission should still be positive");
    }

    #[test]
    fn fee_income_extends_pool() {
        let mut calc = RewardCalculator::new();
        let pool_before = calc.remaining_pool();

        // Add fee income
        calc.add_fee_income(1_000_000_000); // 1 PI
        let pool_after = calc.remaining_pool();
        assert_eq!(pool_after, pool_before + 1_000_000_000);

        // Can mine more than base pool thanks to fee income
        let cap = MINING_POOL_U64;
        calc.set_total_mined(cap);
        // Without fee income, pool would be exhausted. But fee income extends it.
        assert_eq!(calc.remaining_pool(), 1_000_000_000);

        let reward = calc.reward_for_digits(1000);
        assert!(reward > 0, "fee income should allow continued mining");
    }

    #[test]
    fn year_1_emission_is_gentle() {
        let calc = RewardCalculator::new();
        let y1 = calc.annual_emission(1);
        let total_supply_pi = TOTAL_SUPPLY / BASE_UNITS_PER_PI as u128;
        let y1_pi = y1 / BASE_UNITS_PER_PI;

        // Year 1 should be ~5.3% of total supply (not 25% like old model)
        let pct = y1_pi * 100 / total_supply_pi as u64;
        assert!(
            pct <= 6,
            "year 1 should be ≤6% of total supply, got {}%",
            pct
        );
        assert!(
            pct >= 4,
            "year 1 should be ≥4% of total supply, got {}%",
            pct
        );
    }
}
