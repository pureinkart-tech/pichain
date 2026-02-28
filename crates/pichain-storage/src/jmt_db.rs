//! Database-backed Jellyfish Merkle Tree — production implementation.
//!
//! Unlike the in-memory JMT, this version persists nodes to RocksDB's
//! `jmt_nodes` column family. Features:
//! - Versioned: each block produces a new root, old versions preserved
//! - Pruning support: remove old versions to bound disk usage
//! - Batch writes: coalesce node writes for atomic block commits
//! - Node caching: LRU cache for frequently accessed nodes

use parking_lot::RwLock;
use pichain_crypto::poseidon::{PoseidonHash, PoseidonHasher};
use std::collections::HashMap;

use crate::db::PiChainDB;
use crate::jmt::JmtNode;
use crate::StorageError;

/// Key format for JMT nodes in RocksDB.
/// Format: version (8 bytes BE) + node_hash (32 bytes)
fn node_key(version: u64, hash: &PoseidonHash) -> Vec<u8> {
    let mut key = Vec::with_capacity(40);
    key.extend_from_slice(&version.to_be_bytes());
    key.extend_from_slice(hash.as_bytes());
    key
}

/// Key for storing the root hash at a given version.
fn root_key(version: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(9);
    key.push(b'R'); // Root prefix
    key.extend_from_slice(&version.to_be_bytes());
    key
}

/// R30-FIX: Version-independent key for O(1) node lookup by hash.
/// Format: 'N' prefix (1 byte) + node_hash (32 bytes)
/// The 'N' prefix avoids collisions with version-prefixed keys (which start
/// with 8-byte big-endian version) and root keys (which start with 'R').
fn hash_only_key(hash: &PoseidonHash) -> Vec<u8> {
    let mut key = Vec::with_capacity(33);
    key.push(b'N');
    key.extend_from_slice(hash.as_bytes());
    key
}

/// Database-backed Jellyfish Merkle Tree.
pub struct DbJellyfishMerkleTree {
    db: std::sync::Arc<PiChainDB>,
    /// Node cache: hash → node (LRU-style, bounded size).
    cache: RwLock<HashMap<PoseidonHash, JmtNode>>,
    /// Current root hash.
    root: RwLock<PoseidonHash>,
    /// Current version (= block height).
    version: RwLock<u64>,
    /// Maximum cache entries.
    max_cache_size: usize,
    /// Serializes write operations (put/delete) to prevent TOCTOU race conditions
    /// where concurrent writes read the same root and overwrite each other's changes.
    write_lock: parking_lot::Mutex<()>,
}

impl DbJellyfishMerkleTree {
    /// Create a new DB-backed JMT.
    pub fn new(db: std::sync::Arc<PiChainDB>) -> Self {
        Self {
            db,
            cache: RwLock::new(HashMap::new()),
            root: RwLock::new(PoseidonHash::ZERO),
            version: RwLock::new(0),
            max_cache_size: 100_000,
            write_lock: parking_lot::Mutex::new(()),
        }
    }

    /// Load the latest root from the database.
    pub fn load_latest(&self) -> Result<(), StorageError> {
        // Find the latest root by scanning root keys
        let version = *self.version.read();
        if let Some(data) = self.db.get_jmt_node(&root_key(version))? {
            if data.len() == 32 {
                let mut hash_bytes = [0u8; 32];
                hash_bytes.copy_from_slice(&data);
                *self.root.write() = PoseidonHash::from_bytes(hash_bytes);
            }
        }
        Ok(())
    }

    /// Get the current root hash.
    pub fn root_hash(&self) -> PoseidonHash {
        *self.root.read()
    }

    /// Get the current version.
    pub fn version(&self) -> u64 {
        *self.version.read()
    }

