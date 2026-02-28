//! WASM contract storage persistence.
//!
//! Key format: "W" + contract_address (20 bytes) + storage_key (variable).
//! Stored in the `state` column family.

use crate::db::PiChainDB;
use crate::StorageError;
use pichain_crypto::ed25519::Address;

const CONTRACT_STORAGE_PREFIX: u8 = b'W';

/// WASM contract storage store.
pub struct ContractStorageStore<'a> {
    db: &'a PiChainDB,
}

impl<'a> ContractStorageStore<'a> {
    pub fn new(db: &'a PiChainDB) -> Self {
        Self { db }
    }

    fn make_key(contract: &Address, storage_key: &[u8]) -> Vec<u8> {
        // Length-delimited format: prefix(1) + address(20) + key_len(4 BE) + storage_key(N)
        // The 4-byte length prefix prevents ambiguity where a crafted storage_key
        // could overflow into another contract's namespace.
        let mut key = Vec::with_capacity(1 + 20 + 4 + storage_key.len());
        key.push(CONTRACT_STORAGE_PREFIX);
        key.extend_from_slice(&contract.0);
        key.extend_from_slice(&(storage_key.len() as u32).to_be_bytes());
        key.extend_from_slice(storage_key);
        key
    }

    fn contract_prefix(contract: &Address) -> Vec<u8> {
        let mut prefix = Vec::with_capacity(21);
        prefix.push(CONTRACT_STORAGE_PREFIX);
        prefix.extend_from_slice(&contract.0);
        prefix
    }

    /// Get a single contract storage value.
    pub fn get(&self, contract: &Address, storage_key: &[u8]) -> Result<Option<Vec<u8>>, StorageError> {
        let key = Self::make_key(contract, storage_key);
        self.db.get_state(&key)
    }

    /// Put a single contract storage value.
    pub fn put(&self, contract: &Address, storage_key: &[u8], value: &[u8]) -> Result<(), StorageError> {
        let key = Self::make_key(contract, storage_key);
        self.db.put_state(&key, value)
    }

    /// Add a contract storage write to a batch.
    pub fn batch_put(&self, batch: &mut rocksdb::WriteBatch, contract: &Address, storage_key: &[u8], value: &[u8]) {
        let key = Self::make_key(contract, storage_key);
        self.db.batch_put_state(batch, &key, value);
    }

    /// Add a contract storage deletion to a batch.
    pub fn batch_delete(&self, batch: &mut rocksdb::WriteBatch, contract: &Address, storage_key: &[u8]) {
        let key = Self::make_key(contract, storage_key);
        self.db.batch_delete_state(batch, &key);
    }

    /// Scan all storage for a specific contract.
    ///
    /// Returns `(key_suffix, value)` pairs where `key_suffix` is the portion
    /// of the RocksDB key after the `contract_prefix` (`'W' + address`) has
    /// been stripped. **Note:** `key_suffix` still includes the 4-byte
    /// big-endian length prefix followed by the raw storage key bytes, i.e.:
    ///
    /// ```text
    /// key_suffix = key_len (4 bytes BE) + storage_key (key_len bytes)
    /// ```
    ///
    /// Callers must parse the 4-byte length prefix to extract the actual
    /// storage key. See `scan_all()` for an example of this parsing.
    pub fn scan_contract(&self, contract: &Address) -> Result<Vec<(Vec<u8>, Vec<u8>)>, StorageError> {
        let prefix = Self::contract_prefix(contract);
        let entries = self.db.scan_state_prefix(&prefix)?;
        Ok(entries)
    }

    /// Scan all contract storage entries (for startup reload).
    /// Returns (contract_address, storage_key, value) tuples.
    pub fn scan_all(&self) -> Result<Vec<(Address, Vec<u8>, Vec<u8>)>, StorageError> {
        let prefix = &[CONTRACT_STORAGE_PREFIX];
        let entries = self.db.scan_state_prefix(prefix)?;
        let mut result = Vec::new();
        for (key_suffix, value) in entries {
            // Key suffix format: address(20) + key_len(4 BE) + storage_key(N)
            if key_suffix.len() < 24 { continue; } // 20 addr + 4 len minimum
            let mut addr_bytes = [0u8; 20];
            addr_bytes.copy_from_slice(&key_suffix[..20]);
            let contract = Address(addr_bytes);
            let key_len = u32::from_be_bytes(
                key_suffix[20..24].try_into().unwrap_or([0; 4])
            ) as usize;
            // Guard against overflow on 32-bit: 24 + key_len could wrap
            let key_end = match 24usize.checked_add(key_len) {
                Some(end) => end,
                None => continue, // corrupted length field; skip entry
            };
            if key_suffix.len() < key_end { continue; }
            let storage_key = key_suffix[24..key_end].to_vec();
            result.push((contract, storage_key, value));
        }
        Ok(result)
    }
}
