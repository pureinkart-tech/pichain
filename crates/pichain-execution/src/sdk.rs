//! Smart contract SDK — types and utilities for PIChain contract development.
//!
//! Provides the interface that smart contracts use to interact with the blockchain:
//! - Storage read/write
//! - Token transfers
//! - Event emission
//! - Cross-contract calls
//! - Contract metadata and ABI definitions
//!
//! Contracts are compiled to WASM and executed by the WasmVM.

use pichain_crypto::keys::Address;
use pichain_crypto::Hash;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Maximum contract code size (~3.14 MB = π × 10^6 bytes).
pub const MAX_CONTRACT_SIZE: usize = 3_141_592;

/// Maximum contract storage key size (256 bytes).
pub const MAX_STORAGE_KEY_SIZE: usize = 256;

/// Maximum contract storage value size (64 KB).
pub const MAX_STORAGE_VALUE_SIZE: usize = 65_536;

/// Maximum number of events per transaction (π × 100).
pub const MAX_EVENTS_PER_TX: usize = 314;

/// Maximum cross-contract call depth (π × 10).
pub const MAX_CALL_DEPTH: u32 = 31;

/// Contract metadata — describes a deployed contract.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContractMetadata {
    /// Contract address.
    pub address: Address,
    /// Deployer address.
    pub deployer: Address,
    /// Code hash (Blake3 of WASM bytecode).
    pub code_hash: Hash,
    /// Contract ABI (function signatures and types).
    pub abi: ContractAbi,
    /// Contract name.
    pub name: String,
    /// Contract version.
    pub version: String,
    /// Deployment block height.
    pub deployed_at: u64,
    /// Whether the contract is upgradeable.
    pub upgradeable: bool,
}

/// Contract ABI — describes the contract's public interface.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContractAbi {
    /// Contract functions.
    pub functions: Vec<AbiFunction>,
    /// Contract events.
    pub events: Vec<AbiEvent>,
    /// Constructor parameters.
    pub constructor: Option<Vec<AbiParam>>,
}

impl ContractAbi {
    pub fn new() -> Self {
        Self {
            functions: vec![],
            events: vec![],
            constructor: None,
        }
    }

    /// Find a function by name.
    pub fn function(&self, name: &str) -> Option<&AbiFunction> {
        self.functions.iter().find(|f| f.name == name)
    }

    /// Compute the 4-byte function selector (first 4 bytes of Blake3 hash of signature).
    pub fn selector(signature: &str) -> [u8; 4] {
        let hash = pichain_crypto::hash(signature.as_bytes());
        let mut sel = [0u8; 4];
        sel.copy_from_slice(&hash.as_bytes()[..4]);
        sel
    }
}

impl Default for ContractAbi {
    fn default() -> Self {
        Self::new()
    }
}

/// An ABI function definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbiFunction {
    /// Function name.
    pub name: String,
    /// Input parameters.
    pub inputs: Vec<AbiParam>,
    /// Output parameters.
    pub outputs: Vec<AbiParam>,
    /// Whether this function modifies state.
    pub mutability: Mutability,
    /// 4-byte function selector.
    pub selector: [u8; 4],
}

/// An ABI event definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbiEvent {
    /// Event name.
    pub name: String,
    /// Event parameters.
    pub params: Vec<AbiEventParam>,
}

/// An ABI parameter.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbiParam {
    /// Parameter name.
    pub name: String,
    /// Parameter type.
    pub param_type: AbiType,
}

/// An ABI event parameter.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbiEventParam {
    /// Parameter name.
    pub name: String,
    /// Parameter type.
    pub param_type: AbiType,
    /// Whether this parameter is indexed (searchable).
    pub indexed: bool,
}

/// ABI type system for contract parameters.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AbiType {
    /// Unsigned integer (bits: 8, 16, 32, 64, 128, 256).
    Uint(u16),
    /// Signed integer.
    Int(u16),
    /// Boolean.
    Bool,
    /// Address (20 bytes).
    Address,
    /// Fixed-size byte array.
    FixedBytes(u8),
    /// Dynamic byte array.
    Bytes,
    /// UTF-8 string.
    String,
    /// Fixed-size array.
    Array(Box<AbiType>, u32),
    /// Dynamic-size array.
    DynArray(Box<AbiType>),
    /// Tuple of types.
    Tuple(Vec<AbiType>),
}

