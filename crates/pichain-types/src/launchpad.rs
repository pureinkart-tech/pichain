//! Token launchpad types — one-click meme coin creation with bonding curves.
//!
//! Supports two launch types:
//! - **FairLaunch**: Fixed price, first come first served, with cap.
//! - **BondingCurve**: Price increases with each purchase (pump.fun style).
//!
//! After the target is reached or the creator finalizes, remaining tokens
//! and raised PI are seeded into an AMM pool automatically.

use pichain_crypto::ed25519::Address;
use serde::{Deserialize, Serialize};

use crate::token::MintId;

/// Unique identifier for a token launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct LaunchId(pub [u8; 32]);

impl LaunchId {
    /// Derive a launch ID from mint ID.
    pub fn from_mint(mint: &MintId) -> Self {
        let mut data = Vec::with_capacity(32 + 16);
        data.extend_from_slice(&mint.0);
        data.extend_from_slice(b"pichain-launch");
        let hash = pichain_crypto::hash(&data);
        Self(*hash.as_bytes())
    }
}

impl std::fmt::Display for LaunchId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Launch type — determines the pricing mechanism.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LaunchType {
    /// Fixed price per token. Simple and predictable.
    FairLaunch {
        /// Price per token in base PI units.
        price_per_token: u64,
    },
    /// Bonding curve — price increases with supply purchased.
    /// Price = base_price + slope * tokens_sold
    BondingCurve {
        /// Base price per token in base PI units.
        base_price: u64,
        /// Price increase per token sold (in base PI units per token).
        slope: u64,
    },
}

/// Launch state — tracks the lifecycle of a token launch.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LaunchState {
    /// Accepting contributions.
    Active,
    /// Target reached, pending finalization.
    TargetReached,
    /// Finalized — AMM pool created.
    Finalized,
    /// Cancelled by creator (refunds available).
    Cancelled,
}

/// A token launch — manages the lifecycle of a new token.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenLaunch {
    /// Unique launch identifier.
    pub id: LaunchId,
    /// The token being launched.
    pub mint: MintId,
    /// Launch creator.
    pub creator: Address,
    /// Launch type (pricing mechanism).
    pub launch_type: LaunchType,
    /// Current launch state.
    pub state: LaunchState,
    /// Total tokens allocated for the launch.
    pub tokens_for_sale: u64,
    /// Total tokens sold so far.
    pub tokens_sold: u64,
    /// Total PI raised so far.
    pub pi_raised: u64,
    /// Target PI to raise before auto-finalization.
    pub target_pi: u64,
    /// Percentage of raised PI that goes to the AMM pool (in bps, e.g., 8000 = 80%).
    pub liquidity_bps: u16,
    /// Percentage of tokens reserved for AMM pool (in bps).
    pub token_liquidity_bps: u16,
    /// Maximum PI contribution per address.
    pub max_per_address: u64,
    /// Per-address contribution tracking.
    pub contributions: std::collections::HashMap<Address, u64>,
    /// Creation timestamp.
    pub created_at_ms: u64,
}

impl TokenLaunch {
    /// Standard liquidity: 80% of raised PI goes to the AMM pool.
    pub const DEFAULT_LIQUIDITY_BPS: u16 = 8000;

    /// Standard token liquidity: 20% of unsold tokens go to AMM pool.
    pub const DEFAULT_TOKEN_LIQUIDITY_BPS: u16 = 2000;

    /// Calculate the cost to buy `amount` tokens at current state.
    pub fn calculate_cost(&self, amount: u64) -> Option<u64> {
        if amount == 0 {
            return None;
        }

        let remaining = self.tokens_for_sale.saturating_sub(self.tokens_sold);
        if amount > remaining {
            return None;
        }

        match &self.launch_type {
            LaunchType::FairLaunch { price_per_token } => {
                // Simple: cost = amount * price
                amount.checked_mul(*price_per_token)
            }
            LaunchType::BondingCurve { base_price, slope } => {
                // Integral of (base_price + slope * x) from tokens_sold to tokens_sold + amount
                // Cost = base_price * amount + slope * (amount * (2*tokens_sold + amount - 1)) / 2
                let amount = amount as u128;
                let sold = self.tokens_sold as u128;
                let base = *base_price as u128;
                let s = *slope as u128;

                let linear_cost = match base.checked_mul(amount) {
                    Some(v) => v,
                    None => return None,
                };
                // Sum of slope * x from sold to sold+amount-1
                // = slope * sum(x, x=sold..sold+amount-1)
                // = slope * (amount * (2*sold + amount - 1) / 2)
                // Ceiling division: (a + b - 1) / b
                let inner = match (2u128.checked_mul(sold))
                    .and_then(|v| v.checked_add(amount))
                    .and_then(|v| v.checked_sub(1))
                    .and_then(|v| v.checked_mul(amount))
                    .and_then(|v| v.checked_mul(s))
                {
                    Some(v) => (v + 1) / 2,  // Changed from `v / 2` to `(v + 1) / 2` (ceiling)
                    None => return None,
                };
                let curve_cost = inner;

                let total = match linear_cost.checked_add(curve_cost) {
                    Some(v) => v,
                    None => return None,
                };

                // Check it fits in u64
                if total > u64::MAX as u128 {
                    None
                } else {
                    Some(total as u64)
                }
            }
        }
    }

