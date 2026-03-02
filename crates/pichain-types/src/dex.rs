//! DEX/AMM types — native constant-product automated market maker.
//!
//! Enables trading any token against any other token (including native PI).
//! Pools use the x*y=k invariant for price discovery.
//!
//! Native PI is represented as `MintId::ZERO`, allowing unified treatment
//! in pools and swaps without special-casing.

use pichain_crypto::ed25519::Address;
use serde::{Deserialize, Serialize};

use crate::token::MintId;

/// Unique identifier for a liquidity pool.
/// Derived from the canonical hash of the two token mints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct PoolId(pub [u8; 32]);

impl PoolId {
    /// Derive a canonical pool ID from two mints.
    /// `pool_id(A, B) == pool_id(B, A)` — mints are sorted before hashing.
    pub fn derive(mint_a: &MintId, mint_b: &MintId) -> Self {
        let (first, second) = if mint_a <= mint_b {
            (mint_a, mint_b)
        } else {
            (mint_b, mint_a)
        };

        let mut data = Vec::with_capacity(64 + 12);
        data.extend_from_slice(&first.0);
        data.extend_from_slice(&second.0);
        data.extend_from_slice(b"pichain-pool");
        let hash = pichain_crypto::hash(&data);
        Self(*hash.as_bytes())
    }
}

impl std::fmt::Display for PoolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// A liquidity pool — holds reserves of two tokens.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LiquidityPool {
    /// Unique pool identifier.
    pub id: PoolId,
    /// First token mint (always the "smaller" MintId for canonical ordering).
    pub mint_a: MintId,
    /// Second token mint.
    pub mint_b: MintId,
    /// Reserve of token A.
    pub reserve_a: u64,
    /// Reserve of token B.
    pub reserve_b: u64,
    /// Total LP token supply issued to liquidity providers.
    pub lp_supply: u64,
    /// Fee in basis points (e.g., 30 = 0.30%).
    pub fee_bps: u16,
    /// Pool creator address.
    pub creator: Address,
    /// Whether the pool is active for trading.
    pub active: bool,
    /// Creation timestamp.
    pub created_at_ms: u64,
    /// Cumulative volume of token A traded through this pool.
    pub cumulative_volume_a: u128,
    /// Cumulative volume of token B traded through this pool.
    pub cumulative_volume_b: u128,
}

impl LiquidityPool {
    /// Standard fee: 0.30% (matching Uniswap V2).
    pub const DEFAULT_FEE_BPS: u16 = 30;

    /// Minimum liquidity locked forever to prevent manipulation (like Uniswap).
    pub const MINIMUM_LIQUIDITY: u64 = 1_000;

    /// Create a new liquidity pool.
    pub fn new(mint_a: MintId, mint_b: MintId, creator: Address, created_at_ms: u64) -> Self {
        // Canonical ordering
        let (first, second) = if mint_a <= mint_b {
            (mint_a, mint_b)
        } else {
            (mint_b, mint_a)
        };

        let id = PoolId::derive(&first, &second);

        Self {
            id,
            mint_a: first,
            mint_b: second,
            reserve_a: 0,
            reserve_b: 0,
            lp_supply: 0,
            fee_bps: Self::DEFAULT_FEE_BPS,
            creator,
            active: true,
            created_at_ms,
            cumulative_volume_a: 0,
            cumulative_volume_b: 0,
        }
    }

