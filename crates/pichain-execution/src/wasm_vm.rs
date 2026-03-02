//! WASM Virtual Machine — Wasmtime-based smart contract execution.
//!
//! PIChain's primary VM for smart contracts. Supports contracts written in:
//! - Rust (compiled to WASM)
//! - C/C++ (compiled to WASM)
//! - AssemblyScript (TypeScript-like, compiled to WASM)
//! - Any language with a WASM target
//!
//! Features:
//! - JIT compilation for near-native performance
//! - Gas metering via fuel consumption
//! - Memory isolation per contract
//! - Host functions for blockchain state access

use pichain_crypto::ed25519::Address;
use pichain_crypto::Hash;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::info;
use wasmtime::*;

use crate::ExecutionError;

/// Gas cost constants for WASM operations.
const GAS_PER_FUEL: u64 = 1; // 1 gas = 1 wasmtime fuel unit
const BASE_CONTRACT_CALL_GAS: u64 = 10_000;
const GAS_PER_STATE_READ: u64 = 200;
const GAS_PER_STATE_WRITE: u64 = 5_000;
const GAS_PER_LOG: u64 = 375;
const GAS_PER_ALLOC_BYTE: u64 = 1; // Gas charged per byte of host allocation (key + value)
const MAX_WASM_MODULE_SIZE: usize = 2 * 1024 * 1024; // 2MB max module size
const MAX_HOST_ALLOC_SIZE: usize = 1024 * 1024; // 1MB max allocation in host functions
const DEFAULT_SAFE_ARGS_OFFSET: usize = 65536; // 1 WASM page — safe default when no __data_end/__heap_base
const MAX_WASM_MEMORY_BYTES: usize = 16 * 1024 * 1024; // 16MB max WASM linear memory per contract
const MAX_WASM_TABLE_ELEMENTS: usize = 10_000; // Max entries per table
const MAX_CONTRACT_STATE_CHANGES: usize = 1024 * 1024; // 1MB total state changes per call
const MAX_LOGS_PER_CALL: usize = 1_000; // Max logs per contract call
const MAX_TOTAL_LOG_DATA: usize = 10 * 1024 * 1024; // 10MB total log data per call

/// Contract metadata stored alongside the WASM bytecode.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContractMetadata {
    /// Contract address (derived from deployer + nonce).
    pub address: Address,
    /// Deployer address.
    pub deployer: Address,
    /// Blake3 hash of the WASM bytecode.
    pub code_hash: Hash,
    /// Size of the WASM module in bytes.
    pub code_size: u32,
    /// Block height at which the contract was deployed.
    pub deployed_at: u64,
    /// ABI/interface version.
    pub abi_version: u32,
}

/// Result of executing a WASM contract call.
#[derive(Clone, Debug)]
pub struct WasmExecutionResult {
    /// Return data from the contract.
    pub return_data: Vec<u8>,
    /// Gas consumed.
    pub gas_used: u64,
    /// State changes: key → value (empty value = deletion).
    pub state_changes: HashMap<Vec<u8>, Vec<u8>>,
    /// Emitted log entries.
    pub logs: Vec<ContractLog>,
    /// Whether the execution succeeded.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
}

/// A log entry emitted by a contract.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContractLog {
    /// Contract that emitted the log.
    pub contract: Address,
    /// Log topics (indexed fields for filtering).
    pub topics: Vec<Hash>,
    /// Log data (non-indexed).
    pub data: Vec<u8>,
}

/// Host state accessible from within WASM contracts via host functions.
struct HostState {
    /// Caller's address.
    caller: Address,
    /// Contract's own address.
    contract_address: Address,
    /// Contract's storage (key → value).
    storage: HashMap<Vec<u8>, Vec<u8>>,
    /// Emitted logs during execution.
    logs: Vec<ContractLog>,
    /// State changes made during execution.
    state_changes: HashMap<Vec<u8>, Vec<u8>>,
    /// Remaining gas.
    gas_remaining: u64,
    /// Gas consumed so far.
    gas_used: u64,
    /// Block height.
    block_height: u64,
    /// Block timestamp.
    block_timestamp: u64,
    /// Return data set by the contract.
    return_data: Vec<u8>,
}