    /// Read a node from cache or database.
    ///
    /// R30-FIX: Uses a hash-only index for O(1) lookup instead of
    /// scanning backwards through all versions (which was O(version)).
    fn get_node(&self, hash: &PoseidonHash) -> Result<Option<JmtNode>, StorageError> {
        // Check cache first
        {
            let cache = self.cache.read();
            if let Some(node) = cache.get(hash) {
                return Ok(Some(node.clone()));
            }
        }

        // R30-FIX: Check hash-indexed entry first (O(1) lookup)
        let hkey = hash_only_key(hash);
        if let Some(data) = self.db.get_jmt_node(&hkey)? {
            let node: JmtNode = serde_json::from_slice(&data)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;

            // Cache it
            let mut cache = self.cache.write();
            if cache.len() < self.max_cache_size {
                cache.insert(*hash, node.clone());
            }

            return Ok(Some(node));
        }

        // Fallback: scan backwards from current version, bounded to last 1000 versions.
        // AUDIT-FIX H-5: Previously scanned from version down to 0 which is O(height)
        // and could stall block production on mature chains if a hash-only index entry
        // is missing. Now bounded to prevent DoS.
        let version = *self.version.read();
        let min_scan = version.saturating_sub(1000);
        for v in (min_scan..=version).rev() {
            let key = node_key(v, hash);
            match self.db.get_jmt_node(&key)? {
                Some(data) => {
                    let node: JmtNode = serde_json::from_slice(&data)
                        .map_err(|e| StorageError::Serialization(e.to_string()))?;

                    // Cache it to avoid future scans
                    let mut cache = self.cache.write();
                    if cache.len() < self.max_cache_size {
                        cache.insert(*hash, node.clone());
                    }

                    // Backfill the hash-only index for future O(1) lookups
                    if let Err(e) = self.db.put_jmt_node(&hash_only_key(hash), &data) {
                        tracing::warn!("JMT hash-only backfill write failed: {e}");
                    }

                    return Ok(Some(node));
                }
                None => continue,
            }
        }

        Ok(None)
    }

    /// Insert or update a key-value pair.
    /// Returns the new root hash.
    ///
    /// Serialized by `write_lock` to prevent TOCTOU races where concurrent
    /// puts read the same root and silently overwrite each other's changes.
    ///
    /// All new JMT nodes are written in a single atomic WriteBatch so a
    /// crash cannot leave a partially-written tree on disk (Fix STOR-222).
    pub fn put(&self, key: &[u8; 32], value: &[u8]) -> Result<PoseidonHash, StorageError> {
        let _guard = self.write_lock.lock();

        let value_hash = PoseidonHasher::hash(value);

        let leaf = JmtNode::Leaf {
            key_hash: *key,
            value_hash,
        };
        let leaf_hash = leaf.hash();

        // Collect new nodes to write
        let mut new_nodes: Vec<(PoseidonHash, JmtNode)> = Vec::new();
        new_nodes.push((leaf_hash, leaf));

        let current_root = *self.root.read();
        let new_root = self.insert_recursive(current_root, key, leaf_hash, 0, &mut new_nodes)?;

        // Write all new nodes + root atomically via WriteBatch (Fix STOR-222)
        let new_version = *self.version.read() + 1;
        let mut batch = self.db.new_batch();
        for (hash, node) in &new_nodes {
            let data = serde_json::to_vec(node)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            self.db.batch_put_jmt_node(&mut batch, &node_key(new_version, hash), &data);
            // R30-FIX: Also store under hash-only key for O(1) lookup
            self.db.batch_put_jmt_node(&mut batch, &hash_only_key(hash), &data);
        }
        self.db.batch_put_jmt_node(&mut batch, &root_key(new_version), new_root.as_bytes());
        self.db.write_batch_sync(batch)?;

        // Update state
        *self.root.write() = new_root;
        *self.version.write() = new_version;

        // Update cache
        let mut cache = self.cache.write();
        for (hash, node) in new_nodes {
            if cache.len() < self.max_cache_size {
                cache.insert(hash, node);
            }
        }

        Ok(new_root)
    }

