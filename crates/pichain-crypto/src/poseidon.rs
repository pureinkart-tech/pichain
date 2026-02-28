//! Poseidon hash function — ZK-friendly state trie hashing.
//!
//! Poseidon uses ~300 constraints in ZK circuits vs ~25,000 for SHA-256,
//! making it 80x cheaper for ZK proofs. Used for Jellyfish Merkle Tree nodes.
//!
//! This is a simplified reference implementation. Production deployment
//! would use optimized field arithmetic over BN254 or BLS12-381 scalar field.

use serde::{Deserialize, Serialize};

use crate::blake3_hash;

/// Poseidon hash output (32 bytes).
/// In production, this would be a field element. For the initial implementation,
/// we use Blake3 as a stand-in with the same interface, to be replaced with
/// a proper Poseidon implementation using arkworks once the state trie is built.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PoseidonHash(pub [u8; 32]);

impl PoseidonHash {
    pub const ZERO: Self = Self([0u8; 32]);

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for PoseidonHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Poseidon({}..)", hex::encode(&self.0[..8]))
    }
}

impl std::fmt::Display for PoseidonHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Poseidon hasher for Jellyfish Merkle Tree nodes.
///
/// Currently delegates to Blake3 for correct behavior.
/// Will be replaced with proper Poseidon over BN254 scalar field
/// using `ark-crypto-primitives` when ZK circuits are built.
pub struct PoseidonHasher;

impl PoseidonHasher {
    /// Hash two child nodes (for binary Merkle tree internal nodes).
    pub fn hash_two(left: &[u8; 32], right: &[u8; 32]) -> PoseidonHash {
        // Domain separation: prefix with 0x02 for two-input hashes.
        // This prevents collision with leaf (0x00) and internal (0x01) hashes.
        let h = blake3_hash::hash_concat(&[&[0x02], left.as_slice(), right.as_slice()]);
        PoseidonHash(*h.as_bytes())
    }

    /// Hash a leaf node (key-value pair).
    pub fn hash_leaf(key: &[u8; 32], value: &[u8]) -> PoseidonHash {
        // Domain separation: prefix with 0x00 for leaf nodes
        let h = blake3_hash::hash_concat(&[&[0x00], key.as_slice(), value]);
        PoseidonHash(*h.as_bytes())
    }

    /// Hash an internal node.
    pub fn hash_internal(left: &PoseidonHash, right: &PoseidonHash) -> PoseidonHash {
        // Domain separation: prefix with 0x01 for internal nodes
        let h = blake3_hash::hash_concat(&[
            &[0x01],
            left.0.as_slice(),
            right.0.as_slice(),
        ]);
        PoseidonHash(*h.as_bytes())
    }

    /// Hash arbitrary data (domain-separated from raw Blake3).
    pub fn hash(data: &[u8]) -> PoseidonHash {
        let h = blake3_hash::hash_concat(&[b"poseidon", data]);
        PoseidonHash(*h.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_two_deterministic() {
        let left = [1u8; 32];
        let right = [2u8; 32];
        let h1 = PoseidonHasher::hash_two(&left, &right);
        let h2 = PoseidonHasher::hash_two(&left, &right);
        assert_eq!(h1, h2);
    }

    #[test]
    fn different_inputs_different_hashes() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let h1 = PoseidonHasher::hash_two(&a, &b);
        let h2 = PoseidonHasher::hash_two(&b, &a);
        assert_ne!(h1, h2);
    }

    #[test]
    fn leaf_vs_internal_domain_separation() {
        let data = [0u8; 32];
        let leaf = PoseidonHasher::hash_leaf(&data, &data);
        let internal = PoseidonHasher::hash_internal(
            &PoseidonHash(data),
            &PoseidonHash(data),
        );
        // Domain separation ensures different outputs
        assert_ne!(leaf, internal);
    }

    #[test]
    fn all_three_domains_produce_distinct_hashes() {
        let data = [0u8; 32];
        let leaf = PoseidonHasher::hash_leaf(&data, &data);
        let internal = PoseidonHasher::hash_internal(
            &PoseidonHash(data),
            &PoseidonHash(data),
        );
        let two = PoseidonHasher::hash_two(&data, &data);
        // All three domain-separated hashes must differ
        assert_ne!(leaf, internal);
        assert_ne!(leaf, two);
        assert_ne!(internal, two);
    }
}
