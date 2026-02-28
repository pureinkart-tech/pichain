//! Blake3 hashing for PIChain — 10-15 GB/s, 4-7x faster than SHA-256.
//!
//! Used for transaction hashing, block hashing, and general-purpose hashing.

use serde::{Deserialize, Serialize};

/// 32-byte Blake3 hash output.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    pub const ZERO: Self = Self([0u8; 32]);

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.0.to_vec()
    }
}

impl AsRef<[u8]> for Hash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Hash({})", hex::encode(&self.0[..8]))
    }
}

impl std::fmt::Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Incremental Blake3 hasher.
pub struct Hasher {
    inner: blake3::Hasher,
}

impl Hasher {
    pub fn new() -> Self {
        Self {
            inner: blake3::Hasher::new(),
        }
    }

    pub fn update(&mut self, data: &[u8]) -> &mut Self {
        self.inner.update(data);
        self
    }

    pub fn finalize(&self) -> Hash {
        Hash(*self.inner.finalize().as_bytes())
    }
}

impl Default for Hasher {
    fn default() -> Self {
        Self::new()
    }
}

/// Hash a single byte slice.
pub fn hash(data: &[u8]) -> Hash {
    Hash(*blake3::hash(data).as_bytes())
}

/// Hash multiple byte slices with length-prefix domain separation.
///
/// Each slice is preceded by its 8-byte little-endian length, preventing
/// boundary-shift collisions where `hash_concat(&[b"AB", b"CD"])` would
/// otherwise equal `hash_concat(&[b"A", b"BCD"])`.
pub fn hash_concat(slices: &[&[u8]]) -> Hash {
    let mut hasher = Hasher::new();
    for slice in slices {
        hasher.update(&(slice.len() as u64).to_le_bytes());
        hasher.update(slice);
    }
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let h1 = hash(b"PIChain");
        let h2 = hash(b"PIChain");
        assert_eq!(h1, h2);
    }

    #[test]
    fn different_input_different_hash() {
        let h1 = hash(b"block 1");
        let h2 = hash(b"block 2");
        assert_ne!(h1, h2);
    }

    #[test]
    fn incremental_equals_oneshot() {
        let data = b"hello world PIChain";
        let oneshot = hash(data);
        let mut hasher = Hasher::new();
        hasher.update(b"hello ");
        hasher.update(b"world ");
        hasher.update(b"PIChain");
        assert_eq!(oneshot, hasher.finalize());
    }

    #[test]
    fn hash_concat_works() {
        let h = hash_concat(&[b"hello", b"world"]);
        // With length-prefix domain separation, hash_concat includes
        // each slice's length as an 8-byte LE prefix before the slice data
        let mut hasher = Hasher::new();
        hasher.update(&5u64.to_le_bytes()); // len("hello")
        hasher.update(b"hello");
        hasher.update(&5u64.to_le_bytes()); // len("world")
        hasher.update(b"world");
        assert_eq!(h, hasher.finalize());
    }

    #[test]
    fn hash_concat_domain_separation() {
        // hash_concat(&["AB", "CD"]) must NOT equal hash_concat(&["A", "BCD"])
        let h1 = hash_concat(&[b"AB", b"CD"]);
        let h2 = hash_concat(&[b"A", b"BCD"]);
        assert_ne!(h1, h2, "hash_concat must prevent boundary-shift collisions");

        // Also must differ from hash_concat(&["ABCD"])
        let h3 = hash_concat(&[b"ABCD"]);
        assert_ne!(h1, h3);
        assert_ne!(h2, h3);
    }
}
