//! Betting store — persists betting matches in RocksDB.
//!
//! Uses the `state` column family with key prefix:
//! - `m:` + match_id → BettingMatch data

use pichain_types::betting::{BettingMatch, MatchId};

use crate::db::PiChainDB;
use crate::StorageError;

/// Prefix for match entries in the state column family.
const MATCH_PREFIX: u8 = b'm';

/// Betting storage operations.
pub struct BettingStore<'a> {
    db: &'a PiChainDB,
}

impl<'a> BettingStore<'a> {
    /// Create a new betting store backed by the given database.
    pub fn new(db: &'a PiChainDB) -> Self {
        Self { db }
    }

    /// Get a betting match by ID.
    pub fn get_match(&self, id: &MatchId) -> Result<Option<BettingMatch>, StorageError> {
        let key = match_key(id);
        match self.db.get_state(&key)? {
            Some(data) => {
                let m: BettingMatch = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(m))
            }
            None => Ok(None),
        }
    }

    /// Add a match write to a batch (for atomic commits).
    pub fn batch_put_match(
        &self,
        batch: &mut rocksdb::WriteBatch,
        m: &BettingMatch,
    ) -> Result<(), StorageError> {
        let mut key = vec![MATCH_PREFIX];
        key.extend_from_slice(&m.id.0);
        let data =
            serde_json::to_vec(m).map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.batch_put_state(batch, &key, &data);
        Ok(())
    }

    /// Scan all matches from storage.
    pub fn scan_all_matches(&self) -> Result<Vec<BettingMatch>, StorageError> {
        let prefix = &[MATCH_PREFIX];
        let entries = self.db.scan_state_prefix(prefix)?;
        let mut result = Vec::new();
        for (_, value) in entries {
            let m: BettingMatch = serde_json::from_slice(&value)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            result.push(m);
        }
        Ok(result)
    }

    /// Store a betting match directly (non-batch).
    pub fn put_match(&self, m: &BettingMatch) -> Result<(), StorageError> {
        let key = match_key(&m.id);
        let data =
            serde_json::to_vec(m).map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.put_state(&key, &data)
    }
}

/// Create a RocksDB key for a betting match.
fn match_key(id: &MatchId) -> Vec<u8> {
    let mut key = Vec::with_capacity(33);
    key.push(MATCH_PREFIX);
    key.extend_from_slice(&id.0);
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::ed25519::Address;
    use pichain_types::betting::{GameCategory, MatchState, HOUSE_FEE_BPS};

    fn test_match(creator: Address) -> BettingMatch {
        let id = MatchId::derive(&creator, 0);
        BettingMatch {
            id,
            category: GameCategory::Pvp,
            game_id: "fps".to_string(),
            state: MatchState::Open,
            creator,
            wager: 1_000_000_000,
            max_players: 2,
            participants: vec![creator],
            escrow_total: 1_000_000_000,
            house_fee_bps: HOUSE_FEE_BPS,
            created_at_height: 100,
            resolve_deadline_height: 23030,
            created_at_ms: 12345,
            server_seed_hash: [0u8; 32],
            client_seeds: vec![],
            server_seed_revealed: None,
            entropy_block_hash: None,
            creator_nonce: 0,
        }
    }

    #[test]
    fn match_roundtrip() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = BettingStore::new(&db);

        let creator = Address([1u8; 20]);
        let m = test_match(creator);
        let id = m.id;

        store.put_match(&m).unwrap();
        let loaded = store.get_match(&id).unwrap().unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.wager, 1_000_000_000);
        assert_eq!(loaded.game_id, "fps");
        assert!(matches!(loaded.state, MatchState::Open));
    }

    #[test]
    fn missing_match_returns_none() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = BettingStore::new(&db);
        let id = MatchId::derive(&Address([99u8; 20]), 0);
        assert!(store.get_match(&id).unwrap().is_none());
    }

    #[test]
    fn update_match_state() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = BettingStore::new(&db);

        let creator = Address([1u8; 20]);
        let mut m = test_match(creator);
        let id = m.id;

        store.put_match(&m).unwrap();

        // Update to InProgress
        m.state = MatchState::InProgress;
        m.participants.push(Address([2u8; 20]));
        m.escrow_total = 2_000_000_000;
        store.put_match(&m).unwrap();

        let loaded = store.get_match(&id).unwrap().unwrap();
        assert!(matches!(loaded.state, MatchState::InProgress));
        assert_eq!(loaded.participants.len(), 2);
        assert_eq!(loaded.escrow_total, 2_000_000_000);
    }

    #[test]
    fn scan_all_matches_works() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = BettingStore::new(&db);

        let m1 = test_match(Address([1u8; 20]));
        let m2 = test_match(Address([2u8; 20]));
        store.put_match(&m1).unwrap();
        store.put_match(&m2).unwrap();

        let all = store.scan_all_matches().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn batch_put_match_works() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = BettingStore::new(&db);

        let m1 = test_match(Address([1u8; 20]));
        let m2 = test_match(Address([2u8; 20]));

        let mut batch = rocksdb::WriteBatch::default();
        store.batch_put_match(&mut batch, &m1).unwrap();
        store.batch_put_match(&mut batch, &m2).unwrap();
        db.write_batch(batch).unwrap();

        assert!(store.get_match(&m1.id).unwrap().is_some());
        assert!(store.get_match(&m2.id).unwrap().is_some());
    }
}
