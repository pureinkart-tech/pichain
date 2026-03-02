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

/// Serde helper for HashMap<Address, u64> — serializes Address keys as hex strings.
mod address_map_serde {
    use super::*;
    use serde::ser::SerializeMap;
    use std::collections::HashMap;

    pub fn serialize<S>(map: &HashMap<Address, u64>, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer {
        let mut m = serializer.serialize_map(Some(map.len()))?;
        for (addr, val) in map {
            m.serialize_entry(&hex::encode(addr.0), val)?;
        }
        m.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<HashMap<Address, u64>, D::Error>
    where D: serde::Deserializer<'de> {
        let string_map: HashMap<String, u64> = HashMap::deserialize(deserializer)?;
        let mut result = HashMap::with_capacity(string_map.len());
        for (hex_str, val) in string_map {
            let bytes = hex::decode(&hex_str).map_err(serde::de::Error::custom)?;
            if bytes.len() != 20 {
                return Err(serde::de::Error::custom(format!("address must be 20 bytes, got {}", bytes.len())));
            }
            let mut addr = [0u8; 20];
            addr.copy_from_slice(&bytes);
            result.insert(Address(addr), val);
        }
        Ok(result)
    }
}

fn default_price_scale() -> u64 {
    1
}

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
    /// Price = base_price + slope * display_tokens_sold
    /// Actual cost = (base_price * display_amount + slope * display_amount * (2*display_sold + display_amount - 1) / 2) / price_scale
    BondingCurve {
        /// Base price per display token (scaled by price_scale).
        base_price: u64,
        /// Price increase per display token sold (scaled by price_scale).
        slope: u64,
        /// Divisor for price calculations. Enables fractional pricing for tokens
        /// with large supplies. Default: 1 (backward compatible).
        #[serde(default = "default_price_scale")]
        price_scale: u64,
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
    #[serde(with = "address_map_serde")]
    pub contributions: std::collections::HashMap<Address, u64>,
    /// Creation timestamp.
    pub created_at_ms: u64,
    /// Token decimals (for display-unit bonding curve math).
    #[serde(default)]
    pub token_decimals: u8,
}

impl TokenLaunch {
    /// Standard liquidity: 80% of raised PI goes to the AMM pool.
    pub const DEFAULT_LIQUIDITY_BPS: u16 = 8000;

    /// Standard token liquidity: 20% of unsold tokens go to AMM pool.
    pub const DEFAULT_TOKEN_LIQUIDITY_BPS: u16 = 2000;

    /// Calculate the cost to buy `amount` tokens (in base units) at current state.
    /// Returns the cost in base PI units.
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
            LaunchType::BondingCurve { base_price, slope, price_scale } => {
                let ps = (*price_scale).max(1) as u128;

                // Convert to display units if token has decimals
                let scale = if self.token_decimals > 0 {
                    10u128.pow(self.token_decimals as u32)
                } else {
                    1u128
                };
                let a = (amount as u128) / scale;
                let s = (self.tokens_sold as u128) / scale;

                if a == 0 {
                    // Amount too small for one display token — minimum cost
                    return Some(1);
                }

                let bp = *base_price as u128;
                let sl = *slope as u128;

                // Cost = (base_price * a + slope * a * (2*s + a - 1) / 2) / price_scale
                let linear_cost = bp.checked_mul(a)?;
                let inner = match (2u128.checked_mul(s))
                    .and_then(|v| v.checked_add(a))
                    .and_then(|v| v.checked_sub(1))
                    .and_then(|v| v.checked_mul(a))
                    .and_then(|v| v.checked_mul(sl))
                {
                    Some(v) => v.div_ceil(2),
                    None => return None,
                };

                let total = match linear_cost.checked_add(inner) {
                    Some(v) => v / ps,
                    None => return None,
                };

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
            LaunchType::BondingCurve { base_price, slope, .. } => {
                // Guard against zero-cost infinite loop
                if *base_price == 0 && *slope == 0 {
                    return 0;
                }

                // Binary search for the maximum tokens buyable.
                // calculate_cost() already handles display-unit conversion and price_scale.
                let mut lo: u64 = 0;
                let mut hi = remaining;

                while lo < hi {
                    let mid = lo + (hi - lo).div_ceil(2);
                    match self.calculate_cost(mid) {
                        Some(cost) if cost <= pi_amount => lo = mid,
                        _ => hi = mid - 1,
                    }
                }

                lo
            }
        }
    }