impl HostState {
    fn consume_gas(&mut self, amount: u64) -> Result<(), ExecutionError> {
        if amount > self.gas_remaining {
            return Err(ExecutionError::OutOfGas {
                limit: self.gas_used + self.gas_remaining,
                used: self.gas_used + amount,
            });
        }
        self.gas_remaining -= amount;
        self.gas_used += amount;
        Ok(())
    }

    /// Total bytes currently stored in state_changes.
    fn state_changes_size(&self) -> usize {
        self.state_changes.iter().map(|(k, v)| k.len() + v.len()).sum()
    }
}

/// Resource limiter preventing WASM contracts from exhausting host memory.
/// Caps linear memory at 16MB and limits tables/instances.
impl ResourceLimiter for HostState {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool> {
        Ok(desired <= MAX_WASM_MEMORY_BYTES)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool> {
        Ok(desired <= MAX_WASM_TABLE_ELEMENTS)
    }
}

/// The PIChain WASM Virtual Machine.
pub struct WasmVM {
    engine: Engine,
}

impl WasmVM {
    /// Create a new WASM VM with optimized configuration.
    pub fn new() -> Result<Self, ExecutionError> {
        let mut config = Config::new();
        config.consume_fuel(true); // Enable gas metering via fuel
        config.wasm_bulk_memory(true);
        config.wasm_multi_value(true);
        // Disable features that expand attack surface without clear contract need.
        // reference_types: externref CVEs (CVE-2021-39216, CVE-2021-39218)
        // simd: Cranelift miscompilation CVEs (CVE-2023-26489)
        // threads: shared memory risks, not needed for single-threaded contracts
        // memory64: unnecessary for 16MB-capped linear memory
        config.wasm_reference_types(false);
        config.wasm_simd(false);
        config.wasm_relaxed_simd(false);
        config.wasm_threads(false);
        config.cranelift_opt_level(OptLevel::Speed);
        config.memory_init_cow(true);

        let engine = Engine::new(&config)
            .map_err(|e| ExecutionError::ContractError(format!("failed to create WASM engine: {e}")))?;

        info!("WASM VM initialized (Wasmtime JIT)");
        Ok(Self { engine })
    }

    /// Validate a WASM module before deployment.
    pub fn validate_module(&self, bytecode: &[u8]) -> Result<(), ExecutionError> {
        if bytecode.len() > MAX_WASM_MODULE_SIZE {
            return Err(ExecutionError::ContractError(format!(
                "module too large: {} bytes (max {})",
                bytecode.len(),
                MAX_WASM_MODULE_SIZE
            )));
        }

        Module::validate(&self.engine, bytecode).map_err(|e| {
            ExecutionError::ContractError(format!("invalid WASM module: {e}"))
        })?;

        Ok(())
    }

    /// Compile a WASM module (used during contract deployment).
    pub fn compile_module(&self, bytecode: &[u8]) -> Result<Module, ExecutionError> {
        self.validate_module(bytecode)?;

        Module::new(&self.engine, bytecode).map_err(|e| {
            ExecutionError::ContractError(format!("failed to compile WASM module: {e}"))
        })
    }

