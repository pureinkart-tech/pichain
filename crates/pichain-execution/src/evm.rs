//! EVM compatibility layer using revm.
//!
//! Provides Ethereum Virtual Machine execution on PIChain, allowing:
//! - Solidity smart contract deployment and execution
//! - MetaMask and Ethereum tooling compatibility
//! - Token bridging between native PI and ERC-20 wrapped PI
//!
//! Native PI uses 9 decimals (like SOL), while EVM uses 18 decimals.
//! The conversion is handled transparently.

use pichain_crypto::ed25519::Address as PiAddress;
use pichain_types::PiAmount;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// EVM address (20 bytes, same as Ethereum).
pub type EvmAddress = [u8; 20];

/// EVM word (256-bit, same as Ethereum U256).
pub type U256 = [u8; 32];

/// Conversion factor: 1 PI (9 decimals) = 10^9 EVM wei-equivalents more
/// PI base unit * 10^9 = EVM-compatible 18-decimal representation
const PI_TO_EVM_DECIMALS: u64 = 1_000_000_000; // 10^9

/// Maximum EVM gas per transaction.
const MAX_EVM_GAS: u64 = 30_000_000;

/// EVM execution result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvmResult {
    /// Whether execution succeeded.
    pub success: bool,
    /// Gas used.
    pub gas_used: u64,
    /// Return data (ABI-encoded).
    pub output: Vec<u8>,
    /// Logs emitted.
    pub logs: Vec<EvmLog>,
    /// State changes.
    pub state_changes: Vec<EvmStateChange>,
    /// Revert reason (if failed).
    pub revert_reason: Option<String>,
}

/// EVM log entry (Ethereum event).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvmLog {
    /// Contract address that emitted the log.
    pub address: EvmAddress,
    /// Indexed topics (up to 4).
    pub topics: Vec<[u8; 32]>,
    /// Non-indexed data.
    pub data: Vec<u8>,
}

/// EVM state change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvmStateChange {
    /// Account address.
    pub address: EvmAddress,
    /// Balance change (in EVM 18-decimal units).
    pub balance_change: i128,
    /// Nonce change.
    pub nonce_change: i64,
    /// Storage slots changed: (key, new_value).
    pub storage_changes: Vec<([u8; 32], [u8; 32])>,
    /// New code deployed (if any).
    pub code: Option<Vec<u8>>,
}

/// EVM account state (parallel to native PIChain account state).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvmAccountState {
    /// Balance in 18-decimal EVM units.
    pub balance: u128,
    /// EVM nonce.
    pub nonce: u64,
    /// Contract bytecode (empty for EOA).
    pub code: Vec<u8>,
    /// Contract storage slots.
    pub storage: HashMap<[u8; 32], [u8; 32]>,
}

/// EVM transaction (Ethereum-style).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvmTransaction {
    /// Sender address.
    pub from: EvmAddress,
    /// Recipient (None for contract creation).
    pub to: Option<EvmAddress>,
    /// Value in EVM 18-decimal units.
    pub value: u128,
    /// Input data (calldata or init code).
    pub data: Vec<u8>,
    /// Gas limit.
    pub gas_limit: u64,
    /// Gas price (in EVM units).
    pub gas_price: u128,
    /// Nonce.
    pub nonce: u64,
    /// Chain ID.
    pub chain_id: u64,
}

/// Precompiled contracts available on PIChain EVM.
#[derive(Clone, Debug)]
pub enum Precompile {
    /// ecRecover (address 0x01).
    EcRecover,
    /// SHA-256 (address 0x02).
    Sha256,
    /// RIPEMD-160 (address 0x03).
    Ripemd160,
    /// Identity (address 0x04).
    Identity,
    /// Modular exponentiation (address 0x05).
    ModExp,
    /// BN256 curve addition (address 0x06).
    Bn256Add,
    /// BN256 curve scalar multiplication (address 0x07).
    Bn256ScalarMul,
    /// BN256 pairing check (address 0x08).
    Bn256Pairing,
    /// Blake2f compression (address 0x09).
    Blake2f,
    /// PIChain bridge precompile (address 0x314) — transfer between native and EVM.
    PiChainBridge,
    /// PIChain staking precompile (address 0x315) — stake from EVM contracts.
    PiChainStaking,
}

