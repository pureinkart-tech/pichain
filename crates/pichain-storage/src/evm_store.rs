//! EVM state persistence for PIChain.
//!
//! Stores EVM account state (balance, nonce, code) and storage slots
//! in CF_EVM_STATE so they survive node restarts.

use crate::db::{CF_EVM_STATE, PiChainDB};
use crate::StorageError;
use rocksdb::WriteBatch;
use serde::{Deserialize, Serialize};

/// Prefix bytes for EVM state namespacing within CF_EVM_STATE.
const PREFIX_ACCOUNT: u8 = 0x01;
const PREFIX_CODE: u8 = 0x02;
const PREFIX_STORAGE: u8 = 0x03;

/// Persisted EVM account state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvmAccountState {
    /// Native balance in wei-equivalent base units.
    pub balance: u64,
    /// Transaction nonce.
    pub nonce: u64,
    /// Whether the account has code deployed.
    pub has_code: bool,
}

/// Store for EVM-specific state in RocksDB.
pub struct EvmStore<'a> {
    db: &'a PiChainDB,
}

impl<'a> EvmStore<'a> {
    pub fn new(db: &'a PiChainDB) -> Self {
        Self { db }
    }

    // --- Account state ---

    fn account_key(address: &[u8; 20]) -> Vec<u8> {
        let mut key = Vec::with_capacity(21);
        key.push(PREFIX_ACCOUNT);
        key.extend_from_slice(address);
        key
    }