    /// Execute a contract call.
    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &self,
        bytecode: &[u8],
        function_name: &str,
        args: &[u8],
        caller: Address,
        contract_address: Address,
        storage: HashMap<Vec<u8>, Vec<u8>>,
        gas_limit: u64,
        block_height: u64,
        block_timestamp: u64,
    ) -> WasmExecutionResult {
        // Compile the module
        let module = match self.compile_module(bytecode) {
            Ok(m) => m,
            Err(e) => {
                return WasmExecutionResult {
                    return_data: vec![],
                    gas_used: BASE_CONTRACT_CALL_GAS,
                    state_changes: HashMap::new(),
                    logs: vec![],
                    success: false,
                    error: Some(format!("compilation error: {e}")),
                };
            }
        };

        // Create the host state
        let initial_gas_remaining = gas_limit.saturating_sub(BASE_CONTRACT_CALL_GAS);
        let host_state = HostState {
            caller,
            contract_address,
            storage,
            logs: vec![],
            state_changes: HashMap::new(),
            gas_remaining: initial_gas_remaining,
            gas_used: BASE_CONTRACT_CALL_GAS,
            block_height,
            block_timestamp,
            return_data: vec![],
        };

        // Create a new store with fuel metering and resource limits
        let mut store = Store::new(&self.engine, host_state);
        store.limiter(|state| state as &mut dyn ResourceLimiter);
        // R30-FIX: Fuel budget shares the same pool as host gas.
        // Set fuel to match gas_remaining so both draw from the same budget,
        // preventing double-counting of host gas and Wasmtime fuel.
        if let Err(e) = store.set_fuel(initial_gas_remaining / GAS_PER_FUEL) {
            return WasmExecutionResult {
                return_data: vec![],
                gas_used: gas_limit,
                state_changes: std::collections::HashMap::new(),
                logs: vec![],
                success: false,
                error: Some(format!("fuel metering not available: {e}")),
            };
        }

        // Create the linker with host functions
        let mut linker = Linker::new(&self.engine);
        if let Err(e) = register_host_functions(&mut linker) {
            return WasmExecutionResult {
                return_data: vec![],
                gas_used: gas_limit,
                state_changes: std::collections::HashMap::new(),
                logs: vec![],
                success: false,
                error: Some(format!("host function registration failed: {e}")),
            };
        }

        // Instantiate the module
        let instance = match linker.instantiate(&mut store, &module) {
            Ok(inst) => inst,
            Err(e) => {
                let state = store.into_data();
                return WasmExecutionResult {
                    return_data: vec![],
                    gas_used: state.gas_used,
                    state_changes: HashMap::new(),
                    logs: vec![],
                    success: false,
                    error: Some(format!("instantiation error: {e}")),
                };
            }
        };

        // Find and call the target function
        let func = match instance.get_typed_func::<(i32, i32), i32>(&mut store, function_name) {
            Ok(f) => f,
            Err(_) => {
                // Try with no args
                match instance.get_typed_func::<(), i32>(&mut store, function_name) {
                    Ok(f) => {
                        match f.call(&mut store, ()) {
                            Ok(_result) => {
                                // R31-FIX: Account for both gas pools
                                let fuel_remaining = store.get_fuel().unwrap_or(0);
                                let state = store.into_data();
                                let effective_remaining = state.gas_remaining.min(fuel_remaining);
                                return WasmExecutionResult {
                                    return_data: state.return_data,
                                    gas_used: gas_limit.saturating_sub(effective_remaining).max(BASE_CONTRACT_CALL_GAS),
                                    state_changes: state.state_changes,
                                    logs: state.logs,
                                    success: true,
                                    error: None,
                                };
                            }
                            Err(e) => {
                                // R31-FIX: Account for both gas pools
                                let fuel_remaining = store.get_fuel().unwrap_or(0);
                                let state = store.into_data();
                                let effective_remaining = state.gas_remaining.min(fuel_remaining);
                                return WasmExecutionResult {
                                    return_data: vec![],
                                    gas_used: gas_limit.saturating_sub(effective_remaining).max(BASE_CONTRACT_CALL_GAS),
                                    state_changes: HashMap::new(),
                                    logs: vec![],
                                    success: false,
                                    error: Some(format!("execution error: {e}")),
                                };
                            }
                        }
                    }
                    Err(e) => {
                        let state = store.into_data();
                        return WasmExecutionResult {
                            return_data: vec![],
                            gas_used: state.gas_used,
                            state_changes: HashMap::new(),
                            logs: vec![],
                            success: false,
                            error: Some(format!("function '{function_name}' not found: {e}")),
                        };
                    }
                }
            }
        };

        // Write args into WASM memory
        let memory = match instance.get_memory(&mut store, "memory") {
            Some(m) => m,
            None => {
                let state = store.into_data();
                return WasmExecutionResult {
                    return_data: vec![],
                    gas_used: state.gas_used,
                    state_changes: HashMap::new(),
                    logs: vec![],
                    success: false,
                    error: Some("no memory export found".to_string()),
                };
            }
        };

        // Determine safe offset for writing args (avoids corrupting data segment)
        let args_offset = resolve_args_offset(&instance, &mut store);
        let args_len = args.len().min(i32::MAX as usize) as i32;

        // Ensure memory is large enough to hold args at the resolved offset
        let required_end = args_offset.saturating_add(args.len());
        let current_size = memory.data_size(&store);
        if required_end > current_size {
            if required_end > MAX_WASM_MEMORY_BYTES {
                let state = store.into_data();
                return WasmExecutionResult {
                    return_data: vec![],
                    gas_used: state.gas_used,
                    state_changes: HashMap::new(),
                    logs: vec![],
                    success: false,
                    error: Some(format!(
                        "args require {} bytes at offset {}, exceeds {}MB memory limit",
                        args.len(), args_offset, MAX_WASM_MEMORY_BYTES / (1024 * 1024)
                    )),
                };
            }
            let deficit = required_end - current_size;
            let pages_needed = deficit.div_ceil(65536) as u64;
            if let Err(e) = memory.grow(&mut store, pages_needed) {
                let state = store.into_data();
                return WasmExecutionResult {
                    return_data: vec![],
                    gas_used: state.gas_used,
                    state_changes: HashMap::new(),
                    logs: vec![],
                    success: false,
                    error: Some(format!(
                        "cannot grow memory for args: need {} bytes at offset {}, have {} bytes: {e}",
                        args.len(), args_offset, current_size
                    )),
                };
            }
        }

        let args_ptr = args_offset as i32;
        if !args.is_empty() {
            if let Err(e) = memory.write(&mut store, args_offset, args) {
                let state = store.into_data();
                return WasmExecutionResult {
                    return_data: vec![],
                    gas_used: state.gas_used,
                    state_changes: HashMap::new(),
                    logs: vec![],
                    success: false,
                    error: Some(format!("memory write error: {e}")),
                };
            }
        }

        // Call the function
        match func.call(&mut store, (args_ptr, args_len)) {
            Ok(_result) => {
                // R31-FIX: Account for BOTH host gas AND Wasmtime fuel.
                // Host functions consume gas_remaining but not Wasmtime fuel,
                // and WASM instructions burn Wasmtime fuel but not gas_remaining.
                // Use the minimum of the two as the effective remaining gas.
                let fuel_remaining = store.get_fuel().unwrap_or(0);
                let state = store.into_data();
                let effective_remaining = state.gas_remaining.min(fuel_remaining);

                WasmExecutionResult {
                    return_data: state.return_data,
                    gas_used: gas_limit.saturating_sub(effective_remaining).max(BASE_CONTRACT_CALL_GAS),
                    state_changes: state.state_changes,
                    logs: state.logs,
                    success: true,
                    error: None,
                }
            }
            Err(e) => {
                let fuel_remaining = store.get_fuel().unwrap_or(0);
                let state = store.into_data();

                // Check if it was an out-of-fuel error
                let is_oog = fuel_remaining == 0;
                // R31-FIX: Use minimum of both gas pools as effective remaining
                let effective_remaining = state.gas_remaining.min(fuel_remaining);
                WasmExecutionResult {
                    return_data: vec![],
                    gas_used: gas_limit.saturating_sub(effective_remaining).max(BASE_CONTRACT_CALL_GAS),
                    state_changes: HashMap::new(), // Revert on failure
                    logs: vec![],
                    success: false,
                    error: Some(if is_oog {
                        "out of gas".to_string()
                    } else {
                        format!("execution error: {e}")
                    }),
                }
            }
        }
    }