/// Function mutability (how it interacts with state).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Mutability {
    /// Reads and writes state.
    Mutable,
    /// Only reads state (view function).
    View,
    /// Doesn't access state at all (pure function).
    Pure,
    /// Accepts PI tokens (payable).
    Payable,
}

/// Contract call context — provided to the contract during execution.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallContext {
    /// Caller address (msg.sender).
    pub caller: Address,
    /// Contract address (this).
    pub contract: Address,
    /// Value sent with the call (in PI base units).
    pub value: u64,
    /// Block height.
    pub block_height: u64,
    /// Block timestamp (ms).
    pub block_timestamp: u64,
    /// Gas remaining.
    pub gas_remaining: u64,
    /// Call depth (0 for external calls).
    pub call_depth: u32,
    /// Whether this is a delegate call.
    pub is_delegate_call: bool,
}

/// Contract call result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallResult {
    /// Whether the call succeeded.
    pub success: bool,
    /// Return data.
    pub return_data: Vec<u8>,
    /// Gas used.
    pub gas_used: u64,
    /// Events emitted.
    pub events: Vec<ContractEvent>,
    /// State changes.
    pub state_writes: Vec<(Vec<u8>, Vec<u8>)>,
    /// Revert reason (if failed).
    pub revert_reason: Option<String>,
}

/// A contract event.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContractEvent {
    /// Contract that emitted the event.
    pub contract: Address,
    /// Event name.
    pub event_name: String,
    /// Indexed fields (for efficient filtering).
    pub indexed: Vec<Vec<u8>>,
    /// Non-indexed data.
    pub data: Vec<u8>,
}

/// Contract registry — tracks all deployed contracts.
pub struct ContractRegistry {
    /// Deployed contracts, keyed by address.
    contracts: HashMap<Address, ContractMetadata>,
    /// Contract code, keyed by code hash.
    code: HashMap<Hash, Vec<u8>>,
    /// Contracts by deployer.
    by_deployer: HashMap<Address, Vec<Address>>,
}

impl ContractRegistry {
    pub fn new() -> Self {
        Self {
            contracts: HashMap::new(),
            code: HashMap::new(),
            by_deployer: HashMap::new(),
        }
    }

    /// Clear all state. Used during gas-limit trimming re-execution.
    pub fn clear(&mut self) {
        self.contracts.clear();
        self.code.clear();
        self.by_deployer.clear();
    }

    /// Register a new contract deployment.
    #[allow(clippy::too_many_arguments)]
    pub fn deploy(
        &mut self,
        contract_addr: Address,
        deployer: Address,
        bytecode: Vec<u8>,
        abi: ContractAbi,
        name: String,
        version: String,
        block_height: u64,
        upgradeable: bool,
    ) -> Result<&ContractMetadata, ContractError> {
        if bytecode.len() > MAX_CONTRACT_SIZE {
            return Err(ContractError::CodeTooLarge(bytecode.len()));
        }
        if self.contracts.contains_key(&contract_addr) {
            return Err(ContractError::AlreadyDeployed(contract_addr));
        }

        let code_hash = pichain_crypto::hash(&bytecode);
        self.code.insert(code_hash, bytecode);

        let metadata = ContractMetadata {
            address: contract_addr,
            deployer,
            code_hash,
            abi,
            name,
            version,
            deployed_at: block_height,
            upgradeable,
        };
        self.contracts.insert(contract_addr, metadata);
        self.by_deployer
            .entry(deployer)
            .or_default()
            .push(contract_addr);

        self.contracts
            .get(&contract_addr)
            .ok_or(ContractError::NotFound(contract_addr))
    }

    /// Upgrade a contract (replace bytecode, keep state).
    pub fn upgrade(
        &mut self,
        contract_addr: Address,
        caller: Address,
        new_bytecode: Vec<u8>,
        new_abi: ContractAbi,
        new_version: String,
    ) -> Result<(), ContractError> {
        let metadata = self
            .contracts
            .get_mut(&contract_addr)
            .ok_or(ContractError::NotFound(contract_addr))?;

        if !metadata.upgradeable {
            return Err(ContractError::NotUpgradeable(contract_addr));
        }
        if metadata.deployer != caller {
            return Err(ContractError::Unauthorized(caller));
        }
        if new_bytecode.len() > MAX_CONTRACT_SIZE {
            return Err(ContractError::CodeTooLarge(new_bytecode.len()));
        }

        let code_hash = pichain_crypto::hash(&new_bytecode);
        self.code.insert(code_hash, new_bytecode);
        metadata.code_hash = code_hash;
        metadata.abi = new_abi;
        metadata.version = new_version;

        Ok(())
    }

