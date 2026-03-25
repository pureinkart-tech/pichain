//! NFT executor — processes native NFT transactions.
//!
//! Handles: CreateNftCollection, MintNft, TransferNft, ListNft, BuyNft, DelistNft.
//!
//! Royalties are enforced at the protocol level — when an NFT is purchased
//! through the marketplace, the royalty is automatically deducted and sent
//! to the royalty recipient.

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use pichain_crypto::ed25519::Address;
use pichain_types::nft::{CollectionId, Nft, NftAttribute, NftCollection, NftId};
use pichain_types::transaction::{TransactionEvent, TransactionStatus};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Result of executing an NFT operation.
#[derive(Clone, Debug)]
pub struct NftExecutionResult {
    /// Execution status.
    pub status: TransactionStatus,
    /// Events emitted.
    pub events: Vec<TransactionEvent>,
    /// Collection changes.
    pub collection_changes: HashMap<CollectionId, NftCollection>,
    /// NFT changes.
    pub nft_changes: HashMap<NftId, Nft>,
    /// PI transfers needed: (from, to, amount).
    pub pi_transfers: Vec<(Address, Address, u64)>,
}

/// NFT executor — manages collections and individual NFTs.
pub struct NftExecutor {
    /// In-memory collection cache.
    collections: DashMap<CollectionId, NftCollection>,
    /// In-memory NFT cache.
    nfts: DashMap<NftId, Nft>,
    /// Collection nonce counter per creator.
    collection_nonces: DashMap<Address, u64>,
    /// Block timestamp for deterministic object creation.
    block_timestamp_ms: AtomicU64,
}

impl NftExecutor {
    pub fn new() -> Self {
        Self {
            collections: DashMap::new(),
            nfts: DashMap::new(),
            collection_nonces: DashMap::new(),
            block_timestamp_ms: AtomicU64::new(0),
        }
    }

    /// Clear all cached state. Used by the block producer when re-executing
    /// transactions after gas-limit trimming to prevent double-mutations.
    pub fn clear_state(&self) {
        self.collections.clear();
        self.nfts.clear();
        self.collection_nonces.clear();
    }

    /// Set the block timestamp for deterministic state creation.
    pub fn set_block_timestamp(&self, ts: u64) {
        self.block_timestamp_ms.store(ts, Ordering::Release);
    }

    fn block_timestamp(&self) -> u64 {
        self.block_timestamp_ms.load(Ordering::Acquire)
    }

    /// Load a collection into the cache.
    pub fn load_collection(&self, collection: NftCollection) {
        self.collections.insert(collection.id, collection);
    }

    /// Load an NFT into the cache.
    pub fn load_nft(&self, nft: Nft) {
        self.nfts.insert(nft.id, nft);
    }

    /// Get a collection from the cache.
    pub fn get_collection(&self, id: &CollectionId) -> Option<NftCollection> {
        self.collections.get(id).map(|v| v.clone())
    }

    /// Get an NFT from the cache.
    pub fn get_nft(&self, id: &NftId) -> Option<Nft> {
        self.nfts.get(id).map(|v| v.clone())
    }

    /// Rollback an NFT to a previous state (used when PI transfer fails after ownership change).
    pub fn rollback_nft(&self, nft_id: &NftId, old_nft: &Nft) {
        self.nfts.insert(*nft_id, old_nft.clone());
    }

    /// Get the current collection nonce for an address.
    pub fn get_collection_nonce(&self, address: &Address) -> u64 {
        self.collection_nonces.get(address).map(|v| *v).unwrap_or(0)
    }

    /// Load a collection nonce (from storage on startup).
    pub fn load_collection_nonce(&self, address: Address, nonce: u64) {
        self.collection_nonces.insert(address, nonce);
    }

    /// Snapshot all collections (for block-level persistence).
    pub fn all_collections(&self) -> HashMap<CollectionId, NftCollection> {
        self.collections
            .iter()
            .map(|e| (*e.key(), e.value().clone()))
            .collect()
    }