    /// Get the engine for advanced operations.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}

/// Determine the safe memory offset for writing contract arguments.
///
/// WASM modules compiled from Rust/C/C++/AssemblyScript export `__data_end` or
/// `__heap_base` globals indicating where static data ends and free memory begins.
/// Writing args at or above this offset avoids corrupting the module's data segment.
fn resolve_args_offset(instance: &Instance, store: &mut Store<HostState>) -> usize {
    // Try __heap_base first (accounts for both data and stack)
    if let Some(global) = instance.get_global(&mut *store, "__heap_base") {
        if let Val::I32(offset) = global.get(&mut *store) {
            if offset > 0 {
                return offset as usize;
            }
        }
    }
    // Fall back to __data_end
    if let Some(global) = instance.get_global(&mut *store, "__data_end") {
        if let Val::I32(offset) = global.get(&mut *store) {
            if offset > 0 {
                return offset as usize;
            }
        }
    }
    DEFAULT_SAFE_ARGS_OFFSET
}

/// Register host functions that WASM contracts can call.
fn register_host_functions(linker: &mut Linker<HostState>) -> Result<(), String> {
    // pi_storage_read(key_ptr, key_len, val_ptr, val_max_len) -> val_len
    // Returns actual value length (may exceed val_max_len — only val_max_len bytes written).
    // Guest should call with val_max_len=0 first to query the size, then allocate and re-call.
    linker
        .func_wrap(
            "pichain",
            "storage_read",
            |mut caller: Caller<'_, HostState>, key_ptr: i32, key_len: i32, val_ptr: i32, val_max_len: i32| -> i32 {
                let state = caller.data_mut();
                if state.consume_gas(GAS_PER_STATE_READ).is_err() {
                    return -1;
                }

                let memory = caller.get_export("memory").and_then(|e| e.into_memory());
                let memory = match memory {
                    Some(m) => m,
                    None => return -1,
                };

                // Read key from WASM memory (with bounds check)
                if key_len < 0 || key_len as usize > MAX_HOST_ALLOC_SIZE {
                    return -2; // Invalid key length
                }
                if key_ptr < 0 {
                    return -2; // Invalid pointer
                }

                // Explicit bounds check: key_ptr + key_len must fit in WASM memory
                let key_end = (key_ptr as u64).checked_add(key_len as u64);
                match key_end {
                    None => return -2, // Overflow
                    Some(end) => {
                        if end > memory.data_size(&caller) as u64 {
                            return -3; // Out of bounds
                        }
                    }
                }

                // Charge gas proportional to allocation size (key bytes)
                let alloc_gas = (key_len as u64).saturating_mul(GAS_PER_ALLOC_BYTE);
                if caller.data_mut().consume_gas(alloc_gas).is_err() {
                    return -1; // Out of gas
                }

                let mut key = vec![0u8; key_len as usize];
                if memory.read(&caller, key_ptr as usize, &mut key).is_err() {
                    return -1;
                }

                // Lookup in state_changes first (read-after-write), then storage
                let state = caller.data();
                match state.state_changes.get(&key).or_else(|| state.storage.get(&key)) {
                    Some(value) => {
                        let actual_len = value.len() as i32;
                        // Only write up to val_max_len bytes to prevent guest buffer overflow.
                        // Guest can call with val_max_len=0 to query size first.
                        let write_len = if val_max_len <= 0 {
                            0usize
                        } else {
                            (value.len()).min(val_max_len as usize)
                        };
                        if write_len > 0 {
                            let value = value.clone();
                            if memory
                                .write(&mut caller, val_ptr as usize, &value[..write_len])
                                .is_err()
                            {
                                return -1;
                            }
                        }
                        // Return the actual value length so guest knows full size
                        actual_len
                    }
                    None => 0,
                }
            },
        )
        .map_err(|e| format!("register storage_read: {e}"))?;