    /// Calculate how many tokens can be bought with the given PI amount.
    pub fn tokens_for_pi(&self, pi_amount: u64) -> u64 {
        if pi_amount == 0 {
            return 0;
        }

        let remaining = self.tokens_for_sale.saturating_sub(self.tokens_sold);
        if remaining == 0 {
            return 0;
        }

        match &self.launch_type {
            LaunchType::FairLaunch { price_per_token } => {
                if *price_per_token == 0 {
                    return 0;
                }
                let tokens = pi_amount / price_per_token;
                std::cmp::min(tokens, remaining)
            }
            LaunchType::BondingCurve { base_price, slope } => {
                // Guard against zero-cost infinite loop
                if *base_price == 0 && *slope == 0 {
                    return 0;
                }

                // Binary search for the maximum tokens buyable
                let mut lo: u64 = 0;
                let mut hi = remaining;

                while lo < hi {
                    let mid = lo + (hi - lo + 1) / 2;
                    match self.calculate_cost(mid) {
                        Some(cost) if cost <= pi_amount => lo = mid,
                        _ => hi = mid - 1,
                    }
                }

                lo
            }
        }
    }

    /// Calculate the current price per token.
    pub fn current_price(&self) -> u64 {
        match &self.launch_type {
            LaunchType::FairLaunch { price_per_token } => *price_per_token,
            LaunchType::BondingCurve { base_price, slope } => {
                // Use u128 to prevent overflow on slope * tokens_sold
                let price = (*base_price as u128)
                    .saturating_add((*slope as u128).saturating_mul(self.tokens_sold as u128));
                // Cap at u64::MAX if overflow
                if price > u64::MAX as u128 { u64::MAX } else { price as u64 }
            }
        }
    }

