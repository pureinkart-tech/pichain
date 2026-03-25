//! NFT types — native NFT standard with collections, royalties, and marketplace.
//!
//! Each NFT belongs to a `NftCollection` and has unique metadata.
//! Royalties are enforced at the protocol level on marketplace sales.

use pichain_crypto::keys::Address;
use serde::{Deserialize, Serialize};

/// Unique identifier for an NFT collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct CollectionId(pub [u8; 32]);

impl CollectionId {
    /// Derive a collection ID from creator and nonce.
    pub fn derive(creator: &Address, nonce: u64) -> Self {
        let mut data = Vec::with_capacity(20 + 8 + 18);
        data.extend_from_slice(&creator.0);
        data.extend_from_slice(&nonce.to_le_bytes());
        data.extend_from_slice(b"pichain-nft-collection");
        let hash = pichain_crypto::hash(&data);
        Self(*hash.as_bytes())
    }
}

impl std::fmt::Display for CollectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Unique identifier for an individual NFT within a collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct NftId(pub [u8; 32]);

impl NftId {
    /// Derive an NFT ID from collection and token index.
    pub fn derive(collection: &CollectionId, index: u64) -> Self {
        let mut data = Vec::with_capacity(32 + 8 + 10);
        data.extend_from_slice(&collection.0);
        data.extend_from_slice(&index.to_le_bytes());
        data.extend_from_slice(b"pichain-nft");
        let hash = pichain_crypto::hash(&data);
        Self(*hash.as_bytes())
    }
}

impl std::fmt::Display for NftId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// An NFT collection — defines a series of related NFTs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NftCollection {
    /// Unique collection identifier.
    pub id: CollectionId,
    /// Collection name.
    pub name: String,
    /// Collection symbol.
    pub symbol: String,
    /// Creator address.
    pub creator: Address,
    /// Maximum NFTs in this collection (0 = unlimited).
    pub max_supply: u64,
    /// Number of NFTs minted so far.
    pub minted: u64,
    /// Royalty in basis points (e.g., 500 = 5%).
    pub royalty_bps: u16,
    /// Royalty recipient address.
    pub royalty_recipient: Address,
    /// Base URI for metadata (individual NFTs append their index).
    pub base_uri: String,
    /// Whether new NFTs can still be minted.
    pub minting_active: bool,
    /// Creation timestamp.
    pub created_at_ms: u64,
}

impl NftCollection {
    /// Maximum royalty: 50%.
    pub const MAX_ROYALTY_BPS: u16 = 5000;

    /// Create a new NFT collection.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: CollectionId,
        name: String,
        symbol: String,
        creator: Address,
        max_supply: u64,
        royalty_bps: u16,
        base_uri: String,
        created_at_ms: u64,
    ) -> Self {
        Self {
            id,
            name,
            symbol,
            creator,
            max_supply,
            minted: 0,
            royalty_bps: std::cmp::min(royalty_bps, Self::MAX_ROYALTY_BPS),
            royalty_recipient: creator,
            base_uri,
            minting_active: true,
            created_at_ms,
        }
    }

    /// Check if more NFTs can be minted.
    pub fn can_mint(&self) -> bool {
        self.minting_active && (self.max_supply == 0 || self.minted < self.max_supply)
    }
}

/// An individual NFT.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Nft {
    /// Unique NFT identifier.
    pub id: NftId,
    /// Collection this NFT belongs to.
    pub collection: CollectionId,
    /// Index within the collection.
    pub index: u64,
    /// Current owner.
    pub owner: Address,
    /// Metadata URI (e.g., IPFS link to JSON).
    pub metadata_uri: String,
    /// NFT name.
    pub name: String,
    /// Optional attributes.
    pub attributes: Vec<NftAttribute>,
    /// Whether this NFT is listed for sale on the marketplace.
    pub listed: bool,
    /// Listed price in base PI units (only relevant if listed = true).
    pub listed_price: u64,
    /// Creation timestamp.
    pub created_at_ms: u64,
}

/// An NFT attribute (key-value metadata).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NftAttribute {
    /// Attribute name (e.g., "Background").
    pub trait_type: String,
    /// Attribute value (e.g., "Blue").
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_id_deterministic() {
        let addr = Address([1u8; 20]);
        let id1 = CollectionId::derive(&addr, 0);
        let id2 = CollectionId::derive(&addr, 0);
        assert_eq!(id1, id2);

        let id3 = CollectionId::derive(&addr, 1);
        assert_ne!(id1, id3);
    }

    #[test]
    fn nft_id_deterministic() {
        let collection = CollectionId::derive(&Address([1u8; 20]), 0);
        let id1 = NftId::derive(&collection, 0);
        let id2 = NftId::derive(&collection, 0);
        assert_eq!(id1, id2);

        let id3 = NftId::derive(&collection, 1);
        assert_ne!(id1, id3);
    }

    #[test]
    fn collection_can_mint() {
        let addr = Address([1u8; 20]);
        let mut collection = NftCollection::new(
            CollectionId::derive(&addr, 0),
            "Test".to_string(),
            "TST".to_string(),
            addr,
            10,
            500,
            String::new(),
            0,
        );

        assert!(collection.can_mint());
        collection.minted = 10;
        assert!(!collection.can_mint()); // At max supply

        collection.max_supply = 0; // Unlimited
        assert!(collection.can_mint());

        collection.minting_active = false;
        assert!(!collection.can_mint()); // Minting disabled
    }

    #[test]
    fn royalty_capped() {
        let addr = Address([1u8; 20]);
        let collection = NftCollection::new(
            CollectionId::derive(&addr, 0),
            "Test".to_string(),
            "TST".to_string(),
            addr,
            0,
            10_000, // Try 100% royalty
            String::new(),
            0,
        );
        // Should be capped at 50%
        assert_eq!(collection.royalty_bps, NftCollection::MAX_ROYALTY_BPS);
    }

    #[test]
    fn nft_creation() {
        let addr = Address([1u8; 20]);
        let collection_id = CollectionId::derive(&addr, 0);
        let nft_id = NftId::derive(&collection_id, 0);

        let nft = Nft {
            id: nft_id,
            collection: collection_id,
            index: 0,
            owner: addr,
            metadata_uri: "ipfs://test".to_string(),
            name: "Test NFT #0".to_string(),
            attributes: vec![NftAttribute {
                trait_type: "Background".to_string(),
                value: "Blue".to_string(),
            }],
            listed: false,
            listed_price: 0,
            created_at_ms: 0,
        };

        assert_eq!(nft.owner, addr);
        assert!(!nft.listed);
        assert_eq!(nft.attributes.len(), 1);
    }
}
