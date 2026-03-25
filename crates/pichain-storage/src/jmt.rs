//! Jellyfish Merkle Tree — binary trie with Poseidon hashing.
//!
//! Superior to Ethereum's Merkle Patricia Trie:
//! - Binary trie (2 children) vs hexary (16) → shallower, fewer disk reads
//! - Keys are hash-distributed → uniform depth
//! - Poseidon hash → 80x cheaper in ZK circuits
//! - Versioned: each block gets its own state root

use pichain_crypto::poseidon::{PoseidonHash, PoseidonHasher};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A Jellyfish Merkle Tree node.
///
/// # Invariant
/// `Internal { left: None, right: None }` is **invalid** and must never be
/// constructed or persisted. An internal node with zero children is logically
/// empty and should be represented as `Null` or `PoseidonHash::ZERO` instead.
/// The `delete_recursive` implementations in both the in-memory and DB-backed
/// JMTs enforce this by returning `PoseidonHash::ZERO` when both children are
/// removed, but deserializing untrusted data could produce this state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JmtNode {
    /// Internal node with two children.
    /// Invariant: at least one of `left` or `right` must be `Some`.
    Internal {
        left: Option<PoseidonHash>,
        right: Option<PoseidonHash>,
    },
    /// Leaf node containing a key-value pair.
    Leaf {
        /// Full key hash (for verification).
        key_hash: [u8; 32],
        /// Serialized value.
        value_hash: PoseidonHash,
    },
    /// Null/empty node.
    Null,
}

impl JmtNode {
    /// Compute the hash of this node.
    pub fn hash(&self) -> PoseidonHash {
        match self {
            JmtNode::Internal { left, right } => {
                let l = left.unwrap_or(PoseidonHash::ZERO);
                let r = right.unwrap_or(PoseidonHash::ZERO);
                PoseidonHasher::hash_internal(&l, &r)
            }
            JmtNode::Leaf {
                key_hash,
                value_hash,
            } => PoseidonHasher::hash_leaf(key_hash, value_hash.as_bytes()),
            JmtNode::Null => PoseidonHash::ZERO,
        }
    }
}

/// In-memory Jellyfish Merkle Tree.
///
/// For the initial implementation, the tree is stored in a HashMap.
/// Production version will be backed by RocksDB via the `jmt_nodes` column family.
#[derive(Clone)]
pub struct JellyfishMerkleTree {
    /// Node storage: hash → node.
    nodes: HashMap<PoseidonHash, JmtNode>,
    /// Current root hash.
    root: PoseidonHash,
    /// Tree version (incremented per block).
    version: u64,
}

