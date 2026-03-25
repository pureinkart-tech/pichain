//! PIChain PI-DAG Consensus
//!
//! Hybrid consensus combining:
//! - Narwhal: DAG-based data availability (all validators contribute simultaneously)
//! - Bullshark: Zero-overhead ordering on the DAG (2-round finality)
//! - Avalanche: Sub-sampling fast-path for simple transactions (<500ms)
//! - Unified ConsensusEngine: coordinates all components with finality tracking

pub mod dag;
pub mod engine;
pub mod fast_path;
pub mod pq_consensus;
pub mod staking;
pub mod validator;

pub use dag::{Certificate, DagMempool, DagRound, EquivocationEvidence, Header, ValidatorKeyMap};
pub use engine::{ConsensusConfig, ConsensusEngine, ConsensusMetrics, FinalityStatus};
pub use fast_path::AvalancheFastPath;
pub use pq_consensus::{
    PqConsensusSignatures, PqConsensusSig, PqConsensusVersion,
    PqValidatorKeyMap, consensus_message, sign_consensus, verify_consensus_sig,
};
pub use staking::{StakingManager, StakeEntry, Delegation, SlashEvent, SlashReason, StakingError};
pub use validator::{Validator, ValidatorSet};

#[derive(Debug, thiserror::Error)]
pub enum ConsensusError {
    #[error("invalid certificate: {0}")]
    InvalidCertificate(String),
    #[error("insufficient votes: got {got}, need {need}")]
    InsufficientVotes { got: usize, need: usize },
    #[error("round mismatch: expected {expected}, got {got}")]
    RoundMismatch { expected: u64, got: u64 },
    #[error("validator not found: {0}")]
    ValidatorNotFound(String),
    #[error("duplicate vote from {0}")]
    DuplicateVote(String),
    #[error("storage error: {0}")]
    Storage(String),
}