    pub fn get_account(&self, address: &[u8; 20]) -> Result<Option<EvmAccountState>, StorageError> {
        let key = Self::account_key(address);
        match self.db.inner().get_cf(&self.cf(), key)? {
            Some(bytes) => {
                let state: EvmAccountState = serde_json::from_slice(&bytes)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    pub fn put_account(&self, address: &[u8; 20], state: &EvmAccountState) -> Result<(), StorageError> {
        let key = Self::account_key(address);
        let value = serde_json::to_vec(state)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.inner().put_cf(&self.cf(), key, value)?;
        Ok(())
    }

    pub fn batch_put_account(&self, batch: &mut WriteBatch, address: &[u8; 20], state: &EvmAccountState) {
        let key = Self::account_key(address);
        let value = serde_json::to_vec(state).expect("EvmAccountState serialization");
        batch.put_cf(&self.cf(), key, value);
    }

    // --- Contract code ---

    fn code_key(address: &[u8; 20]) -> Vec<u8> {
        let mut key = Vec::with_capacity(21);
        key.push(PREFIX_CODE);
        key.extend_from_slice(address);
        key
    }

    pub fn get_code(&self, address: &[u8; 20]) -> Result<Option<Vec<u8>>, StorageError> {
        let key = Self::code_key(address);
        Ok(self.db.inner().get_cf(&self.cf(), key)?)
    }

    pub fn put_code(&self, address: &[u8; 20], code: &[u8]) -> Result<(), StorageError> {
        let key = Self::code_key(address);
        self.db.inner().put_cf(&self.cf(), key, code)?;
        Ok(())
    }

    pub fn batch_put_code(&self, batch: &mut WriteBatch, address: &[u8; 20], code: &[u8]) {
        let key = Self::code_key(address);
        batch.put_cf(&self.cf(), key, code);
    }

    // --- Storage slots ---

    fn storage_key(address: &[u8; 20], slot: &[u8; 32]) -> Vec<u8> {
        let mut key = Vec::with_capacity(53);
        key.push(PREFIX_STORAGE);
        key.extend_from_slice(address);
        key.extend_from_slice(slot);
        key
    }

    pub fn get_storage(&self, address: &[u8; 20], slot: &[u8; 32]) -> Result<Option<Vec<u8>>, StorageError> {
        let key = Self::storage_key(address, slot);
        Ok(self.db.inner().get_cf(&self.cf(), key)?)
    }

    pub fn put_storage(&self, address: &[u8; 20], slot: &[u8; 32], value: &[u8]) -> Result<(), StorageError> {
        let key = Self::storage_key(address, slot);
        self.db.inner().put_cf(&self.cf(), key, value)?;
        Ok(())
    }

    pub fn batch_put_storage(&self, batch: &mut WriteBatch, address: &[u8; 20], slot: &[u8; 32], value: &[u8]) {
        let key = Self::storage_key(address, slot);
        batch.put_cf(&self.cf(), key, value);
    }

    pub fn batch_delete_storage(&self, batch: &mut WriteBatch, address: &[u8; 20], slot: &[u8; 32]) {
        let key = Self::storage_key(address, slot);
        batch.delete_cf(&self.cf(), key);
    }

    // --- Scan all EVM accounts ---

    pub fn scan_all_accounts(&self) -> Result<Vec<([u8; 20], EvmAccountState)>, StorageError> {
        use rocksdb::IteratorMode;
        let prefix = [PREFIX_ACCOUNT];
        let cf = self.cf();
        let mut result = Vec::new();
        let iter = self.db.inner().iterator_cf(&cf, IteratorMode::From(&prefix, rocksdb::Direction::Forward));
        for item in iter {
            let (key, value) = item?;
            if key.is_empty() || key[0] != PREFIX_ACCOUNT {
                break;
            }
            if key.len() != 21 {
                continue;
            }
            let mut addr = [0u8; 20];
            addr.copy_from_slice(&key[1..21]);
            let state: EvmAccountState = serde_json::from_slice(&value)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            result.push((addr, state));
        }
        Ok(result)
    }

    fn cf(&self) -> std::sync::Arc<rocksdb::BoundColumnFamily<'_>> {
        self.db.inner().cf_handle(CF_EVM_STATE)
            .expect("CF_EVM_STATE must exist")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_roundtrip() {
        let (db, _tmp) = PiChainDB::open_temp().unwrap();
        let store = EvmStore::new(&db);

        let addr = [0xABu8; 20];
        let state = EvmAccountState { balance: 1000, nonce: 5, has_code: true };
        store.put_account(&addr, &state).unwrap();

        let loaded = store.get_account(&addr).unwrap().unwrap();
        assert_eq!(loaded.balance, 1000);
        assert_eq!(loaded.nonce, 5);
        assert!(loaded.has_code);
    }

    #[test]
    fn code_roundtrip() {
        let (db, _tmp) = PiChainDB::open_temp().unwrap();
        let store = EvmStore::new(&db);

        let addr = [0x01u8; 20];
        let code = vec![0x60, 0x80, 0x60, 0x40]; // EVM bytecode
        store.put_code(&addr, &code).unwrap();

        let loaded = store.get_code(&addr).unwrap().unwrap();
        assert_eq!(loaded, code);
    }

    #[test]
    fn storage_roundtrip() {
        let (db, _tmp) = PiChainDB::open_temp().unwrap();
        let store = EvmStore::new(&db);

        let addr = [0x02u8; 20];
        let slot = [0x03u8; 32];
        let value = [0xFFu8; 32];
        store.put_storage(&addr, &slot, &value).unwrap();

        let loaded = store.get_storage(&addr, &slot).unwrap().unwrap();
        assert_eq!(loaded, value);
    }

    #[test]
    fn scan_accounts() {
        let (db, _tmp) = PiChainDB::open_temp().unwrap();
        let store = EvmStore::new(&db);

        let addr1 = [0x01u8; 20];
        let addr2 = [0x02u8; 20];
        store.put_account(&addr1, &EvmAccountState { balance: 100, nonce: 0, has_code: false }).unwrap();
        store.put_account(&addr2, &EvmAccountState { balance: 200, nonce: 1, has_code: true }).unwrap();

        let accounts = store.scan_all_accounts().unwrap();
        assert_eq!(accounts.len(), 2);
    }

    #[test]
    fn missing_returns_none() {
        let (db, _tmp) = PiChainDB::open_temp().unwrap();
        let store = EvmStore::new(&db);
        assert!(store.get_account(&[0u8; 20]).unwrap().is_none());
        assert!(store.get_code(&[0u8; 20]).unwrap().is_none());
        assert!(store.get_storage(&[0u8; 20], &[0u8; 32]).unwrap().is_none());
    }
}
