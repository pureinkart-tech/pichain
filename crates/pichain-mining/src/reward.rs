//! Mining reward calculation using π-smooth emission.
//!
//! Emission decays smoothly every year with a half-life of π years (3.14159...).
//! No cliff halvings like Bitcoin — rewards decrease 19.8% per year continuously.
//!
//! Formula: emission(year) = YEAR1_EMISSION × ANNUAL_RETAIN^(year-1)
//!   where ANNUAL_RETAIN = 0.5^(1/π) ≈ 0.80197
//!
//! Key properties:
//! - Year 1: ~19.8% of pool (528M PI) — fair launch, not front-loaded
//! - 50% mined after π years (~3.14 years)
//! - 99.9% mined after π decades (~31.4 years)
//! - Mining NEVER hits zero — asymptotic like π digits
//! - No sudden cliffs — miners can plan, no mass bankruptcies
//! - Transaction fee income flows back into the pool
//!
//! Supply tracking ensures total rewards never exceed the mining pool cap.

use pichain_types::{PiAmount, BASE_UNITS_PER_PI, TOTAL_SUPPLY};

/// Total mining pool in base units: exactly 85% of TOTAL_SUPPLY.
///
/// Satoshi-style launch: 85% to miners, no team/treasury allocations.
/// Miners earn this pool over time via π-smooth decay, supplemented
/// by transaction fee income (25% of base fees flow back).
const MINING_POOL_BASE: u128 = TOTAL_SUPPLY * 85 / 100;

/// Compile-time assertion: MINING_POOL_BASE must fit in u64.
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

/// Annual retention factor in parts-per-million (ppm).
///
/// ANNUAL_RETAIN = 0.5^(1/π) ≈ 0.801974
/// Each year's emission = previous year × ANNUAL_RETAIN
/// This gives a half-life of exactly π years.
///
/// Stored as fixed-point ppm (801974) to avoid floating-point in consensus.
/// 801974 / 1_000_000 = 0.801974
const ANNUAL_RETAIN_PPM: u128 = 801_974;
const PPM: u128 = 1_000_000;

/// Year 1 emission in base units.
///
/// YEAR1 = POOL × (1 - ANNUAL_RETAIN) = POOL × 0.198026
/// This ensures the infinite geometric sum equals the entire pool:
///   YEAR1 / (1 - ANNUAL_RETAIN) = POOL  ✓
///
/// Year 1 ≈ 528,706,334 PI (19.8% of pool, 16.8% of total supply)
const YEAR1_EMISSION: u128 = MINING_POOL_BASE * (PPM - ANNUAL_RETAIN_PPM) / PPM;

/// Milliseconds per year (365.25 days for leap year averaging).
const MS_PER_YEAR: u64 = 365 * 24 * 3600 * 1000 + 6 * 3600 * 1000;

