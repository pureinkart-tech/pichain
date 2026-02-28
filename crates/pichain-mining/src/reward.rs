//! Mining reward calculation based on the emission schedule.
//!
//! Year 1: 314,159,265 PI (25% of pool)
//! Year 2: 251,327,412 PI (20%)
//! Year 3: 188,495,559 PI (15%)
//! ... decreasing over 7+ years
//!
//! Supply tracking ensures total rewards never exceed the mining pool cap.
//! Year is determined from genesis timestamp + current block timestamp.

use pichain_types::{PiAmount, BASE_UNITS_PER_PI, TOTAL_SUPPLY};

/// Total mining pool in base units: exactly 40% of TOTAL_SUPPLY.
///
/// Derived from TOTAL_SUPPLY to avoid rounding inconsistencies between this
/// crate and pichain-node (which computes the same value as TOTAL_SUPPLY * 40 / 100).
/// Previously this was calculated as `1_256_637_061 * BASE_UNITS_PER_PI` which
/// was 200_000_000 base units (0.2 PI) less than the canonical value.
const MINING_POOL_BASE: u128 = TOTAL_SUPPLY * 40 / 100;

/// Total mining pool in whole PI (for display / documentation only).
/// 40% of 3,141,592,653 PI = 1,256,637,061.2 PI (truncated to integer PI).
#[allow(dead_code)]
const MINING_POOL_PI: u64 = (MINING_POOL_BASE / BASE_UNITS_PER_PI as u128) as u64;

/// Milliseconds per year (365.25 days for leap year averaging).
const MS_PER_YEAR: u64 = 365 * 24 * 3600 * 1000 + 6 * 3600 * 1000;

/// Blocks per year (assuming 314ms block time, 365-day year).
/// Uses a flat 365-day year (not 365.25) because block production doesn't
/// observe leap years. MS_PER_YEAR uses 365.25 days for timestamp math,
/// but height-based year calculations use this constant instead.
pub(crate) const BLOCKS_PER_YEAR: u64 = 365 * 24 * 3600 * 1000 / 314;

/// Emission schedule: (year, percentage of pool × 100).
const EMISSION_SCHEDULE: &[(u32, u32)] = &[
    (1, 2500), // 25%
    (2, 2000), // 20%
    (3, 1500), // 15%
    (4, 1200), // 12%
    (5, 900),  //  9%
    (6, 700),  //  7%
    (7, 600),  //  6%
];

/// Reward calculator for PI digit mining with supply tracking.
#[derive(Clone)]
pub struct RewardCalculator {
    /// Blocks per year (assuming 314ms block time).
    blocks_per_year: u64,
    /// Genesis timestamp in milliseconds (for year calculation).
    genesis_timestamp_ms: u64,
    /// Total rewards distributed so far (in base units).
    total_mined: u64,
}