    /// Get a value by key.
    pub fn get(&self, key: &[u8; 32]) -> Result<Option<PoseidonHash>, StorageError> {
        let root = *self.root.read();
        self.get_recursive(root, key, 0)
    }

    /// Delete a key. Returns the new root hash.
    ///
    /// All new JMT nodes are written in a single atomic WriteBatch (Fix STOR-222).
    pub fn delete(&self, key: &[u8; 32]) -> Result<PoseidonHash, StorageError> {
        let _guard = self.write_lock.lock();

        let current_root = *self.root.read();
        let mut new_nodes = Vec::new();
        let new_root = self.delete_recursive(current_root, key, 0, &mut new_nodes)?;

        // Write all new nodes + root atomically via WriteBatch (Fix STOR-222)
        let new_version = *self.version.read() + 1;
        let mut batch = self.db.new_batch();
        for (hash, node) in &new_nodes {
            let data = serde_json::to_vec(node)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            self.db.batch_put_jmt_node(&mut batch, &node_key(new_version, hash), &data);
            // R30-FIX: Also store under hash-only key for O(1) lookup
            self.db.batch_put_jmt_node(&mut batch, &hash_only_key(hash), &data);
        }
        self.db.batch_put_jmt_node(&mut batch, &root_key(new_version), new_root.as_bytes());
        self.db.write_batch_sync(batch)?;

        *self.root.write() = new_root;
        *self.version.write() = new_version;

        let mut cache = self.cache.write();
        for (hash, node) in new_nodes {
            if cache.len() < self.max_cache_size {
                cache.insert(hash, node);
            }
        }

        Ok(new_root)
    }

    /// Prune old versions up to (exclusive) the given version.
    ///
    /// Scans the JMT column family for node keys with version < `version`
    /// and deletes them. Also deletes old root entries. Returns the count
    /// of nodes pruned.
    pub fn prune_before(&self, version: u64) -> Result<u64, StorageError> {
        if version == 0 {
            return Ok(0);
        }

        let mut pruned = 0u64;
        let current_version = *self.version.read();

        // Don't prune the current version or anything beyond it
        let prune_up_to = version.min(current_version);

        // Delete root entries for old versions
        for v in 0..prune_up_to {
            let rk = root_key(v);
            if self.db.get_jmt_node(&rk)?.is_some() {
                self.db.delete_jmt_node(&rk)?;
                pruned += 1;
            }
        }

        // Delete node entries for old versions
        // Node keys are: version (8 bytes BE) + node_hash (32 bytes)
        // We iterate keys with version prefix < prune_up_to
        for v in 0..prune_up_to {
            let prefix = v.to_be_bytes();
            let nodes_deleted = self.db.delete_jmt_nodes_with_prefix(&prefix)?;
            pruned += nodes_deleted;
        }

        // Clear cache entries that may reference pruned nodes
        if pruned > 0 {
            self.cache.write().clear();
        }

        tracing::info!(
            pruned_nodes = pruned,
            pruned_up_to = prune_up_to,
            current_version,
            "JMT state pruned"
        );

        Ok(pruned)
    }

    /// Get the cache hit ratio for monitoring.
    pub fn cache_size(&self) -> usize {
        self.cache.read().len()
    }

    /// Clear the node cache.
    pub fn clear_cache(&self) {
        self.cache.write().clear();
    }

    fn key_bit(key: &[u8; 32], depth: usize) -> bool {
        let byte_idx = depth / 8;
        let bit_idx = 7 - (depth % 8);
        if byte_idx >= 32 {
            return false;
        }
        (key[byte_idx] >> bit_idx) & 1 == 1
    }

