//! NFT store — persists NFT collections and individual NFTs in RocksDB.
//!
//! Uses the `state` column family with key prefixes:
//! - `c:` + collection_id → NftCollection data
//! - `n:` + nft_id → Nft data

use pichain_types::nft::{CollectionId, Nft, NftCollection, NftId};

use crate::db::PiChainDB;
use crate::StorageError;

/// Prefix for NFT collection entries.
const COLLECTION_PREFIX: u8 = b'c';
/// Prefix for individual NFT entries.
const NFT_PREFIX: u8 = b'n';

/// NFT storage operations.
pub struct NftStore<'a> {
    db: &'a PiChainDB,
}

impl<'a> NftStore<'a> {
    /// Create a new NFT store backed by the given database.
    pub fn new(db: &'a PiChainDB) -> Self {
        Self { db }
    }

    // --- Collection operations ---

    /// Get an NFT collection by ID.
    pub fn get_collection(&self, id: &CollectionId) -> Result<Option<NftCollection>, StorageError> {
        let key = collection_key(id);
        match self.db.get_state(&key)? {
            Some(data) => {
                let collection: NftCollection = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(collection))
            }
            None => Ok(None),
        }
    }

    /// Store an NFT collection.
    pub fn put_collection(&self, collection: &NftCollection) -> Result<(), StorageError> {
        let key = collection_key(&collection.id);
        let data = serde_json::to_vec(collection)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.put_state(&key, &data)
    }

    // --- NFT operations ---

    /// Get an individual NFT by ID.
    pub fn get_nft(&self, id: &NftId) -> Result<Option<Nft>, StorageError> {
        let key = nft_key(id);
        match self.db.get_state(&key)? {
            Some(data) => {
                let nft: Nft = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(nft))
            }
            None => Ok(None),
        }
    }

    /// Add a collection write to a batch (for atomic commits).
    pub fn batch_put_collection(
        &self,
        batch: &mut rocksdb::WriteBatch,
        collection: &NftCollection,
    ) -> Result<(), StorageError> {
        let mut key = vec![COLLECTION_PREFIX];
        key.extend_from_slice(&collection.id.0);
        let data = serde_json::to_vec(collection)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.batch_put_state(batch, &key, &data);
        Ok(())
    }

    /// Add an NFT write to a batch (for atomic commits).
    pub fn batch_put_nft(
        &self,
        batch: &mut rocksdb::WriteBatch,
        nft: &Nft,
    ) -> Result<(), StorageError> {
        let mut key = vec![NFT_PREFIX];
        key.extend_from_slice(&nft.id.0);
        let data =
            serde_json::to_vec(nft).map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.batch_put_state(batch, &key, &data);
        Ok(())
    }

    /// Scan all collections from storage.
    pub fn scan_all_collections(&self) -> Result<Vec<NftCollection>, StorageError> {
        let prefix = &[COLLECTION_PREFIX];
        let entries = self.db.scan_state_prefix(prefix)?;
        let mut result = Vec::new();
        for (_, value) in entries {
            let collection: NftCollection = serde_json::from_slice(&value)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            result.push(collection);
        }
        Ok(result)
    }

    /// Scan all NFTs from storage.
    pub fn scan_all_nfts(&self) -> Result<Vec<Nft>, StorageError> {
        let prefix = &[NFT_PREFIX];
        let entries = self.db.scan_state_prefix(prefix)?;
        let mut result = Vec::new();
        for (_, value) in entries {
            let nft: Nft = serde_json::from_slice(&value)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            result.push(nft);
        }
        Ok(result)
    }

    /// Store an individual NFT.
    pub fn put_nft(&self, nft: &Nft) -> Result<(), StorageError> {
        let key = nft_key(&nft.id);
        let data =
            serde_json::to_vec(nft).map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.put_state(&key, &data)
    }
}

/// Create a RocksDB key for an NFT collection.
fn collection_key(id: &CollectionId) -> Vec<u8> {
    let mut key = Vec::with_capacity(33);
    key.push(COLLECTION_PREFIX);
    key.extend_from_slice(&id.0);
    key
}

/// Create a RocksDB key for an individual NFT.
fn nft_key(id: &NftId) -> Vec<u8> {
    let mut key = Vec::with_capacity(33);
    key.push(NFT_PREFIX);
    key.extend_from_slice(&id.0);
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::ed25519::Address;
    use pichain_types::nft::NftAttribute;

    #[test]
    fn collection_roundtrip() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = NftStore::new(&db);

        let creator = Address([1u8; 20]);
        let collection_id = CollectionId::derive(&creator, 0);

        let collection = NftCollection::new(
            collection_id,
            "TestNFTs".to_string(),
            "TNFT".to_string(),
            creator,
            100,
            500,
            "https://test.com/".to_string(),
            0,
        );

        store.put_collection(&collection).unwrap();
        let loaded = store.get_collection(&collection_id).unwrap().unwrap();
        assert_eq!(loaded.name, "TestNFTs");
        assert_eq!(loaded.max_supply, 100);
        assert_eq!(loaded.royalty_bps, 500);
    }

    #[test]
    fn nft_roundtrip() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = NftStore::new(&db);

        let creator = Address([1u8; 20]);
        let collection_id = CollectionId::derive(&creator, 0);
        let nft_id = NftId::derive(&collection_id, 0);

        let nft = Nft {
            id: nft_id,
            collection: collection_id,
            index: 0,
            owner: creator,
            metadata_uri: "ipfs://test".to_string(),
            name: "Test NFT #0".to_string(),
            attributes: vec![NftAttribute {
                trait_type: "Rarity".to_string(),
                value: "Legendary".to_string(),
            }],
            listed: true,
            listed_price: 1_000_000_000,
            created_at_ms: 0,
        };

        store.put_nft(&nft).unwrap();
        let loaded = store.get_nft(&nft_id).unwrap().unwrap();
        assert_eq!(loaded.name, "Test NFT #0");
        assert_eq!(loaded.owner, creator);
        assert!(loaded.listed);
        assert_eq!(loaded.listed_price, 1_000_000_000);
        assert_eq!(loaded.attributes.len(), 1);
        assert_eq!(loaded.attributes[0].value, "Legendary");
    }

    #[test]
    fn missing_returns_none() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = NftStore::new(&db);

        let id = CollectionId::derive(&Address([99u8; 20]), 0);
        assert!(store.get_collection(&id).unwrap().is_none());

        let nft_id = NftId::derive(&id, 0);
        assert!(store.get_nft(&nft_id).unwrap().is_none());
    }
}