    /// Calculate the output amount for a swap using constant-product formula.
    /// Returns (amount_out, fee_amount).
    ///
    /// Formula: amount_out = (reserve_out * amount_in_after_fee) / (reserve_in + amount_in_after_fee)
    pub fn calculate_swap_output(
        &self,
        amount_in: u64,
        is_a_to_b: bool,
    ) -> Option<(u64, u64)> {
        if amount_in == 0 {
            return None;
        }
        if self.fee_bps > 10_000 {
            return None;
        }

        let (reserve_in, reserve_out) = if is_a_to_b {
            (self.reserve_a as u128, self.reserve_b as u128)
        } else {
            (self.reserve_b as u128, self.reserve_a as u128)
        };

        if reserve_in == 0 || reserve_out == 0 {
            return None;
        }

        let amount_in = amount_in as u128;

        // R28-FIX: Use ceiling division to prevent zero-fee micro-swaps.
        // Without this, swaps with amount_in <= 333 (at 30bps) pay zero fees,
        // enabling fee-free arbitrage via many small swaps.
        let fee_amount = (amount_in * self.fee_bps as u128).div_ceil(10_000);
        let amount_in_after_fee = amount_in.checked_sub(fee_amount)?;

        // Constant product: x * y = k
        // amount_out = reserve_out * amount_in_after_fee / (reserve_in + amount_in_after_fee)
        let numerator = reserve_out * amount_in_after_fee;
        let denominator = reserve_in + amount_in_after_fee;

        if denominator == 0 {
            return None;
        }

        let amount_out = numerator / denominator;

        // Output must be less than total reserve
        if amount_out >= reserve_out || amount_out == 0 {
            return None;
        }

        // Verify constant product invariant: k_new >= k_old
        // After the swap, the new reserves must maintain x*y=k (with fees, k only grows).
        let k_before = reserve_in * reserve_out;
        let k_after = (reserve_in + amount_in_after_fee) * (reserve_out - amount_out);
        if k_after < k_before {
            return None; // Invariant violated — rounding error too large
        }

        // Safe to cast back — both are bounded by reserves which are u64
        Some((amount_out as u64, fee_amount as u64))
    }

    /// Calculate the price impact of a swap in basis points.
    pub fn price_impact_bps(&self, amount_in: u64, is_a_to_b: bool) -> u64 {
        let (reserve_in, reserve_out) = if is_a_to_b {
            (self.reserve_a as u128, self.reserve_b as u128)
        } else {
            (self.reserve_b as u128, self.reserve_a as u128)
        };

        if reserve_in == 0 || reserve_out == 0 || amount_in == 0 {
            return 10_000; // 100% impact
        }

        // Impact = 1 - (execution_price / spot_price)
        // Use cross-multiplication to avoid truncation with extreme reserve ratios.
        // spot = reserve_out / reserve_in, exec = amount_out / amount_in
        // impact = 1 - (exec / spot) = 1 - (amount_out * reserve_in) / (amount_in * reserve_out)
        if let Some((amount_out, _)) = self.calculate_swap_output(amount_in, is_a_to_b) {
            let exec_num = amount_out as u128 * reserve_in; // exec * denominator
            let spot_num = amount_in as u128 * reserve_out; // spot * denominator

            if spot_num == 0 {
                return 10_000;
            }

            let impact = if spot_num > exec_num {
                let diff = spot_num - exec_num;
                // Divide first to avoid u128 overflow on multiply
                if spot_num >= 10_000 {
                    (diff / (spot_num / 10_000)).min(10_000)
                } else {
                    // Safe for small values — no overflow risk
                    diff * 10_000 / spot_num
                }
            } else {
                0
            };

            impact as u64
        } else {
            10_000 // Max impact if swap isn't possible
        }
    }

