//! PIChain Core Types
//!
//! Defines the fundamental data structures: Transaction, Block, Account,
//! Object (Sui-inspired ownership model), and genesis configuration.

pub mod account;
pub mod betting;
pub mod block;
pub mod dex;
pub mod genesis;
pub mod launchpad;
pub mod multisig;
pub mod nft;
pub mod object;
pub mod token;
pub mod transaction;

pub use account::{Account, AccountState};
pub use betting::{BettingMatch, MatchId};
pub use block::{Block, BlockHeader};
pub use dex::{LiquidityPool, PoolId};
pub use genesis::GenesisConfig;
pub use launchpad::{LaunchId, TokenLaunch};
pub use nft::{CollectionId, Nft, NftCollection, NftId};
pub use object::{Object, ObjectId, ObjectOwner, ObjectType};
pub use token::{MintId, TokenAccount, TokenMint};
pub use transaction::{
    SignedTransaction, Transaction, TransactionData, TransactionEffect, TransactionKind,
    TransactionStatus,
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

/// Maximum gas per block (π × 10^8).
pub const MAX_BLOCK_GAS: Gas = 314_159_265;

/// Base units per PI token (1 PI = 1 billion base units).
pub const BASE_UNITS_PER_PI: u64 = 1_000_000_000;

/// Total supply in base units: 3,141,592,653 PI × 10^9.
pub const TOTAL_SUPPLY: u128 = 3_141_592_653 * 1_000_000_000u128;

/// Target block time in milliseconds.
pub const TARGET_BLOCK_TIME_MS: u64 = 314;

/// Maximum validators in the network.
pub const MAX_VALIDATORS: u32 = 3_141;

/// Epoch length in blocks.
pub const EPOCH_LENGTH: u64 = 31_415;

/// Fee burn rate (31.41% = π × 1000 bps — permanently burned, deflationary).
pub const FEE_BURN_RATE_BPS: u16 = 3141;

/// Fee to miners rate (18.59% — flows back into mining pool).
/// Complement of burn to keep burn + miner + staker = 100%.
pub const FEE_MINER_RATE_BPS: u16 = 1859;

/// Fee to stakers/proposer rate (50% of base fee — credited to block proposer).
pub const FEE_STAKER_RATE_BPS: u16 = 5000;

/// Network mode — determines genesis config and safety guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NetworkMode {
    /// Mainnet (chain_id 314159): strict validation, real money.
    Mainnet,
    /// Devnet (chain_id 31415): relaxed validation, test tokens.
    Devnet,
    /// Custom chain_id: operator-defined behavior.
    Custom(u64),
}

impl NetworkMode {
    /// Derive mode from chain_id.
    pub fn from_chain_id(chain_id: u64) -> Self {
        match chain_id {
            314159 => NetworkMode::Mainnet,
            31415 => NetworkMode::Devnet,
            other => NetworkMode::Custom(other),
        }
    }

    /// Whether this is the production mainnet.
    pub fn is_mainnet(&self) -> bool {
        matches!(self, NetworkMode::Mainnet)
    }

    /// Whether this is devnet.
    pub fn is_devnet(&self) -> bool {
        matches!(self, NetworkMode::Devnet)
    }

    /// Chain ID for this mode.
    pub fn chain_id(&self) -> u64 {
        match self {
            NetworkMode::Mainnet => 314159,
            NetworkMode::Devnet => 31415,
            NetworkMode::Custom(id) => *id,
        }
    }
}