impl JellyfishMerkleTree {
    /// Create an empty tree.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            root: PoseidonHash::ZERO,
            version: 0,
        }
    }

    /// Get the current root hash.
    pub fn root_hash(&self) -> PoseidonHash {
        self.root
    }

    /// Get the current version.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Maximum nodes before triggering emergency garbage collection.
    /// With 256-bit keys, each insert creates O(depth) new internal nodes.
    /// This cap prevents memory exhaustion from mass account creation.
    const EMERGENCY_GC_THRESHOLD: usize = 500_000;

    /// Insert or update a key-value pair. Returns the new root hash.
    pub fn put(&mut self, key: &[u8; 32], value: &[u8]) -> PoseidonHash {
        // Emergency GC: if we've accumulated too many stale nodes, collect now
        if self.nodes.len() > Self::EMERGENCY_GC_THRESHOLD {
            self.gc_stale_nodes();
        }

        let value_hash = PoseidonHasher::hash(value);

        let leaf = JmtNode::Leaf {
            key_hash: *key,
            value_hash,
        };
        let leaf_hash = leaf.hash();
        self.nodes.insert(leaf_hash, leaf);

        // Build the tree path from the key bits
        self.root = self.insert_recursive(self.root, key, leaf_hash, 0);
        self.version = self.version.saturating_add(1);
        self.root
    }

    /// Get a value by key.
    pub fn get(&self, key: &[u8; 32]) -> Option<PoseidonHash> {
        self.get_recursive(self.root, key, 0)
    }

    /// Delete a key. Returns the new root hash.
    pub fn delete(&mut self, key: &[u8; 32]) -> PoseidonHash {
        self.root = self.delete_recursive(self.root, key, 0);
        self.version = self.version.saturating_add(1);
        self.root
    }

    /// Get a bit from the key at a given depth.
    fn key_bit(key: &[u8; 32], depth: usize) -> bool {
        let byte_idx = depth / 8;
        let bit_idx = 7 - (depth % 8);
        if byte_idx >= 32 {
            return false;
        }
        (key[byte_idx] >> bit_idx) & 1 == 1
    }

    fn insert_recursive(
        &mut self,
        node_hash: PoseidonHash,
        key: &[u8; 32],
        leaf_hash: PoseidonHash,
        depth: usize,
    ) -> PoseidonHash {
        if depth >= 256 {
            return leaf_hash;
        }

        if node_hash == PoseidonHash::ZERO {
            // Empty spot → place the leaf here
            return leaf_hash;
        }

        let node = match self.nodes.get(&node_hash) {
            Some(n) => n.clone(),
            None => return leaf_hash,
        };

        match node {
            JmtNode::Leaf {
                key_hash: existing_key,
                ..
            } => {
                if existing_key == *key {
                    // Update existing leaf
                    leaf_hash
                } else {
                    // Collision: create internal node and push both leaves down
                    let existing_bit = Self::key_bit(&existing_key, depth);
                    let new_bit = Self::key_bit(key, depth);

                    if existing_bit == new_bit {
                        // Same direction → recurse deeper
                        let child = self.insert_recursive(node_hash, key, leaf_hash, depth + 1);
                        let (left, right) = if existing_bit {
                            (None, Some(child))
                        } else {
                            (Some(child), None)
                        };
                        let internal = JmtNode::Internal { left, right };
                        let h = internal.hash();
                        self.nodes.insert(h, internal);
                        h
                    } else {
                        // Different directions → split
                        let (left, right) = if new_bit {
                            (Some(node_hash), Some(leaf_hash))
                        } else {
                            (Some(leaf_hash), Some(node_hash))
                        };
                        let internal = JmtNode::Internal { left, right };
                        let h = internal.hash();
                        self.nodes.insert(h, internal);
                        h
                    }
                }
            }
            JmtNode::Internal { left, right } => {
                let bit = Self::key_bit(key, depth);
                let (new_left, new_right) = if bit {
                    let new_right = self.insert_recursive(
                        right.unwrap_or(PoseidonHash::ZERO),
                        key,
                        leaf_hash,
                        depth + 1,
                    );
                    (left, Some(new_right))
                } else {
                    let new_left = self.insert_recursive(
                        left.unwrap_or(PoseidonHash::ZERO),
                        key,
                        leaf_hash,
                        depth + 1,
                    );
                    (Some(new_left), right)
                };
                let internal = JmtNode::Internal {
                    left: new_left,
                    right: new_right,
                };
                let h = internal.hash();
                self.nodes.insert(h, internal);
                h
            }
            JmtNode::Null => leaf_hash,
        }
    }

    fn get_recursive(
        &self,
        node_hash: PoseidonHash,
        key: &[u8; 32],
        depth: usize,
    ) -> Option<PoseidonHash> {
        if node_hash == PoseidonHash::ZERO || depth >= 256 {
            return None;
        }

        let node = self.nodes.get(&node_hash)?;

        match node {
            JmtNode::Leaf {
                key_hash,
                value_hash,
            } => {
                if key_hash == key {
                    Some(*value_hash)
                } else {
                    None
                }
            }
            JmtNode::Internal { left, right } => {
                let bit = Self::key_bit(key, depth);
                if bit {
                    self.get_recursive(right.unwrap_or(PoseidonHash::ZERO), key, depth + 1)
                } else {
                    self.get_recursive(left.unwrap_or(PoseidonHash::ZERO), key, depth + 1)
                }
            }
            JmtNode::Null => None,
        }
    }

    fn delete_recursive(
        &mut self,
        node_hash: PoseidonHash,
        key: &[u8; 32],
        depth: usize,
    ) -> PoseidonHash {
        if node_hash == PoseidonHash::ZERO || depth >= 256 {
            return PoseidonHash::ZERO;
        }

        let node = match self.nodes.get(&node_hash) {
            Some(n) => n.clone(),
            None => return PoseidonHash::ZERO,
        };

        match node {
            JmtNode::Leaf { key_hash, .. } => {
                if key_hash == *key {
                    PoseidonHash::ZERO
                } else {
                    node_hash // not our key
                }
            }
            JmtNode::Internal { left, right } => {
                let bit = Self::key_bit(key, depth);
                let (new_left, new_right) = if bit {
                    let new_right =
                        self.delete_recursive(right.unwrap_or(PoseidonHash::ZERO), key, depth + 1);
                    (left, Some(new_right))
                } else {
                    let new_left =
                        self.delete_recursive(left.unwrap_or(PoseidonHash::ZERO), key, depth + 1);
                    (Some(new_left), right)
                };

                // Collapse if only one child remains
                let l = new_left.unwrap_or(PoseidonHash::ZERO);
                let r = new_right.unwrap_or(PoseidonHash::ZERO);
                if l == PoseidonHash::ZERO && r == PoseidonHash::ZERO {
                    return PoseidonHash::ZERO;
                }

                // R30-FIX: Collapse internal node with single remaining leaf child.
                // Without this, "insert A, insert B, delete A" produces a different
                // root than "insert B alone" — a consensus-breaking divergence.
                if l == PoseidonHash::ZERO {
                    // Only right child remains — if it's a leaf, collapse
                    if let Some(node) = self.nodes.get(&r) {
                        if matches!(node, JmtNode::Leaf { .. }) {
                            return r;
                        }
                    }
                }
                if r == PoseidonHash::ZERO {
                    // Only left child remains — if it's a leaf, collapse
                    if let Some(node) = self.nodes.get(&l) {
                        if matches!(node, JmtNode::Leaf { .. }) {
                            return l;
                        }
                    }
                }

                let internal = JmtNode::Internal {
                    left: if l == PoseidonHash::ZERO {
                        None
                    } else {
                        Some(l)
                    },
                    right: if r == PoseidonHash::ZERO {
                        None
                    } else {
                        Some(r)
                    },
                };
                let h = internal.hash();
                self.nodes.insert(h, internal);
                h
            }
            JmtNode::Null => PoseidonHash::ZERO,
        }
    }

    /// Garbage-collect stale nodes not reachable from the current root.
    /// Call periodically (e.g., every N blocks) to bound memory usage.
    /// Uses iterative traversal to avoid stack overflow on deep trees.
    pub fn gc_stale_nodes(&mut self) {
        let mut reachable = std::collections::HashSet::new();
        let mut stack = vec![self.root];
        while let Some(hash) = stack.pop() {
            if hash == PoseidonHash::ZERO || !reachable.insert(hash) {
                continue;
            }
            if let Some(node) = self.nodes.get(&hash) {
                match node {
                    JmtNode::Internal { left, right } => {
                        if let Some(l) = left {
                            stack.push(*l);
                        }
                        if let Some(r) = right {
                            stack.push(*r);
                        }
                    }
                    JmtNode::Leaf { .. } | JmtNode::Null => {}
                }
            }
        }
        self.nodes.retain(|hash, _| reachable.contains(hash));
    }

    /// Number of nodes currently in the tree (for monitoring).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Generate a Merkle inclusion proof for a key.
    ///
    /// The proof is a list of sibling hashes from the leaf to the root.
    /// A verifier can recompute the root from the leaf + proof.
    pub fn generate_proof(&self, key: &[u8; 32]) -> StateProof {
        let mut siblings = Vec::new();
        let mut existing_leaf_hash = None;
        self.collect_proof(self.root, key, 0, &mut siblings, &mut existing_leaf_hash);
        let value_hash = self.get(key);

        StateProof {
            key: *key,
            value_hash,
            root: self.root,
            siblings,
            existing_leaf_hash,
        }
    }

    fn collect_proof(
        &self,
        node_hash: PoseidonHash,
        key: &[u8; 32],
        depth: usize,
        siblings: &mut Vec<ProofSibling>,
        existing_leaf_hash: &mut Option<PoseidonHash>,
    ) {
        if node_hash == PoseidonHash::ZERO || depth >= 256 {
            return;
        }

        let node = match self.nodes.get(&node_hash) {
            Some(n) => n,
            None => return,
        };

        match node {
            JmtNode::Leaf { key_hash, .. } => {
                if key_hash != key {
                    // R30-FIX: Record the conflicting leaf hash for exclusion proofs
                    // instead of pushing it as a sibling. The existing leaf IS the
                    // node at this position, not a sibling of it.
                    *existing_leaf_hash = Some(node_hash);
                }
            }
            JmtNode::Internal { left, right } => {
                let bit = Self::key_bit(key, depth);
                if bit {
                    siblings.push(ProofSibling {
                        hash: left.unwrap_or(PoseidonHash::ZERO),
                        is_left: true,
                    });
                    self.collect_proof(
                        right.unwrap_or(PoseidonHash::ZERO),
                        key,
                        depth + 1,
                        siblings,
                        existing_leaf_hash,
                    );
                } else {
                    siblings.push(ProofSibling {
                        hash: right.unwrap_or(PoseidonHash::ZERO),
                        is_left: false,
                    });
                    self.collect_proof(
                        left.unwrap_or(PoseidonHash::ZERO),
                        key,
                        depth + 1,
                        siblings,
                        existing_leaf_hash,
                    );
                }
            }
            JmtNode::Null => {}
        }
    }
}