    /// Calculate LP tokens to mint when adding liquidity.
    /// Returns (lp_tokens, actual_amount_a, actual_amount_b).
    pub fn calculate_add_liquidity(
        &self,
        amount_a: u64,
        amount_b: u64,
    ) -> Option<(u64, u64, u64)> {
        if amount_a == 0 || amount_b == 0 {
            return None;
        }

        if self.reserve_a == 0 && self.reserve_b == 0 {
            // First liquidity — LP tokens = sqrt(amount_a * amount_b)
            let product = amount_a as u128 * amount_b as u128;
            let lp = isqrt(product);

            if lp <= Self::MINIMUM_LIQUIDITY as u128 {
                return None; // Too small
            }

            // Subtract minimum liquidity (locked forever)
            let lp_minted = (lp - Self::MINIMUM_LIQUIDITY as u128) as u64;
            Some((lp_minted, amount_a, amount_b))
        } else if self.reserve_a == 0 || self.reserve_b == 0 {
            // One reserve is zero but not both — corrupted/drained pool state.
            // Cannot calculate LP proportionally; reject to prevent division by zero.
            None
        } else {
            // Subsequent liquidity — maintain ratio
            let reserve_a = self.reserve_a as u128;
            let reserve_b = self.reserve_b as u128;
            let supply = self.lp_supply as u128 + Self::MINIMUM_LIQUIDITY as u128;

            // Calculate optimal amounts
            let optimal_b = amount_a as u128 * reserve_b / reserve_a;

            let (actual_a, actual_b) = if optimal_b <= amount_b as u128 {
                (amount_a as u128, optimal_b)
            } else {
                let optimal_a = amount_b as u128 * reserve_a / reserve_b;
                (optimal_a, amount_b as u128)
            };

            if actual_a == 0 || actual_b == 0 {
                return None;
            }

            // LP tokens proportional to contribution
            let lp_a = actual_a * supply / reserve_a;
            let lp_b = actual_b * supply / reserve_b;
            let lp_minted = std::cmp::min(lp_a, lp_b);

            if lp_minted == 0 {
                return None;
            }

            Some((lp_minted as u64, actual_a as u64, actual_b as u64))
        }
    }

    /// Calculate tokens returned when removing liquidity.
    /// Returns (amount_a, amount_b).
    pub fn calculate_remove_liquidity(&self, lp_amount: u64) -> Option<(u64, u64)> {
        if lp_amount == 0 || self.lp_supply == 0 {
            return None;
        }

        let total_supply = self.lp_supply as u128 + Self::MINIMUM_LIQUIDITY as u128;
        let amount_a = self.reserve_a as u128 * lp_amount as u128 / total_supply;
        let amount_b = self.reserve_b as u128 * lp_amount as u128 / total_supply;

        if amount_a == 0 || amount_b == 0 {
            return None;
        }

        Some((amount_a as u64, amount_b as u64))
    }
}