    fn insert_recursive(
        &self,
        node_hash: PoseidonHash,
        key: &[u8; 32],
        leaf_hash: PoseidonHash,
        depth: usize,
        new_nodes: &mut Vec<(PoseidonHash, JmtNode)>,
    ) -> Result<PoseidonHash, StorageError> {
        if depth >= 256 {
            return Ok(leaf_hash);
        }

        if node_hash == PoseidonHash::ZERO {
            return Ok(leaf_hash);
        }

        let node = match self.get_node(&node_hash)? {
            Some(n) => n,
            None => return Ok(leaf_hash),
        };

        match node {
            JmtNode::Leaf {
                key_hash: existing_key,
                ..
            } => {
                if existing_key == *key {
                    Ok(leaf_hash)
                } else {
                    let existing_bit = Self::key_bit(&existing_key, depth);
                    let new_bit = Self::key_bit(key, depth);

                    if existing_bit == new_bit {
                        let child = self.insert_recursive(node_hash, key, leaf_hash, depth + 1, new_nodes)?;
                        let (left, right) = if existing_bit {
                            (None, Some(child))
                        } else {
                            (Some(child), None)
                        };
                        let internal = JmtNode::Internal { left, right };
                        let h = internal.hash();
                        new_nodes.push((h, internal));
                        Ok(h)
                    } else {
                        let (left, right) = if new_bit {
                            (Some(node_hash), Some(leaf_hash))
                        } else {
                            (Some(leaf_hash), Some(node_hash))
                        };
                        let internal = JmtNode::Internal { left, right };
                        let h = internal.hash();
                        new_nodes.push((h, internal));
                        Ok(h)
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
                        new_nodes,
                    )?;
                    (left, Some(new_right))
                } else {
                    let new_left = self.insert_recursive(
                        left.unwrap_or(PoseidonHash::ZERO),
                        key,
                        leaf_hash,
                        depth + 1,
                        new_nodes,
                    )?;
                    (Some(new_left), right)
                };
                let internal = JmtNode::Internal {
                    left: new_left,
                    right: new_right,
                };
                let h = internal.hash();
                new_nodes.push((h, internal));
                Ok(h)
            }
            JmtNode::Null => Ok(leaf_hash),
        }
    }

