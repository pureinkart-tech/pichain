//! DEX store — persists liquidity pools and LP balances in RocksDB.
//!
//! Uses the `state` column family with key prefixes:
//! - `p:` + pool_id → LiquidityPool data
//! - `L:` + pool_id + owner → LP token balance (u64)

use pichain_crypto::ed25519::Address;
use pichain_types::dex::{LiquidityPool, PoolId};

use crate::db::PiChainDB;
use crate::StorageError;

/// Prefix for pool entries in the state column family.
const POOL_PREFIX: u8 = b'p';
/// Prefix for LP balance entries in the state column family.
const LP_BALANCE_PREFIX: u8 = b'L';

/// DEX storage operations.
pub struct DexStore<'a> {
    db: &'a PiChainDB,
}

impl<'a> DexStore<'a> {
    /// Create a new DEX store backed by the given database.
    pub fn new(db: &'a PiChainDB) -> Self {
        Self { db }
    }

    // --- Pool operations ---

    /// Get a liquidity pool by ID.
    pub fn get_pool(&self, id: &PoolId) -> Result<Option<LiquidityPool>, StorageError> {
        let key = pool_key(id);
        match self.db.get_state(&key)? {
            Some(data) => {
                let pool: LiquidityPool = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(pool))
            }
            None => Ok(None),
        }
    }

    /// Store a liquidity pool.
    pub fn put_pool(&self, pool: &LiquidityPool) -> Result<(), StorageError> {
        let key = pool_key(&pool.id);
        let data = serde_json::to_vec(pool)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.put_state(&key, &data)
    }

    // --- LP balance operations ---

    /// Get an LP token balance.
    pub fn get_lp_balance(
        &self,
        pool_id: &PoolId,
        owner: &Address,
    ) -> Result<u64, StorageError> {
        let key = lp_balance_key(pool_id, owner);
        match self.db.get_state(&key)? {
            Some(data) => {
                if let Ok(arr) = <[u8; 8]>::try_from(data.as_slice()) {
                    Ok(u64::from_le_bytes(arr))
                } else {
                    Err(StorageError::Serialization(
                        "invalid LP balance data".to_string(),
                    ))
                }
            }
            None => Ok(0),
        }
    }

    /// Add a pool write to a batch (for atomic commits).
    pub fn batch_put_pool(&self, batch: &mut rocksdb::WriteBatch, pool: &LiquidityPool) -> Result<(), StorageError> {
        let mut key = vec![POOL_PREFIX];
        key.extend_from_slice(&pool.id.0);
        let data = serde_json::to_vec(pool)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.batch_put_state(batch, &key, &data);
        Ok(())
    }

    /// Add an LP balance write to a batch (for atomic commits).
    pub fn batch_put_lp_balance(&self, batch: &mut rocksdb::WriteBatch, pool_id: &PoolId, address: &Address, balance: u64) -> Result<(), StorageError> {
        let mut key = vec![LP_BALANCE_PREFIX];
        key.extend_from_slice(&pool_id.0);
        key.extend_from_slice(&address.0);
        let data = balance.to_le_bytes();
        self.db.batch_put_state(batch, &key, &data);
        Ok(())
    }

    /// Scan all pools from storage.
    pub fn scan_all_pools(&self) -> Result<Vec<LiquidityPool>, StorageError> {
        let prefix = &[POOL_PREFIX];
        let entries = self.db.scan_state_prefix(prefix)?;
        let mut result = Vec::new();
        for (_, value) in entries {
            let pool: LiquidityPool = serde_json::from_slice(&value)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            result.push(pool);
        }
        Ok(result)
    }

    /// Scan all LP balances from storage.
    pub fn scan_all_lp_balances(&self) -> Result<Vec<(PoolId, Address, u64)>, StorageError> {
        let prefix = &[LP_BALANCE_PREFIX];
        let entries = self.db.scan_state_prefix(prefix)?;
        let mut result = Vec::new();
        for (key_suffix, value) in entries {
            if key_suffix.len() != 52 { continue; } // 32 (pool_id) + 20 (address)
            let mut pool_bytes = [0u8; 32];
            pool_bytes.copy_from_slice(&key_suffix[..32]);
            let pool_id = PoolId(pool_bytes);
            let mut addr_bytes = [0u8; 20];
            addr_bytes.copy_from_slice(&key_suffix[32..52]);
            let address = Address(addr_bytes);
            if let Ok(arr) = <[u8; 8]>::try_from(value.as_slice()) {
                let balance = u64::from_le_bytes(arr);
                result.push((pool_id, address, balance));
            }
        }
        Ok(result)
    }

    /// Store an LP token balance.
    pub fn put_lp_balance(
        &self,
        pool_id: &PoolId,
        owner: &Address,
        balance: u64,
    ) -> Result<(), StorageError> {
        let key = lp_balance_key(pool_id, owner);
        self.db.put_state(&key, &balance.to_le_bytes())
    }
}