/// Integer square root (Newton's method).
pub fn isqrt(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_id_canonical() {
        let mint_a = MintId::derive(&Address([1u8; 20]), 0);
        let mint_b = MintId::derive(&Address([2u8; 20]), 0);

        // Order shouldn't matter
        let id1 = PoolId::derive(&mint_a, &mint_b);
        let id2 = PoolId::derive(&mint_b, &mint_a);
        assert_eq!(id1, id2);
    }

    #[test]
    fn pool_id_unique_per_pair() {
        let mint_a = MintId::derive(&Address([1u8; 20]), 0);
        let mint_b = MintId::derive(&Address([2u8; 20]), 0);
        let mint_c = MintId::derive(&Address([3u8; 20]), 0);

        let id_ab = PoolId::derive(&mint_a, &mint_b);
        let id_ac = PoolId::derive(&mint_a, &mint_c);
        assert_ne!(id_ab, id_ac);
    }

    #[test]
    fn swap_calculation() {
        let mut pool = LiquidityPool::new(MintId::ZERO, MintId::derive(&Address([1u8; 20]), 0), Address([1u8; 20]), 0);
        pool.reserve_a = 1_000_000;
        pool.reserve_b = 1_000_000;
        pool.lp_supply = 1_000_000;

        // Swap 1000 tokens A → B (with 0.30% fee)
        let (out, fee) = pool.calculate_swap_output(1000, true).unwrap();

        // Fee should be ~3 (0.30% of 1000)
        assert_eq!(fee, 3);

        // Output should be slightly less than 997 due to x*y=k curve
        assert!(out > 0 && out < 1000);

        // Verify constant product: new_reserve_a * new_reserve_b >= old_k
        let old_k = pool.reserve_a as u128 * pool.reserve_b as u128;
        let new_a = pool.reserve_a as u128 + 1000;
        let new_b = pool.reserve_b as u128 - out as u128;
        assert!(new_a * new_b >= old_k);
    }

    #[test]
    fn swap_empty_pool_fails() {
        let pool = LiquidityPool::new(MintId::ZERO, MintId::derive(&Address([1u8; 20]), 0), Address([1u8; 20]), 0);
        assert!(pool.calculate_swap_output(1000, true).is_none());
    }

    #[test]
    fn swap_zero_amount_fails() {
        let mut pool = LiquidityPool::new(MintId::ZERO, MintId::derive(&Address([1u8; 20]), 0), Address([1u8; 20]), 0);
        pool.reserve_a = 1_000_000;
        pool.reserve_b = 1_000_000;
        assert!(pool.calculate_swap_output(0, true).is_none());
    }

    #[test]
    fn first_liquidity_provision() {
        let pool = LiquidityPool::new(MintId::ZERO, MintId::derive(&Address([1u8; 20]), 0), Address([1u8; 20]), 0);

        let (lp_tokens, actual_a, actual_b) = pool.calculate_add_liquidity(1_000_000, 1_000_000).unwrap();

        // LP tokens = sqrt(1M * 1M) - MINIMUM_LIQUIDITY = 1M - 1000
        assert_eq!(actual_a, 1_000_000);
        assert_eq!(actual_b, 1_000_000);
        assert_eq!(lp_tokens, 1_000_000 - LiquidityPool::MINIMUM_LIQUIDITY);
    }

    #[test]
    fn subsequent_liquidity_maintains_ratio() {
        let mut pool = LiquidityPool::new(MintId::ZERO, MintId::derive(&Address([1u8; 20]), 0), Address([1u8; 20]), 0);
        pool.reserve_a = 1_000_000;
        pool.reserve_b = 2_000_000;
        pool.lp_supply = 1_000_000;

        // Add liquidity with correct ratio (1:2)
        let (lp_tokens, actual_a, actual_b) = pool.calculate_add_liquidity(100_000, 200_000).unwrap();

        assert_eq!(actual_a, 100_000);
        assert_eq!(actual_b, 200_000);
        assert!(lp_tokens > 0);
    }

    #[test]
    fn remove_liquidity() {
        let mut pool = LiquidityPool::new(MintId::ZERO, MintId::derive(&Address([1u8; 20]), 0), Address([1u8; 20]), 0);
        pool.reserve_a = 1_000_000;
        pool.reserve_b = 2_000_000;
        pool.lp_supply = 1_000_000;

        // Remove 10% of LP supply
        let (amount_a, amount_b) = pool.calculate_remove_liquidity(100_000).unwrap();

        // Should get roughly 10% of each reserve (accounting for MINIMUM_LIQUIDITY)
        assert!(amount_a > 0 && amount_a <= 100_000);
        assert!(amount_b > 0 && amount_b <= 200_000);
    }

    #[test]
    fn price_impact_increases_with_size() {
        let mut pool = LiquidityPool::new(MintId::ZERO, MintId::derive(&Address([1u8; 20]), 0), Address([1u8; 20]), 0);
        pool.reserve_a = 1_000_000;
        pool.reserve_b = 1_000_000;

        let small_impact = pool.price_impact_bps(1_000, true);
        let large_impact = pool.price_impact_bps(100_000, true);

        assert!(large_impact > small_impact);
    }

    #[test]
    fn isqrt_correctness() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(1), 1);
        assert_eq!(isqrt(4), 2);
        assert_eq!(isqrt(9), 3);
        assert_eq!(isqrt(1_000_000), 1_000);
        assert_eq!(isqrt(1_000_000_000_000), 1_000_000);
    }

    #[test]
    fn native_pi_pool() {
        // PI paired with a custom token
        let custom = MintId::derive(&Address([1u8; 20]), 0);
        let pool = LiquidityPool::new(MintId::ZERO, custom, Address([1u8; 20]), 0);

        assert!(pool.mint_a.is_native_pi() || pool.mint_b.is_native_pi());
    }
}
