//! Launchpad store — persists token launches in RocksDB.
//!
//! Uses the `state` column family with key prefix:
//! - `l:` + launch_id → TokenLaunch data

use pichain_types::launchpad::{LaunchId, TokenLaunch};

use crate::db::PiChainDB;
use crate::StorageError;

/// Prefix for launch entries in the state column family.
const LAUNCH_PREFIX: u8 = b'l';

/// Launchpad storage operations.
pub struct LaunchpadStore<'a> {
    db: &'a PiChainDB,
}

impl<'a> LaunchpadStore<'a> {
    /// Create a new launchpad store backed by the given database.
    pub fn new(db: &'a PiChainDB) -> Self {
        Self { db }
    }

    /// Get a token launch by ID.
    pub fn get_launch(&self, id: &LaunchId) -> Result<Option<TokenLaunch>, StorageError> {
        let key = launch_key(id);
        match self.db.get_state(&key)? {
            Some(data) => {
                let launch: TokenLaunch = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(launch))
            }
            None => Ok(None),
        }
    }

    /// Add a launch write to a batch (for atomic commits).
    pub fn batch_put_launch(&self, batch: &mut rocksdb::WriteBatch, launch: &TokenLaunch) -> Result<(), StorageError> {
        let mut key = vec![LAUNCH_PREFIX];
        key.extend_from_slice(&launch.id.0);
        let data = serde_json::to_vec(launch)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.batch_put_state(batch, &key, &data);
        Ok(())
    }

    /// Scan all launches from storage.
    pub fn scan_all_launches(&self) -> Result<Vec<TokenLaunch>, StorageError> {
        let prefix = &[LAUNCH_PREFIX];
        let entries = self.db.scan_state_prefix(prefix)?;
        let mut result = Vec::new();
        for (_, value) in entries {
            let launch: TokenLaunch = serde_json::from_slice(&value)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            result.push(launch);
        }
        Ok(result)
    }

    /// Store a token launch.
    pub fn put_launch(&self, launch: &TokenLaunch) -> Result<(), StorageError> {
        let key = launch_key(&launch.id);
        let data = serde_json::to_vec(launch)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.put_state(&key, &data)
    }
}

/// Create a RocksDB key for a token launch.
fn launch_key(id: &LaunchId) -> Vec<u8> {
    let mut key = Vec::with_capacity(33);
    key.push(LAUNCH_PREFIX);
    key.extend_from_slice(&id.0);
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::ed25519::Address;
    use pichain_types::launchpad::{LaunchState, LaunchType};
    use pichain_types::token::MintId;
    use std::collections::HashMap;

    #[test]
    fn launch_roundtrip() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = LaunchpadStore::new(&db);

        let creator = Address([1u8; 20]);
        let mint = MintId::derive(&creator, 0);
        let launch_id = LaunchId::from_mint(&mint);

        let launch = TokenLaunch {
            id: launch_id,
            mint,
            creator,
            launch_type: LaunchType::FairLaunch {
                price_per_token: 1_000,
            },
            state: LaunchState::Active,
            tokens_for_sale: 1_000_000,
            tokens_sold: 0,
            pi_raised: 0,
            target_pi: 1_000_000_000,
            liquidity_bps: 8000,
            token_liquidity_bps: 2000,
            max_per_address: 100_000_000,
            contributions: HashMap::new(),
            created_at_ms: 12345,
        };

        store.put_launch(&launch).unwrap();
        let loaded = store.get_launch(&launch_id).unwrap().unwrap();
        assert_eq!(loaded.mint, mint);
        assert_eq!(loaded.tokens_for_sale, 1_000_000);
        assert_eq!(loaded.state, LaunchState::Active);
    }

    #[test]
    fn missing_launch_returns_none() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = LaunchpadStore::new(&db);
        let id = LaunchId::from_mint(&MintId::derive(&Address([99u8; 20]), 0));
        assert!(store.get_launch(&id).unwrap().is_none());
    }

    #[test]
    fn update_launch_state() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = LaunchpadStore::new(&db);

        let creator = Address([1u8; 20]);
        let mint = MintId::derive(&creator, 0);
        let launch_id = LaunchId::from_mint(&mint);

        let mut launch = TokenLaunch {
            id: launch_id,
            mint,
            creator,
            launch_type: LaunchType::BondingCurve {
                base_price: 100,
                slope: 1,
            },
            state: LaunchState::Active,
            tokens_for_sale: 10_000,
            tokens_sold: 0,
            pi_raised: 0,
            target_pi: 10_000_000,
            liquidity_bps: 8000,
            token_liquidity_bps: 2000,
            max_per_address: u64::MAX,
            contributions: HashMap::new(),
            created_at_ms: 0,
        };

        store.put_launch(&launch).unwrap();

        // Update state
        launch.tokens_sold = 5000;
        launch.pi_raised = 500_000;
        launch.state = LaunchState::TargetReached;
        store.put_launch(&launch).unwrap();

        let loaded = store.get_launch(&launch_id).unwrap().unwrap();
        assert_eq!(loaded.tokens_sold, 5000);
        assert_eq!(loaded.pi_raised, 500_000);
        assert_eq!(loaded.state, LaunchState::TargetReached);
    }
}
