//! PIChain Core Types
//!
//! Defines the fundamental data structures: Transaction, Block, Account,
//! Object (Sui-inspired ownership model), and genesis configuration.

pub mod account;
pub mod block;
pub mod dex;
pub mod genesis;
pub mod launchpad;
pub mod nft;
pub mod object;
pub mod token;
pub mod transaction;

pub use account::{Account, AccountState};
pub use block::{Block, BlockHeader};
pub use dex::{LiquidityPool, PoolId};
pub use genesis::GenesisConfig;
pub use launchpad::{LaunchId, TokenLaunch};
pub use nft::{CollectionId, Nft, NftCollection, NftId};
pub use object::{Object, ObjectId, ObjectOwner, ObjectType};
pub use token::{MintId, TokenAccount, TokenMint};
pub use transaction::{
    SignedTransaction, Transaction, TransactionData, TransactionEffect,
    TransactionKind, TransactionStatus,
};

/// PIChain epoch number.
pub type Epoch = u64;

/// PIChain round number (within consensus DAG).
pub type Round = u64;

/// Block height / sequence number.
pub type BlockHeight = u64;

/// Nonce for transaction ordering per account.
pub type Nonce = u64;

/// Gas units for compute metering.
pub type Gas = u64;

/// PI token amount in base units (1 PI = 10^9 base units, like lamports).
pub type PiAmount = u64;

/// Maximum gas per block.
pub const MAX_BLOCK_GAS: Gas = 100_000_000;

/// Base units per PI token (1 PI = 1 billion base units).
pub const BASE_UNITS_PER_PI: u64 = 1_000_000_000;

/// Total supply in base units: 3,141,592,653 PI * 10^9.
pub const TOTAL_SUPPLY: u128 = 3_141_592_653 * 1_000_000_000u128;

/// Target block time in milliseconds.
pub const TARGET_BLOCK_TIME_MS: u64 = 314;

/// Maximum validators in the network.
pub const MAX_VALIDATORS: u32 = 3_141;

/// Epoch length in blocks.
pub const EPOCH_LENGTH: u64 = 31_415;

/// Fee burn rate (25% of base fee).
pub const FEE_BURN_RATE_BPS: u16 = 2500;

/// Fee to stakers rate (65% of base fee).
pub const FEE_STAKER_RATE_BPS: u16 = 6500;

/// Fee to treasury rate (10% of base fee).
pub const FEE_TREASURY_RATE_BPS: u16 = 1000;