    /// Calculate the current price per display token (in base PI units).
    pub fn current_price(&self) -> u64 {
        match &self.launch_type {
            LaunchType::FairLaunch { price_per_token } => *price_per_token,
            LaunchType::BondingCurve { base_price, slope, price_scale } => {
                let ps = (*price_scale).max(1) as u128;

                // Convert tokens_sold to display units
                let scale = if self.token_decimals > 0 {
                    10u128.pow(self.token_decimals as u32)
                } else {
                    1u128
                };
                let s = (self.tokens_sold as u128) / scale;

                // Price per display token = (base_price + slope * display_sold) / price_scale
                let price = (*base_price as u128)
                    .saturating_add((*slope as u128).saturating_mul(s));
                let price = price / ps;

                if price > u64::MAX as u128 { u64::MAX } else { price as u64 }
            }
        }
    }

    /// Calculate PI returned for selling `token_amount` back to the bonding curve.
    /// This is the reverse of calculate_cost: integrates the curve from
    /// (tokens_sold - token_amount) to tokens_sold.
    pub fn calculate_sell_return(&self, token_amount: u64) -> Option<u64> {
        if token_amount == 0 || token_amount > self.tokens_sold {
            return None;
        }

        match &self.launch_type {
            LaunchType::FairLaunch { price_per_token } => {
                token_amount.checked_mul(*price_per_token)
            }
            LaunchType::BondingCurve { base_price, slope, price_scale } => {
                let ps = (*price_scale).max(1) as u128;
                let scale = if self.token_decimals > 0 {
                    10u128.pow(self.token_decimals as u32)
                } else {
                    1u128
                };

                let a = (token_amount as u128) / scale; // display tokens to sell
                let s = (self.tokens_sold as u128) / scale; // current display tokens sold
                let s_new = s.checked_sub(a)?; // display tokens after sell

                if a == 0 {
                    return Some(0);
                }

                let bp = *base_price as u128;
                let sl = *slope as u128;

                // Return = integral from s_new to s of price(x) dx
                // = (base_price * a + slope * a * (2*s_new + a - 1) / 2) / price_scale
                let linear = bp.checked_mul(a)?;
                let inner = (2u128.checked_mul(s_new)?)
                    .checked_add(a)?
                    .checked_sub(1)?
                    .checked_mul(a)?
                    .checked_mul(sl)?;
                let inner = inner.div_ceil(2);
                let total = linear.checked_add(inner)? / ps;

                if total > u64::MAX as u128 { None } else { Some(total as u64) }
            }
        }
    }