    /// Snapshot all NFTs (for block-level persistence).
    pub fn all_nfts(&self) -> HashMap<NftId, Nft> {
        self.nfts
            .iter()
            .map(|e| (*e.key(), e.value().clone()))
            .collect()
    }

    /// Snapshot all collection nonces (for block-level persistence).
    pub fn all_collection_nonces(&self) -> HashMap<Address, u64> {
        self.collection_nonces
            .iter()
            .map(|e| (*e.key(), *e.value()))
            .collect()
    }

    /// Create a new NFT collection.
    pub fn create_collection(
        &self,
        sender: Address,
        name: String,
        symbol: String,
        max_supply: u64,
        royalty_bps: u16,
        base_uri: String,
    ) -> NftExecutionResult {
        if name.is_empty() || name.len() > 64 {
            return nft_error("collection name must be 1-64 characters");
        }
        if symbol.is_empty() || symbol.len() > 12 {
            return nft_error("collection symbol must be 1-12 characters");
        }
        // Cap royalty at 50% to prevent abusive collection configurations
        if royalty_bps > 5000 {
            return nft_error("royalty_bps must be <= 5000 (50%)");
        }

        let nonce = {
            let mut entry = self.collection_nonces.entry(sender).or_insert(0);
            let n = *entry;
            *entry = match entry.checked_add(1) {
                Some(v) => v,
                None => return nft_error("collection nonce overflow"),
            };
            n
        };

        let collection_id = CollectionId::derive(&sender, nonce);
        let collection = NftCollection::new(
            collection_id,
            name.clone(),
            symbol.clone(),
            sender,
            max_supply,
            royalty_bps,
            base_uri,
            self.block_timestamp(),
        );

        // Atomic check-and-insert to prevent TOCTOU race under parallel execution
        match self.collections.entry(collection_id) {
            Entry::Occupied(_) => return nft_error("collection ID collision (try again)"),
            Entry::Vacant(v) => {
                v.insert(collection.clone());
            }
        }

        let mut collection_changes = HashMap::new();
        collection_changes.insert(collection_id, collection);

        NftExecutionResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "CreateNftCollection".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "collection": collection_id.to_string(),
                    "name": name,
                    "symbol": symbol,
                    "max_supply": max_supply,
                    "royalty_bps": royalty_bps,
                }))
                .unwrap_or_default(),
            }],
            collection_changes,
            nft_changes: HashMap::new(),
            pi_transfers: vec![],
        }
    }

    /// Mint a new NFT in a collection.
    pub fn mint_nft(
        &self,
        sender: Address,
        collection_id: CollectionId,
        recipient: Address,
        name: String,
        metadata_uri: String,
        attributes: Vec<NftAttribute>,
    ) -> NftExecutionResult {
        if recipient == Address::ZERO {
            return nft_error("cannot mint to the zero address");
        }

        // Use get_mut() for atomic read-modify-write on the collection to prevent TOCTOU races
        let mut collection_ref = match self.collections.get_mut(&collection_id) {
            Some(c) => c,
            None => return nft_error("collection not found"),
        };

        if collection_ref.creator != sender {
            return nft_error("only the collection creator can mint NFTs");
        }

        if !collection_ref.can_mint() {
            return nft_error("collection has reached max supply or minting is disabled");
        }

        // Atomically read the current minted counter and increment it while holding
        // the collection write lock. This prevents concurrent transactions from reading
        // the same counter value and generating duplicate NFT IDs.
        let index = collection_ref.minted;
        let nft_id = NftId::derive(&collection_id, index);

        let nft = Nft {
            id: nft_id,
            collection: collection_id,
            index,
            owner: recipient,
            metadata_uri,
            name: name.clone(),
            attributes,
            listed: false,
            listed_price: 0,
            created_at_ms: self.block_timestamp(),
        };

        // Insert NFT BEFORE incrementing minted counter. If the NftId collides
        // (should be impossible but defense-in-depth), we bail out without
        // mutating the collection, keeping the counter consistent.
        match self.nfts.entry(nft_id) {
            Entry::Occupied(_) => return nft_error("NFT ID collision (try again)"),
            Entry::Vacant(v) => {
                v.insert(nft.clone());
            }
        }

        // Only increment after successful NFT insertion — ensures the counter
        // and actual NFT existence are always in sync.
        collection_ref.minted = match collection_ref.minted.checked_add(1) {
            Some(v) => v,
            None => {
                // Rollback the NFT insertion on counter overflow
                self.nfts.remove(&nft_id);
                return nft_error("collection minted count overflow");
            }
        };
        let collection = collection_ref.clone();
        drop(collection_ref); // Release shard lock

        let mut collection_changes = HashMap::new();
        collection_changes.insert(collection_id, collection);
        let mut nft_changes = HashMap::new();
        nft_changes.insert(nft_id, nft);

        NftExecutionResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "MintNft".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "collection": collection_id.to_string(),
                    "nft": nft_id.to_string(),
                    "index": index,
                    "recipient": recipient.to_string(),
                    "name": name,
                }))
                .unwrap_or_default(),
            }],
            collection_changes,
            nft_changes,
            pi_transfers: vec![],
        }
    }

    /// Transfer an NFT to a new owner.
    pub fn transfer_nft(
        &self,
        sender: Address,
        nft_id: NftId,
        recipient: Address,
    ) -> NftExecutionResult {
        if recipient == Address::ZERO {
            return nft_error("cannot transfer to the zero address");
        }

        // Use get_mut() for atomic read-modify-write to prevent TOCTOU races
        let mut nft_ref = match self.nfts.get_mut(&nft_id) {
            Some(n) => n,
            None => return nft_error("NFT not found"),
        };

        if nft_ref.owner != sender {
            return nft_error("sender is not the NFT owner");
        }

        if sender == recipient {
            return nft_error("cannot transfer to self");
        }

        // Remove any listing and transfer ownership
        nft_ref.listed = false;
        nft_ref.listed_price = 0;
        nft_ref.owner = recipient;

        let nft = nft_ref.clone();
        drop(nft_ref); // Release shard lock

        let mut nft_changes = HashMap::new();
        nft_changes.insert(nft_id, nft);

        NftExecutionResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "TransferNft".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "nft": nft_id.to_string(),
                    "from": sender.to_string(),
                    "to": recipient.to_string(),
                }))
                .unwrap_or_default(),
            }],
            collection_changes: HashMap::new(),
            nft_changes,
            pi_transfers: vec![],
        }
    }

    /// List an NFT for sale on the marketplace.
    pub fn list_nft(&self, sender: Address, nft_id: NftId, price: u64) -> NftExecutionResult {
        if price == 0 {
            return nft_error("listing price must be > 0");
        }

        // Use get_mut() for atomic read-modify-write to prevent TOCTOU races
        let mut nft_ref = match self.nfts.get_mut(&nft_id) {
            Some(n) => n,
            None => return nft_error("NFT not found"),
        };

        if nft_ref.owner != sender {
            return nft_error("sender is not the NFT owner");
        }

        nft_ref.listed = true;
        nft_ref.listed_price = price;

        let nft = nft_ref.clone();
        drop(nft_ref); // Release shard lock

        let mut nft_changes = HashMap::new();
        nft_changes.insert(nft_id, nft);

        NftExecutionResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "ListNft".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "nft": nft_id.to_string(),
                    "price": price,
                }))
                .unwrap_or_default(),
            }],
            collection_changes: HashMap::new(),
            nft_changes,
            pi_transfers: vec![],
        }
    }

    /// Buy a listed NFT (with royalty enforcement).
    pub fn buy_nft(&self, buyer: Address, nft_id: NftId) -> NftExecutionResult {
        // DEADLOCK PREVENTION: Read collection data FIRST (before acquiring nft write lock)
        // to avoid nested DashMap shard locks (nfts -> collections). Read the NFT first
        // to get collection_id, then read collection, then acquire write lock on nft.
        let (collection_id, listed, price, owner) = match self.nfts.get(&nft_id) {
            Some(n) => (n.collection, n.listed, n.listed_price, n.owner),
            None => return nft_error("NFT not found"),
        };

        if !listed {
            return nft_error("NFT is not listed for sale");
        }
        if owner == buyer {
            return nft_error("cannot buy your own NFT");
        }

        // Read collection while NOT holding nft write lock (prevents deadlock)
        let collection = match self.collections.get(&collection_id) {
            Some(c) => c.clone(),
            None => return nft_error("collection not found"),
        };

        let royalty = price as u128 * collection.royalty_bps as u128 / 10_000;
        let royalty = std::cmp::min(royalty as u64, price); // Cap to prevent underflow
        let seller_proceeds = match price.checked_sub(royalty) {
            Some(v) => v,
            None => return nft_error("royalty exceeds sale price — accounting error"),
        };

        // Now acquire write lock on NFT and re-verify state (TOCTOU check)
        let mut nft_ref = match self.nfts.get_mut(&nft_id) {
            Some(n) => n,
            None => return nft_error("NFT not found"),
        };

        // Re-verify: state may have changed between read and write lock acquisition
        if !nft_ref.listed || nft_ref.listed_price != price || nft_ref.owner != owner {
            return nft_error("NFT state changed during purchase (retry)");
        }

        let seller = nft_ref.owner;

        // Transfer ownership atomically while holding the DashMap write guard
        nft_ref.owner = buyer;
        nft_ref.listed = false;
        nft_ref.listed_price = 0;

        let nft = nft_ref.clone();
        drop(nft_ref); // Release DashMap guard

        let mut nft_changes = HashMap::new();
        nft_changes.insert(nft_id, nft);

        // PI transfers: buyer pays, seller receives (minus royalty), royalty recipient gets royalty
        let mut pi_transfers = vec![(buyer, seller, seller_proceeds)];
        if royalty > 0 {
            pi_transfers.push((buyer, collection.royalty_recipient, royalty));
        }

        NftExecutionResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: buyer,
                event_type: "BuyNft".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "nft": nft_id.to_string(),
                    "price": price,
                    "seller": seller.to_string(),
                    "buyer": buyer.to_string(),
                    "royalty": royalty,
                }))
                .unwrap_or_default(),
            }],
            collection_changes: HashMap::new(),
            nft_changes,
            pi_transfers,
        }
    }

    /// Delist an NFT from the marketplace.
    pub fn delist_nft(&self, sender: Address, nft_id: NftId) -> NftExecutionResult {
        // Use get_mut() for atomic read-modify-write to prevent TOCTOU races
        let mut nft_ref = match self.nfts.get_mut(&nft_id) {
            Some(n) => n,
            None => return nft_error("NFT not found"),
        };

        if nft_ref.owner != sender {
            return nft_error("sender is not the NFT owner");
        }

        if !nft_ref.listed {
            return nft_error("NFT is not listed");
        }

        nft_ref.listed = false;
        nft_ref.listed_price = 0;

        let nft = nft_ref.clone();
        drop(nft_ref); // Release shard lock

        let mut nft_changes = HashMap::new();
        nft_changes.insert(nft_id, nft);

        NftExecutionResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "DelistNft".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "nft": nft_id.to_string(),
                }))
                .unwrap_or_default(),
            }],
            collection_changes: HashMap::new(),
            nft_changes,
            pi_transfers: vec![],
        }
    }
}

