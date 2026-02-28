//! Token store — persists token mints and token accounts in RocksDB.
//!
//! Uses the `state` column family with key prefixes:
//! - `t:` + mint_id → TokenMint data
//! - `T:` + token_account_key → TokenAccount data
//!
//! This avoids creating new column families while maintaining clean separation.

use pichain_crypto::ed25519::Address;
use pichain_types::token::{MintId, TokenAccount, TokenMint, token_account_key};

use crate::db::PiChainDB;
use crate::StorageError;

/// Prefix for token mint entries in the state column family.
const MINT_PREFIX: u8 = b't';
/// Prefix for token account entries in the state column family.
const TOKEN_ACCOUNT_PREFIX: u8 = b'T';

/// Token storage operations.
pub struct TokenStore<'a> {
    db: &'a PiChainDB,
}

impl<'a> TokenStore<'a> {
    /// Create a new token store backed by the given database.
    pub fn new(db: &'a PiChainDB) -> Self {
        Self { db }
    }

    // --- Mint operations ---

    /// Get a token mint by ID.
    pub fn get_mint(&self, id: &MintId) -> Result<Option<TokenMint>, StorageError> {
        let key = mint_key(id);
        match self.db.get_state(&key)? {
            Some(data) => {
                let mint: TokenMint = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(mint))
            }
            None => Ok(None),
        }
    }

    /// Store a token mint.
    pub fn put_mint(&self, mint: &TokenMint) -> Result<(), StorageError> {
        let key = mint_key(&mint.id);
        let data = serde_json::to_vec(mint)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.put_state(&key, &data)
    }

    // --- Token account operations ---

    /// Get a token account by owner and mint.
    pub fn get_token_account(
        &self,
        owner: &Address,
        mint: &MintId,
    ) -> Result<Option<TokenAccount>, StorageError> {
        let key = account_storage_key(owner, mint);
        match self.db.get_state(&key)? {
            Some(data) => {
                let account: TokenAccount = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(account))
            }
            None => Ok(None),
        }
    }

    /// Get a token account by its precomputed key.
    pub fn get_token_account_by_key(
        &self,
        account_key: &[u8; 32],
    ) -> Result<Option<TokenAccount>, StorageError> {
        let key = account_storage_key_from_raw(account_key);
        match self.db.get_state(&key)? {
            Some(data) => {
                let account: TokenAccount = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(account))
            }
            None => Ok(None),
        }
    }

    /// Store a token account.
    pub fn put_token_account(&self, account: &TokenAccount) -> Result<(), StorageError> {
        let key = account_storage_key(&account.owner, &account.mint);
        let data = serde_json::to_vec(account)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.put_state(&key, &data)
    }

    /// Add a mint write to a batch (for atomic commits).
    pub fn batch_put_mint(&self, batch: &mut rocksdb::WriteBatch, mint: &TokenMint) -> Result<(), StorageError> {
        let mut key = vec![MINT_PREFIX];
        key.extend_from_slice(&mint.id.0);
        let data = serde_json::to_vec(mint)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.batch_put_state(batch, &key, &data);
        Ok(())
    }

    /// Add a token account write to a batch (for atomic commits).
    pub fn batch_put_token_account(&self, batch: &mut rocksdb::WriteBatch, account: &TokenAccount) -> Result<(), StorageError> {
        let mut key = vec![TOKEN_ACCOUNT_PREFIX];
        key.extend_from_slice(&account.key);
        let data = serde_json::to_vec(account)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.batch_put_state(batch, &key, &data);
        Ok(())
    }

    /// Scan all mints from storage.
    pub fn scan_all_mints(&self) -> Result<Vec<TokenMint>, StorageError> {
        let prefix = &[MINT_PREFIX];
        let entries = self.db.scan_state_prefix(prefix)?;
        let mut result = Vec::new();
        for (_, value) in entries {
            let mint: TokenMint = serde_json::from_slice(&value)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            result.push(mint);
        }
        Ok(result)
    }

    /// Scan all token accounts from storage.
    pub fn scan_all_token_accounts(&self) -> Result<Vec<TokenAccount>, StorageError> {
        let prefix = &[TOKEN_ACCOUNT_PREFIX];
        let entries = self.db.scan_state_prefix(prefix)?;
        let mut result = Vec::new();
        for (_, value) in entries {
            let account: TokenAccount = serde_json::from_slice(&value)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            result.push(account);
        }
        Ok(result)
    }

    /// Get or create a token account (returns existing account or a new one with zero balance).
    pub fn get_or_create_token_account(
        &self,
        owner: &Address,
        mint: &MintId,
    ) -> Result<TokenAccount, StorageError> {
        match self.get_token_account(owner, mint)? {
            Some(account) => Ok(account),
            None => Ok(TokenAccount::new(*owner, *mint)),
        }
    }
}

