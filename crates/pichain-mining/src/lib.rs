//! PIChain Mining: Proof of Useful Work via PI Digit Computation
//!
//! Miners compute new hexadecimal digits of PI using the Bailey-Borwein-Plouffe (BBP)
//! formula, which allows computing individual hex digits of PI without computing all
//! preceding digits. This is naturally parallelizable — different miners work on
//! different digit ranges.
//!
//! Verified results are stored on-chain as permanent mathematical artifacts.
//! "PIChain — Computing the Infinite"

pub mod bbp;
pub mod difficulty;
pub mod onchain;
pub mod proof;
pub mod registry;
pub mod reward;
pub mod vdf;

pub use bbp::BbpComputer;
pub use difficulty::{DifficultyAdjuster, check_proof_difficulty, find_nonce_parallel};
pub use onchain::{MiningProcessor, MiningStats, VerificationResult};
pub use proof::{MiningProof, ProofVerifier};
pub use registry::{DigitRange, DigitRegistry, DigitStats};
pub use reward::RewardCalculator;
pub use vdf::{VdfProof, vdf_compute, vdf_verify, vdf_seed, required_iterations};

#[derive(Debug, thiserror::Error)]
pub enum MiningError {
    #[error("invalid proof: {0}")]
    InvalidProof(String),
    #[error("digit range already computed: position {0}")]
    AlreadyComputed(u64),
    #[error("computation error: {0}")]
    ComputationError(String),
    #[error("reward calculation error: {0}")]
    RewardError(String),
}