    /// Get contract metadata.
    pub fn get(&self, address: &Address) -> Option<&ContractMetadata> {
        self.contracts.get(address)
    }

    /// Get contract bytecode.
    pub fn get_code(&self, code_hash: &Hash) -> Option<&[u8]> {
        self.code.get(code_hash).map(|v| v.as_slice())
    }

    /// Get all contracts deployed by an address.
    pub fn by_deployer(&self, deployer: &Address) -> &[Address] {
        self.by_deployer
            .get(deployer)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Remove a contract from the registry (e.g., when init fails).
    pub fn remove(&mut self, address: &Address) {
        if let Some(metadata) = self.contracts.remove(address) {
            // Clean up code if no other contract references it
            let code_hash = metadata.code_hash;
            let still_used = self.contracts.values().any(|m| m.code_hash == code_hash);
            if !still_used {
                self.code.remove(&code_hash);
            }
            // Remove from deployer index
            if let Some(deployed) = self.by_deployer.get_mut(&metadata.deployer) {
                deployed.retain(|a| a != address);
            }
        }
    }

    /// Get total deployed contract count.
    pub fn count(&self) -> usize {
        self.contracts.len()
    }
}

impl Default for ContractRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Contract-related errors.
#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("contract code too large: {0} bytes (max {MAX_CONTRACT_SIZE})")]
    CodeTooLarge(usize),
    #[error("contract already deployed at {0}")]
    AlreadyDeployed(Address),
    #[error("contract not found at {0}")]
    NotFound(Address),
    #[error("contract at {0} is not upgradeable")]
    NotUpgradeable(Address),
    #[error("unauthorized caller: {0}")]
    Unauthorized(Address),
    #[error("call depth exceeded (max {MAX_CALL_DEPTH})")]
    CallDepthExceeded,
    #[error("storage key too large: {0} bytes")]
    StorageKeyTooLarge(usize),
    #[error("storage value too large: {0} bytes")]
    StorageValueTooLarge(usize),
    #[error("too many events: {0} (max {MAX_EVENTS_PER_TX})")]
    TooManyEvents(usize),
    #[error("execution error: {0}")]
    Execution(String),
}

/// Standard token interface (like ERC-20 but for WASM contracts).
/// Contracts implementing this interface are recognized as PIChain tokens.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenStandard {
    /// Token name.
    pub name: String,
    /// Token symbol.
    pub symbol: String,
    /// Decimal places.
    pub decimals: u8,
    /// Total supply.
    pub total_supply: u128,
}