/// The PIChain EVM executor.
///
/// Wraps revm to provide Ethereum-compatible execution on PIChain.
/// Contract calls and creation use the full revm interpreter for real
/// EVM bytecode execution. Simple value transfers are handled natively
/// for efficiency.
pub struct EvmExecutor {
    /// EVM account states.
    accounts: HashMap<EvmAddress, EvmAccountState>,
    /// Chain ID (314159 for mainnet, 31415 for devnet).
    chain_id: u64,
    /// Block height (for BLOCK opcode).
    block_height: u64,
    /// Block timestamp (for TIMESTAMP opcode).
    block_timestamp: u64,
    /// Base fee (for BASEFEE opcode).
    base_fee: u128,
    /// Coinbase address (block producer).
    coinbase: EvmAddress,
}

impl EvmExecutor {
    /// Create a new EVM executor.
    pub fn new(chain_id: u64) -> Self {
        Self {
            accounts: HashMap::new(),
            chain_id,
            block_height: 0,
            block_timestamp: 0,
            base_fee: 1_000_000_000, // 1 Gwei equivalent
            coinbase: [0u8; 20],
        }
    }

    /// Set the block context for EVM execution.
    pub fn set_block_context(
        &mut self,
        height: u64,
        timestamp: u64,
        base_fee: u128,
        coinbase: EvmAddress,
    ) {
        self.block_height = height;
        self.block_timestamp = timestamp;
        self.base_fee = base_fee;
        self.coinbase = coinbase;
    }

    /// Convert PI amount (9 decimals) to EVM amount (18 decimals).
    pub fn pi_to_evm(pi_amount: PiAmount) -> u128 {
        pi_amount as u128 * PI_TO_EVM_DECIMALS as u128
    }

    /// Convert EVM amount (18 decimals) to PI amount (9 decimals).
    /// Truncates sub-PI-unit amounts. Caps at u64::MAX to prevent silent truncation.
    pub fn evm_to_pi(evm_amount: u128) -> PiAmount {
        let pi_amount = evm_amount / PI_TO_EVM_DECIMALS as u128;
        pi_amount.min(u64::MAX as u128) as u64
    }

    /// Convert a PIChain address to an EVM address.
    pub fn pi_addr_to_evm(addr: &PiAddress) -> EvmAddress {
        addr.0
    }

    /// Convert an EVM address to a PIChain address.
    pub fn evm_addr_to_pi(addr: &EvmAddress) -> PiAddress {
        PiAddress(*addr)
    }

    /// Get or create an EVM account.
    pub fn get_account(&self, address: &EvmAddress) -> EvmAccountState {
        self.accounts.get(address).cloned().unwrap_or_default()
    }

    /// Set an EVM account state.
    pub fn set_account(&mut self, address: EvmAddress, state: EvmAccountState) {
        self.accounts.insert(address, state);
    }

    /// Load a PIChain account into the EVM state.
    pub fn load_pi_account(&mut self, address: PiAddress, balance: PiAmount) {
        let evm_addr = Self::pi_addr_to_evm(&address);
        let evm_balance = Self::pi_to_evm(balance);
        let state = self.accounts.entry(evm_addr).or_default();
        state.balance = evm_balance;
    }

    /// Execute an EVM transaction.
    ///
    /// Currently provides a reference implementation. When revm is integrated,
    /// this will delegate to `revm::Evm::transact()`.
    pub fn execute(&mut self, tx: &EvmTransaction) -> EvmResult {
        // Validate transaction
        if tx.gas_limit > MAX_EVM_GAS {
            return EvmResult {
                success: false,
                gas_used: 0,
                output: vec![],
                logs: vec![],
                state_changes: vec![],
                revert_reason: Some("gas limit exceeds maximum".to_string()),
            };
        }

        let sender = self.accounts.entry(tx.from).or_default();
        if sender.nonce != tx.nonce {
            return EvmResult {
                success: false,
                gas_used: 0,
                output: vec![],
                logs: vec![],
                state_changes: vec![],
                revert_reason: Some(format!(
                    "nonce mismatch: expected {}, got {}",
                    sender.nonce, tx.nonce
                )),
            };
        }

        let gas_cost = match (tx.gas_limit as u128).checked_mul(tx.gas_price) {
            Some(v) => v,
            None => {
                return EvmResult {
                    success: false,
                    gas_used: 0,
                    output: vec![],
                    logs: vec![],
                    state_changes: vec![],
                    revert_reason: Some("gas cost overflow".to_string()),
                };
            }
        };
        let total_cost = match tx.value.checked_add(gas_cost) {
            Some(v) => v,
            None => {
                return EvmResult {
                    success: false,
                    gas_used: 0,
                    output: vec![],
                    logs: vec![],
                    state_changes: vec![],
                    revert_reason: Some("total cost overflow".to_string()),
                };
            }
        };
        if sender.balance < total_cost {
            return EvmResult {
                success: false,
                gas_used: 0,
                output: vec![],
                logs: vec![],
                state_changes: vec![],
                revert_reason: Some("insufficient balance".to_string()),
            };
        }

        // Execute based on transaction type
        match tx.to {
            None => self.execute_create(tx),
            Some(to) => {
                let code = self
                    .accounts
                    .get(&to)
                    .map(|a| a.code.clone())
                    .unwrap_or_default();
                if code.is_empty() {
                    // Simple value transfer
                    self.execute_transfer(tx, to)
                } else {
                    // Contract call
                    self.execute_call(tx, to)
                }
            }
        }
    }