/// A sibling node in a Merkle proof.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProofSibling {
    /// Hash of the sibling node.
    pub hash: PoseidonHash,
    /// Whether this sibling is on the left (true) or right (false).
    pub is_left: bool,
}

/// A Merkle state proof for a key-value pair in the JMT.
///
/// Allows light clients to verify that a key exists (or doesn't exist)
/// in the tree without downloading the entire state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateProof {
    /// The key being proved.
    pub key: [u8; 32],
    /// The value hash (None if key doesn't exist — exclusion proof).
    pub value_hash: Option<PoseidonHash>,
    /// The state root at the time of proof generation.
    pub root: PoseidonHash,
    /// Sibling hashes from leaf to root.
    pub siblings: Vec<ProofSibling>,
    /// R30-FIX: For exclusion proofs where a different leaf occupies
    /// the path, this carries the conflicting leaf's hash so the
    /// verifier can reconstruct upward from the correct starting point.
    pub existing_leaf_hash: Option<PoseidonHash>,
}

impl StateProof {
    /// Verify this proof against a known state root.
    ///
    /// Returns true if the proof is valid for the given root.
    pub fn verify(&self, expected_root: &PoseidonHash) -> bool {
        if self.root != *expected_root {
            return false;
        }

        if let Some(value_hash) = &self.value_hash {
            let leaf = JmtNode::Leaf {
                key_hash: self.key,
                value_hash: *value_hash,
            };
            let mut current_hash = leaf.hash();

            for sibling in &self.siblings {
                let internal = if sibling.is_left {
                    JmtNode::Internal {
                        left: Some(sibling.hash),
                        right: Some(current_hash),
                    }
                } else {
                    JmtNode::Internal {
                        left: Some(current_hash),
                        right: Some(sibling.hash),
                    }
                };
                current_hash = internal.hash();
            }

            current_hash == *expected_root
        } else {
            // Exclusion proof: verify the Merkle path proves the key's slot is empty
            // or occupied by a different leaf.
            if self.siblings.is_empty() {
                if let Some(leaf_hash) = self.existing_leaf_hash {
                    // Single-node tree with a different key: root IS the leaf
                    return leaf_hash == *expected_root;
                }
                // Empty tree: root must be ZERO (no entries exist)
                return *expected_root == PoseidonHash::ZERO;
            }

            // R30-FIX: Start from the existing leaf hash if a different leaf
            // occupies this path, otherwise start from ZERO (empty position).
            let mut current_hash = self.existing_leaf_hash.unwrap_or(JmtNode::Null.hash());

            for sibling in &self.siblings {
                let internal = if sibling.is_left {
                    JmtNode::Internal {
                        left: Some(sibling.hash),
                        right: Some(current_hash),
                    }
                } else {
                    JmtNode::Internal {
                        left: Some(current_hash),
                        right: Some(sibling.hash),
                    }
                };
                current_hash = internal.hash();
            }

            current_hash == *expected_root
        }
    }