/// Blocks per year (assuming 314ms block time, 365-day year).
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
    pub fn year_from_timestamp(&self, block_timestamp_ms: u64) -> u32 {
        if self.genesis_timestamp_ms == 0 || block_timestamp_ms <= self.genesis_timestamp_ms {
            return 1;
        }
        let elapsed = block_timestamp_ms - self.genesis_timestamp_ms;
        (elapsed / MS_PER_YEAR) as u32 + 1
    }

    /// Determine the emission year from a block height (deterministic, replay-safe).
    pub fn year_from_height(&self, block_height: u64) -> u32 {
        (block_height / BLOCKS_PER_YEAR) as u32 + 1
    }

    /// Calculate the mining reward per digit for a given year.
    pub fn reward_per_digit(&self, year: u32) -> PiAmount {
        let annual_emission = self.annual_emission(year);
        let expected_digits_per_year = self.blocks_per_year * 1000;
        if expected_digits_per_year == 0 {
            return 0;
        }
        annual_emission / expected_digits_per_year
    }

    /// Calculate the annual emission for a given year (in base PI units).
    ///
    /// Uses π-smooth decay: emission(year) = YEAR1 × RETAIN^(year-1)
    /// Half-life = π years. No cliff halvings.
    ///
    /// Year 1:  ~528M PI (19.8% of pool)
    /// Year 3:  ~340M PI (50% of pool cumulative)
    /// Year 10: ~72M PI  (89% cumulative)
    /// Year 31: ~705K PI (99.9% cumulative)
    pub fn annual_emission(&self, year: u32) -> PiAmount {
        if year == 0 || year > 200 {
            return 0;
        }

        // emission = YEAR1_EMISSION × (ANNUAL_RETAIN_PPM / PPM)^(year-1)
        // Computed iteratively in u128 to avoid floating point.
        let mut emission: u128 = YEAR1_EMISSION;
        for _ in 1..year {
            emission = emission * ANNUAL_RETAIN_PPM / PPM;
        }
        emission as PiAmount
    }

    /// Calculate the total reward for a mining proof.
    pub fn calculate_reward(&self, year: u32, digit_count: u32) -> PiAmount {
        self.reward_per_digit(year)
            .saturating_mul(digit_count as u64)
    }

    /// Calculate reward for a given number of digits at a block timestamp.
    /// Enforces the mining pool supply cap.
    pub fn reward_for_digits_at_time(&self, digit_count: u32, block_timestamp_ms: u64) -> PiAmount {
        let year = self.year_from_timestamp(block_timestamp_ms);
        let reward = self.calculate_reward(year, digit_count);
        let remaining = self.remaining_pool();
        if remaining == 0 {
            return 0;
        }
        reward.min(remaining)
    }

    /// Record a reward distribution. Call this after a proof is accepted.
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
    pub fn remaining_pool(&self) -> PiAmount {
        let effective_pool = MINING_POOL_U64.saturating_add(self.fee_income);
        effective_pool.saturating_sub(self.total_mined)
    }

    /// Get the theoretical remaining pool for a given year (emission schedule).
    pub fn remaining_pool_at_year(&self, year: u32) -> PiAmount {
        // Sum of emissions for years 1..year-1, then subtract from pool
        let mut total_emitted: u128 = 0;
        let mut emission: u128 = YEAR1_EMISSION;
        for _ in 1..year {
            total_emitted += emission;
            emission = emission * ANNUAL_RETAIN_PPM / PPM;
        }
        let remaining = MINING_POOL_BASE.saturating_sub(total_emitted);
        remaining as PiAmount
    }

    /// Calculate reward at a block height (deterministic, replay-safe).
    pub fn reward_for_digits_at_height(&self, digit_count: u32, block_height: u64) -> PiAmount {
        let year = self.year_from_height(block_height);
        let reward = self.calculate_reward(year, digit_count);
        let remaining = self.remaining_pool();
        if remaining == 0 {
            return 0;
        }
        reward.min(remaining)
    }

    /// Calculate reward for digits (year 1 default, for process_proof).
    pub fn reward_for_digits(&self, digit_count: u32) -> PiAmount {
        let reward = self.calculate_reward(1, digit_count);
        let remaining = self.remaining_pool();
        if remaining == 0 {
            return 0;
        }
        reward.min(remaining)
    }

    /// Calculate the epoch emission for proportional rewards.
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
    fn year_1_emission_is_correct() {
        let calc = RewardCalculator::new();
        let emission = calc.annual_emission(1);
        // Year 1 = POOL × (1 - 0.801974) = POOL × 0.198026
        let expected = (MINING_POOL_BASE * (PPM - ANNUAL_RETAIN_PPM) / PPM) as u64;
        assert_eq!(emission, expected);
        // ~528M PI (19.8% of pool)
        let emission_pi = emission / BASE_UNITS_PER_PI;
        assert!(emission_pi > 500_000_000, "year 1 should be >500M PI");
        assert!(emission_pi < 550_000_000, "year 1 should be <550M PI");
    }

    #[test]
    fn mining_pool_consistent_with_total_supply() {
        let node_pool = pichain_types::TOTAL_SUPPLY * 85 / 100;
        assert_eq!(MINING_POOL_BASE, node_pool);
    }

    #[test]
    fn emission_decreases_smoothly() {
        let calc = RewardCalculator::new();
        let y1 = calc.annual_emission(1);
        let y2 = calc.annual_emission(2);
        let y3 = calc.annual_emission(3);
        assert!(y1 > y2);
        assert!(y2 > y3);
        // Each year retains ~80.2% of previous
        let ratio = y2 as f64 / y1 as f64;
        assert!(ratio > 0.79, "ratio should be ~0.802, got {ratio}");
        assert!(ratio < 0.82, "ratio should be ~0.802, got {ratio}");
    }

    #[test]
    fn half_life_is_pi_years() {
        let calc = RewardCalculator::new();
        // After π years (~3.14), cumulative should be ~50% of pool
        let mut total: u128 = 0;
        for year in 1..=3 {
            total += calc.annual_emission(year) as u128;
        }
        let pct = total * 100 / MINING_POOL_BASE;
        // After 3 years should be ~48%, close to 50%
        assert!(pct >= 45, "after 3 years should be ~48% mined, got {pct}%");
        assert!(pct <= 52, "after 3 years should be ~48% mined, got {pct}%");
    }

    #[test]
    fn pi_decades_mines_99_percent() {
        let calc = RewardCalculator::new();
        let mut total: u128 = 0;
        for year in 1..=31 {
            total += calc.annual_emission(year) as u128;
        }
        let pct = total * 1000 / MINING_POOL_BASE; // permille
        assert!(pct >= 998, "after 31 years should be >99.8% mined, got {}.{}%", pct/10, pct%10);
    }

    #[test]
    fn reward_per_digit_is_nonzero() {
        let calc = RewardCalculator::new();
        let reward = calc.reward_per_digit(1);
        assert!(reward > 0);
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
        assert_eq!(calc.year_from_timestamp(1_000_000_000_000 + MS_PER_YEAR / 2), 1);
        assert_eq!(calc.year_from_timestamp(1_000_000_000_000 + MS_PER_YEAR), 2);
        assert_eq!(calc.year_from_timestamp(1_000_000_000_000 + MS_PER_YEAR * 3), 4);
    }

    #[test]
    fn record_reward_rejects_overflow() {
        let mut calc = RewardCalculator::new();
        calc.set_total_mined(MINING_POOL_U64);
        assert!(calc.record_reward(1).is_err());
    }

    #[test]
    fn remaining_pool_at_year_decreases() {
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
    fn reward_decreases_by_year() {
        let calc = RewardCalculator::new();
        let r_y1 = calc.reward_for_digits_at_height(1000, 0);
        let r_y2 = calc.reward_for_digits_at_height(1000, BLOCKS_PER_YEAR);
        assert!(r_y1 > r_y2);
    }

    #[test]
    fn mining_pool_never_fully_drained() {
        let calc = RewardCalculator::new();
        let mut total_emitted: u128 = 0;
        for year in 1..=100 {
            total_emitted += calc.annual_emission(year) as u128;
        }
        // After 100 years, should be extremely close to pool but not exceed it
        assert!(
            total_emitted <= MINING_POOL_BASE,
            "total emitted ({total_emitted}) must not exceed pool ({MINING_POOL_BASE})"
        );
        // Year 100 should still emit something (never zero)
        let y100 = calc.annual_emission(100);
        assert!(y100 > 0, "year 100 should still emit >0");
    }

    #[test]
    fn fee_income_extends_pool() {
        let mut calc = RewardCalculator::new();
        let pool_before = calc.remaining_pool();
        calc.add_fee_income(1_000_000_000);
        let pool_after = calc.remaining_pool();
        assert_eq!(pool_after, pool_before + 1_000_000_000);

        let cap = MINING_POOL_U64;
        calc.set_total_mined(cap);
        assert_eq!(calc.remaining_pool(), 1_000_000_000);

        let reward = calc.reward_for_digits(1000);
        assert!(reward > 0);
    }

    #[test]
    fn year_1_not_too_aggressive() {
        let calc = RewardCalculator::new();
        let y1 = calc.annual_emission(1);
        let y1_pi = y1 / BASE_UNITS_PER_PI;
        let total_supply_pi = (TOTAL_SUPPLY / BASE_UNITS_PER_PI as u128) as u64;
        let pct = y1_pi * 100 / total_supply_pi;
        // Year 1 should be ~16-17% of total supply (19.8% of 85% pool)
        assert!(pct >= 14, "year 1 should be >=14% of supply, got {pct}%");
        assert!(pct <= 20, "year 1 should be <=20% of supply, got {pct}%");
    }
}