    /// Execute a simple value transfer.
    fn execute_transfer(&mut self, tx: &EvmTransaction, to: EvmAddress) -> EvmResult {
        let gas_used = 21_000u64; // Standard transfer gas

        // Debit sender (checked arithmetic; balance already validated above)
        let sender = self.accounts.entry(tx.from).or_default();
        let gas_cost = gas_used as u128 * tx.gas_price;
        let total_debit = tx.value.saturating_add(gas_cost);
        if sender.balance < total_debit {
            return EvmResult {
                success: false,
                gas_used,
                output: vec![],
                logs: vec![],
                state_changes: vec![],
                revert_reason: Some("balance underflow during transfer".to_string()),
            };
        }
        sender.balance = match sender
            .balance
            .checked_sub(tx.value)
            .and_then(|b| b.checked_sub(gas_cost))
        {
            Some(b) => b,
            None => {
                // Should be unreachable — total_debit was already validated above.
                // Defensive: log and set to 0 rather than underflow.
                tracing::error!(
                    sender_balance = sender.balance,
                    value = tx.value,
                    gas_cost = gas_cost,
                    "EVM execute_transfer: unexpected underflow in sender balance subtraction"
                );
                0
            }
        };
        sender.nonce = sender.nonce.saturating_add(1);

        // Credit recipient
        let recipient = self.accounts.entry(to).or_default();
        recipient.balance = recipient.balance.saturating_add(tx.value);

        EvmResult {
            success: true,
            gas_used,
            output: vec![],
            logs: vec![],
            state_changes: vec![
                EvmStateChange {
                    address: tx.from,
                    balance_change: (tx.value.min(i128::MAX as u128) as i128)
                        .wrapping_neg()
                        .saturating_sub(
                            (gas_used as i128)
                                .saturating_mul(tx.gas_price.min(i128::MAX as u128) as i128),
                        ),
                    nonce_change: 1,
                    storage_changes: vec![],
                    code: None,
                },
                EvmStateChange {
                    address: to,
                    balance_change: tx.value.min(i128::MAX as u128) as i128,
                    nonce_change: 0,
                    storage_changes: vec![],
                    code: None,
                },
            ],
            revert_reason: None,
        }
    }

