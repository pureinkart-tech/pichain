//! PIChain Networking Layer
//!
//! libp2p + QUIC transport with GossipSub for transaction propagation
//! and Turbine-style erasure-coded block streaming.

pub mod bridge;
pub mod gossip;
pub mod node;
pub mod swarm;
pub mod sync;
pub mod testnet;
pub mod turbine;

pub use bridge::{BridgeManager, BridgeTransfer, BridgeChain, BridgeError};
pub use node::NetworkNode;
pub use swarm::{PiChainSwarm, SwarmBroadcaster, SwarmConfig, SwarmMessage, peer_id_from_ed25519, TOPIC_SYNC};
pub use sync::{StateSyncManager, SyncMode, SyncState, SyncRequest, SyncResponse};
pub use testnet::{LocalTestnet, TestnetConfig, TestNodeConfig};

use pichain_types::transaction::SignedTransaction;

#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("gossip error: {0}")]
    Gossip(String),
    #[error("peer not found: {0}")]
    PeerNotFound(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("connection error: {0}")]
    Connection(String),
}

/// Protocol version for message compatibility.
pub const PROTOCOL_VERSION: u32 = 1;

/// Lightweight sync messages for GossipSub-based chain synchronization.
/// Used by nodes to discover peer chain tips and request missing blocks.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum PeerSyncMessage {
    /// Request: "What is your chain tip?"
    GetChainTip,
    /// Response: "My chain tip is height H with hash."
    ChainTip {
        height: u64,
        block_hash: [u8; 32],
    },
    /// Request: "Send me blocks from start_height to end_height (inclusive)."
    GetBlocks {
        start_height: u64,
        end_height: u64,
    },
    /// Response: a single block (sent as JSON bytes).
    BlockData {
        height: u64,
        data: Vec<u8>,
    },
    /// Equivocation evidence: two conflicting certificates from the same validator
    /// in the same round. All nodes must process the same evidence for deterministic
    /// slashing, preventing validator set state divergence.
    EquivocationEvidence {
        /// Validator address that equivocated (20 bytes).
        author: [u8; 20],
        /// DAG round where equivocation occurred.
        round: u64,
        /// First certificate (serialized JSON).
        cert_a: Vec<u8>,
        /// Second conflicting certificate (serialized JSON).
        cert_b: Vec<u8>,
    },
}

/// Messages that flow through the P2P network.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum NetworkMessage {
    /// New transaction to be propagated.
    Transaction(SignedTransaction),
    /// Block data shred (Turbine-style erasure-coded piece).
    /// Use `turbine::Shred` for the full authenticated shred type.
    BlockShred {
        block_height: u64,
        shred_index: u32,
        total_shreds: u32,
        data: Vec<u8>,
    },
    /// Consensus DAG certificate.
    DagCertificate {
        round: u64,
        /// Ed25519 public key of the certificate author (32 bytes).
        author: [u8; 32],
        /// Serialized certificate data.
        data: Vec<u8>,
        /// Ed25519 signature over (round || data) by the author (64 bytes).
        signature: Vec<u8>,
    },
    /// Peer discovery / validator announcement.
    /// Includes a signature to prevent Sybil attacks.
    ValidatorAnnounce {
        /// Validator's Ed25519 public key (32 bytes).
        public_key: [u8; 32],
        /// Derived PIChain address (20 bytes).
        address: [u8; 20],
        /// Self-reported stake amount.
        stake: u64,
        /// Ed25519 signature over ("pichain-validator:" || public_key || stake_le_bytes) (64 bytes).
        signature: Vec<u8>,
    },
}
