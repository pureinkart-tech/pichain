//! PIChain object model (Sui-inspired).
//!
//! Every piece of on-chain state is an "object" with clear ownership.
//! Owned objects can use the fast-path (bypass full consensus).
//! Shared objects go through full DAG consensus.

use pichain_crypto::keys::Address;
use pichain_crypto::Hash;
use serde::{Deserialize, Serialize};

/// Unique object identifier (32 bytes = Blake3 hash of creation tx + index).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectId(pub [u8; 32]);

impl ObjectId {
    pub fn from_tx_hash(tx_hash: &Hash, index: u32) -> Self {
        let combined = pichain_crypto::hash_concat(&[tx_hash.as_bytes(), &index.to_le_bytes()]);
        Self(*combined.as_bytes())
    }
}

impl std::fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Obj({}..)", hex::encode(&self.0[..8]))
    }
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Who owns an object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectOwner {
    /// Owned by a specific address — eligible for fast-path consensus.
    Address(Address),
    /// Shared object — must go through full DAG consensus.
    Shared,
    /// Immutable — frozen forever, can be read but never modified.
    Immutable,
    /// Owned by another object (nested ownership).
    Object(ObjectId),
}

impl ObjectOwner {
    /// Check if this object can use the Avalanche fast-path.
    pub fn is_fast_path_eligible(&self) -> bool {
        matches!(self, ObjectOwner::Address(_) | ObjectOwner::Immutable)
    }
}

/// What kind of object this is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectType {
    /// PI token balance (fungible).
    Coin,
    /// NFT or unique digital asset.
    NFT,
    /// Smart contract code module.
    Module,
    /// Generic data blob.
    Data,
    /// Validator stake record.
    StakeRecord,
    /// Mining proof record (PI digit computation).
    MiningRecord,
}

/// An on-chain object.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Object {
    /// Unique identifier.
    pub id: ObjectId,
    /// Current owner.
    pub owner: ObjectOwner,
    /// Object type.
    pub object_type: ObjectType,
    /// Version number (incremented on each modification).
    pub version: u64,
    /// Content hash (Blake3 of serialized data).
    pub content_hash: Hash,
    /// Serialized object data.
    pub data: Vec<u8>,
    /// Data size in bytes (for state rent).
    pub size: u64,
}

impl Object {
    /// Create a new coin object (PI token balance).
    pub fn new_coin(id: ObjectId, owner: Address, amount: u64) -> Self {
        let data = amount.to_le_bytes().to_vec();
        let content_hash = pichain_crypto::hash(&data);
        Self {
            id,
            owner: ObjectOwner::Address(owner),
            object_type: ObjectType::Coin,
            version: 1,
            content_hash,
            size: data.len() as u64,
            data,
        }
    }

    /// Check if this object is eligible for fast-path consensus.
    pub fn is_fast_path_eligible(&self) -> bool {
        self.owner.is_fast_path_eligible()
    }

    /// Get the coin amount (panics if not a coin object).
    pub fn coin_amount(&self) -> Option<u64> {
        if self.object_type != ObjectType::Coin || self.data.len() < 8 {
            return None;
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&self.data[..8]);
        Some(u64::from_le_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coin_object_creation() {
        let id = ObjectId([1u8; 32]);
        let owner = Address::ZERO;
        let coin = Object::new_coin(id, owner, 1_000_000_000); // 1 PI

        assert_eq!(coin.coin_amount(), Some(1_000_000_000));
        assert!(coin.is_fast_path_eligible());
        assert_eq!(coin.object_type, ObjectType::Coin);
    }

    #[test]
    fn shared_objects_not_fast_path() {
        let mut obj = Object::new_coin(ObjectId([2u8; 32]), Address::ZERO, 100);
        obj.owner = ObjectOwner::Shared;
        assert!(!obj.is_fast_path_eligible());
    }

    #[test]
    fn object_id_from_tx_hash() {
        let tx_hash = pichain_crypto::hash(b"genesis");
        let id1 = ObjectId::from_tx_hash(&tx_hash, 0);
        let id2 = ObjectId::from_tx_hash(&tx_hash, 1);
        assert_ne!(id1, id2);
    }
}