/// Create a RocksDB key for a token mint.
fn mint_key(id: &MintId) -> Vec<u8> {
    let mut key = Vec::with_capacity(33);
    key.push(MINT_PREFIX);
    key.extend_from_slice(&id.0);
    key
}

/// Create a RocksDB key for a token account.
fn account_storage_key(owner: &Address, mint: &MintId) -> Vec<u8> {
    let account_key = token_account_key(owner, mint);
    account_storage_key_from_raw(&account_key)
}

/// Create a RocksDB key for a token account from its raw key.
fn account_storage_key_from_raw(account_key: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(33);
    key.push(TOKEN_ACCOUNT_PREFIX);
    key.extend_from_slice(account_key);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_roundtrip() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = TokenStore::new(&db);

        let mint_id = MintId::derive(&Address([1u8; 20]), 0);
        let mint = TokenMint::new(
            mint_id,
            "TestCoin".to_string(),
            "TST".to_string(),
            9,
            1_000_000_000,
            Address([1u8; 20]),
            String::new(),
            0,
        );

        store.put_mint(&mint).unwrap();
        let loaded = store.get_mint(&mint_id).unwrap().unwrap();
        assert_eq!(loaded.name, "TestCoin");
        assert_eq!(loaded.symbol, "TST");
        assert_eq!(loaded.decimals, 9);
        assert_eq!(loaded.max_supply, 1_000_000_000);
    }

    #[test]
    fn token_account_roundtrip() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = TokenStore::new(&db);

        let owner = Address([1u8; 20]);
        let mint = MintId::derive(&Address([2u8; 20]), 0);

        let mut account = TokenAccount::new(owner, mint);
        account.balance = 42_000;

        store.put_token_account(&account).unwrap();
        let loaded = store.get_token_account(&owner, &mint).unwrap().unwrap();
        assert_eq!(loaded.balance, 42_000);
        assert_eq!(loaded.owner, owner);
        assert_eq!(loaded.mint, mint);
    }

    #[test]
    fn missing_mint_returns_none() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = TokenStore::new(&db);
        let id = MintId::derive(&Address([99u8; 20]), 0);
        assert!(store.get_mint(&id).unwrap().is_none());
    }

    #[test]
    fn get_or_create_account() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = TokenStore::new(&db);

        let owner = Address([1u8; 20]);
        let mint = MintId::derive(&Address([2u8; 20]), 0);

        // Should create a new account with zero balance
        let account = store.get_or_create_token_account(&owner, &mint).unwrap();
        assert_eq!(account.balance, 0);

        // Put an account with balance
        let mut account = TokenAccount::new(owner, mint);
        account.balance = 100;
        store.put_token_account(&account).unwrap();

        // Should return existing account
        let loaded = store.get_or_create_token_account(&owner, &mint).unwrap();
        assert_eq!(loaded.balance, 100);
    }

    #[test]
    fn different_mints_different_accounts() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = TokenStore::new(&db);

        let owner = Address([1u8; 20]);
        let mint_a = MintId::derive(&Address([2u8; 20]), 0);
        let mint_b = MintId::derive(&Address([3u8; 20]), 0);

        let mut acct_a = TokenAccount::new(owner, mint_a);
        acct_a.balance = 100;
        store.put_token_account(&acct_a).unwrap();

        let mut acct_b = TokenAccount::new(owner, mint_b);
        acct_b.balance = 200;
        store.put_token_account(&acct_b).unwrap();

        let loaded_a = store.get_token_account(&owner, &mint_a).unwrap().unwrap();
        let loaded_b = store.get_token_account(&owner, &mint_b).unwrap().unwrap();
        assert_eq!(loaded_a.balance, 100);
        assert_eq!(loaded_b.balance, 200);
    }
}