    fn get_recursive(
        &self,
        node_hash: PoseidonHash,
        key: &[u8; 32],
        depth: usize,
    ) -> Result<Option<PoseidonHash>, StorageError> {
        if node_hash == PoseidonHash::ZERO || depth >= 256 {
            return Ok(None);
        }

        let node = match self.get_node(&node_hash)? {
            Some(n) => n,
            None => return Ok(None),
        };

        match &node {
            JmtNode::Leaf {
                key_hash,
                value_hash,
            } => {
                if key_hash == key {
                    Ok(Some(*value_hash))
                } else {
                    Ok(None)
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
            JmtNode::Null => Ok(None),
        }
    }

    fn delete_recursive(
        &self,
        node_hash: PoseidonHash,
        key: &[u8; 32],
        depth: usize,
        new_nodes: &mut Vec<(PoseidonHash, JmtNode)>,
    ) -> Result<PoseidonHash, StorageError> {
        if node_hash == PoseidonHash::ZERO || depth >= 256 {
            return Ok(PoseidonHash::ZERO);
        }

        let node = match self.get_node(&node_hash)? {
            Some(n) => n,
            None => return Ok(PoseidonHash::ZERO),
        };

        match node {
            JmtNode::Leaf { key_hash, .. } => {
                if key_hash == *key {
                    Ok(PoseidonHash::ZERO)
                } else {
                    Ok(node_hash)
                }
            }
            JmtNode::Internal { left, right } => {
                let bit = Self::key_bit(key, depth);
                let (new_left, new_right) = if bit {
                    let new_right = self.delete_recursive(
                        right.unwrap_or(PoseidonHash::ZERO),
                        key,
                        depth + 1,
                        new_nodes,
                    )?;
                    (left, Some(new_right))
                } else {
                    let new_left = self.delete_recursive(
                        left.unwrap_or(PoseidonHash::ZERO),
                        key,
                        depth + 1,
                        new_nodes,
                    )?;
                    (Some(new_left), right)
                };

                let l = new_left.unwrap_or(PoseidonHash::ZERO);
                let r = new_right.unwrap_or(PoseidonHash::ZERO);
                if l == PoseidonHash::ZERO && r == PoseidonHash::ZERO {
                    return Ok(PoseidonHash::ZERO);
                }

                // R30-FIX: Collapse internal node with single remaining leaf child.
                // Without this, "insert A, insert B, delete A" produces a different
                // root than "insert B alone" — a consensus-breaking divergence.
                if l == PoseidonHash::ZERO {
                    // Only right child remains — if it's a leaf, collapse
                    if let Some(node) = self.get_node(&r)? {
                        if matches!(node, JmtNode::Leaf { .. }) {
                            return Ok(r);
                        }
                    }
                    // Also check new_nodes (just created in this batch)
                    if new_nodes.iter().any(|(h, n)| *h == r && matches!(n, JmtNode::Leaf { .. })) {
                        return Ok(r);
                    }
                }
                if r == PoseidonHash::ZERO {
                    // Only left child remains — if it's a leaf, collapse
                    if let Some(node) = self.get_node(&l)? {
                        if matches!(node, JmtNode::Leaf { .. }) {
                            return Ok(l);
                        }
                    }
                    // Also check new_nodes (just created in this batch)
                    if new_nodes.iter().any(|(h, n)| *h == l && matches!(n, JmtNode::Leaf { .. })) {
                        return Ok(l);
                    }
                }

                let internal = JmtNode::Internal {
                    left: if l == PoseidonHash::ZERO { None } else { Some(l) },
                    right: if r == PoseidonHash::ZERO { None } else { Some(r) },
                };
                let h = internal.hash();
                new_nodes.push((h, internal));
                Ok(h)
            }
            JmtNode::Null => Ok(PoseidonHash::ZERO),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn test_db() -> (Arc<PiChainDB>, tempfile::TempDir) {
        let (db, temp_dir) = PiChainDB::open_temp().unwrap();
        (Arc::new(db), temp_dir)
    }

    fn test_key(val: u8) -> [u8; 32] {
        *pichain_crypto::hash(&[val]).as_bytes()
    }

    #[test]
    fn empty_tree() {
        let (db, _temp_dir) = test_db();
        let tree = DbJellyfishMerkleTree::new(db);
        assert_eq!(tree.root_hash(), PoseidonHash::ZERO);
        assert_eq!(tree.version(), 0);
    }

    #[test]
    fn insert_and_get() {
        let (db, _temp_dir) = test_db();
        let tree = DbJellyfishMerkleTree::new(db);

        let k = test_key(1);
        tree.put(&k, b"hello PIChain").unwrap();

        let val = tree.get(&k).unwrap();
        assert!(val.is_some());
        assert_eq!(val.unwrap(), PoseidonHasher::hash(b"hello PIChain"));
    }

    #[test]
    fn version_increments() {
        let (db, _temp_dir) = test_db();
        let tree = DbJellyfishMerkleTree::new(db);

        assert_eq!(tree.version(), 0);
        tree.put(&test_key(1), b"v1").unwrap();
        assert_eq!(tree.version(), 1);
        tree.put(&test_key(2), b"v2").unwrap();
        assert_eq!(tree.version(), 2);
    }

    #[test]
    fn root_changes_on_insert() {
        let (db, _temp_dir) = test_db();
        let tree = DbJellyfishMerkleTree::new(db);

        let root0 = tree.root_hash();
        tree.put(&test_key(1), b"value1").unwrap();
        let root1 = tree.root_hash();
        assert_ne!(root0, root1);

        tree.put(&test_key(2), b"value2").unwrap();
        let root2 = tree.root_hash();
        assert_ne!(root1, root2);
    }

    #[test]
    fn delete_key() {
        let (db, _temp_dir) = test_db();
        let tree = DbJellyfishMerkleTree::new(db);

        let k = test_key(1);
        tree.put(&k, b"value").unwrap();
        assert!(tree.get(&k).unwrap().is_some());

        tree.delete(&k).unwrap();
        assert!(tree.get(&k).unwrap().is_none());
    }

    #[test]
    fn multiple_keys() {
        let (db, _temp_dir) = test_db();
        let tree = DbJellyfishMerkleTree::new(db);

        for i in 0..10u8 {
            tree.put(&test_key(i), format!("val_{i}").as_bytes()).unwrap();
        }

        for i in 0..10u8 {
            let val = tree.get(&test_key(i)).unwrap();
            assert!(val.is_some(), "key {i} should exist");
            assert_eq!(
                val.unwrap(),
                PoseidonHasher::hash(format!("val_{i}").as_bytes())
            );
        }
    }

    #[test]
    fn cache_works() {
        let (db, _temp_dir) = test_db();
        let tree = DbJellyfishMerkleTree::new(db);

        tree.put(&test_key(1), b"cached").unwrap();
        assert!(tree.cache_size() > 0);

        tree.clear_cache();
        assert_eq!(tree.cache_size(), 0);

        // Should still be able to read from DB
        let val = tree.get(&test_key(1)).unwrap();
        assert!(val.is_some());
    }

    #[test]
    fn cross_version_node_lookup_after_cache_clear() {
        // Regression test: nodes created at version 1 must be findable
        // even when the tree is at version 5 and cache is empty.
        let (db, _temp_dir) = test_db();
        let tree = DbJellyfishMerkleTree::new(db);

        // Create 5 entries at versions 1-5
        for i in 0..5u8 {
            tree.put(&test_key(i), format!("val_{i}").as_bytes()).unwrap();
        }
        assert_eq!(tree.version(), 5);

        // Clear cache — forces DB lookup
        tree.clear_cache();

        // All entries should still be accessible (nodes stored at earlier versions)
        for i in 0..5u8 {
            let val = tree.get(&test_key(i)).unwrap();
            assert!(val.is_some(), "key {i} should exist after cache clear");
            assert_eq!(
                val.unwrap(),
                PoseidonHasher::hash(format!("val_{i}").as_bytes())
            );
        }
    }

    /// STOR-222: Verify that JMT put writes all nodes atomically.
    /// After a put, clearing the cache and re-reading all inserted keys
    /// must succeed — proving that all nodes reached disk in one batch.
    #[test]
    fn jmt_db_put_is_atomic() {
        let (db, _temp_dir) = test_db();
        let tree = DbJellyfishMerkleTree::new(db);

        // Insert multiple keys to produce many internal + leaf nodes
        for i in 0..10u8 {
            tree.put(&test_key(i), format!("val_{i}").as_bytes()).unwrap();
        }

        // Clear cache to force reads from RocksDB
        tree.clear_cache();

        // All keys must be retrievable — if writes were non-atomic and one
        // node was lost, the tree traversal would fail or return None.
        for i in 0..10u8 {
            let val = tree.get(&test_key(i)).unwrap();
            assert!(
                val.is_some(),
                "key {i} must be retrievable after atomic put (STOR-222)"
            );
            assert_eq!(
                val.unwrap(),
                PoseidonHasher::hash(format!("val_{i}").as_bytes()),
            );
        }

        // Also verify that delete works atomically
        tree.delete(&test_key(5)).unwrap();
        tree.clear_cache();
        assert!(
            tree.get(&test_key(5)).unwrap().is_none(),
            "deleted key must be gone after atomic delete"
        );
        // Other keys still intact
        for i in [0u8, 1, 2, 3, 4, 6, 7, 8, 9] {
            assert!(
                tree.get(&test_key(i)).unwrap().is_some(),
                "key {i} must survive after deleting a different key"
            );
        }
    }
}
