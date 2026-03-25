//! RocksDB wrapper with column families optimized for blockchain workloads.

use rocksdb::{
    BoundColumnFamily, ColumnFamilyDescriptor, DBWithThreadMode, MultiThreaded, Options,
    WriteBatch,
};
use std::path::Path;
use std::sync::Arc;

use crate::StorageError;

/// Column family names for different data categories.
pub const CF_STATE: &str = "state";
pub const CF_BLOCKS: &str = "blocks";
pub const CF_TRANSACTIONS: &str = "transactions";
pub const CF_RECEIPTS: &str = "receipts";
pub const CF_METADATA: &str = "metadata";
pub const CF_OBJECTS: &str = "objects";
pub const CF_JMT_NODES: &str = "jmt_nodes";
pub const CF_TX_HISTORY: &str = "tx_history";
pub const CF_EVENT_INDEX: &str = "event_index";
pub const CF_EVM_STATE: &str = "evm_state";

const ALL_CFS: &[&str] = &[
    CF_STATE,
    CF_BLOCKS,
    CF_TRANSACTIONS,
    CF_RECEIPTS,
    CF_METADATA,
    CF_OBJECTS,
    CF_JMT_NODES,
    CF_TX_HISTORY,
    CF_EVENT_INDEX,
    CF_EVM_STATE,
];

type DB = DBWithThreadMode<MultiThreaded>;
type ScanResult = Result<Vec<(Vec<u8>, Vec<u8>)>, StorageError>;

/// PIChain database backed by RocksDB.
pub struct PiChainDB {
    db: Arc<DB>,
    /// Serializes block writes to prevent TOCTOU race conditions.
    /// Block writes are infrequent (one per 314ms) so contention is negligible.
    block_write_lock: std::sync::Mutex<()>,
}

impl PiChainDB {
    /// Attempt to repair a corrupted database at the given path.
    pub fn repair<P: AsRef<Path>>(path: P) -> Result<(), StorageError> {
        let mut db_opts = Options::default();
        db_opts.create_if_missing(false);
        DB::repair(&db_opts, path.as_ref())?;
        Ok(())
    }

    /// Open or create the database at the given path.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);
        db_opts.set_write_buffer_size(256 * 1024 * 1024); // 256MB
        db_opts.set_max_write_buffer_number(6);
        db_opts.set_target_file_size_base(64 * 1024 * 1024); // 64MB
        db_opts.set_max_background_jobs(8);
        db_opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
        db_opts.set_bottommost_compression_type(rocksdb::DBCompressionType::Zstd);
        db_opts.increase_parallelism(num_cpus::get() as i32);

        // WAL configuration for crash safety
        db_opts.set_wal_ttl_seconds(3600); // Keep WAL files for 1 hour
        db_opts.set_wal_size_limit_mb(1024); // Cap WAL at 1GB

        // Single shared 512MB block cache across all column families
        let shared_cache = rocksdb::Cache::new_lru_cache(512 * 1024 * 1024);

        let cf_descriptors: Vec<ColumnFamilyDescriptor> = ALL_CFS
            .iter()
            .map(|name| {
                let mut cf_opts = Options::default();
                cf_opts.set_compression_type(rocksdb::DBCompressionType::Lz4);

                // Bloom filters per CF + shared block cache
                let mut block_opts = rocksdb::BlockBasedOptions::default();
                block_opts.set_bloom_filter(10.0, false);
                block_opts.set_block_cache(&shared_cache);
                cf_opts.set_block_based_table_factory(&block_opts);

                ColumnFamilyDescriptor::new(*name, cf_opts)
            })
            .collect();