    /// Calculate amounts for AMM pool seeding at finalization.
    /// Returns (pi_for_pool, tokens_for_pool).
    ///
    /// When unsold tokens remain, a percentage of them seeds the pool.
    /// When ALL tokens are sold (bonding curve fully subscribed), new tokens
    /// are minted as a percentage of total supply to seed the pool — this is
    /// the standard pump.fun-style graduation model.
    pub fn finalization_amounts(&self) -> (u64, u64) {
        let pi_for_pool_u128 = self.pi_raised as u128 * self.liquidity_bps as u128 / 10_000;
        let pi_for_pool = if pi_for_pool_u128 > u64::MAX as u128 {
            u64::MAX
        } else {
            pi_for_pool_u128 as u64
        };

        let unsold = self.tokens_for_sale.saturating_sub(self.tokens_sold);

        let tokens_for_pool = if unsold > 0 {
            // Use percentage of unsold tokens
            let t = unsold as u128 * self.token_liquidity_bps as u128 / 10_000;
            std::cmp::min(t as u64, unsold)
        } else {
            // All tokens sold — mint new tokens as percentage of total sale
            let t = self.tokens_for_sale as u128 * self.token_liquidity_bps as u128 / 10_000;
            if t > u64::MAX as u128 { u64::MAX } else { t as u64 }
        };

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
            token_decimals: 0,
        }
    }

    /// Simple bonding curve with no decimals (legacy-compatible: price_scale=1, token_decimals=0)
    fn make_bonding_launch() -> TokenLaunch {
        let mint = MintId::derive(&Address([2u8; 20]), 0);
        TokenLaunch {
            id: LaunchId::from_mint(&mint),
            mint,
            creator: Address([2u8; 20]),
            launch_type: LaunchType::BondingCurve {
                base_price: 100,
                slope: 1,
                price_scale: 1,
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
            token_decimals: 0,
        }
    }

    /// Realistic bonding curve: 1B tokens with 9 decimals, price_scale enables fractional pricing.
    /// Target: ~1 PI for 2% of supply, ~100 PI total.
    fn make_scaled_bonding_launch() -> TokenLaunch {
        let mint = MintId::derive(&Address([3u8; 20]), 0);
        // 1B display tokens * 10^9 = 10^18 base units
        // 80% for sale = 800M display tokens = 8 * 10^17 base units
        let tokens_for_sale: u64 = 800_000_000 * 1_000_000_000; // 8e17
        TokenLaunch {
            id: LaunchId::from_mint(&mint),
            mint,
            creator: Address([3u8; 20]),
            launch_type: LaunchType::BondingCurve {
                // price_scale = 1e9 to allow sub-integer base_price per display token
                // base_price / price_scale = 62.5 base PI per display token at start
                base_price: 62_500_000_000, // 6.25e10
                slope: 156,
                price_scale: 1_000_000_000, // 1e9
            },
            state: LaunchState::Active,
            tokens_for_sale,
            tokens_sold: 0,
            pi_raised: 0,
            target_pi: 100_000_000_000, // 100 PI
            liquidity_bps: TokenLaunch::DEFAULT_LIQUIDITY_BPS,
            token_liquidity_bps: TokenLaunch::DEFAULT_TOKEN_LIQUIDITY_BPS,
            max_per_address: u64::MAX,
            contributions: std::collections::HashMap::new(),
            created_at_ms: 0,
            token_decimals: 9,
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

    #[test]
    fn scaled_bonding_curve_2_percent_costs_about_1_pi() {
        let launch = make_scaled_bonding_launch();
        // 2% of 800M display tokens = 16M display tokens = 16e15 base units
        let two_percent_base = 16_000_000u64 * 1_000_000_000; // 1.6e16
        let cost = launch.calculate_cost(two_percent_base).unwrap();
        // Should be roughly 1-2 PI (1e9 to 2e9 base PI)
        assert!(
            cost >= 500_000_000 && cost <= 3_000_000_000,
            "2% should cost ~1 PI, got {} ({:.3} PI)",
            cost, cost as f64 / 1e9
        );
    }

    #[test]
    fn scaled_bonding_curve_total_about_100_pi() {
        let launch = make_scaled_bonding_launch();
        // Cost of all tokens
        let total_cost = launch.calculate_cost(launch.tokens_for_sale).unwrap();
        // Should be roughly 50-150 PI (target_pi = 100 PI)
        assert!(
            total_cost >= 30_000_000_000 && total_cost <= 200_000_000_000,
            "total cost should be ~100 PI, got {} ({:.1} PI)",
            total_cost, total_cost as f64 / 1e9
        );
    }

    #[test]
    fn scaled_bonding_curve_tokens_for_pi() {
        let launch = make_scaled_bonding_launch();
        // 1 PI should buy roughly 2% of tokens
        let one_pi = 1_000_000_000u64;
        let tokens = launch.tokens_for_pi(one_pi);
        let pct = tokens as f64 / launch.tokens_for_sale as f64 * 100.0;
        assert!(
            pct >= 0.5 && pct <= 5.0,
            "1 PI should buy ~2% of tokens, got {:.2}% ({} tokens)",
            pct, tokens
        );
    }

    #[test]
    fn scaled_bonding_curve_price_increases() {
        let mut launch = make_scaled_bonding_launch();
        let price1 = launch.current_price();
        // Sell 10% of tokens
        launch.tokens_sold = launch.tokens_for_sale / 10;
        let price2 = launch.current_price();
        assert!(
            price2 > price1,
            "price should increase after sales: {} vs {}",
            price1, price2
        );
    }

    #[test]
    fn scaled_bonding_curve_cost_consistency() {
        let launch = make_scaled_bonding_launch();
        let one_pi = 1_000_000_000u64;
        for pi_amount in [one_pi / 10, one_pi, one_pi * 5, one_pi * 10, one_pi * 50] {
            let tokens = launch.tokens_for_pi(pi_amount);
            if tokens > 0 {
                let cost = launch.calculate_cost(tokens).unwrap();
                assert!(
                    cost <= pi_amount,
                    "inconsistency: {} tokens cost {} but tokens_for_pi({}) returned {}",
                    tokens, cost, pi_amount, tokens
                );
            }
        }
    }
}