    /// Execute a contract creation using revm.
    /// Runs the constructor bytecode through the real EVM interpreter.
    fn execute_create(&mut self, tx: &EvmTransaction) -> EvmResult {
        use revm::primitives::{
            AccountInfo, Address as RevmAddress, Bytecode, ExecutionResult as RevmExecResult,
            TxKind, B256, U256 as RevmU256,
        };
        use revm::{Evm, InMemoryDB};

        let mut db = InMemoryDB::default();

        // Load sender account into revm DB
        for (addr, acct) in &self.accounts {
            let revm_addr = RevmAddress::from_slice(addr);
            let code_hash = if acct.code.is_empty() {
                revm::primitives::KECCAK_EMPTY
            } else {
                B256::from_slice(&pichain_crypto::hash(&acct.code).as_bytes()[..32])
            };

            let info = AccountInfo {
                balance: RevmU256::from(acct.balance),
                nonce: acct.nonce,
                code_hash,
                code: if acct.code.is_empty() {
                    None
                } else {
                    Some(Bytecode::new_raw(revm::primitives::Bytes::copy_from_slice(
                        &acct.code,
                    )))
                },
            };
            db.insert_account_info(revm_addr, info);

            // Load storage slots so constructor can read existing contract state
            for (key, val) in &acct.storage {
                let slot = RevmU256::from_be_bytes(*key);
                let value = RevmU256::from_be_bytes(*val);
                db.insert_account_storage(revm_addr, slot, value).ok();
            }
        }

        let caller = RevmAddress::from_slice(&tx.from);
        let chain_id = self.chain_id;
        let block_height = self.block_height;
        let block_timestamp = self.block_timestamp;
        let base_fee = self.base_fee;
        let coinbase_addr = RevmAddress::from_slice(&self.coinbase);
        let tx_value = tx.value;
        let tx_data = tx.data.clone();
        let tx_gas_limit = tx.gas_limit;
        let tx_gas_price = tx.gas_price;
        let tx_nonce = tx.nonce;

        let mut evm = Evm::builder()
            .with_db(db)
            .modify_cfg_env(|cfg| {
                cfg.chain_id = chain_id;
            })
            .modify_block_env(|block| {
                block.number = RevmU256::from(block_height);
                block.timestamp = RevmU256::from(block_timestamp);
                block.basefee = RevmU256::from(base_fee);
                block.coinbase = coinbase_addr;
            })
            .modify_tx_env(|tx_env| {
                tx_env.caller = caller;
                tx_env.transact_to = TxKind::Create;
                tx_env.value = RevmU256::from(tx_value);
                tx_env.data = revm::primitives::Bytes::copy_from_slice(&tx_data);
                tx_env.gas_limit = tx_gas_limit;
                tx_env.gas_price = RevmU256::from(tx_gas_price);
                tx_env.nonce = Some(tx_nonce);
            })
            .build();

        let result = match evm.transact_commit() {
            Ok(r) => r,
            Err(e) => {
                return EvmResult {
                    success: false,
                    gas_used: tx.gas_limit,
                    output: vec![],
                    logs: vec![],
                    state_changes: vec![],
                    revert_reason: Some(format!("EVM create error: {e:?}")),
                };
            }
        };

        match result {
            RevmExecResult::Success {
                gas_used,
                output,
                logs,
                ..
            } => {
                let (output_bytes, contract_addr_opt) = match output {
                    revm::primitives::Output::Create(bytes, addr) => (bytes.to_vec(), addr),
                    revm::primitives::Output::Call(bytes) => (bytes.to_vec(), None),
                };

                // Apply state changes back from revm
                let committed_db = &evm.context.evm.db;
                let mut state_changes = Vec::new();

                for (addr, db_acct) in &committed_db.accounts {
                    let mut our_addr = [0u8; 20];
                    our_addr.copy_from_slice(addr.as_slice());

                    let old_acct = self.accounts.get(&our_addr).cloned().unwrap_or_default();
                    let new_balance: u128 = db_acct.info.balance.to::<u128>();
                    let new_nonce = db_acct.info.nonce;

                    let acct = self.accounts.entry(our_addr).or_default();
                    acct.balance = new_balance;
                    acct.nonce = new_nonce;

                    // Apply storage changes (constructor may initialize mappings/state)
                    let mut storage_changes = Vec::new();
                    for (slot, value) in &db_acct.storage {
                        let key = slot.to_be_bytes::<32>();
                        let val = value.to_be_bytes::<32>();
                        acct.storage.insert(key, val);
                        storage_changes.push((key, val));
                    }

                    let new_code = db_acct
                        .info
                        .code
                        .as_ref()
                        .map(|c| c.original_bytes().to_vec());
                    if let Some(ref code) = new_code {
                        if !code.is_empty() {
                            acct.code = code.clone();
                        }
                    }

                    let balance_change = (new_balance.min(i128::MAX as u128) as i128)
                        .saturating_sub(old_acct.balance.min(i128::MAX as u128) as i128);
                    // Use i128 intermediate to avoid i64 wrapping for large nonces
                    let nonce_change_128 = new_nonce as i128 - old_acct.nonce as i128;
                    let nonce_change =
                        nonce_change_128.clamp(i64::MIN as i128, i64::MAX as i128) as i64;

                    if balance_change != 0
                        || nonce_change != 0
                        || !storage_changes.is_empty()
                        || new_code.is_some()
                    {
                        state_changes.push(EvmStateChange {
                            address: our_addr,
                            balance_change,
                            nonce_change,
                            storage_changes,
                            code: new_code.filter(|c| !c.is_empty()),
                        });
                    }
                }

                let output_result = if let Some(addr) = contract_addr_opt {
                    addr.as_slice().to_vec()
                } else {
                    output_bytes
                };

                let evm_logs: Vec<EvmLog> = logs
                    .iter()
                    .map(|log| {
                        let mut addr = [0u8; 20];
                        addr.copy_from_slice(log.address.as_slice());
                        EvmLog {
                            address: addr,
                            topics: log
                                .data
                                .topics()
                                .iter()
                                .map(|t| {
                                    let mut topic = [0u8; 32];
                                    topic.copy_from_slice(t.as_slice());
                                    topic
                                })
                                .collect(),
                            data: log.data.data.to_vec(),
                        }
                    })
                    .collect();

                EvmResult {
                    success: true,
                    gas_used,
                    output: output_result,
                    logs: evm_logs,
                    state_changes,
                    revert_reason: None,
                }
            }
            RevmExecResult::Revert { gas_used, output } => EvmResult {
                success: false,
                gas_used,
                output: output.to_vec(),
                logs: vec![],
                state_changes: vec![],
                revert_reason: Some("contract creation reverted".to_string()),
            },
            RevmExecResult::Halt { reason, gas_used } => EvmResult {
                success: false,
                gas_used,
                output: vec![],
                logs: vec![],
                state_changes: vec![],
                revert_reason: Some(format!("contract creation halted: {reason:?}")),
            },
        }
    }