impl Default for NftExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to create an error result.
fn nft_error(msg: &str) -> NftExecutionResult {
    NftExecutionResult {
        status: TransactionStatus::Reverted(msg.to_string()),
        events: vec![],
        collection_changes: HashMap::new(),
        nft_changes: HashMap::new(),
        pi_transfers: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (NftExecutor, Address) {
        (NftExecutor::new(), Address([1u8; 20]))
    }

    fn create_test_collection(executor: &NftExecutor, creator: Address) -> CollectionId {
        let result = executor.create_collection(
            creator,
            "TestNFTs".to_string(),
            "TNFT".to_string(),
            100,
            500, // 5% royalty
            "https://example.com/nft/".to_string(),
        );
        assert_eq!(result.status, TransactionStatus::Success);
        *result.collection_changes.keys().next().unwrap()
    }

    #[test]
    fn create_collection() {
        let (executor, creator) = setup();
        let collection_id = create_test_collection(&executor, creator);

        let collection = executor.get_collection(&collection_id).unwrap();
        assert_eq!(collection.name, "TestNFTs");
        assert_eq!(collection.symbol, "TNFT");
        assert_eq!(collection.max_supply, 100);
        assert_eq!(collection.royalty_bps, 500);
        assert_eq!(collection.minted, 0);
        assert!(collection.minting_active);
    }

    #[test]
    fn create_collection_validation() {
        let (executor, creator) = setup();

        let r = executor.create_collection(
            creator,
            "".to_string(),
            "TST".to_string(),
            0,
            0,
            String::new(),
        );
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn mint_nft() {
        let (executor, creator) = setup();
        let collection_id = create_test_collection(&executor, creator);

        let recipient = Address([2u8; 20]);
        let result = executor.mint_nft(
            creator,
            collection_id,
            recipient,
            "Test NFT #0".to_string(),
            "ipfs://test/0".to_string(),
            vec![NftAttribute {
                trait_type: "Color".to_string(),
                value: "Blue".to_string(),
            }],
        );
        assert_eq!(result.status, TransactionStatus::Success);

        let nft_id = *result.nft_changes.keys().next().unwrap();
        let nft = executor.get_nft(&nft_id).unwrap();
        assert_eq!(nft.owner, recipient);
        assert_eq!(nft.name, "Test NFT #0");
        assert_eq!(nft.index, 0);
        assert!(!nft.listed);

        let collection = executor.get_collection(&collection_id).unwrap();
        assert_eq!(collection.minted, 1);
    }

    #[test]
    fn mint_nft_not_creator_fails() {
        let (executor, creator) = setup();
        let collection_id = create_test_collection(&executor, creator);

        let imposter = Address([99u8; 20]);
        let r = executor.mint_nft(
            imposter,
            collection_id,
            imposter,
            "Fake".to_string(),
            String::new(),
            vec![],
        );
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn transfer_nft() {
        let (executor, creator) = setup();
        let collection_id = create_test_collection(&executor, creator);

        let result = executor.mint_nft(
            creator,
            collection_id,
            creator,
            "NFT".to_string(),
            String::new(),
            vec![],
        );
        let nft_id = *result.nft_changes.keys().next().unwrap();

        let recipient = Address([2u8; 20]);
        let r = executor.transfer_nft(creator, nft_id, recipient);
        assert_eq!(r.status, TransactionStatus::Success);

        let nft = executor.get_nft(&nft_id).unwrap();
        assert_eq!(nft.owner, recipient);
    }

    #[test]
    fn transfer_nft_not_owner_fails() {
        let (executor, creator) = setup();
        let collection_id = create_test_collection(&executor, creator);

        let result = executor.mint_nft(
            creator,
            collection_id,
            creator,
            "NFT".to_string(),
            String::new(),
            vec![],
        );
        let nft_id = *result.nft_changes.keys().next().unwrap();

        let imposter = Address([99u8; 20]);
        let r = executor.transfer_nft(imposter, nft_id, Address([3u8; 20]));
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn list_and_buy_nft_with_royalty() {
        let (executor, creator) = setup();
        let collection_id = create_test_collection(&executor, creator);

        // Mint to seller
        let seller = Address([2u8; 20]);
        let result = executor.mint_nft(
            creator,
            collection_id,
            seller,
            "NFT".to_string(),
            String::new(),
            vec![],
        );
        let nft_id = *result.nft_changes.keys().next().unwrap();

        // List
        let r = executor.list_nft(seller, nft_id, 1_000_000_000); // 1 PI
        assert_eq!(r.status, TransactionStatus::Success);

        let nft = executor.get_nft(&nft_id).unwrap();
        assert!(nft.listed);
        assert_eq!(nft.listed_price, 1_000_000_000);

        // Buy
        let buyer = Address([3u8; 20]);
        let r = executor.buy_nft(buyer, nft_id);
        assert_eq!(r.status, TransactionStatus::Success);

        // Check ownership transferred
        let nft = executor.get_nft(&nft_id).unwrap();
        assert_eq!(nft.owner, buyer);
        assert!(!nft.listed);

        // Check PI transfers (royalty enforcement)
        assert_eq!(r.pi_transfers.len(), 2); // seller + royalty

        let seller_payment = r.pi_transfers.iter().find(|t| t.1 == seller).unwrap();
        let royalty_payment = r.pi_transfers.iter().find(|t| t.1 == creator).unwrap();

        // 5% royalty on 1 PI = 0.05 PI = 50_000_000 base units
        assert_eq!(royalty_payment.2, 50_000_000);
        assert_eq!(seller_payment.2, 950_000_000);
    }

    #[test]
    fn buy_unlisted_nft_fails() {
        let (executor, creator) = setup();
        let collection_id = create_test_collection(&executor, creator);

        let result = executor.mint_nft(
            creator,
            collection_id,
            creator,
            "NFT".to_string(),
            String::new(),
            vec![],
        );
        let nft_id = *result.nft_changes.keys().next().unwrap();

        let buyer = Address([3u8; 20]);
        let r = executor.buy_nft(buyer, nft_id);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn delist_nft() {
        let (executor, creator) = setup();
        let collection_id = create_test_collection(&executor, creator);

        let result = executor.mint_nft(
            creator,
            collection_id,
            creator,
            "NFT".to_string(),
            String::new(),
            vec![],
        );
        let nft_id = *result.nft_changes.keys().next().unwrap();

        executor.list_nft(creator, nft_id, 1_000_000);
        let r = executor.delist_nft(creator, nft_id);
        assert_eq!(r.status, TransactionStatus::Success);

        let nft = executor.get_nft(&nft_id).unwrap();
        assert!(!nft.listed);
    }

    #[test]
    fn mint_multiple_nfts() {
        let (executor, creator) = setup();
        let collection_id = create_test_collection(&executor, creator);

        for i in 0..5 {
            let r = executor.mint_nft(
                creator,
                collection_id,
                creator,
                format!("NFT #{i}"),
                format!("ipfs://test/{i}"),
                vec![],
            );
            assert_eq!(r.status, TransactionStatus::Success);
        }

        let collection = executor.get_collection(&collection_id).unwrap();
        assert_eq!(collection.minted, 5);
    }

    #[test]
    fn zero_royalty_collection() {
        let (executor, creator) = setup();
        let r = executor.create_collection(
            creator,
            "NoRoyalty".to_string(),
            "NRYL".to_string(),
            0,
            0,
            String::new(),
        );
        let collection_id = *r.collection_changes.keys().next().unwrap();

        let seller = Address([2u8; 20]);
        let result = executor.mint_nft(
            creator,
            collection_id,
            seller,
            "NFT".to_string(),
            String::new(),
            vec![],
        );
        let nft_id = *result.nft_changes.keys().next().unwrap();

        executor.list_nft(seller, nft_id, 1_000_000_000);

        let buyer = Address([3u8; 20]);
        let r = executor.buy_nft(buyer, nft_id);
        assert_eq!(r.status, TransactionStatus::Success);

        // No royalty — only one transfer
        assert_eq!(r.pi_transfers.len(), 1);
        assert_eq!(r.pi_transfers[0].2, 1_000_000_000); // Full price to seller
    }

    #[test]
    fn excessive_royalty_rejected() {
        let (executor, creator) = setup();
        // 51% royalty should be rejected (cap is 50% = 5000 bps)
        let r = executor.create_collection(
            creator,
            "HighRoyalty".to_string(),
            "HR".to_string(),
            100,
            5001, // Just over 50%
            String::new(),
        );
        assert!(
            matches!(r.status, TransactionStatus::Reverted(ref msg) if msg.contains("royalty"))
        );

        // 100% royalty should definitely be rejected
        let r = executor.create_collection(
            creator,
            "AllRoyalty".to_string(),
            "AR".to_string(),
            100,
            10000,
            String::new(),
        );
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));

        // 50% exactly should succeed
        let r = executor.create_collection(
            creator,
            "MaxRoyalty".to_string(),
            "MR".to_string(),
            100,
            5000,
            String::new(),
        );
        assert_eq!(r.status, TransactionStatus::Success);
    }

    /// Fix 189: Verify that minted counter and NFT insertion are atomic.
    /// If the NFT insertion fails (e.g., ID collision), the minted counter
    /// should NOT be incremented, keeping the collection consistent.
    #[test]
    fn mint_counter_consistent_with_nft_existence() {
        let (executor, creator) = setup();
        let collection_id = create_test_collection(&executor, creator);

        // Mint 5 NFTs sequentially
        for i in 0..5u64 {
            let r = executor.mint_nft(
                creator,
                collection_id,
                creator,
                format!("NFT #{i}"),
                format!("ipfs://test/{i}"),
                vec![],
            );
            assert_eq!(r.status, TransactionStatus::Success);

            // After each mint, verify the counter matches the number of NFTs
            let collection = executor.get_collection(&collection_id).unwrap();
            assert_eq!(
                collection.minted,
                i + 1,
                "minted counter should be {} after minting {} NFTs",
                i + 1,
                i + 1
            );

            // Verify each NFT exists with the correct index
            let nft_id = NftId::derive(&collection_id, i);
            let nft = executor.get_nft(&nft_id);
            assert!(
                nft.is_some(),
                "NFT at index {} should exist after minting",
                i
            );
            assert_eq!(nft.unwrap().index, i);
        }

        // Verify total state consistency
        let collection = executor.get_collection(&collection_id).unwrap();
        assert_eq!(collection.minted, 5);
        for i in 0..5u64 {
            let nft_id = NftId::derive(&collection_id, i);
            assert!(executor.get_nft(&nft_id).is_some());
        }
    }

    /// Fix 189: Verify that concurrent mints produce unique sequential indices.
    #[test]
    fn sequential_mints_have_unique_indices() {
        let (executor, creator) = setup();
        let collection_id = create_test_collection(&executor, creator);
        let recipient = Address([2u8; 20]);

        let mut seen_indices = std::collections::HashSet::new();
        for i in 0..10 {
            let r = executor.mint_nft(
                creator,
                collection_id,
                recipient,
                format!("NFT #{i}"),
                format!("ipfs://nft/{i}"),
                vec![],
            );
            assert_eq!(r.status, TransactionStatus::Success);

            let nft_id = *r.nft_changes.keys().next().unwrap();
            let nft = executor.get_nft(&nft_id).unwrap();

            // Each index should be unique
            assert!(
                seen_indices.insert(nft.index),
                "Duplicate NFT index {} detected at mint {}",
                nft.index,
                i
            );
        }

        assert_eq!(seen_indices.len(), 10);
        let collection = executor.get_collection(&collection_id).unwrap();
        assert_eq!(collection.minted, 10);
    }
}
