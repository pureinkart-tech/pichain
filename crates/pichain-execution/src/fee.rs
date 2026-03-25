//! EIP-1559-style fee calculation with PI burn mechanism.
//!
//! Base fee distribution:
//!   25% burned permanently (deflationary pressure)
//!   25% flows to mining pool (miner sustainability)
//!   50% to block proposer/stakers
//!
//! Priority fee distribution:
//!   100% to block producer (incentivizes including transactions)

use pichain_types::{Gas, PiAmount, FEE_BURN_RATE_BPS, FEE_MINER_RATE_BPS, FEE_STAKER_RATE_BPS};

/// Fee calculator implementing PIChain's deflationary fee model.
pub struct FeeCalculator;

impl FeeCalculator {
    pub fn new() -> Self {
        Self
    }

    /// Calculate total fee for a transaction (overflow-safe, saturating).
    pub fn calculate_fee(
        &self,
        gas_used: Gas,
        base_fee_per_gas: PiAmount,
        priority_fee_per_gas: PiAmount,
    ) -> PiAmount {
        let base_fee = Self::safe_mul(gas_used, base_fee_per_gas);
        let priority_fee = Self::safe_mul(gas_used, priority_fee_per_gas);
        base_fee.saturating_add(priority_fee)
    }

    /// Calculate base fee portion only (for burn/miner/staker split).
    pub fn calculate_base_fee(&self, gas_used: Gas, base_fee_per_gas: PiAmount) -> PiAmount {
        Self::safe_mul(gas_used, base_fee_per_gas)
    }

    /// Calculate priority fee portion only (100% to block producer).
    pub fn calculate_priority_fee(
        &self,
        gas_used: Gas,
        priority_fee_per_gas: PiAmount,
    ) -> PiAmount {
        Self::safe_mul(gas_used, priority_fee_per_gas)
    }

    /// Saturating u64 multiplication via u128 intermediate.
    /// Returns u64::MAX if the product exceeds u64 range.
    fn safe_mul(a: u64, b: u64) -> u64 {
        let result = a as u128 * b as u128;
        result.min(u64::MAX as u128) as u64
    }

    /// Calculate the amount of PI to burn (25% of base fee portion).
    pub fn calculate_burn(&self, base_fee: PiAmount) -> PiAmount {
        (base_fee as u128 * FEE_BURN_RATE_BPS as u128 / 10_000) as u64
    }

    /// Calculate the amount flowing to the mining pool (25% of base fee portion).
    pub fn miner_share(&self, base_fee: PiAmount) -> PiAmount {
        (base_fee as u128 * FEE_MINER_RATE_BPS as u128 / 10_000) as u64
    }

    /// Calculate the amount distributed to stakers/proposer (50% of base fee portion).
    pub fn staker_reward(&self, base_fee: PiAmount) -> PiAmount {
        (base_fee as u128 * FEE_STAKER_RATE_BPS as u128 / 10_000) as u64
    }

    /// Split a fee into its components.
    /// Takes the TOTAL fee and the BASE FEE portion.
    /// Priority fee = total_fee - base_fee_portion goes 100% to block producer.
    /// Base fee portion is split: 25% burn, 25% miners, remainder to stakers.
    pub fn split_fee(&self, total_fee: PiAmount, base_fee_portion: PiAmount) -> FeeSplit {
        let base_fee = base_fee_portion.min(total_fee);
        let priority_fee = total_fee.saturating_sub(base_fee);

        let burned = self.calculate_burn(base_fee);
        let miners = self.miner_share(base_fee);
        // Assign remainder of base fee to stakers to guarantee no dust lost
        let stakers = base_fee.saturating_sub(burned).saturating_sub(miners);

        FeeSplit {
            burned,
            miners,
            stakers,
            proposer: priority_fee,
        }
    }