    /// Execute a contract call using the revm EVM interpreter.
    /// This runs actual EVM bytecode (Solidity, Vyper, etc.) through the revm engine.
    fn execute_call(&mut self, tx: &EvmTransaction, to: EvmAddress) -> EvmResult {
        use revm::primitives::{
            AccountInfo, Address as RevmAddress, Bytecode, ExecutionResult as RevmExecResult,
            TxKind, B256, U256 as RevmU256,
        };
        use revm::{Evm, InMemoryDB};

        // Build in-memory database from our account state
        let mut db = InMemoryDB::default();

        for (addr, acct) in &self.accounts {
            let revm_addr = RevmAddress::from_slice(addr);
            let code_hash = if acct.code.is_empty() {
                revm::primitives::KECCAK_EMPTY
            } else {
                B256::from_slice(&pichain_crypto::hash(&acct.code).as_bytes()[..32])
            };

            let info = AccountInfo {
                balance: RevmU256::from(acct.balance),
                nonce: acct.nonce,
                code_hash,
                code: if acct.code.is_empty() {
                    None
                } else {
                    Some(Bytecode::new_raw(revm::primitives::Bytes::copy_from_slice(
                        &acct.code,
                    )))
                },
            };

            db.insert_account_info(revm_addr, info);

            // Insert storage slots
            for (key, val) in &acct.storage {
                let slot = RevmU256::from_be_bytes(*key);
                let value = RevmU256::from_be_bytes(*val);
                db.insert_account_storage(revm_addr, slot, value).ok();
            }
        }

        // Build the EVM instance
        let caller = RevmAddress::from_slice(&tx.from);
        let to_addr = RevmAddress::from_slice(&to);
        let chain_id = self.chain_id;
        let block_height = self.block_height;
        let block_timestamp = self.block_timestamp;
        let base_fee = self.base_fee;
        let coinbase_addr = RevmAddress::from_slice(&self.coinbase);
        let tx_value = tx.value;
        let tx_data = tx.data.clone();
        let tx_gas_limit = tx.gas_limit;
        let tx_gas_price = tx.gas_price;
        let tx_nonce = tx.nonce;

        let mut evm = Evm::builder()
            .with_db(db)
            .modify_cfg_env(|cfg| {
                cfg.chain_id = chain_id;
            })
            .modify_block_env(|block| {
                block.number = RevmU256::from(block_height);
                block.timestamp = RevmU256::from(block_timestamp);
                block.basefee = RevmU256::from(base_fee);
                block.coinbase = coinbase_addr;
            })
            .modify_tx_env(|tx_env| {
                tx_env.caller = caller;
                tx_env.transact_to = TxKind::Call(to_addr);
                tx_env.value = RevmU256::from(tx_value);
                tx_env.data = revm::primitives::Bytes::copy_from_slice(&tx_data);
                tx_env.gas_limit = tx_gas_limit;
                tx_env.gas_price = RevmU256::from(tx_gas_price);
                tx_env.nonce = Some(tx_nonce);
            })
            .build();

        // Execute the transaction
        let result = match evm.transact_commit() {
            Ok(r) => r,
            Err(e) => {
                return EvmResult {
                    success: false,
                    gas_used: tx.gas_limit,
                    output: vec![],
                    logs: vec![],
                    state_changes: vec![],
                    revert_reason: Some(format!("EVM error: {e:?}")),
                };
            }
        };

        // Convert revm result to our EvmResult
        match result {
            RevmExecResult::Success {
                gas_used,
                logs,
                output,
                ..
            } => {
                let output_bytes = match output {
                    revm::primitives::Output::Call(b) => b.to_vec(),
                    revm::primitives::Output::Create(b, _) => b.to_vec(),
                };

                let evm_logs: Vec<EvmLog> = logs
                    .iter()
                    .map(|log| {
                        let mut addr = [0u8; 20];
                        addr.copy_from_slice(log.address.as_slice());
                        EvmLog {
                            address: addr,
                            topics: log
                                .data
                                .topics()
                                .iter()
                                .map(|t| {
                                    let mut topic = [0u8; 32];
                                    topic.copy_from_slice(t.as_slice());
                                    topic
                                })
                                .collect(),
                            data: log.data.data.to_vec(),
                        }
                    })
                    .collect();

                // Apply state changes back to our accounts from the committed DB
                let committed_db = &evm.context.evm.db;
                let mut state_changes = Vec::new();

                for (addr, db_acct) in &committed_db.accounts {
                    let mut our_addr = [0u8; 20];
                    our_addr.copy_from_slice(addr.as_slice());

                    let old_acct = self.accounts.get(&our_addr).cloned().unwrap_or_default();
                    let new_balance: u128 = db_acct.info.balance.to::<u128>();
                    let new_nonce = db_acct.info.nonce;

                    // Update our internal state
                    let acct = self.accounts.entry(our_addr).or_default();
                    acct.balance = new_balance;
                    acct.nonce = new_nonce;

                    // Apply storage changes
                    let mut storage_changes = Vec::new();
                    for (slot, value) in &db_acct.storage {
                        let key = slot.to_be_bytes::<32>();
                        let val = value.to_be_bytes::<32>();
                        acct.storage.insert(key, val);
                        storage_changes.push((key, val));
                    }

                    // Apply code if changed
                    let new_code = db_acct
                        .info
                        .code
                        .as_ref()
                        .map(|c| c.original_bytes().to_vec());
                    if let Some(ref code) = new_code {
                        if !code.is_empty() && code != &old_acct.code {
                            acct.code = code.clone();
                        }
                    }

                    let balance_change = (new_balance.min(i128::MAX as u128) as i128)
                        .saturating_sub(old_acct.balance.min(i128::MAX as u128) as i128);
                    let nonce_change_128 = new_nonce as i128 - old_acct.nonce as i128;
                    let nonce_change =
                        nonce_change_128.clamp(i64::MIN as i128, i64::MAX as i128) as i64;

                    if balance_change != 0 || nonce_change != 0 || !storage_changes.is_empty() {
                        state_changes.push(EvmStateChange {
                            address: our_addr,
                            balance_change,
                            nonce_change,
                            storage_changes,
                            code: new_code.filter(|c| !c.is_empty() && c != &old_acct.code),
                        });
                    }
                }

                EvmResult {
                    success: true,
                    gas_used,
                    output: output_bytes,
                    logs: evm_logs,
                    state_changes,
                    revert_reason: None,
                }
            }
            RevmExecResult::Revert { gas_used, output } => EvmResult {
                success: false,
                gas_used,
                output: output.to_vec(),
                logs: vec![],
                state_changes: vec![],
                revert_reason: Some("execution reverted".to_string()),
            },
            RevmExecResult::Halt { reason, gas_used } => EvmResult {
                success: false,
                gas_used,
                output: vec![],
                logs: vec![],
                state_changes: vec![],
                revert_reason: Some(format!("execution halted: {reason:?}")),
            },
        }
    }