impl TokenStandard {
    /// Generate the ABI for a standard token contract.
    pub fn standard_abi() -> ContractAbi {
        ContractAbi {
            functions: vec![
                AbiFunction {
                    name: "name".to_string(),
                    inputs: vec![],
                    outputs: vec![AbiParam {
                        name: "".to_string(),
                        param_type: AbiType::String,
                    }],
                    mutability: Mutability::View,
                    selector: ContractAbi::selector("name()"),
                },
                AbiFunction {
                    name: "symbol".to_string(),
                    inputs: vec![],
                    outputs: vec![AbiParam {
                        name: "".to_string(),
                        param_type: AbiType::String,
                    }],
                    mutability: Mutability::View,
                    selector: ContractAbi::selector("symbol()"),
                },
                AbiFunction {
                    name: "decimals".to_string(),
                    inputs: vec![],
                    outputs: vec![AbiParam {
                        name: "".to_string(),
                        param_type: AbiType::Uint(8),
                    }],
                    mutability: Mutability::View,
                    selector: ContractAbi::selector("decimals()"),
                },
                AbiFunction {
                    name: "total_supply".to_string(),
                    inputs: vec![],
                    outputs: vec![AbiParam {
                        name: "".to_string(),
                        param_type: AbiType::Uint(128),
                    }],
                    mutability: Mutability::View,
                    selector: ContractAbi::selector("total_supply()"),
                },
                AbiFunction {
                    name: "balance_of".to_string(),
                    inputs: vec![AbiParam {
                        name: "account".to_string(),
                        param_type: AbiType::Address,
                    }],
                    outputs: vec![AbiParam {
                        name: "".to_string(),
                        param_type: AbiType::Uint(128),
                    }],
                    mutability: Mutability::View,
                    selector: ContractAbi::selector("balance_of(address)"),
                },
                AbiFunction {
                    name: "transfer".to_string(),
                    inputs: vec![
                        AbiParam {
                            name: "to".to_string(),
                            param_type: AbiType::Address,
                        },
                        AbiParam {
                            name: "amount".to_string(),
                            param_type: AbiType::Uint(128),
                        },
                    ],
                    outputs: vec![AbiParam {
                        name: "".to_string(),
                        param_type: AbiType::Bool,
                    }],
                    mutability: Mutability::Mutable,
                    selector: ContractAbi::selector("transfer(address,uint128)"),
                },
                AbiFunction {
                    name: "approve".to_string(),
                    inputs: vec![
                        AbiParam {
                            name: "spender".to_string(),
                            param_type: AbiType::Address,
                        },
                        AbiParam {
                            name: "amount".to_string(),
                            param_type: AbiType::Uint(128),
                        },
                    ],
                    outputs: vec![AbiParam {
                        name: "".to_string(),
                        param_type: AbiType::Bool,
                    }],
                    mutability: Mutability::Mutable,
                    selector: ContractAbi::selector("approve(address,uint128)"),
                },
                AbiFunction {
                    name: "transfer_from".to_string(),
                    inputs: vec![
                        AbiParam {
                            name: "from".to_string(),
                            param_type: AbiType::Address,
                        },
                        AbiParam {
                            name: "to".to_string(),
                            param_type: AbiType::Address,
                        },
                        AbiParam {
                            name: "amount".to_string(),
                            param_type: AbiType::Uint(128),
                        },
                    ],
                    outputs: vec![AbiParam {
                        name: "".to_string(),
                        param_type: AbiType::Bool,
                    }],
                    mutability: Mutability::Mutable,
                    selector: ContractAbi::selector("transfer_from(address,address,uint128)"),
                },
            ],
            events: vec![
                AbiEvent {
                    name: "Transfer".to_string(),
                    params: vec![
                        AbiEventParam {
                            name: "from".to_string(),
                            param_type: AbiType::Address,
                            indexed: true,
                        },
                        AbiEventParam {
                            name: "to".to_string(),
                            param_type: AbiType::Address,
                            indexed: true,
                        },
                        AbiEventParam {
                            name: "amount".to_string(),
                            param_type: AbiType::Uint(128),
                            indexed: false,
                        },
                    ],
                },
                AbiEvent {
                    name: "Approval".to_string(),
                    params: vec![
                        AbiEventParam {
                            name: "owner".to_string(),
                            param_type: AbiType::Address,
                            indexed: true,
                        },
                        AbiEventParam {
                            name: "spender".to_string(),
                            param_type: AbiType::Address,
                            indexed: true,
                        },
                        AbiEventParam {
                            name: "amount".to_string(),
                            param_type: AbiType::Uint(128),
                            indexed: false,
                        },
                    ],
                },
            ],
            constructor: Some(vec![
                AbiParam {
                    name: "name".to_string(),
                    param_type: AbiType::String,
                },
                AbiParam {
                    name: "symbol".to_string(),
                    param_type: AbiType::String,
                },
                AbiParam {
                    name: "decimals".to_string(),
                    param_type: AbiType::Uint(8),
                },
                AbiParam {
                    name: "initial_supply".to_string(),
                    param_type: AbiType::Uint(128),
                },
            ]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_deployment() {
        let mut registry = ContractRegistry::new();
        let deployer = Address([1u8; 20]);
        let contract = Address([2u8; 20]);
        let bytecode = vec![0x00, 0x61, 0x73, 0x6d]; // WASM magic

        let meta = registry
            .deploy(
                contract,
                deployer,
                bytecode,
                ContractAbi::new(),
                "test".to_string(),
                "0.1.0".to_string(),
                1,
                false,
            )
            .unwrap();

        assert_eq!(meta.address, contract);
        assert_eq!(meta.deployer, deployer);
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn reject_duplicate_deployment() {
        let mut registry = ContractRegistry::new();
        let addr = Address([1u8; 20]);
        registry
            .deploy(
                addr,
                addr,
                vec![1],
                ContractAbi::new(),
                "a".into(),
                "1".into(),
                0,
                false,
            )
            .unwrap();
        assert!(registry
            .deploy(
                addr,
                addr,
                vec![2],
                ContractAbi::new(),
                "b".into(),
                "1".into(),
                0,
                false
            )
            .is_err());
    }

    #[test]
    fn reject_oversized_contract() {
        let mut registry = ContractRegistry::new();
        let big_code = vec![0u8; MAX_CONTRACT_SIZE + 1];
        assert!(registry
            .deploy(
                Address([1u8; 20]),
                Address([1u8; 20]),
                big_code,
                ContractAbi::new(),
                "big".into(),
                "1".into(),
                0,
                false
            )
            .is_err());
    }

    #[test]
    fn upgradeable_contract() {
        let mut registry = ContractRegistry::new();
        let deployer = Address([1u8; 20]);
        let contract = Address([2u8; 20]);

        registry
            .deploy(
                contract,
                deployer,
                vec![1, 2, 3],
                ContractAbi::new(),
                "test".into(),
                "0.1.0".into(),
                0,
                true,
            )
            .unwrap();

        // Upgrade
        registry
            .upgrade(
                contract,
                deployer,
                vec![4, 5, 6],
                ContractAbi::new(),
                "0.2.0".into(),
            )
            .unwrap();

        let meta = registry.get(&contract).unwrap();
        assert_eq!(meta.version, "0.2.0");
    }

    #[test]
    fn non_upgradeable_contract_rejects_upgrade() {
        let mut registry = ContractRegistry::new();
        let deployer = Address([1u8; 20]);
        let contract = Address([2u8; 20]);

        registry
            .deploy(
                contract,
                deployer,
                vec![1],
                ContractAbi::new(),
                "test".into(),
                "1.0.0".into(),
                0,
                false,
            )
            .unwrap();

        assert!(registry
            .upgrade(
                contract,
                deployer,
                vec![2],
                ContractAbi::new(),
                "2.0.0".into(),
            )
            .is_err());
    }

    #[test]
    fn unauthorized_upgrade_rejected() {
        let mut registry = ContractRegistry::new();
        let deployer = Address([1u8; 20]);
        let attacker = Address([99u8; 20]);
        let contract = Address([2u8; 20]);

        registry
            .deploy(
                contract,
                deployer,
                vec![1],
                ContractAbi::new(),
                "test".into(),
                "1.0.0".into(),
                0,
                true,
            )
            .unwrap();

        assert!(registry
            .upgrade(
                contract,
                attacker,
                vec![2],
                ContractAbi::new(),
                "2.0.0".into(),
            )
            .is_err());
    }

    #[test]
    fn function_selector_deterministic() {
        let sel1 = ContractAbi::selector("transfer(address,uint128)");
        let sel2 = ContractAbi::selector("transfer(address,uint128)");
        assert_eq!(sel1, sel2);

        let sel3 = ContractAbi::selector("approve(address,uint128)");
        assert_ne!(sel1, sel3);
    }

    #[test]
    fn standard_token_abi() {
        let abi = TokenStandard::standard_abi();
        assert_eq!(abi.functions.len(), 8);
        assert_eq!(abi.events.len(), 2);
        assert!(abi.constructor.is_some());

        // Verify function names
        assert!(abi.function("transfer").is_some());
        assert!(abi.function("balance_of").is_some());
        assert!(abi.function("approve").is_some());
    }

    #[test]
    fn contracts_by_deployer() {
        let mut registry = ContractRegistry::new();
        let deployer = Address([1u8; 20]);

        registry
            .deploy(
                Address([2u8; 20]),
                deployer,
                vec![1],
                ContractAbi::new(),
                "a".into(),
                "1".into(),
                0,
                false,
            )
            .unwrap();
        registry
            .deploy(
                Address([3u8; 20]),
                deployer,
                vec![2],
                ContractAbi::new(),
                "b".into(),
                "1".into(),
                0,
                false,
            )
            .unwrap();

        assert_eq!(registry.by_deployer(&deployer).len(), 2);
        assert_eq!(registry.by_deployer(&Address([99u8; 20])).len(), 0);
    }
}
