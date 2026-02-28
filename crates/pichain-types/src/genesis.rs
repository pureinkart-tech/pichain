//! PIChain genesis configuration.
//!
//! Defines the initial state of the blockchain at genesis,
//! including token distribution and initial validators.

use std::collections::HashSet;
use pichain_crypto::ed25519::Address;
use serde::{Deserialize, Serialize};

use crate::{PiAmount, BASE_UNITS_PER_PI, TOTAL_SUPPLY};

/// Genesis configuration for PIChain.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenesisConfig {
    /// Chain identifier.
    pub chain_id: u64,
    /// Genesis timestamp (milliseconds since Unix epoch).
    pub timestamp_ms: u64,
    /// Initial token distribution.
    pub allocations: Vec<GenesisAllocation>,
    /// Initial validator set.
    pub validators: Vec<GenesisValidator>,
}

/// A genesis token allocation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenesisAllocation {
    /// Recipient address.
    pub address: Address,
    /// Amount in base PI units.
    pub amount: PiAmount,
    /// Label for this allocation.
    pub label: String,
    /// AUDIT-FIX H-9: If true, this allocation is virtual — no real balance is created.
    /// Used for the mining pool: the 85% is tracked in RewardCalculator and minted
    /// over time via mining rewards. Without this flag, the mining pool would be
    /// double-counted (real balance + minted rewards = 170% of intended allocation).
    #[serde(default)]
    pub virtual_pool: bool,
}

/// A genesis validator.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenesisValidator {
    /// Validator address.
    pub address: Address,
    /// Initial stake in base PI units.
    pub stake: PiAmount,
    /// Validator name.
    pub name: String,
}

impl GenesisConfig {
    /// Create the official PIChain genesis configuration.
    /// Total supply: 3,141,592,653 PI = 3.14B tokens.
    ///
    /// Satoshi model: no team, no foundation, no treasury.
    /// 85% Mining Pool — earned by computing Pi digits (2π% geometric decay)
    /// 10% Validator Staking — secures BFT consensus
    ///  5% Initial Liquidity — DEX bootstrapping
    pub fn pichain_mainnet() -> Self {
        let mining = (TOTAL_SUPPLY * 85 / 100) as PiAmount;
        let validators = (TOTAL_SUPPLY * 10 / 100) as PiAmount;
        let liquidity = (TOTAL_SUPPLY as PiAmount) - mining - validators; // remainder = exact 5%

        Self {
            chain_id: 314159, // digits of PI
            timestamp_ms: 0,  // set at actual genesis ceremony
            allocations: vec![
                GenesisAllocation {
                    address: Address::ZERO, // virtual — no real balance created
                    amount: mining,
                    label: "Mining Pool (85%)".to_string(),
                    virtual_pool: true, // AUDIT-FIX: tracked in RewardCalculator, minted over time
                },
                GenesisAllocation {
                    address: Address::ZERO, // placeholder — validator staking reserve
                    amount: validators,
                    label: "Validator Staking Reserve (10%)".to_string(),
                    virtual_pool: false,
                },
                GenesisAllocation {
                    address: Address::ZERO, // placeholder — initial DEX liquidity
                    amount: liquidity,
                    label: "Initial Liquidity (5%)".to_string(),
                    virtual_pool: false,
                },
            ],
            validators: vec![],
        }
    }