        let db = DB::open_cf_descriptors(&db_opts, path, cf_descriptors)?;
        Ok(Self {
            db: Arc::new(db),
            block_write_lock: std::sync::Mutex::new(()),
        })
    }

    /// Open a temporary database for testing.
    /// Returns the TempDir alongside the DB so the directory stays alive
    /// for the duration of the test.
    #[cfg(test)]
    pub fn open_temp() -> Result<(Self, tempfile::TempDir), StorageError> {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let db = Self::open(temp_dir.path())?;
        Ok((db, temp_dir))
    }

    /// Get a column family handle. Column families are created during `open()` with
    /// `create_missing_column_families(true)`, so this only fails if the DB is corrupted.
    pub(crate) fn cf(&self, name: &str) -> Arc<BoundColumnFamily<'_>> {
        self.db.cf_handle(name)
            .unwrap_or_else(|| panic!("column family '{name}' missing — database may be corrupted"))
    }

    // --- State operations ---

    pub fn get_state(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.db.get_cf(&self.cf(CF_STATE), key)?)
    }

    pub fn put_state(&self, key: &[u8], value: &[u8]) -> Result<(), StorageError> {
        self.db.put_cf(&self.cf(CF_STATE), key, value)?;
        Ok(())
    }

    pub fn delete_state(&self, key: &[u8]) -> Result<(), StorageError> {
        self.db.delete_cf(&self.cf(CF_STATE), key)?;
        Ok(())
    }

    // --- Block operations ---

    /// AUDIT-FIX H-6: Check for existing block before writing to prevent
    /// silent overwrites that could corrupt canonical chain state.
    ///
    /// SECURITY: Uses a mutex to serialize block writes, preventing the TOCTOU
    /// race where two threads both pass the get_cf check and one overwrites the other.
    pub fn put_block(&self, height: u64, data: &[u8]) -> Result<(), StorageError> {
        let _guard = self.block_write_lock.lock()
            .map_err(|e| StorageError::IntegrityViolation(format!("block write lock poisoned: {e}")))?;
        let key = height.to_be_bytes();
        if self.db.get_cf(&self.cf(CF_BLOCKS), key)?.is_some() {
            return Err(StorageError::IntegrityViolation(format!(
                "block already exists at height {height} — refusing to overwrite"
            )));
        }
        self.db.put_cf(&self.cf(CF_BLOCKS), key, data)?;
        Ok(())
    }

    pub fn get_block(&self, height: u64) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.db.get_cf(&self.cf(CF_BLOCKS), height.to_be_bytes())?)
    }

    // --- Transaction operations ---

    pub fn put_transaction(&self, tx_hash: &[u8; 32], data: &[u8]) -> Result<(), StorageError> {
        self.db
            .put_cf(&self.cf(CF_TRANSACTIONS), tx_hash, data)?;
        Ok(())
    }

    pub fn get_transaction(&self, tx_hash: &[u8; 32]) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.db.get_cf(&self.cf(CF_TRANSACTIONS), tx_hash)?)
    }

    // --- Receipt operations ---

    pub fn put_receipt(&self, tx_hash: &[u8; 32], data: &[u8]) -> Result<(), StorageError> {
        self.db.put_cf(&self.cf(CF_RECEIPTS), tx_hash, data)?;
        Ok(())
    }

    pub fn get_receipt(&self, tx_hash: &[u8; 32]) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.db.get_cf(&self.cf(CF_RECEIPTS), tx_hash)?)
    }

    // --- Object operations ---

    pub fn put_object(&self, id: &[u8; 32], data: &[u8]) -> Result<(), StorageError> {
        self.db.put_cf(&self.cf(CF_OBJECTS), id, data)?;
        Ok(())
    }

    pub fn get_object(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.db.get_cf(&self.cf(CF_OBJECTS), id)?)
    }

    pub fn delete_object(&self, id: &[u8; 32]) -> Result<(), StorageError> {
        self.db.delete_cf(&self.cf(CF_OBJECTS), id)?;
        Ok(())
    }

    /// Add an object write to a batch (for atomic commits). (H6)
    pub fn batch_put_object(&self, batch: &mut WriteBatch, id: &[u8; 32], data: &[u8]) {
        batch.put_cf(&self.cf(CF_OBJECTS), id, data);
    }

    // --- JMT node operations ---

    pub fn put_jmt_node(&self, key: &[u8], data: &[u8]) -> Result<(), StorageError> {
        self.db.put_cf(&self.cf(CF_JMT_NODES), key, data)?;
        Ok(())
    }

    pub fn get_jmt_node(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.db.get_cf(&self.cf(CF_JMT_NODES), key)?)
    }

    pub fn delete_jmt_node(&self, key: &[u8]) -> Result<(), StorageError> {
        self.db.delete_cf(&self.cf(CF_JMT_NODES), key)?;
        Ok(())
    }

    /// Add a JMT node write to a batch (for atomic commits). (Fix STOR-222)
    pub fn batch_put_jmt_node(&self, batch: &mut WriteBatch, key: &[u8], data: &[u8]) {
        batch.put_cf(&self.cf(CF_JMT_NODES), key, data);
    }

    /// Add a JMT node deletion to a batch (for atomic commits). (Fix STOR-222)
    pub fn batch_delete_jmt_node(&self, batch: &mut WriteBatch, key: &[u8]) {
        batch.delete_cf(&self.cf(CF_JMT_NODES), key);
    }

    /// Delete all JMT nodes whose key starts with the given prefix.
    /// Returns the count of deleted entries.
    pub fn delete_jmt_nodes_with_prefix(&self, prefix: &[u8]) -> Result<u64, StorageError> {
        use rocksdb::IteratorMode;
        let cf = self.cf(CF_JMT_NODES);
        let mut count = 0u64;
        let mut batch = WriteBatch::default();

        let iter = self.db.iterator_cf(&cf, IteratorMode::From(prefix, rocksdb::Direction::Forward));
        for item in iter {
            let (key, _) = item?;
            if !key.starts_with(prefix) {
                break;
            }
            batch.delete_cf(&cf, &key);
            count += 1;

            // Flush in batches of 10000 to avoid huge memory use
            if count % 10_000 == 0 {
                self.db.write(std::mem::take(&mut batch))?;
            }
        }

        if !batch.is_empty() {
            self.db.write(batch)?;
        }

        Ok(count)
    }

    // --- Metadata operations ---

    pub fn put_metadata(&self, key: &[u8], value: &[u8]) -> Result<(), StorageError> {
        self.db.put_cf(&self.cf(CF_METADATA), key, value)?;
        Ok(())
    }

    pub fn get_metadata(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.db.get_cf(&self.cf(CF_METADATA), key)?)
    }

    // --- Batch operations ---

    pub fn write_batch(&self, batch: WriteBatch) -> Result<(), StorageError> {
        self.db.write(batch)?;
        Ok(())
    }

    /// Write a batch with WAL sync — ensures data survives power failures.
    /// Use for critical writes like block persistence.
    pub fn write_batch_sync(&self, batch: WriteBatch) -> Result<(), StorageError> {
        let mut write_opts = rocksdb::WriteOptions::default();
        write_opts.set_sync(true);
        self.db.write_opt(batch, &write_opts)?;
        Ok(())
    }

    pub fn new_batch(&self) -> WriteBatch {
        WriteBatch::default()
    }

    /// Add a block write to a batch (for atomic commits).
    /// AUDIT-FIX H-6: Check for existing block to prevent overwrites.
    pub fn batch_put_block(&self, batch: &mut WriteBatch, height: u64, data: &[u8]) -> Result<(), StorageError> {
        let key = height.to_be_bytes();
        if self.db.get_cf(&self.cf(CF_BLOCKS), key)?.is_some() {
            return Err(StorageError::IntegrityViolation(format!(
                "block already exists at height {height} — refusing to overwrite"
            )));
        }
        batch.put_cf(&self.cf(CF_BLOCKS), key, data);
        Ok(())
    }

    /// Add a transaction write to a batch (for atomic commits).
    pub fn batch_put_transaction(&self, batch: &mut WriteBatch, tx_hash: &[u8; 32], data: &[u8]) {
        batch.put_cf(&self.cf(CF_TRANSACTIONS), tx_hash, data);
    }

    /// Add a receipt write to a batch (for atomic commits).
    pub fn batch_put_receipt(&self, batch: &mut WriteBatch, tx_hash: &[u8; 32], data: &[u8]) {
        batch.put_cf(&self.cf(CF_RECEIPTS), tx_hash, data);
    }

    /// Add a state write to a batch (for atomic commits).
    pub fn batch_put_state(&self, batch: &mut WriteBatch, key: &[u8], value: &[u8]) {
        batch.put_cf(&self.cf(CF_STATE), key, value);
    }

    /// Add a metadata write to a batch (for atomic commits).
    pub fn batch_put_metadata(&self, batch: &mut WriteBatch, key: &[u8], value: &[u8]) {
        batch.put_cf(&self.cf(CF_METADATA), key, value);
    }

    /// Store a tx_hash → block_height mapping (secondary index).
    pub fn put_tx_block_height(&self, tx_hash: &[u8; 32], height: u64) -> Result<(), StorageError> {
        let mut key = Vec::with_capacity(33);
        key.push(b't'); // prefix for tx→height namespace
        key.extend_from_slice(tx_hash);
        self.db.put_cf(&self.cf(CF_METADATA), &key, height.to_be_bytes())?;
        Ok(())
    }

    /// Look up which block height contains a given transaction.
    pub fn get_tx_block_height(&self, tx_hash: &[u8; 32]) -> Result<Option<u64>, StorageError> {
        let mut key = Vec::with_capacity(33);
        key.push(b't');
        key.extend_from_slice(tx_hash);
        match self.db.get_cf(&self.cf(CF_METADATA), &key)? {
            Some(bytes) => {
                let arr: [u8; 8] = bytes.try_into()
                    .map_err(|_| StorageError::Serialization("tx_block_height is not 8 bytes".into()))?;
                Ok(Some(u64::from_be_bytes(arr)))
            }
            _ => Ok(None),
        }
    }

    /// Add a tx→height mapping to a batch (for atomic commits).
    pub fn batch_put_tx_height(&self, batch: &mut WriteBatch, tx_hash: &[u8; 32], height: u64) {
        let mut key = Vec::with_capacity(33);
        key.push(b't');
        key.extend_from_slice(tx_hash);
        batch.put_cf(&self.cf(CF_METADATA), &key, height.to_be_bytes());
    }

    /// Add a state deletion to a batch (for atomic commits).
    pub fn batch_delete_state(&self, batch: &mut WriteBatch, key: &[u8]) {
        batch.delete_cf(&self.cf(CF_STATE), key);
    }

    /// Scan all state entries with a given prefix.
    /// Returns (key_suffix, value) pairs where key_suffix has the prefix stripped.
    pub fn scan_state_prefix(&self, prefix: &[u8]) -> ScanResult {
        use rocksdb::IteratorMode;
        let cf = self.cf(CF_STATE);
        let mut result = Vec::new();
        let iter = self.db.iterator_cf(&cf, IteratorMode::From(prefix, rocksdb::Direction::Forward));
        for item in iter {
            let (key, value) = item?;
            if !key.starts_with(prefix) {
                break;
            }
            result.push((key[prefix.len()..].to_vec(), value.to_vec()));
        }
        Ok(result)
    }

    /// Scan all objects in the objects column family.
    /// Returns (key, value) pairs where key is the 32-byte object ID.
    pub fn scan_all_objects(&self) -> ScanResult {
        use rocksdb::IteratorMode;
        let cf = self.cf(CF_OBJECTS);
        let mut result = Vec::new();
        let iter = self.db.iterator_cf(&cf, IteratorMode::Start);
        for item in iter {
            let (key, value) = item?;
            result.push((key.to_vec(), value.to_vec()));
        }
        Ok(result)
    }

    /// Scan all metadata entries with a given prefix.
    /// Returns (key_suffix, value) pairs where key_suffix has the prefix stripped.
    pub fn scan_metadata_prefix(&self, prefix: &[u8]) -> ScanResult {
        use rocksdb::IteratorMode;
        let cf = self.cf(CF_METADATA);
        let mut result = Vec::new();
        let iter = self.db.iterator_cf(&cf, IteratorMode::From(prefix, rocksdb::Direction::Forward));
        for item in iter {
            let (key, value) = item?;
            if !key.starts_with(prefix) {
                break;
            }
            result.push((key[prefix.len()..].to_vec(), value.to_vec()));
        }
        Ok(result)
    }

    /// Get the inner Arc<DB> for advanced operations.
    pub fn inner(&self) -> &Arc<DB> {
        &self.db
    }

    // --- Transaction History Index (CF_TX_HISTORY) ---
    // Key format: addr(20) | height_be(8) | tx_idx_be(2) → tx_hash(32)
    // Range scan newest-first using reverse iteration.

    /// Index a transaction for an address (sender or recipient).
    pub fn batch_index_tx_for_address(
        &self,
        batch: &mut WriteBatch,
        address: &[u8; 20],
        height: u64,
        tx_idx: u16,
        tx_hash: &[u8; 32],
    ) {
        let mut key = Vec::with_capacity(30);
        key.extend_from_slice(address);
        key.extend_from_slice(&height.to_be_bytes());
        key.extend_from_slice(&tx_idx.to_be_bytes());
        batch.put_cf(&self.cf(CF_TX_HISTORY), key, tx_hash);
    }

    /// Get transaction hashes for an address, newest-first.
    /// `before_height`: if Some, start scanning below this height (for pagination).
    /// Returns up to `limit` tx hashes with their (height, tx_idx).
    pub fn get_address_transactions(
        &self,
        address: &[u8; 20],
        before_height: Option<u64>,
        limit: usize,
    ) -> Result<Vec<([u8; 32], u64, u16)>, StorageError> {
        use rocksdb::{Direction, IteratorMode};
        let cf = self.cf(CF_TX_HISTORY);

        // Build the start key for reverse iteration
        let start_height = before_height.unwrap_or(u64::MAX);
        let mut seek_key = Vec::with_capacity(30);
        seek_key.extend_from_slice(address);
        seek_key.extend_from_slice(&start_height.to_be_bytes());
        seek_key.extend_from_slice(&u16::MAX.to_be_bytes());

        let mut results = Vec::with_capacity(limit);
        let iter = self.db.iterator_cf(&cf, IteratorMode::From(&seek_key, Direction::Reverse));
        for item in iter {
            let (key, value) = item?;
            if key.len() != 30 || !key.starts_with(address) {
                break;
            }
            let height_bytes: [u8; 8] = key[20..28].try_into().unwrap();
            let idx_bytes: [u8; 2] = key[28..30].try_into().unwrap();
            let height = u64::from_be_bytes(height_bytes);
            let tx_idx = u16::from_be_bytes(idx_bytes);
            if value.len() != 32 {
                tracing::warn!(height, tx_idx, value_len = value.len(), "corrupted tx index entry, skipping");
                continue;
            }
            let mut tx_hash = [0u8; 32];
            tx_hash.copy_from_slice(&value);
            results.push((tx_hash, height, tx_idx));
            if results.len() >= limit {
                break;
            }
        }
        Ok(results)
    }

    // --- Event Index (CF_EVENT_INDEX) ---
    // Topic index: "t:" | topic(32) | height_be(8) | idx_be(2) → tx_hash(32)
    // Address index: "a:" | addr(20) | height_be(8) | idx_be(2) → tx_hash(32)

    /// Index an event by topic.
    pub fn batch_index_event_topic(
        &self,
        batch: &mut WriteBatch,
        topic: &[u8; 32],
        height: u64,
        idx: u16,
        tx_hash: &[u8; 32],
    ) {
        let mut key = Vec::with_capacity(44);
        key.extend_from_slice(b"t:");
        key.extend_from_slice(topic);
        key.extend_from_slice(&height.to_be_bytes());
        key.extend_from_slice(&idx.to_be_bytes());
        batch.put_cf(&self.cf(CF_EVENT_INDEX), key, tx_hash);
    }

    /// Index an event by address.
    pub fn batch_index_event_address(
        &self,
        batch: &mut WriteBatch,
        address: &[u8; 20],
        height: u64,
        idx: u16,
        tx_hash: &[u8; 32],
    ) {
        let mut key = Vec::with_capacity(32);
        key.extend_from_slice(b"a:");
        key.extend_from_slice(address);
        key.extend_from_slice(&height.to_be_bytes());
        key.extend_from_slice(&idx.to_be_bytes());
        batch.put_cf(&self.cf(CF_EVENT_INDEX), key, tx_hash);
    }

    /// Query events by topic, newest-first.
    pub fn query_events_by_topic(
        &self,
        topic: &[u8; 32],
        limit: usize,
    ) -> Result<Vec<([u8; 32], u64, u16)>, StorageError> {
        use rocksdb::{Direction, IteratorMode};
        let cf = self.cf(CF_EVENT_INDEX);

        let mut seek_key = Vec::with_capacity(44);
        seek_key.extend_from_slice(b"t:");
        seek_key.extend_from_slice(topic);
        seek_key.extend_from_slice(&u64::MAX.to_be_bytes());
        seek_key.extend_from_slice(&u16::MAX.to_be_bytes());

        let prefix_len = 34; // "t:" + 32-byte topic
        let mut results = Vec::with_capacity(limit);
        let iter = self.db.iterator_cf(&cf, IteratorMode::From(&seek_key, Direction::Reverse));
        for item in iter {
            let (key, value) = item?;
            if key.len() != 44 || !key[..prefix_len].starts_with(&seek_key[..prefix_len]) {
                break;
            }
            let height = u64::from_be_bytes(key[prefix_len..prefix_len+8].try_into().unwrap());
            let idx = u16::from_be_bytes(key[prefix_len+8..prefix_len+10].try_into().unwrap());
            if value.len() != 32 {
                tracing::warn!(height, idx, value_len = value.len(), "corrupted topic event index, skipping");
                continue;
            }
            let mut tx_hash = [0u8; 32];
            tx_hash.copy_from_slice(&value);
            results.push((tx_hash, height, idx));
            if results.len() >= limit { break; }
        }
        Ok(results)
    }

    /// Query events by address, newest-first.
    pub fn query_events_by_address(
        &self,
        address: &[u8; 20],
        limit: usize,
    ) -> Result<Vec<([u8; 32], u64, u16)>, StorageError> {
        use rocksdb::{Direction, IteratorMode};
        let cf = self.cf(CF_EVENT_INDEX);

        let mut seek_key = Vec::with_capacity(32);
        seek_key.extend_from_slice(b"a:");
        seek_key.extend_from_slice(address);
        seek_key.extend_from_slice(&u64::MAX.to_be_bytes());
        seek_key.extend_from_slice(&u16::MAX.to_be_bytes());

        let prefix_len = 22; // "a:" + 20-byte address
        let mut results = Vec::with_capacity(limit);
        let iter = self.db.iterator_cf(&cf, IteratorMode::From(&seek_key, Direction::Reverse));
        for item in iter {
            let (key, value) = item?;
            if key.len() != 32 || !key[..prefix_len].starts_with(&seek_key[..prefix_len]) {
                break;
            }
            let height = u64::from_be_bytes(key[prefix_len..prefix_len+8].try_into().unwrap());
            let idx = u16::from_be_bytes(key[prefix_len+8..prefix_len+10].try_into().unwrap());
            if value.len() != 32 {
                tracing::warn!(height, idx, value_len = value.len(), "corrupted address event index, skipping");
                continue;
            }
            let mut tx_hash = [0u8; 32];
            tx_hash.copy_from_slice(&value);
            results.push((tx_hash, height, idx));
            if results.len() >= limit { break; }
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_and_write() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        db.put_state(b"key1", b"value1").unwrap();
        let val = db.get_state(b"key1").unwrap();
        assert_eq!(val, Some(b"value1".to_vec()));
    }

    #[test]
    fn block_operations() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        db.put_block(0, b"genesis_block_data").unwrap();
        let val = db.get_block(0).unwrap();
        assert_eq!(val, Some(b"genesis_block_data".to_vec()));
        assert_eq!(db.get_block(1).unwrap(), None);
    }

    #[test]
    fn missing_key_returns_none() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        assert_eq!(db.get_state(b"nonexistent").unwrap(), None);
    }

    #[test]
    fn delete_works() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        db.put_state(b"key", b"val").unwrap();
        db.delete_state(b"key").unwrap();
        assert_eq!(db.get_state(b"key").unwrap(), None);
    }
}