    // pi_storage_write(key_ptr, key_len, val_ptr, val_len)
    linker
        .func_wrap(
            "pichain",
            "storage_write",
            |mut caller: Caller<'_, HostState>,
             key_ptr: i32,
             key_len: i32,
             val_ptr: i32,
             val_len: i32|
             -> i32 {
                let state = caller.data_mut();
                if state.consume_gas(GAS_PER_STATE_WRITE).is_err() {
                    return -1;
                }

                let memory = caller.get_export("memory").and_then(|e| e.into_memory());
                let memory = match memory {
                    Some(m) => m,
                    None => return -1,
                };

                if key_len < 0 || key_len as usize > MAX_HOST_ALLOC_SIZE
                    || val_len < 0 || val_len as usize > MAX_HOST_ALLOC_SIZE
                {
                    return -2; // Invalid length
                }
                if key_ptr < 0 || val_ptr < 0 {
                    return -2; // Invalid pointer
                }

                // Explicit bounds check: key_ptr + key_len must fit in WASM memory
                let key_end = (key_ptr as u64).checked_add(key_len as u64);
                match key_end {
                    None => return -2, // Overflow
                    Some(end) => {
                        if end > memory.data_size(&caller) as u64 {
                            return -3; // Out of bounds
                        }
                    }
                }

                // Explicit bounds check: val_ptr + val_len must fit in WASM memory
                let val_end = (val_ptr as u64).checked_add(val_len as u64);
                match val_end {
                    None => return -2, // Overflow
                    Some(end) => {
                        if end > memory.data_size(&caller) as u64 {
                            return -3; // Out of bounds
                        }
                    }
                }

                // Charge gas proportional to allocation size (key + value bytes)
                let alloc_gas = (key_len as u64)
                    .saturating_add(val_len as u64)
                    .saturating_mul(GAS_PER_ALLOC_BYTE);
                if caller.data_mut().consume_gas(alloc_gas).is_err() {
                    return -1; // Out of gas
                }

                let mut key = vec![0u8; key_len as usize];
                let mut val = vec![0u8; val_len as usize];
                if memory.read(&caller, key_ptr as usize, &mut key).is_err()
                    || memory.read(&caller, val_ptr as usize, &mut val).is_err()
                {
                    return -1;
                }

                let state = caller.data_mut();
                // Enforce total state changes size limit to prevent memory exhaustion.
                // Account for replaced keys: subtract old entry size if key already exists.
                let old_size: usize = state.state_changes.get(&key).map(|v| key.len() + v.len()).unwrap_or(0);
                let new_size = state.state_changes_size() - old_size + key.len() + val.len();
                if new_size > MAX_CONTRACT_STATE_CHANGES {
                    return -1;
                }
                state.state_changes.insert(key, val);
                0
            },
        )
        .map_err(|e| format!("register storage_write: {e}"))?;

