//! PIChain Cryptographic Primitives
//!
//! Provides Ed25519 (user transactions), BLS12-381 (consensus attestations),
//! Blake3 (general hashing), and Poseidon (ZK-friendly state trie hashing).

pub mod blake3_hash;
pub mod bls;
pub mod ed25519;
pub mod poseidon;

pub use blake3_hash::{hash, hash_concat, Hash, Hasher};
pub use bls::{
    AggregateSignature, BlsKeypair, BlsPublicKey, BlsSecretKey, BlsSignature,
};
pub use ed25519::{
    verify_batch, Address, Keypair, PublicKey, SecretKey, Signature,
};
pub use poseidon::PoseidonHasher;

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
