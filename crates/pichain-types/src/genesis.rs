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
    pub fn pichain_mainnet() -> Self {
        // Token distribution per the blueprint:
        // 40% Community & Ecosystem (mining rewards) = 1,256,637,061 PI
        // 20% Validator Rewards Reserve               = 628,318,530 PI
        // 15% Foundation & Development                 = 471,238,897 PI
        // 10% Ecosystem Grants Fund                    = 314,159,265 PI
        //  8% Team & Early Contributors                = 251,327,412 PI
        //  5% Public Sale / IDO                        = 157,079,632 PI
        //  2% Strategic Partners                       =  62,831,853 PI
        //                                         Total: 3,141,592,650 PI
        // (3 PI remainder goes to foundation to round)

        Self {
            chain_id: 314159, // digits of PI
            timestamp_ms: 0,  // set at actual genesis ceremony
            allocations: vec![
                GenesisAllocation {
                    address: Address::ZERO, // placeholder — mining pool contract
                    amount: 1_256_637_061 * BASE_UNITS_PER_PI,
                    label: "Community & Mining Pool (40%)".to_string(),
                },
                GenesisAllocation {
                    address: Address::ZERO,
                    amount: 628_318_530 * BASE_UNITS_PER_PI,
                    label: "Validator Rewards Reserve (20%)".to_string(),
                },
                GenesisAllocation {
                    address: Address::ZERO,
                    amount: 471_238_900 * BASE_UNITS_PER_PI, // +3 for rounding
                    label: "Foundation & Development (15%)".to_string(),
                },
                GenesisAllocation {
                    address: Address::ZERO,
                    amount: 314_159_265 * BASE_UNITS_PER_PI,
                    label: "Ecosystem Grants Fund (10%)".to_string(),
                },
                GenesisAllocation {
                    address: Address::ZERO,
                    amount: 251_327_412 * BASE_UNITS_PER_PI,
                    label: "Team & Early Contributors (8%)".to_string(),
                },
                GenesisAllocation {
                    address: Address::ZERO,
                    amount: 157_079_632 * BASE_UNITS_PER_PI,
                    label: "Public Sale / IDO (5%)".to_string(),
                },
                GenesisAllocation {
                    address: Address::ZERO,
                    amount: 62_831_853 * BASE_UNITS_PER_PI,
                    label: "Strategic Partners (2%)".to_string(),
                },
            ],
            validators: vec![],
        }
    }

    /// Create a devnet genesis with faucet and test allocations.
    /// Allocates the full TOTAL_SUPPLY to ensure supply validation passes.
    pub fn devnet() -> Self {
        // Devnet faucet address: derived from a known seed for testing.
        // Address bytes: Blake3("pichain-devnet-faucet")[..20]
        let faucet_hash = pichain_crypto::hash(b"pichain-devnet-faucet");
        let mut faucet_bytes = [0u8; 20];
        faucet_bytes.copy_from_slice(&faucet_hash.as_bytes()[..20]);
        let faucet_address = Address(faucet_bytes);

        // Treasury address for devnet
        let treasury_hash = pichain_crypto::hash(b"pichain-devnet-treasury");
        let mut treasury_bytes = [0u8; 20];
        treasury_bytes.copy_from_slice(&treasury_hash.as_bytes()[..20]);
        let treasury_address = Address(treasury_bytes);

        // Mining pool address for devnet
        let mining_hash = pichain_crypto::hash(b"pichain-devnet-mining-pool");
        let mut mining_bytes = [0u8; 20];
        mining_bytes.copy_from_slice(&mining_hash.as_bytes()[..20]);
        let mining_address = Address(mining_bytes);

        // Ecosystem grants address for devnet
        let ecosystem_hash = pichain_crypto::hash(b"pichain-devnet-ecosystem");
        let mut ecosystem_bytes = [0u8; 20];
        ecosystem_bytes.copy_from_slice(&ecosystem_hash.as_bytes()[..20]);
        let ecosystem_address = Address(ecosystem_bytes);

        // Must sum to exactly TOTAL_SUPPLY (3,141,592,653 PI):
        //   Faucet:        1,000,000,000 PI
        //   Treasury:        500,000,000 PI
        //   Mining Pool:   1,256,637,061 PI (40%, same as mainnet)
        //   Ecosystem:       384,955,592 PI (remainder)
        //   Total:         3,141,592,653 PI ✓
        Self {
            chain_id: 31415,
            timestamp_ms: 0,
            allocations: vec![
                GenesisAllocation {
                    address: faucet_address,
                    amount: 1_000_000_000 * BASE_UNITS_PER_PI,
                    label: "Devnet Faucet".to_string(),
                },
                GenesisAllocation {
                    address: treasury_address,
                    amount: 500_000_000 * BASE_UNITS_PER_PI,
                    label: "Devnet Treasury".to_string(),
                },
                GenesisAllocation {
                    address: mining_address,
                    amount: 1_256_637_061 * BASE_UNITS_PER_PI,
                    label: "Devnet Mining Pool (40%)".to_string(),
                },
                GenesisAllocation {
                    address: ecosystem_address,
                    amount: 384_955_592 * BASE_UNITS_PER_PI,
                    label: "Devnet Ecosystem Grants".to_string(),
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
        for alloc in &self.allocations {
            if alloc.address == Address::ZERO && alloc.amount > 0 {
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
        // Validate no duplicate addresses
        let mut seen = HashSet::new();
        for alloc in &self.allocations {
            if !seen.insert(alloc.address) {
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