    // pi_log(data_ptr, data_len)
    linker
        .func_wrap(
            "pichain",
            "log",
            |mut caller: Caller<'_, HostState>, data_ptr: i32, data_len: i32| -> i32 {
                let state = caller.data_mut();
                if state.consume_gas(GAS_PER_LOG).is_err() {
                    return -1;
                }

                let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };

                if data_len < 0 || data_len as usize > MAX_HOST_ALLOC_SIZE {
                    return -1;
                }

                // SECURITY: Limit log count and total log data to prevent memory exhaustion
                {
                    let state = caller.data();
                    if state.logs.len() >= MAX_LOGS_PER_CALL {
                        return -1; // Too many logs
                    }
                    let current_log_data: usize = state.logs.iter().map(|l| l.data.len()).sum();
                    if current_log_data.saturating_add(data_len as usize) > MAX_TOTAL_LOG_DATA {
                        return -1; // Total log data too large
                    }
                }

                let mut data = vec![0u8; data_len as usize];
                if memory.read(&caller, data_ptr as usize, &mut data).is_err() {
                    return -1;
                }

                let contract = caller.data().contract_address;
                caller.data_mut().logs.push(ContractLog {
                    contract,
                    topics: vec![],
                    data,
                });
                0
            },
        )
        .map_err(|e| format!("register log: {e}"))?;

    // pi_caller() -> writes caller address to memory at ptr, returns 20
    linker
        .func_wrap(
            "pichain",
            "caller",
            |mut caller: Caller<'_, HostState>, ptr: i32| -> i32 {
                let state = caller.data_mut();
                if state.consume_gas(100).is_err() {
                    return -1;
                }

                let addr = caller.data().caller;
                let memory = caller.get_export("memory").and_then(|e| e.into_memory());
                let memory = match memory {
                    Some(m) => m,
                    None => return -1,
                };
                if memory.write(&mut caller, ptr as usize, &addr.0).is_err() {
                    return -1;
                }
                20
            },
        )
        .map_err(|e| format!("register caller: {e}"))?;

    // pi_block_height() -> u64
    linker
        .func_wrap(
            "pichain",
            "block_height",
            |caller: Caller<'_, HostState>| -> i64 {
                caller.data().block_height.min(i64::MAX as u64) as i64
            },
        )
        .map_err(|e| format!("register block_height: {e}"))?;

    // pi_block_timestamp() -> u64
    linker
        .func_wrap(
            "pichain",
            "block_timestamp",
            |caller: Caller<'_, HostState>| -> i64 {
                caller.data().block_timestamp.min(i64::MAX as u64) as i64
            },
        )
        .map_err(|e| format!("register block_timestamp: {e}"))?;

    // pi_set_return(ptr, len) — set the return data
    linker
        .func_wrap(
            "pichain",
            "set_return",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
                let memory = caller.get_export("memory").and_then(|e| e.into_memory());
                let memory = match memory {
                    Some(m) => m,
                    None => return,
                };
                if len < 0 || len as usize > MAX_HOST_ALLOC_SIZE {
                    return;
                }
                if ptr < 0 {
                    return;
                }

                // Gas charge: base cost + per-byte cost
                let gas = 100u64 + (len as u64).saturating_mul(1);
                let state_ref = caller.data_mut();
                if state_ref.consume_gas(gas).is_err() {
                    return;
                }

                let mut data = vec![0u8; len as usize];
                if memory.read(&caller, ptr as usize, &mut data).is_ok() {
                    caller.data_mut().return_data = data;
                }
            },
        )
        .map_err(|e| format!("register set_return: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_wasm_vm() {
        let vm = WasmVM::new().unwrap();
        assert!(!std::ptr::eq(vm.engine(), std::ptr::null()));
    }

    #[test]
    fn validate_invalid_module() {
        let vm = WasmVM::new().unwrap();
        let result = vm.validate_module(b"not a wasm module");
        assert!(result.is_err());
    }

    #[test]
    fn validate_module_too_large() {
        let vm = WasmVM::new().unwrap();
        let big = vec![0u8; MAX_WASM_MODULE_SIZE + 1];
        let result = vm.validate_module(&big);
        assert!(result.is_err());
        assert!(
            format!("{:?}", result.unwrap_err()).contains("too large"),
            "should report module too large"
        );
    }

    #[test]
    fn validate_minimal_wasm_module() {
        let vm = WasmVM::new().unwrap();
        // Minimal valid WASM module (magic number + version + empty)
        let minimal_wasm = wat::parse_str("(module)").unwrap();
        assert!(vm.validate_module(&minimal_wasm).is_ok());
    }

    #[test]
    fn execute_simple_module() {
        let vm = WasmVM::new().unwrap();

        // Simple WASM module that returns 42
        let wasm = wat::parse_str(
            r#"
            (module
                (func (export "main") (result i32)
                    i32.const 42
                )
                (memory (export "memory") 1)
            )
            "#,
        )
        .unwrap();

        let result = vm.execute(
            &wasm,
            "main",
            &[],
            Address([1; 20]),
            Address([2; 20]),
            HashMap::new(),
            1_000_000,
            1,
            1000,
        );

        assert!(result.success, "execution should succeed: {:?}", result.error);
    }

    #[test]
    fn gas_metering_works() {
        let vm = WasmVM::new().unwrap();

        // Module with an infinite loop — should run out of gas
        let wasm = wat::parse_str(
            r#"
            (module
                (func (export "main") (result i32)
                    (local i32)
                    (loop $loop
                        (local.set 0 (i32.add (local.get 0) (i32.const 1)))
                        (br $loop)
                    )
                    local.get 0
                )
                (memory (export "memory") 1)
            )
            "#,
        )
        .unwrap();

        let result = vm.execute(
            &wasm,
            "main",
            &[],
            Address([1; 20]),
            Address([2; 20]),
            HashMap::new(),
            100, // Very low gas limit
            1,
            1000,
        );

        assert!(!result.success, "should fail due to out of gas");
    }

    #[test]
    fn args_written_at_heap_base_not_zero() {
        let vm = WasmVM::new().unwrap();

        // Module that exports __heap_base = 1024 with data at offset 0
        let wasm = wat::parse_str(
            r#"
            (module
                (memory (export "memory") 1)
                (data (i32.const 0) "\AA")
                (global (export "__heap_base") i32 (i32.const 1024))
                (func (export "main") (param $args_ptr i32) (param $args_len i32) (result i32)
                    ;; Return the byte at offset 0 — should still be 0xAA (not corrupted)
                    (i32.load8_u (i32.const 0))
                )
            )
            "#,
        )
        .unwrap();

        let result = vm.execute(
            &wasm,
            "main",
            &[0xBB],
            Address([1; 20]),
            Address([2; 20]),
            HashMap::new(),
            1_000_000,
            1,
            1000,
        );

        assert!(result.success, "execution should succeed: {:?}", result.error);
    }

    #[test]
    fn args_fallback_to_data_end() {
        let vm = WasmVM::new().unwrap();

        let wasm = wat::parse_str(
            r#"
            (module
                (memory (export "memory") 1)
                (data (i32.const 0) "\CC")
                (global (export "__data_end") i32 (i32.const 512))
                (func (export "main") (param $args_ptr i32) (param $args_len i32) (result i32)
                    ;; Return args_ptr to verify offset — should be 512
                    (local.get $args_ptr)
                )
            )
            "#,
        )
        .unwrap();

        let result = vm.execute(
            &wasm,
            "main",
            &[0xDD],
            Address([1; 20]),
            Address([2; 20]),
            HashMap::new(),
            1_000_000,
            1,
            1000,
        );

        assert!(result.success, "should succeed: {:?}", result.error);
    }

    #[test]
    fn args_fallback_to_default_offset() {
        let vm = WasmVM::new().unwrap();

        // Module with NO __data_end or __heap_base exports
        let wasm = wat::parse_str(
            r#"
            (module
                (memory (export "memory") 2)
                (data (i32.const 0) "\EE")
                (func (export "main") (param $args_ptr i32) (param $args_len i32) (result i32)
                    ;; Verify data segment at offset 0 is intact
                    (i32.load8_u (i32.const 0))
                )
            )
            "#,
        )
        .unwrap();

        let result = vm.execute(
            &wasm,
            "main",
            &[0xFF],
            Address([1; 20]),
            Address([2; 20]),
            HashMap::new(),
            1_000_000,
            1,
            1000,
        );

        assert!(result.success, "should succeed: {:?}", result.error);
    }

    #[test]
    fn args_trigger_memory_growth_if_needed() {
        let vm = WasmVM::new().unwrap();

        // Module with 1 page (64KB) and __heap_base near end of page
        let wasm = wat::parse_str(
            r#"
            (module
                (memory (export "memory") 1 4)
                (global (export "__heap_base") i32 (i32.const 65500))
                (func (export "main") (param $args_ptr i32) (param $args_len i32) (result i32)
                    i32.const 0
                )
            )
            "#,
        )
        .unwrap();

        // 100 bytes at offset 65500 requires > 1 page
        let args = vec![0x42u8; 100];
        let result = vm.execute(
            &wasm,
            "main",
            &args,
            Address([1; 20]),
            Address([2; 20]),
            HashMap::new(),
            1_000_000,
            1,
            1000,
        );

        assert!(result.success, "should succeed after growing memory: {:?}", result.error);
    }
}
