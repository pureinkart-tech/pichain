//! PIChain Storage Layer
//!
//! RocksDB-backed storage with Jellyfish Merkle Tree for authenticated state,
//! column family separation, and efficient state pruning.

pub mod betting_store;
pub mod comment_store;
pub mod contract_store;
pub mod db;
pub mod dex_store;
pub mod dividend_store;
pub mod evm_store;
pub mod jmt;
pub mod jmt_db;
pub mod launchpad_store;
pub mod migration;
pub mod nft_store;
pub mod state;
pub mod token_store;

pub use betting_store::BettingStore;
pub use comment_store::{Comment, CommentStore};
pub use contract_store::ContractStorageStore;
pub use db::PiChainDB;
pub use dex_store::{DexStore, TradeRecord};
pub use dividend_store::{DividendPool, DividendStore};
pub use evm_store::EvmStore;
pub use jmt::{JellyfishMerkleTree, ProofSibling, StateProof};
pub use jmt_db::DbJellyfishMerkleTree;
pub use launchpad_store::LaunchpadStore;
pub use nft_store::NftStore;
pub use state::StateStore;
pub use token_store::TokenStore;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("RocksDB error: {0}")]
    RocksDB(#[from] rocksdb::Error),
    #[error("key not found: {0}")]
    NotFound(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("state root mismatch")]
    StateRootMismatch,
    #[error("database not initialized")]
    NotInitialized,
    #[error("integrity violation: {0}")]
    IntegrityViolation(String),
}