/// Create a RocksDB key for a liquidity pool.
fn pool_key(id: &PoolId) -> Vec<u8> {
    let mut key = Vec::with_capacity(33);
    key.push(POOL_PREFIX);
    key.extend_from_slice(&id.0);
    key
}

/// Create a RocksDB key for an LP balance.
fn lp_balance_key(pool_id: &PoolId, owner: &Address) -> Vec<u8> {
    let mut key = Vec::with_capacity(53);
    key.push(LP_BALANCE_PREFIX);
    key.extend_from_slice(&pool_id.0);
    key.extend_from_slice(&owner.0);
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_types::token::MintId;

    #[test]
    fn pool_roundtrip() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = DexStore::new(&db);

        let mint_a = MintId::ZERO;
        let mint_b = MintId::derive(&Address([1u8; 20]), 0);
        let creator = Address([1u8; 20]);

        let mut pool = LiquidityPool::new(mint_a, mint_b, creator, 0);
        pool.reserve_a = 1_000_000;
        pool.reserve_b = 2_000_000;
        pool.lp_supply = 1_414_213;

        store.put_pool(&pool).unwrap();
        let loaded = store.get_pool(&pool.id).unwrap().unwrap();
        assert_eq!(loaded.reserve_a, 1_000_000);
        assert_eq!(loaded.reserve_b, 2_000_000);
        assert_eq!(loaded.lp_supply, 1_414_213);
        assert_eq!(loaded.creator, creator);
    }

    #[test]
    fn missing_pool_returns_none() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = DexStore::new(&db);

        let id = PoolId::derive(&MintId::ZERO, &MintId::derive(&Address([99u8; 20]), 0));
        assert!(store.get_pool(&id).unwrap().is_none());
    }

    #[test]
    fn lp_balance_roundtrip() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = DexStore::new(&db);

        let pool_id = PoolId::derive(&MintId::ZERO, &MintId::derive(&Address([1u8; 20]), 0));
        let owner = Address([1u8; 20]);

        // Default is 0
        assert_eq!(store.get_lp_balance(&pool_id, &owner).unwrap(), 0);

        store.put_lp_balance(&pool_id, &owner, 999_000).unwrap();
        assert_eq!(store.get_lp_balance(&pool_id, &owner).unwrap(), 999_000);
    }

    #[test]
    fn different_owners_different_balances() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = DexStore::new(&db);

        let pool_id = PoolId::derive(&MintId::ZERO, &MintId::derive(&Address([1u8; 20]), 0));
        let owner_a = Address([1u8; 20]);
        let owner_b = Address([2u8; 20]);

        store.put_lp_balance(&pool_id, &owner_a, 100).unwrap();
        store.put_lp_balance(&pool_id, &owner_b, 200).unwrap();

        assert_eq!(store.get_lp_balance(&pool_id, &owner_a).unwrap(), 100);
        assert_eq!(store.get_lp_balance(&pool_id, &owner_b).unwrap(), 200);
    }
}