impl RewardCalculator {
    pub fn new() -> Self {
        Self {
            blocks_per_year: BLOCKS_PER_YEAR,
            genesis_timestamp_ms: 0,
            total_mined: 0,
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

    /// Determine the emission year from a block timestamp.
    /// Year 1 starts at genesis, year 2 after one year, etc.
    /// Returns at least 1 (never year 0).
    pub fn year_from_timestamp(&self, block_timestamp_ms: u64) -> u32 {
        if self.genesis_timestamp_ms == 0 || block_timestamp_ms <= self.genesis_timestamp_ms {
            return 1;
        }
        let elapsed = block_timestamp_ms - self.genesis_timestamp_ms;
        let year = (elapsed / MS_PER_YEAR) as u32 + 1;
        year
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
        let year = (block_height / BLOCKS_PER_YEAR) as u32 + 1;
        year
    }

    /// Calculate the mining reward per digit for a given year.
    ///
    /// The reward is proportional to the number of digits computed,
    /// capped by the annual emission for that year.
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
    /// Years 1-7 use fixed percentages of the total pool.
    /// Year 8+ uses exponential tail emission: 6% of the REMAINING pool
    /// after all prior years' emissions, ensuring the pool is never fully
    /// drained (geometric decay).
    pub fn annual_emission(&self, year: u32) -> PiAmount {
        // R28-FIX: Cap year to prevent O(year) DoS loop. After year 200, the
        // remaining pool is negligible (6% compounding for 193 years after year 7
        // reduces it to ~0.0006% of original pool). Return 0 emission.
        if year > 200 {
            return 0;
        }

        // Years 1-7: fixed percentage of total pool
        if let Some((_, pct)) = EMISSION_SCHEDULE.iter().find(|(y, _)| *y == year) {
            return (MINING_POOL_BASE * *pct as u128 / 10_000) as PiAmount;
        }

        // Year 8+: 6% of the REMAINING pool after all prior years
        // Compute cumulative emissions for years 1 through (year-1)
        let mut remaining: u128 = MINING_POOL_BASE;
        for y in 1..year {
            let emission = if let Some((_, pct)) = EMISSION_SCHEDULE.iter().find(|(yr, _)| *yr == y) {
                MINING_POOL_BASE * *pct as u128 / 10_000
            } else {
                // This is a year 8+ year in the cumulative sum: 6% of remaining at that point
                remaining * 6 / 100
            };
            remaining = remaining.saturating_sub(emission);
        }

        // This year's emission: 6% of whatever remains
        (remaining * 6 / 100) as PiAmount
    }

    /// Calculate the total reward for a mining proof.
    pub fn calculate_reward(&self, year: u32, digit_count: u32) -> PiAmount {
        self.reward_per_digit(year).saturating_mul(digit_count as u64)
    }

    /// Calculate reward for a given number of digits, using the block timestamp
    /// to determine the emission year. Enforces the mining pool supply cap.
    ///
    /// Returns 0 if the mining pool is exhausted.
    pub fn reward_for_digits_at_time(
        &self,
        digit_count: u32,
        block_timestamp_ms: u64,
    ) -> PiAmount {
        let year = self.year_from_timestamp(block_timestamp_ms);
        let reward = self.calculate_reward(year, digit_count);

        // Enforce supply cap: never exceed the total mining pool
        let remaining = MINING_POOL_BASE.saturating_sub(self.total_mined as u128);
        if remaining == 0 {
            return 0;
        }
        // Cap the reward at whatever remains in the pool
        reward.min(remaining as u64)
    }

    /// Record a reward distribution. Call this after a proof is accepted.
    /// Returns Err if the reward would exceed the pool cap (should never happen
    /// if reward_for_digits_at_time was used, but defense-in-depth).
    pub fn record_reward(&mut self, amount: u64) -> Result<(), String> {
        let new_total = (self.total_mined as u128) + (amount as u128);
        if new_total > MINING_POOL_BASE {
            return Err(format!(
                "mining reward would exceed pool cap: mined={}, reward={}, cap={}",
                self.total_mined, amount, MINING_POOL_BASE
            ));
        }
        self.total_mined = new_total as u64;
        Ok(())
    }

    /// Get the total remaining mining pool based on actual distributions.
    pub fn remaining_pool(&self) -> PiAmount {
        let total = MINING_POOL_BASE;
        total.saturating_sub(self.total_mined as u128) as PiAmount
    }

    /// Get the total remaining mining pool for a given year (theoretical, based on schedule).
    pub fn remaining_pool_at_year(&self, year: u32) -> PiAmount {
        let mut spent: u128 = 0;
        for y in 1..year {
            spent += self.annual_emission(y) as u128;
        }
        let total = MINING_POOL_BASE;
        (total.saturating_sub(spent)) as PiAmount
    }

    /// Calculate reward for a given number of digits, using the block height
    /// to determine the emission year. Enforces the mining pool supply cap.
    ///
    /// This is the replay-safe alternative to `reward_for_digits_at_time()`.
    /// During historical block replay, the original timestamp may not be available
    /// so the emission year is derived deterministically from block height.
    ///
    /// Returns 0 if the mining pool is exhausted.
    pub fn reward_for_digits_at_height(
        &self,
        digit_count: u32,
        block_height: u64,
    ) -> PiAmount {
        let year = self.year_from_height(block_height);
        let reward = self.calculate_reward(year, digit_count);

        // Enforce supply cap: never exceed the total mining pool
        let remaining = MINING_POOL_BASE.saturating_sub(self.total_mined as u128);
        if remaining == 0 {
            return 0;
        }
        // Cap the reward at whatever remains in the pool
        reward.min(remaining as u64)
    }

    /// Legacy: calculate reward for a given number of digits (year 1 default).
    /// Used by process_proof() which doesn't have timestamp context.
    /// Prefer reward_for_digits_at_time() or reward_for_digits_at_height().
    pub fn reward_for_digits(&self, digit_count: u32) -> PiAmount {
        let reward = self.calculate_reward(1, digit_count);
        // Still enforce supply cap
        let remaining = MINING_POOL_BASE.saturating_sub(self.total_mined as u128);
        if remaining == 0 {
            return 0;
        }
        reward.min(remaining as u64)
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
        // 25% of MINING_POOL_BASE (which is 40% of TOTAL_SUPPLY)
        let expected = MINING_POOL_BASE * 2500 / 10_000;
        assert_eq!(emission, expected as u64);
    }

    #[test]
    fn mining_pool_consistent_with_total_supply() {
        // Verify the mining pool cap in this crate matches the canonical
        // 40% of TOTAL_SUPPLY calculation used in pichain-node.
        let node_pool = pichain_types::TOTAL_SUPPLY * 40 / 100;
        assert_eq!(
            MINING_POOL_BASE, node_pool,
            "mining pool cap must equal 40% of TOTAL_SUPPLY"
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
        // Set total_mined to just below pool cap
        let cap = MINING_POOL_BASE as u64;
        calc.set_total_mined(cap - 100);

        // Should only get 100 (the remainder), not the full reward
        let reward = calc.reward_for_digits(1000);
        assert_eq!(reward, 100);

        // After recording, pool should be empty
        calc.record_reward(reward).unwrap();
        assert_eq!(calc.remaining_pool(), 0);

        // Further mining gets 0 reward
        let reward2 = calc.reward_for_digits(1000);
        assert_eq!(reward2, 0);
    }

    #[test]
    fn year_from_timestamp() {
        let mut calc = RewardCalculator::new();
        calc.set_genesis_timestamp(1_000_000_000_000); // ~2001

        // At genesis: year 1
        assert_eq!(calc.year_from_timestamp(1_000_000_000_000), 1);
        // 6 months later: still year 1
        assert_eq!(
            calc.year_from_timestamp(1_000_000_000_000 + MS_PER_YEAR / 2),
            1
        );
        // 1 year later: year 2
        assert_eq!(
            calc.year_from_timestamp(1_000_000_000_000 + MS_PER_YEAR),
            2
        );
        // 3 years later: year 4
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

        // Year 2 reward should be less than year 1 (20% vs 25%)
        assert!(r_y1 > r_y2, "year 1 reward should exceed year 2");
    }

    #[test]
    fn record_reward_rejects_overflow() {
        let mut calc = RewardCalculator::new();
        let cap = MINING_POOL_BASE as u64;
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

        // Block 0: year 1
        assert_eq!(calc.year_from_height(0), 1);
        // Block at half a year: still year 1
        assert_eq!(calc.year_from_height(BLOCKS_PER_YEAR / 2), 1);
        // Block at exactly 1 year boundary: year 2
        assert_eq!(calc.year_from_height(BLOCKS_PER_YEAR), 2);
        // Block at 3 years: year 4
        assert_eq!(calc.year_from_height(BLOCKS_PER_YEAR * 3), 4);
    }

    #[test]
    fn reward_for_digits_at_height_uses_year() {
        let calc = RewardCalculator::new();

        let r_y1 = calc.reward_for_digits_at_height(1000, 0);
        let r_y2 = calc.reward_for_digits_at_height(1000, BLOCKS_PER_YEAR);

        // Year 2 reward should be less than year 1 (20% vs 25%)
        assert!(r_y1 > r_y2, "year 1 reward should exceed year 2");
    }

    #[test]
    fn height_and_timestamp_year_approximate_agreement() {
        // Verify that height-based and timestamp-based year calculations
        // are within 1 year of each other for a chain running at target block time.
        //
        // They won't be exactly equal because:
        // - BLOCKS_PER_YEAR uses 365-day year (31,536,000,000ms / 314)
        // - MS_PER_YEAR uses 365.25-day year (31,557,600,000ms)
        // This 0.25-day difference means the height-based year transitions
        // slightly earlier than the timestamp-based year, which is acceptable
        // since both provide deterministic replay-safe calculations.
        let mut calc = RewardCalculator::new();
        let genesis_ts: u64 = 1_000_000_000_000;
        calc.set_genesis_timestamp(genesis_ts);

        // Mid-year 1: both should agree
        let mid_y1_height = BLOCKS_PER_YEAR / 2;
        let mid_y1_ts = genesis_ts + mid_y1_height * 314;
        assert_eq!(calc.year_from_height(mid_y1_height), 1);
        assert_eq!(calc.year_from_timestamp(mid_y1_ts), 1);

        // Deep into year 3: both should agree
        let deep_y3_height = BLOCKS_PER_YEAR * 2 + BLOCKS_PER_YEAR / 2;
        let deep_y3_ts = genesis_ts + deep_y3_height * 314;
        assert_eq!(calc.year_from_height(deep_y3_height), 3);
        assert_eq!(calc.year_from_timestamp(deep_y3_ts), 3);

        // At boundary, they may differ by at most 1 year
        let boundary_height = BLOCKS_PER_YEAR;
        let boundary_ts = genesis_ts + boundary_height * 314;
        let year_h = calc.year_from_height(boundary_height);
        let year_t = calc.year_from_timestamp(boundary_ts);
        let diff = (year_h as i32 - year_t as i32).unsigned_abs();
        assert!(diff <= 1, "height and timestamp year should differ by at most 1 at boundary, got h={} t={}", year_h, year_t);
    }

    #[test]
    fn year_8_plus_uses_remaining_pool_not_total() {
        let calc = RewardCalculator::new();

        // Year 7 emission: 6% of total pool
        let y7 = calc.annual_emission(7);
        let expected_y7 = (MINING_POOL_BASE * 600 / 10_000) as u64;
        assert_eq!(y7, expected_y7, "year 7 should be 6% of total pool");

        // Year 8 emission: 6% of REMAINING pool after years 1-7
        let y8 = calc.annual_emission(8);

        // Compute expected remaining after years 1-7
        let mut spent: u128 = 0;
        for year in 1..=7 {
            spent += calc.annual_emission(year) as u128;
        }
        let remaining_after_7 = MINING_POOL_BASE - spent;
        let expected_y8 = (remaining_after_7 * 6 / 100) as u64;
        assert_eq!(y8, expected_y8, "year 8 should be 6% of remaining pool, not total");

        // Year 8 emission must be less than year 7 emission
        // (6% of remaining < 6% of total, since remaining < total)
        assert!(y8 < y7, "year 8 ({}) must be less than year 7 ({}) due to exponential decay", y8, y7);

        // Year 9 should be even less than year 8 (continued decay)
        let y9 = calc.annual_emission(9);
        assert!(y9 < y8, "year 9 ({}) must be less than year 8 ({}) due to exponential decay", y9, y8);
    }

    #[test]
    fn mining_pool_never_fully_drained() {
        let calc = RewardCalculator::new();

        // Sum emissions for 100 years — pool should never be fully drained
        let mut total_emitted: u128 = 0;
        for year in 1..=100 {
            total_emitted += calc.annual_emission(year) as u128;
        }

        // After 100 years the pool must still have remaining tokens
        assert!(
            total_emitted < MINING_POOL_BASE,
            "total emitted over 100 years ({}) must be less than pool ({})",
            total_emitted, MINING_POOL_BASE
        );

        // Verify the tail emission is still positive even at year 100
        let y100 = calc.annual_emission(100);
        assert!(y100 > 0, "year 100 emission should still be positive (exponential tail)");
    }
}
