//! PIChain Execution Engine
//!
//! Block-STM parallel execution (inspired by Aptos):
//! - Optimistically execute all transactions in parallel
//! - Automatically detect read/write conflicts
//! - Re-execute conflicting transactions sequentially
//! - ~8-16x speedup under moderate contention

pub mod block_producer;
pub mod block_stm;
pub mod dex_executor;
pub mod evm;
pub mod executor;
pub mod fee;
pub mod launchpad_executor;
pub mod mempool;
pub mod nft_executor;
pub mod pipeline;
pub mod sdk;
pub mod token_executor;
pub mod wasm_vm;

pub use block_producer::{BlockProducer, BlockProducerConfig, ProducedBlock};
pub use block_stm::{BlockSTM, MVHashMap};
pub use dex_executor::DexExecutor;
pub use evm::{EvmExecutor, EvmTransaction, EvmResult};
pub use executor::{ExecutionResult, SubExecutorChanges, TransactionExecutor};
pub use fee::FeeCalculator;
pub use launchpad_executor::LaunchpadExecutor;
pub use mempool::{MempoolConfig, MempoolError, TransactionPool};
pub use pipeline::{TransactionPipeline, PipelineConfig};
pub use nft_executor::NftExecutor;
pub use sdk::{ContractRegistry, ContractAbi, ContractMetadata, TokenStandard};
pub use token_executor::TokenExecutor;
pub use wasm_vm::WasmVM;

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("insufficient balance: need {need}, have {have}")]
    InsufficientBalance { need: u64, have: u64 },
    #[error("nonce mismatch: expected {expected}, got {got}")]
    NonceMismatch { expected: u64, got: u64 },
    #[error("out of gas: limit {limit}, used {used}")]
    OutOfGas { limit: u64, used: u64 },
    #[error("invalid transaction: {0}")]
    InvalidTransaction(String),
    #[error("contract error: {0}")]
    ContractError(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("account not found: {0}")]
    AccountNotFound(String),
}