    /// Calculate amounts for AMM pool seeding at finalization.
    /// Returns (pi_for_pool, tokens_for_pool).
    pub fn finalization_amounts(&self) -> (u64, u64) {
        let pi_for_pool_u128 = self.pi_raised as u128 * self.liquidity_bps as u128 / 10_000;
        let pi_for_pool = if pi_for_pool_u128 > u64::MAX as u128 {
            u64::MAX
        } else {
            pi_for_pool_u128 as u64
        };

        let unsold = self.tokens_for_sale.saturating_sub(self.tokens_sold);
        // Only unsold tokens go to the pool (percentage of unsold)
        let tokens_for_pool = unsold as u128 * self.token_liquidity_bps as u128 / 10_000;

        // Cap tokens_for_pool to what's available
        let tokens_for_pool = std::cmp::min(tokens_for_pool as u64, unsold);

        (pi_for_pool, tokens_for_pool)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fair_launch() -> TokenLaunch {
        let mint = MintId::derive(&Address([1u8; 20]), 0);
        TokenLaunch {
            id: LaunchId::from_mint(&mint),
            mint,
            creator: Address([1u8; 20]),
            launch_type: LaunchType::FairLaunch {
                price_per_token: 1_000, // 0.000001 PI per token
            },
            state: LaunchState::Active,
            tokens_for_sale: 1_000_000,
            tokens_sold: 0,
            pi_raised: 0,
            target_pi: 1_000_000_000, // 1 PI target
            liquidity_bps: TokenLaunch::DEFAULT_LIQUIDITY_BPS,
            token_liquidity_bps: TokenLaunch::DEFAULT_TOKEN_LIQUIDITY_BPS,
            max_per_address: 100_000_000, // 0.1 PI max
            contributions: std::collections::HashMap::new(),
            created_at_ms: 0,
        }
    }

    fn make_bonding_launch() -> TokenLaunch {
        let mint = MintId::derive(&Address([2u8; 20]), 0);
        TokenLaunch {
            id: LaunchId::from_mint(&mint),
            mint,
            creator: Address([2u8; 20]),
            launch_type: LaunchType::BondingCurve {
                base_price: 100,
                slope: 1,
            },
            state: LaunchState::Active,
            tokens_for_sale: 10_000,
            tokens_sold: 0,
            pi_raised: 0,
            target_pi: 10_000_000,
            liquidity_bps: TokenLaunch::DEFAULT_LIQUIDITY_BPS,
            token_liquidity_bps: TokenLaunch::DEFAULT_TOKEN_LIQUIDITY_BPS,
            max_per_address: u64::MAX,
            contributions: std::collections::HashMap::new(),
            created_at_ms: 0,
        }
    }

    #[test]
    fn fair_launch_cost() {
        let launch = make_fair_launch();
        assert_eq!(launch.calculate_cost(100).unwrap(), 100_000); // 100 * 1000
        assert_eq!(launch.calculate_cost(1_000_000).unwrap(), 1_000_000_000); // 1M * 1000
        assert!(launch.calculate_cost(1_000_001).is_none()); // Exceeds supply
    }

    #[test]
    fn fair_launch_tokens_for_pi() {
        let launch = make_fair_launch();
        assert_eq!(launch.tokens_for_pi(100_000), 100); // 100_000 / 1000
        assert_eq!(launch.tokens_for_pi(0), 0);
    }

    #[test]
    fn bonding_curve_cost() {
        let launch = make_bonding_launch();
        // First token costs base_price = 100
        assert_eq!(launch.calculate_cost(1).unwrap(), 100);
        // First 10 tokens: sum(100 + i for i in 0..10) = 100*10 + 0+1+2+...+9 = 1000 + 45 = 1045
        assert_eq!(launch.calculate_cost(10).unwrap(), 1045);
    }

    #[test]
    fn bonding_curve_price_increases() {
        let mut launch = make_bonding_launch();
        let price1 = launch.current_price();
        launch.tokens_sold = 1000;
        let price2 = launch.current_price();
        assert!(price2 > price1);
    }

    #[test]
    fn bonding_curve_tokens_for_pi() {
        let launch = make_bonding_launch();
        // With base_price=100, slope=1:
        // Cost of N tokens = 100*N + N*(N-1)/2
        // For 100 PI: 100*N + N*(N-1)/2 <= 100
        // N=1: cost=100, yes
        let tokens = launch.tokens_for_pi(100);
        assert_eq!(tokens, 1);

        // For 1045 PI, should get 10 tokens
        let tokens = launch.tokens_for_pi(1045);
        assert_eq!(tokens, 10);
    }

    #[test]
    fn launch_id_deterministic() {
        let mint = MintId::derive(&Address([1u8; 20]), 0);
        let id1 = LaunchId::from_mint(&mint);
        let id2 = LaunchId::from_mint(&mint);
        assert_eq!(id1, id2);
    }

    #[test]
    fn finalization_amounts() {
        let mut launch = make_fair_launch();
        launch.tokens_sold = 500_000;
        launch.pi_raised = 500_000_000;

        let (pi_pool, tokens_pool) = launch.finalization_amounts();
        // 80% of 500M PI raised
        assert_eq!(pi_pool, 400_000_000);
        // tokens_pool should be > 0 (some unsold + some from allocation)
        assert!(tokens_pool > 0);
    }

    #[test]
    fn max_contribution_per_address() {
        let launch = make_fair_launch();
        assert_eq!(launch.max_per_address, 100_000_000);
    }

    #[test]
    fn bonding_curve_cost_consistency() {
        let launch = make_bonding_launch();
        // For any PI amount, tokens_for_pi should give tokens whose cost <= the PI amount
        for pi in [100, 500, 1000, 5000, 10000, 50000, 100000] {
            let tokens = launch.tokens_for_pi(pi);
            if tokens > 0 {
                let cost = launch.calculate_cost(tokens).unwrap();
                assert!(
                    cost <= pi,
                    "inconsistency: {tokens} tokens cost {cost} but tokens_for_pi({pi}) returned {tokens}"
                );
            }
        }
    }
}
