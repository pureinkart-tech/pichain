//! PIChain Cryptographic Primitives
//!
//! Provides post-quantum signatures (ML-DSA-65 + SLH-DSA-SHAKE-128f),
//! BLS12-381 (consensus attestations), Blake3 (general hashing),
//! Poseidon (ZK-friendly state trie hashing), and core key types.

pub mod blake3_hash;
pub mod bls;
pub mod keys;
pub mod poseidon;
pub mod pq;
pub mod pq_wallet;
pub mod stark_aggregation;

pub use blake3_hash::{hash, hash_concat, Hash, Hasher};
pub use bls::{AggregateSignature, BlsKeypair, BlsPublicKey, BlsSecretKey, BlsSignature};
pub use keys::{verify_batch, Address, Keypair, PublicKey, SecretKey, Signature};
pub use poseidon::PoseidonHasher;
pub use pq::{
    pq_address, quantum_safe_hash, CryptoVersion, MlDsaPublicKey, MlDsaSecretKey, MlDsaSignature,
    PqDualSignature, PqKeypair, PqPublicKeys, SlhDsaPublicKey, SlhDsaSecretKey, SlhDsaSignature,
};
pub use pq_wallet::{generate_pq_wallet, restore_pq_wallet, verify_pq_wallet, PqWalletExport};
pub use stark_aggregation::{
    compute_tx_commitment, BlockAggregationProof, PlaceholderProver, ProofSystem, RetentionPolicy,
    StarkProver,
};

/// 32-byte hash output used throughout PIChain.
pub type Hash32 = [u8; 32];

/// 48-byte BLS public key.
pub type BlsPubkeyBytes = [u8; 48];

/// 96-byte BLS signature.
pub type BlsSigBytes = [u8; 96];

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("invalid signature")]
    InvalidSignature,
    #[error("invalid public key")]
    InvalidPublicKey,
    #[error("invalid secret key")]
    InvalidSecretKey,
    #[error("BLS aggregation failed: {0}")]
    BlsAggregation(String),
    #[error("key generation failed: {0}")]
    KeyGeneration(String),
    #[error("serialization error: {0}")]
    Serialization(String),
}