    /// Whether this is an inclusion proof (key exists) or exclusion proof.
    pub fn is_inclusion(&self) -> bool {
        self.value_hash.is_some()
    }
}

impl Default for JellyfishMerkleTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(val: u8) -> [u8; 32] {
        // Hash the value to distribute uniformly
        *pichain_crypto::hash(&[val]).as_bytes()
    }

    #[test]
    fn empty_tree() {
        let tree = JellyfishMerkleTree::new();
        assert_eq!(tree.root_hash(), PoseidonHash::ZERO);
        assert_eq!(tree.version(), 0);
    }

    #[test]
    fn insert_and_get() {
        let mut tree = JellyfishMerkleTree::new();
        let k = key(1);
        tree.put(&k, b"hello PIChain");
        let val = tree.get(&k);
        assert!(val.is_some());
        // Value hash should match direct Poseidon hash
        assert_eq!(val.unwrap(), PoseidonHasher::hash(b"hello PIChain"));
    }

    #[test]
    fn insert_changes_root() {
        let mut tree = JellyfishMerkleTree::new();
        let root0 = tree.root_hash();
        tree.put(&key(1), b"value1");
        let root1 = tree.root_hash();
        assert_ne!(root0, root1);
        tree.put(&key(2), b"value2");
        let root2 = tree.root_hash();
        assert_ne!(root1, root2);
    }

    #[test]
    fn update_existing_key() {
        let mut tree = JellyfishMerkleTree::new();
        let k = key(1);
        tree.put(&k, b"old_value");
        let h1 = tree.get(&k).unwrap();
        tree.put(&k, b"new_value");
        let h2 = tree.get(&k).unwrap();
        assert_ne!(h1, h2);
        assert_eq!(h2, PoseidonHasher::hash(b"new_value"));
    }

    #[test]
    fn delete_key() {
        let mut tree = JellyfishMerkleTree::new();
        let k = key(1);
        tree.put(&k, b"value");
        assert!(tree.get(&k).is_some());
        tree.delete(&k);
        assert!(tree.get(&k).is_none());
    }

    #[test]
    fn multiple_keys() {
        let mut tree = JellyfishMerkleTree::new();
        for i in 0..20u8 {
            tree.put(&key(i), format!("value_{i}").as_bytes());
        }
        for i in 0..20u8 {
            let val = tree.get(&key(i));
            assert!(val.is_some(), "key {i} should exist");
            assert_eq!(
                val.unwrap(),
                PoseidonHasher::hash(format!("value_{i}").as_bytes())
            );
        }
    }

    #[test]
    fn nonexistent_key_returns_none() {
        let mut tree = JellyfishMerkleTree::new();
        tree.put(&key(1), b"value");
        assert!(tree.get(&key(2)).is_none());
    }
}