    /// Compute CREATE address: keccak256(rlp([sender, nonce]))[12:]
    /// Simplified: hash(sender + nonce bytes) and take last 20 bytes.
    #[allow(dead_code)] // Used when CREATE opcode support is enabled
    fn compute_create_address(sender: &EvmAddress, nonce: u64) -> EvmAddress {
        let mut input = Vec::with_capacity(28);
        input.extend_from_slice(sender);
        input.extend_from_slice(&nonce.to_be_bytes());
        let hash = pichain_crypto::hash(&input);
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&hash.as_bytes()[12..32]);
        addr
    }

    /// Get the chain ID.
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Get current block height in EVM context.
    pub fn block_height(&self) -> u64 {
        self.block_height
    }
}

/// RPC methods for Ethereum JSON-RPC compatibility.
/// These map eth_* methods to PIChain state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EthRpcMethod {
    /// eth_chainId
    ChainId,
    /// eth_blockNumber
    BlockNumber,
    /// eth_getBalance
    GetBalance { address: EvmAddress, block: String },
    /// eth_getTransactionCount
    GetTransactionCount { address: EvmAddress, block: String },
    /// eth_sendRawTransaction
    SendRawTransaction { raw_tx: Vec<u8> },
    /// eth_call
    Call { tx: EvmTransaction, block: String },
    /// eth_estimateGas
    EstimateGas { tx: EvmTransaction },
    /// eth_getCode
    GetCode { address: EvmAddress, block: String },
    /// eth_gasPrice
    GasPrice,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_to_evm_conversion() {
        // 1 PI (9 decimals) = 10^18 in EVM (18 decimals)
        let pi = 1_000_000_000u64; // 1 PI
        let evm = EvmExecutor::pi_to_evm(pi);
        assert_eq!(evm, 1_000_000_000_000_000_000u128); // 10^18
    }

    #[test]
    fn evm_to_pi_conversion() {
        let evm = 1_500_000_000_000_000_000u128; // 1.5 in 18 decimals
        let pi = EvmExecutor::evm_to_pi(evm);
        assert_eq!(pi, 1_500_000_000); // 1.5 PI in 9 decimals
    }

    #[test]
    fn address_conversion_roundtrip() {
        let pi_addr = PiAddress([42u8; 20]);
        let evm_addr = EvmExecutor::pi_addr_to_evm(&pi_addr);
        let back = EvmExecutor::evm_addr_to_pi(&evm_addr);
        assert_eq!(pi_addr, back);
    }

    #[test]
    fn simple_transfer() {
        let mut executor = EvmExecutor::new(31415);

        // Fund sender
        let sender = [1u8; 20];
        let recipient = [2u8; 20];
        executor.set_account(
            sender,
            EvmAccountState {
                balance: 10_000_000_000_000_000_000u128, // 10 ETH equivalent
                nonce: 0,
                ..Default::default()
            },
        );

        let tx = EvmTransaction {
            from: sender,
            to: Some(recipient),
            value: 1_000_000_000_000_000_000, // 1 ETH equivalent
            data: vec![],
            gas_limit: 21_000,
            gas_price: 1_000_000_000, // 1 Gwei
            nonce: 0,
            chain_id: 31415,
        };

        let result = executor.execute(&tx);
        assert!(result.success);
        assert_eq!(result.gas_used, 21_000);

        let recipient_acct = executor.get_account(&recipient);
        assert_eq!(recipient_acct.balance, 1_000_000_000_000_000_000);
    }

    #[test]
    fn contract_creation() {
        let mut executor = EvmExecutor::new(31415);

        let sender = [1u8; 20];
        executor.set_account(
            sender,
            EvmAccountState {
                balance: 100_000_000_000_000_000_000u128,
                nonce: 0,
                ..Default::default()
            },
        );

        // Init code that deploys runtime bytecode [0x00] (STOP):
        //   PUSH1 0x00  - push 0 (the runtime byte = STOP opcode)
        //   PUSH1 0x00  - memory offset 0
        //   MSTORE8     - store single byte at memory[0]
        //   PUSH1 0x01  - return size = 1 byte
        //   PUSH1 0x00  - return offset = 0
        //   RETURN      - return the runtime code
        let init_code = vec![0x60, 0x00, 0x60, 0x00, 0x53, 0x60, 0x01, 0x60, 0x00, 0xf3];
        let expected_runtime = vec![0x00u8]; // STOP opcode

        let tx = EvmTransaction {
            from: sender,
            to: None,
            value: 0,
            data: init_code,
            gas_limit: 1_000_000,
            gas_price: 1_000_000_000,
            nonce: 0,
            chain_id: 31415,
        };

        let result = executor.execute(&tx);
        assert!(
            result.success,
            "contract creation failed: {:?}",
            result.revert_reason
        );
        assert!(!result.output.is_empty(), "no contract address returned");

        let contract_addr: EvmAddress = result.output[..20].try_into().unwrap();
        let contract_acct = executor.get_account(&contract_addr);
        assert_eq!(
            contract_acct.code, expected_runtime,
            "deployed runtime code should match"
        );
    }

    #[test]
    fn insufficient_balance_reverts() {
        let mut executor = EvmExecutor::new(31415);

        let sender = [1u8; 20];
        executor.set_account(
            sender,
            EvmAccountState {
                balance: 100, // Very small balance
                nonce: 0,
                ..Default::default()
            },
        );

        let tx = EvmTransaction {
            from: sender,
            to: Some([2u8; 20]),
            value: 1_000_000,
            data: vec![],
            gas_limit: 21_000,
            gas_price: 1,
            nonce: 0,
            chain_id: 31415,
        };

        let result = executor.execute(&tx);
        assert!(!result.success);
        assert_eq!(result.revert_reason.unwrap(), "insufficient balance");
    }

    #[test]
    fn nonce_mismatch_reverts() {
        let mut executor = EvmExecutor::new(31415);

        let sender = [1u8; 20];
        executor.set_account(
            sender,
            EvmAccountState {
                balance: 100_000_000_000_000_000_000u128,
                nonce: 5,
                ..Default::default()
            },
        );

        let tx = EvmTransaction {
            from: sender,
            to: Some([2u8; 20]),
            value: 0,
            data: vec![],
            gas_limit: 21_000,
            gas_price: 1,
            nonce: 0, // Wrong nonce!
            chain_id: 31415,
        };

        let result = executor.execute(&tx);
        assert!(!result.success);
        assert!(result.revert_reason.unwrap().contains("nonce mismatch"));
    }

    #[test]
    fn gas_limit_exceeded_reverts() {
        let mut executor = EvmExecutor::new(31415);
        let tx = EvmTransaction {
            from: [1u8; 20],
            to: None,
            value: 0,
            data: vec![],
            gas_limit: MAX_EVM_GAS + 1,
            gas_price: 1,
            nonce: 0,
            chain_id: 31415,
        };

        let result = executor.execute(&tx);
        assert!(!result.success);
    }

    #[test]
    fn create_address_deterministic() {
        let sender = [1u8; 20];
        let addr1 = EvmExecutor::compute_create_address(&sender, 0);
        let addr2 = EvmExecutor::compute_create_address(&sender, 0);
        assert_eq!(addr1, addr2);

        // Different nonce → different address
        let addr3 = EvmExecutor::compute_create_address(&sender, 1);
        assert_ne!(addr1, addr3);
    }

    #[test]
    fn pi_account_loading() {
        let mut executor = EvmExecutor::new(31415);
        let pi_addr = PiAddress([42u8; 20]);
        let balance = 5_000_000_000u64; // 5 PI

        executor.load_pi_account(pi_addr, balance);

        let evm_addr = EvmExecutor::pi_addr_to_evm(&pi_addr);
        let acct = executor.get_account(&evm_addr);
        assert_eq!(acct.balance, EvmExecutor::pi_to_evm(balance));
    }

    #[test]
    fn evm_transfer_uses_checked_subtraction() {
        let mut executor = EvmExecutor::new(31415);

        // Fund sender with enough for value + gas
        let sender = [1u8; 20];
        let recipient = [2u8; 20];
        let value = 1_000_000_000_000_000_000u128; // 1 ETH equivalent
        let gas_cost = 21_000u128 * 1_000_000_000u128; // 21000 * 1 Gwei
        let total_needed = value + gas_cost;

        executor.set_account(
            sender,
            EvmAccountState {
                balance: total_needed, // Exactly enough
                nonce: 0,
                ..Default::default()
            },
        );

        let tx = EvmTransaction {
            from: sender,
            to: Some(recipient),
            value,
            data: vec![],
            gas_limit: 21_000,
            gas_price: 1_000_000_000,
            nonce: 0,
            chain_id: 31415,
        };

        let result = executor.execute(&tx);
        assert!(result.success, "Transfer should succeed with exact balance");

        // Sender balance should be exactly 0 (no underflow)
        let sender_acct = executor.get_account(&sender);
        assert_eq!(
            sender_acct.balance, 0,
            "Sender balance should be exactly 0 after exact spend"
        );

        // Recipient should have received the value
        let recipient_acct = executor.get_account(&recipient);
        assert_eq!(recipient_acct.balance, value);
    }
}