    /// Create a devnet genesis with faucet and test allocations.
    /// Allocates the full TOTAL_SUPPLY to ensure supply validation passes.
    /// Matches mainnet 85/10/5 proportions: mining pool, validators, liquidity.
    pub fn devnet() -> Self {
        // Devnet faucet address: derived from a known seed for testing.
        let faucet_hash = pichain_crypto::hash(b"pichain-devnet-faucet");
        let mut faucet_bytes = [0u8; 20];
        faucet_bytes.copy_from_slice(&faucet_hash.as_bytes()[..20]);
        let faucet_address = Address(faucet_bytes);

        // Mining pool address for devnet
        let mining_hash = pichain_crypto::hash(b"pichain-devnet-mining-pool");
        let mut mining_bytes = [0u8; 20];
        mining_bytes.copy_from_slice(&mining_hash.as_bytes()[..20]);
        let mining_address = Address(mining_bytes);

        // Validator staking address for devnet
        let validator_hash = pichain_crypto::hash(b"pichain-devnet-validators");
        let mut validator_bytes = [0u8; 20];
        validator_bytes.copy_from_slice(&validator_hash.as_bytes()[..20]);
        let validator_address = Address(validator_bytes);

        // Must sum to exactly TOTAL_SUPPLY (3,141,592,653 PI):
        //   Faucet:        ~10% (testing)
        //   Mining Pool:   85% of supply   (same as mainnet)
        //   Validators:    remainder (~5%)
        //   Total:         3,141,592,653 PI ✓
        let mining_amount = (TOTAL_SUPPLY * 85 / 100) as PiAmount;
        let faucet_amount = (TOTAL_SUPPLY * 10 / 100) as PiAmount;
        let validator_amount = (TOTAL_SUPPLY as PiAmount) - mining_amount - faucet_amount;

        Self {
            chain_id: 31415,
            timestamp_ms: 0,
            allocations: vec![
                GenesisAllocation {
                    address: faucet_address,
                    amount: faucet_amount,
                    label: "Devnet Faucet".to_string(),
                    virtual_pool: false,
                },
                GenesisAllocation {
                    address: mining_address,
                    amount: mining_amount,
                    label: "Devnet Mining Pool (85%)".to_string(),
                    virtual_pool: true, // AUDIT-FIX: virtual — minted over time
                },
                GenesisAllocation {
                    address: validator_address,
                    amount: validator_amount,
                    label: "Devnet Validators & Liquidity".to_string(),
                    virtual_pool: false,
                },
            ],
            validators: vec![],
        }
    }

    /// Get the devnet faucet address.
    pub fn devnet_faucet_address() -> Address {
        let faucet_hash = pichain_crypto::hash(b"pichain-devnet-faucet");
        let mut faucet_bytes = [0u8; 20];
        faucet_bytes.copy_from_slice(&faucet_hash.as_bytes()[..20]);
        Address(faucet_bytes)
    }

    /// Verify that total allocations equal the fixed supply.
    pub fn verify_supply(&self) -> bool {
        let total: u128 = self
            .allocations
            .iter()
            .map(|a| a.amount as u128)
            .sum();
        total == TOTAL_SUPPLY
    }

    /// Validate the genesis configuration. Returns an error if supply doesn't match.
    /// Should be called during node startup to prevent misconfigured deployments.
    pub fn validate(&self) -> Result<(), String> {
        let total: u128 = self
            .allocations
            .iter()
            .map(|a| a.amount as u128)
            .sum();
        if total != TOTAL_SUPPLY {
            return Err(format!(
                "genesis supply mismatch: allocations sum to {} but expected {}",
                total, TOTAL_SUPPLY
            ));
        }
        if self.allocations.is_empty() {
            return Err("genesis has no allocations".to_string());
        }
        // Warn about Address::ZERO allocations — funds will be irrecoverable
        // AUDIT-FIX H-9: Virtual pool allocations are allowed to use Address::ZERO
        // since they don't create real balances (they're tracked in RewardCalculator).
        for alloc in &self.allocations {
            if alloc.address == Address::ZERO && alloc.amount > 0 && !alloc.virtual_pool {
                return Err(format!(
                    "genesis allocation '{}' sends {} PI to Address::ZERO — \
                     funds will be permanently lost. Replace with real addresses before launch.",
                    alloc.label, alloc.amount / BASE_UNITS_PER_PI
                ));
            }
        }
        // Validate no zero-amount allocations
        for alloc in &self.allocations {
            if alloc.amount == 0 {
                return Err(format!(
                    "genesis allocation '{}' has zero amount",
                    alloc.label
                ));
            }
        }
        // Validate no duplicate addresses (skip virtual pools — they share Address::ZERO)
        let mut seen = HashSet::new();
        for alloc in &self.allocations {
            if !alloc.virtual_pool && !seen.insert(alloc.address) {
                return Err(format!(
                    "duplicate genesis allocation for address {}",
                    alloc.address
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mainnet_genesis_supply() {
        let genesis = GenesisConfig::pichain_mainnet();
        let total: u128 = genesis
            .allocations
            .iter()
            .map(|a| a.amount as u128)
            .sum();
        // Should equal 3,141,592,653 * 10^9
        assert_eq!(total, TOTAL_SUPPLY, "Total supply mismatch!");
    }

    #[test]
    fn devnet_genesis() {
        let genesis = GenesisConfig::devnet();
        assert_eq!(genesis.chain_id, 31415);
        // Devnet must also allocate the full TOTAL_SUPPLY
        let total: u128 = genesis
            .allocations
            .iter()
            .map(|a| a.amount as u128)
            .sum();
        assert_eq!(total, TOTAL_SUPPLY, "Devnet supply must equal TOTAL_SUPPLY");
    }
}