    /// Legacy split_fee that splits total fee (backwards compatible for tests).
    /// Treats entire fee as base fee (no priority fee to proposer).
    pub fn split_fee_total(&self, total_fee: PiAmount) -> FeeSplit {
        self.split_fee(total_fee, total_fee)
    }
}

impl Default for FeeCalculator {
    fn default() -> Self {
        Self::new()
    }
}

/// Fee distribution breakdown.
#[derive(Clone, Debug)]
pub struct FeeSplit {
    /// Amount permanently burned.
    pub burned: PiAmount,
    /// Amount flowing to mining pool (miner sustainability).
    pub miners: PiAmount,
    /// Amount distributed to stakers/proposer.
    pub stakers: PiAmount,
    /// Amount to block proposer (from priority fees).
    pub proposer: PiAmount,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_calculation() {
        let calc = FeeCalculator::new();
        let fee = calc.calculate_fee(21_000, 1_000, 100);
        assert_eq!(fee, 21_000 * 1_000 + 21_000 * 100);
    }

    #[test]
    fn fee_calculation_overflow_safe() {
        let calc = FeeCalculator::new();
        let fee = calc.calculate_fee(100_000_000, 100_000_000_000, 50_000_000_000);
        assert!(fee > 0);
    }

    #[test]
    fn burn_rate() {
        let calc = FeeCalculator::new();
        let fee = 10_000;
        let burn = calc.calculate_burn(fee);
        assert_eq!(burn, 3_141); // 31.41% (π × 1000 bps)
    }

    #[test]
    fn miner_rate() {
        let calc = FeeCalculator::new();
        let fee = 10_000;
        let miners = calc.miner_share(fee);
        assert_eq!(miners, 1_859); // 18.59%
    }

    #[test]
    fn fee_split_with_priority() {
        let calc = FeeCalculator::new();
        let total = 11_000;
        let base = 10_000;
        let split = calc.split_fee(total, base);
        // Base fee split: 25% + 25% + 50% = 100% of 10,000
        assert_eq!(split.burned + split.miners + split.stakers, base);
        // Priority fee: 1,000 goes to proposer
        assert_eq!(split.proposer, 1_000);
        // Total must balance
        assert_eq!(
            split.burned + split.miners + split.stakers + split.proposer,
            total
        );
    }

    #[test]
    fn fee_split_sums_correctly() {
        let calc = FeeCalculator::new();
        let total = 10_000;
        let split = calc.split_fee(total, total);
        assert_eq!(
            split.burned + split.miners + split.stakers + split.proposer,
            total
        );
        assert_eq!(split.proposer, 0);
    }

    #[test]
    fn fee_split_rounding_no_pi_lost() {
        let calc = FeeCalculator::new();
        for total in [1, 3, 7, 11, 13, 99, 101, 999, 1001, 9999, 10001, 100003] {
            let split = calc.split_fee(total, total);
            assert_eq!(
                split.burned + split.miners + split.stakers + split.proposer,
                total,
                "fee split does not sum to total for fee={total}"
            );
        }
    }

    #[test]
    fn fee_split_zero_fee() {
        let calc = FeeCalculator::new();
        let split = calc.split_fee(0, 0);
        assert_eq!(split.burned, 0);
        assert_eq!(split.miners, 0);
        assert_eq!(split.stakers, 0);
        assert_eq!(split.proposer, 0);
    }

    #[test]
    fn priority_fee_fully_to_proposer() {
        let calc = FeeCalculator::new();
        let split = calc.split_fee(5_000, 0);
        assert_eq!(split.proposer, 5_000);
        assert_eq!(split.burned, 0);
        assert_eq!(split.miners, 0);
        assert_eq!(split.stakers, 0);
    }

    #[test]
    fn base_and_priority_separate() {
        let calc = FeeCalculator::new();
        let base = calc.calculate_base_fee(21_000, 1_000);
        let priority = calc.calculate_priority_fee(21_000, 100);
        assert_eq!(base, 21_000_000);
        assert_eq!(priority, 2_100_000);
    }
}
