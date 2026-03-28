//! State store — manages accounts and objects using JMT + RocksDB.
//!
//! # JMT State Root Coverage (M22)
//!
//! The JMT (Jellyfish Merkle Tree) state root **only covers native accounts
//! and objects**. Sub-executor state — tokens (TokenMint, TokenAccount),
//! DEX pools (LiquidityPool, LP balances), NFTs (NftCollection, Nft),
//! launchpad (TokenLaunch), and WASM contract storage — is persisted to
//! RocksDB column families but is **NOT** included in the authenticated
//! Merkle state root.
//!
//! This means light-client state proofs can only verify account balances and
//! object ownership. Verifying sub-executor state requires a full node or a
//! future sub-executor Merkle commitment scheme.

use pichain_crypto::keys::Address;
use pichain_types::account::{Account, AccountState};
use pichain_types::object::{Object, ObjectId};

use crate::db::PiChainDB;
use crate::jmt::JellyfishMerkleTree;
use crate::StorageError;

type PrepareResult = Result<
    (
        rocksdb::WriteBatch,
        pichain_crypto::poseidon::PoseidonHash,
        Vec<([u8; 32], Vec<u8>)>,
    ),
    StorageError,
>;

/// High-level state store combining JMT for authenticated state and RocksDB for raw data.
pub struct StateStore {
    db: PiChainDB,
    jmt: JellyfishMerkleTree,
}

impl StateStore {
    /// Create a new state store backed by the given database.
    pub fn new(db: PiChainDB) -> Self {
        Self {
            db,
            jmt: JellyfishMerkleTree::new(),
        }
    }

    /// Get the current state root hash.
    pub fn state_root(&self) -> pichain_crypto::poseidon::PoseidonHash {
        self.jmt.root_hash()
    }

    /// Get an account by address.
    pub fn get_account(&self, address: &Address) -> Result<Option<Account>, StorageError> {
        let key = address_key(address);
        match self.db.get_state(&key)? {
            Some(data) => {
                let state: AccountState = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(Account {
                    address: *address,
                    state,
                }))
            }
            None => Ok(None),
        }
    }

    /// Scan all accounts with non-zero staked balance.
    /// Used during node restart to pre-load staked accounts into the executor cache
    /// so that anti-concentration tracking is accurate from the first block.
    pub fn scan_staked_accounts(&self) -> Result<Vec<Account>, StorageError> {
        let entries = self.db.scan_state_prefix(b"a")?;
        let mut staked = Vec::new();
        for (key_suffix, data) in &entries {
            if key_suffix.len() == 20 {
                let addr = Address(key_suffix[..20].try_into().expect("length checked"));
                let state: AccountState = serde_json::from_slice(data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                if state.staked > 0 {
                    staked.push(Account {
                        address: addr,
                        state,
                    });
                }
            }
        }
        Ok(staked)
    }

    /// Put (create or update) an account.
    ///
    /// # Safety (R32-FIX)
    /// This bypasses the atomic `prepare_block_batch` path. Only use for
    /// genesis initialization or tests. During normal operation, accounts
    /// must be updated via `prepare_block_batch` + `write_batch_sync` to
    /// maintain atomicity with block persistence.
    pub fn put_account(&mut self, account: &Account) -> Result<(), StorageError> {
        let key = address_key(&account.address);
        let data = serde_json::to_vec(&account.state)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        self.db.put_state(&key, &data)?;

        // Update JMT with the account state hash.
        // Domain-separated: byte 0 = 0x01 (account), bytes 1..21 = address, rest = 0.
        let jmt_key = account_jmt_key(&account.address);
        self.jmt.put(&jmt_key, &data);

        Ok(())
    }

    /// Atomically write multiple accounts in a single synced batch.
    ///
    /// Uses the same staging pattern as `persist_block_atomic` (Fix STOR-223b):
    /// JMT updates are deferred until after the batch write succeeds, so a
    /// failed write leaves the in-memory JMT unchanged.
    pub fn put_accounts_atomic(&mut self, accounts: &[&Account]) -> Result<(), StorageError> {
        let mut batch = self.db.new_batch();
        let mut pending_jmt_updates: Vec<([u8; 32], Vec<u8>)> = Vec::new();
        for account in accounts {
            let key = address_key(&account.address);
            let data = serde_json::to_vec(&account.state)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            self.db.batch_put_state(&mut batch, &key, &data);

            // Stage JMT update (applied only after batch write succeeds)
            let jmt_key = account_jmt_key(&account.address);
            pending_jmt_updates.push((jmt_key, data));
        }
        self.db.write_batch_sync(batch)?;

        // Only now apply JMT updates (Fix STOR-223b).
        for (jmt_key, data) in pending_jmt_updates {
            self.jmt.put(&jmt_key, &data);
        }
        Ok(())
    }

    /// Get an object by ID.
    pub fn get_object(&self, id: &ObjectId) -> Result<Option<Object>, StorageError> {
        match self.db.get_object(&id.0)? {
            Some(data) => {
                let obj: Object = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(obj))
            }
            None => Ok(None),
        }
    }

    /// Put (create or update) an object.
    ///
    /// R32-FIX: Restricted to `pub(crate)` — during normal operation, objects
    /// must be updated via `prepare_block_batch` to maintain atomicity.
    /// Used directly only in tests; production path goes through prepare_block_batch.
    #[allow(dead_code)]
    pub(crate) fn put_object(&mut self, object: &Object) -> Result<(), StorageError> {
        let data =
            serde_json::to_vec(object).map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.put_object(&object.id.0, &data)?;

        // Update JMT with domain-separated key: byte 0 = 0x02 (object).
        let jmt_key = object_jmt_key(&object.id);
        self.jmt.put(&jmt_key, &data);

        Ok(())
    }

    /// Delete an object.
    #[allow(dead_code)]
    pub(crate) fn delete_object(&mut self, id: &ObjectId) -> Result<(), StorageError> {
        self.db.delete_object(&id.0)?;
        self.jmt.delete(&object_jmt_key(id));
        Ok(())
    }

    /// Store a block.
    pub fn put_block(&self, height: u64, block: &pichain_types::Block) -> Result<(), StorageError> {
        let data =
            serde_json::to_vec(block).map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.put_block(height, &data)?;
        Ok(())
    }

    /// Get a block by height.
    pub fn get_block(&self, height: u64) -> Result<Option<pichain_types::Block>, StorageError> {
        match self.db.get_block(height)? {
            Some(data) => {
                let block: pichain_types::Block = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(block))
            }
            None => Ok(None),
        }
    }

    /// Store a transaction by hash.
    pub fn put_transaction(
        &self,
        tx_hash: &pichain_crypto::Hash,
        tx: &pichain_types::SignedTransaction,
    ) -> Result<(), StorageError> {
        let data =
            serde_json::to_vec(tx).map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.put_transaction(tx_hash.as_bytes(), &data)?;
        Ok(())
    }

    /// Get a transaction by hash.
    pub fn get_transaction(
        &self,
        tx_hash: &pichain_crypto::Hash,
    ) -> Result<Option<pichain_types::SignedTransaction>, StorageError> {
        match self.db.get_transaction(tx_hash.as_bytes())? {
            Some(data) => {
                let tx = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(tx))
            }
            None => Ok(None),
        }
    }

    /// Store a transaction receipt.
    pub fn put_receipt(
        &self,
        tx_hash: &pichain_crypto::Hash,
        effect: &pichain_types::TransactionEffect,
    ) -> Result<(), StorageError> {
        let data =
            serde_json::to_vec(effect).map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.put_receipt(tx_hash.as_bytes(), &data)?;
        Ok(())
    }

    /// Get a transaction receipt.
    pub fn get_receipt(
        &self,
        tx_hash: &pichain_crypto::Hash,
    ) -> Result<Option<pichain_types::TransactionEffect>, StorageError> {
        match self.db.get_receipt(tx_hash.as_bytes())? {
            Some(data) => {
                let effect = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(effect))
            }
            None => Ok(None),
        }
    }

    /// Store metadata.
    pub fn put_metadata(&self, key: &[u8], value: &[u8]) -> Result<(), StorageError> {
        self.db.put_metadata(key, value)
    }

    /// Get metadata.
    pub fn get_metadata(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError> {
        self.db.get_metadata(key)
    }

    /// Get the latest block height from metadata.
    ///
    /// Returns 0 if no `latest_height` key is found, which is expected on
    /// first startup (genesis). On restart, callers should cross-check this
    /// value against the highest persisted block header to detect corruption.
    pub fn latest_height(&self) -> Result<u64, StorageError> {
        match self.get_metadata(b"latest_height")? {
            Some(bytes) => {
                let arr: [u8; 8] = bytes.try_into().map_err(|_| {
                    StorageError::Serialization("latest_height is not 8 bytes".into())
                })?;
                Ok(u64::from_be_bytes(arr))
            }
            // No key found — either first startup (genesis) or metadata was lost.
            // Callers in the restart path should log a warning if this returns 0
            // but blocks already exist on disk (indicates metadata corruption).
            _ => Ok(0),
        }
    }

    /// Set the latest block height in metadata.
    pub fn set_latest_height(&self, height: u64) -> Result<(), StorageError> {
        self.put_metadata(b"latest_height", &height.to_be_bytes())
    }

    /// Get raw state data by key.
    pub fn get_state_raw(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError> {
        self.db.get_state(key)
    }

    /// Put raw state data by key.
    pub fn put_state_raw(&self, key: &[u8], value: &[u8]) -> Result<(), StorageError> {
        self.db.put_state(key, value)
    }

    /// Delete raw state data by key.
    pub fn delete_state_raw(&self, key: &[u8]) -> Result<(), StorageError> {
        self.db.delete_state(key)
    }

    /// Access the underlying database.
    pub fn db(&self) -> &PiChainDB {
        &self.db
    }

    /// Access JMT for state root computation.
    pub fn jmt(&self) -> &JellyfishMerkleTree {
        &self.jmt
    }

    /// Generate a JMT inclusion/exclusion proof for an account.
    /// Returns the serialized proof bytes, or None if JMT is empty.
    pub fn get_account_proof(&self, address: &Address) -> Option<Vec<u8>> {
        let jmt_key = account_jmt_key(address);
        let proof = self.jmt.generate_proof(&jmt_key);
        serde_json::to_vec(&proof).ok()
    }

    /// Rebuild the in-memory JMT from all persisted account and object state.
    /// Must be called on startup after resume_from_storage() to restore
    /// correct state roots. Without this, the JMT starts empty and produces
    /// wrong state roots until the next block is produced.
    ///
    /// SECURITY (Fix 192): Both accounts AND objects are stored in the JMT
    /// during normal operation (see `put_account` and `put_object`). If we
    /// only rebuild accounts, post-crash state roots diverge from pre-crash
    /// state roots, causing consensus failures.
    pub fn rebuild_jmt(&mut self) -> Result<usize, StorageError> {
        let entries = self.db.scan_state_prefix(b"a")?;
        let mut count = 0;
        for (key_suffix, data) in &entries {
            if key_suffix.len() == 20 {
                // Domain-separated: byte 0 = 0x01 (account)
                let addr = Address(key_suffix[..20].try_into().expect("length checked"));
                let jmt_key = account_jmt_key(&addr);
                self.jmt.put(&jmt_key, data);
                count += 1;
            }
        }

        // Also rebuild JMT entries for objects (Fix 192).
        // Objects are stored in a separate column family (CF_OBJECTS) with
        // 32-byte keys (ObjectId). During normal operation, `put_object()`
        // inserts both into CF_OBJECTS and the JMT. Without this scan,
        // objects are lost from the JMT after a crash+restart.
        let obj_entries = self.db.scan_all_objects()?;
        for (key, data) in &obj_entries {
            if key.len() == 32 {
                // Domain-separated: byte 0 = 0x02 (object)
                let obj_id = ObjectId(key.as_slice().try_into().expect("length checked"));
                let jmt_key = object_jmt_key(&obj_id);
                self.jmt.put(&jmt_key, data);
                count += 1;
            }
        }

        // Store the rebuilt JMT root in metadata for crash-recovery verification.
        // On next startup we can compare this against the root hash stored in the
        // latest block header to detect state corruption.
        let root_bytes = self.jmt.root_hash().as_bytes().to_vec();
        self.db.put_metadata(b"jmt_root", &root_bytes)?;

        Ok(count)
    }

    /// Look up which block height contains a given transaction.
    pub fn get_tx_block_height(
        &self,
        tx_hash: &pichain_crypto::Hash,
    ) -> Result<Option<u64>, StorageError> {
        self.db.get_tx_block_height(tx_hash.as_bytes())
    }

    /// Atomically persist a block, its transactions, receipts, state changes, and height.
    /// Uses a single WAL-synced WriteBatch so a crash mid-write cannot leave partial state.
    /// Returns the post-execution state root (from JMT updates).
    pub fn persist_block_atomic(
        &mut self,
        height: u64,
        block: &pichain_types::Block,
        txs_and_receipts: &[(
            pichain_crypto::Hash,
            &pichain_types::SignedTransaction,
            Option<&pichain_types::TransactionEffect>,
        )],
        state_changes: &[(
            pichain_crypto::keys::Address,
            &pichain_types::account::AccountState,
        )],
    ) -> Result<pichain_crypto::poseidon::PoseidonHash, StorageError> {
        let (batch, state_root, pending_jmt_updates) =
            self.prepare_block_batch(height, block, txs_and_receipts, state_changes, &[])?;
        self.commit_block_batch(batch, pending_jmt_updates, height)?;
        Ok(state_root)
    }

    /// Build a WriteBatch for block persistence without writing it.
    /// The caller can add additional entries (e.g., sub-executor state) before
    /// calling `commit_block_batch()` to write everything atomically.
    ///
    /// `object_changes` allows object writes to be included in the same atomic
    /// WriteBatch as account state changes, block data, and metadata (H6).
    /// Each entry is an `(ObjectId, serialized_bytes)` pair. Pass an empty
    /// slice if there are no object changes for this block.
    ///
    /// Returns `(batch, state_root, pending_jmt_updates)`. The JMT updates are
    /// NOT applied to `self.jmt` here — they are deferred until
    /// `commit_block_batch()` succeeds, so a failed batch leaves the in-memory
    /// JMT unchanged (Fix STOR-223).
    pub fn prepare_block_batch(
        &mut self,
        height: u64,
        block: &pichain_types::Block,
        txs_and_receipts: &[(
            pichain_crypto::Hash,
            &pichain_types::SignedTransaction,
            Option<&pichain_types::TransactionEffect>,
        )],
        state_changes: &[(
            pichain_crypto::keys::Address,
            &pichain_types::account::AccountState,
        )],
        object_changes: &[(pichain_types::ObjectId, &[u8])],
    ) -> PrepareResult {
        let mut batch = self.db.new_batch();

        // 1. State changes (accounts) — compute JMT updates in a temporary tree
        //    clone to get the correct state root WITHOUT mutating self.jmt (Fix STOR-223).
        let mut pending_jmt_updates: Vec<([u8; 32], Vec<u8>)> = Vec::new();
        let mut staging_jmt = self.jmt.clone();
        for (addr, state) in state_changes {
            let key = address_key(addr);
            let data = serde_json::to_vec(state)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            self.db.batch_put_state(&mut batch, &key, &data);

            // Stage the JMT update (applied only after commit succeeds)
            // Domain-separated: byte 0 = 0x01 (account), bytes 1..21 = address
            let jmt_key = account_jmt_key(addr);
            staging_jmt.put(&jmt_key, &data);
            pending_jmt_updates.push((jmt_key, data));
        }

        // 1b. Object changes (H6) — include in the same atomic WriteBatch
        //     so that object state cannot diverge from account state after a crash.
        for (obj_id, data) in object_changes {
            self.db.batch_put_object(&mut batch, &obj_id.0, data);

            // Stage the JMT update for the object
            let jmt_key = object_jmt_key(obj_id);
            staging_jmt.put(&jmt_key, data);
            pending_jmt_updates.push((jmt_key, data.to_vec()));
        }

        // 2. Get the post-execution state root from the staging JMT
        let state_root = staging_jmt.root_hash();
        let mut corrected_block = block.clone();
        corrected_block.header.state_root = state_root;

        // 3. Block (with corrected state root)
        let block_data = serde_json::to_vec(&corrected_block)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.batch_put_block(&mut batch, height, &block_data)?;

        // 4. Transactions + receipts + tx→height index
        for (tx_hash, tx, receipt_opt) in txs_and_receipts {
            let tx_data =
                serde_json::to_vec(tx).map_err(|e| StorageError::Serialization(e.to_string()))?;
            self.db
                .batch_put_transaction(&mut batch, tx_hash.as_bytes(), &tx_data);
            self.db
                .batch_put_tx_height(&mut batch, tx_hash.as_bytes(), height);

            if let Some(receipt) = receipt_opt {
                let receipt_data = serde_json::to_vec(receipt)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                self.db
                    .batch_put_receipt(&mut batch, tx_hash.as_bytes(), &receipt_data);
            }
        }

        // 5. Latest height metadata
        self.db
            .batch_put_metadata(&mut batch, b"latest_height", &height.to_be_bytes());

        // 6. JMT root hash — written atomically with the block data (Fix STOR-220).
        //    Previously this was a separate put_metadata after write_batch_sync,
        //    meaning a crash between the two left persisted state without the root.
        let root_bytes = state_root.as_bytes().to_vec();
        self.db
            .batch_put_metadata(&mut batch, b"jmt_root", &root_bytes);

        Ok((batch, state_root, pending_jmt_updates))
    }

    /// Commit a prepared block batch with WAL sync.
    /// Also runs periodic JMT garbage collection.
    ///
    /// `pending_jmt_updates` are applied to `self.jmt` ONLY after the batch
    /// write succeeds. If the write fails, the in-memory JMT remains unchanged
    /// (Fix STOR-223).
    pub fn commit_block_batch(
        &mut self,
        batch: rocksdb::WriteBatch,
        pending_jmt_updates: Vec<([u8; 32], Vec<u8>)>,
        height: u64,
    ) -> Result<(), StorageError> {
        // GC stale JMT nodes BEFORE the atomic write, so that a crash after the write
        // leaves the JMT with at-worst extra stale nodes (harmless and cleaned next GC),
        // rather than missing nodes that would cause state root mismatch.
        if height.is_multiple_of(10) {
            self.jmt.gc_stale_nodes();
        }

        // Atomic + synced write.
        // The JMT root is included in the batch (see prepare_block_batch step 6)
        // so it is persisted atomically with the block data (Fix STOR-220).
        self.db.write_batch_sync(batch)?;

        // Only now apply JMT updates to the in-memory tree (Fix STOR-223).
        // If write_batch_sync failed above, we return early and the JMT is untouched.
        for (jmt_key, data) in pending_jmt_updates {
            self.jmt.put(&jmt_key, &data);
        }

        Ok(())
    }

    // --- Wallet activation tracking ---

    /// Check if an address has already been activated (received locked PI grant).
    pub fn is_wallet_activated(&self, address: &Address) -> Result<bool, StorageError> {
        let key = activation_key(address);
        Ok(self.db.get_state(&key)?.is_some())
    }

    /// Mark an address as activated and increment the global counter.
    /// Uses a WriteBatch for atomicity — both writes succeed or neither does.
    pub fn mark_wallet_activated(&mut self, address: &Address) -> Result<(), StorageError> {
        let key = activation_key(address);
        let count = self.activation_count()?.saturating_add(1);
        let mut batch = rocksdb::WriteBatch::default();
        self.db.batch_put_state(&mut batch, &key, &[1]);
        self.db
            .batch_put_state(&mut batch, ACTIVATION_COUNT_KEY, &count.to_le_bytes());
        self.db.write_batch_sync(batch)?;
        Ok(())
    }

    /// Get the total number of activated wallets.
    pub fn activation_count(&self) -> Result<u64, StorageError> {
        match self.db.get_state(ACTIVATION_COUNT_KEY)? {
            Some(bytes) if bytes.len() == 8 => Ok(u64::from_le_bytes(bytes.try_into().unwrap())),
            _ => Ok(0),
        }
    }
}

/// RocksDB key for the global wallet activation counter.
const ACTIVATION_COUNT_KEY: &[u8] = b"wcount";

/// Create a RocksDB key for wallet activation tracking.
/// Prefix 'w' + 20-byte address.
fn activation_key(address: &Address) -> Vec<u8> {
    let mut key = Vec::with_capacity(21);
    key.push(b'w');
    key.extend_from_slice(&address.0);
    key
}

/// Create a RocksDB key for an account address.
fn address_key(address: &Address) -> Vec<u8> {
    let mut key = Vec::with_capacity(21);
    key.push(b'a'); // prefix for account namespace
    key.extend_from_slice(&address.0);
    key
}

/// Domain-separated JMT key for accounts: byte 0 = 0x01, bytes 1..21 = address, rest = 0.
/// This prevents collisions with object keys (prefix 0x02) in the same JMT namespace.
fn account_jmt_key(address: &Address) -> [u8; 32] {
    let mut key = [0u8; 32];
    key[0] = 0x01; // Account domain prefix
    key[1..21].copy_from_slice(&address.0);
    key
}

/// Domain-separated JMT key for objects: byte 0 = 0x02, bytes 1..32 = hash(object_id).
/// Uses a hash of the ObjectId to fit the 31 remaining bytes without truncation.
fn object_jmt_key(id: &ObjectId) -> [u8; 32] {
    let hash = pichain_crypto::hash(&id.0);
    let mut key = [0u8; 32];
    key[0] = 0x02; // Object domain prefix
    key[1..32].copy_from_slice(&hash.as_bytes()[..31]);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_roundtrip() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let mut store = StateStore::new(db);

        let addr = Address([1u8; 20]);
        let account = Account::with_balance(addr, 1_000_000_000);

        store.put_account(&account).unwrap();
        let loaded = store.get_account(&addr).unwrap().unwrap();
        assert_eq!(loaded.state.balance, 1_000_000_000);
    }

    #[test]
    fn missing_account_returns_none() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let store = StateStore::new(db);
        let addr = Address([99u8; 20]);
        assert!(store.get_account(&addr).unwrap().is_none());
    }

    #[test]
    fn state_root_changes() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let mut store = StateStore::new(db);
        let root0 = store.state_root();

        let account = Account::with_balance(Address([1u8; 20]), 100);
        store.put_account(&account).unwrap();
        let root1 = store.state_root();
        assert_ne!(root0, root1);
    }

    #[test]
    fn jmt_rebuild_restores_state_root() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let mut store = StateStore::new(db);

        // Insert several accounts
        let a1 = Account::with_balance(Address([1u8; 20]), 100);
        let a2 = Account::with_balance(Address([2u8; 20]), 200);
        let a3 = Account::with_balance(Address([3u8; 20]), 300);
        store.put_account(&a1).unwrap();
        store.put_account(&a2).unwrap();
        store.put_account(&a3).unwrap();

        let original_root = store.state_root();
        assert_ne!(original_root, pichain_crypto::poseidon::PoseidonHash::ZERO);

        // Simulate restart: create new StateStore from same DB (fresh empty JMT)
        let db2 = store.db; // reuse the DB
        let mut store2 = StateStore {
            db: db2,
            jmt: JellyfishMerkleTree::new(),
        };

        // JMT is empty after "restart"
        assert_eq!(
            store2.state_root(),
            pichain_crypto::poseidon::PoseidonHash::ZERO
        );

        // Rebuild JMT
        let count = store2.rebuild_jmt().unwrap();
        assert_eq!(count, 3);

        // State root should match original
        assert_eq!(store2.state_root(), original_root);
    }

    #[test]
    fn jmt_rebuild_includes_objects() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let mut store = StateStore::new(db);

        // Insert accounts AND objects
        let a1 = Account::with_balance(Address([1u8; 20]), 100);
        store.put_account(&a1).unwrap();

        let id1 = ObjectId([42u8; 32]);
        let obj1 = Object::new_coin(id1, Address([1u8; 20]), 500);
        store.put_object(&obj1).unwrap();

        let id2 = ObjectId([43u8; 32]);
        let obj2 = Object::new_coin(id2, Address([2u8; 20]), 300);
        store.put_object(&obj2).unwrap();

        let original_root = store.state_root();
        assert_ne!(original_root, pichain_crypto::poseidon::PoseidonHash::ZERO);

        // Simulate restart: create new StateStore from same DB (fresh empty JMT)
        let db2 = store.db;
        let mut store2 = StateStore {
            db: db2,
            jmt: JellyfishMerkleTree::new(),
        };

        // JMT is empty after "restart"
        assert_eq!(
            store2.state_root(),
            pichain_crypto::poseidon::PoseidonHash::ZERO
        );

        // Rebuild JMT — should include 1 account + 2 objects = 3 entries
        let count = store2.rebuild_jmt().unwrap();
        assert_eq!(count, 3, "rebuild should include accounts AND objects");

        // State root should match original (Fix 192: previously diverged
        // because objects were excluded from rebuild)
        assert_eq!(store2.state_root(), original_root);
    }

    #[test]
    fn object_roundtrip() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let mut store = StateStore::new(db);

        let id = ObjectId([42u8; 32]);
        let obj = Object::new_coin(id, Address([1u8; 20]), 500);

        store.put_object(&obj).unwrap();
        let loaded = store.get_object(&id).unwrap().unwrap();
        assert_eq!(loaded.coin_amount(), Some(500));
    }

    /// STOR-220: The JMT root must be written inside the same WriteBatch
    /// as the block data, so a crash cannot leave them inconsistent.
    #[test]
    fn jmt_root_persisted_atomically_with_block() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let mut store = StateStore::new(db);

        let addr = Address([1u8; 20]);
        let mut state = pichain_types::account::AccountState::new();
        state.balance = 1_000_000;

        let block = pichain_types::Block::genesis(31415, 1000);
        let (batch, state_root, pending_jmt) = store
            .prepare_block_batch(1, &block, &[], &[(addr, &state)], &[])
            .unwrap();

        // Before commit — JMT root should not yet be in metadata
        // (the batch hasn't been written)
        let pre_root = store.db.get_metadata(b"jmt_root").unwrap();
        assert!(
            pre_root.is_none(),
            "jmt_root should not be persisted before commit"
        );

        // Commit the batch
        store.commit_block_batch(batch, pending_jmt, 1).unwrap();

        // After commit — JMT root should be in metadata
        let persisted = store
            .db
            .get_metadata(b"jmt_root")
            .unwrap()
            .expect("jmt_root must be persisted after commit");
        assert_eq!(
            persisted,
            state_root.as_bytes().to_vec(),
            "persisted JMT root must match the state root from prepare_block_batch"
        );
    }

    /// STOR-223: If commit_block_batch fails, the in-memory JMT must remain unchanged.
    /// We test this by verifying that prepare_block_batch does NOT mutate self.jmt.
    #[test]
    fn jmt_not_updated_on_batch_failure() {
        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let mut store = StateStore::new(db);

        // Insert an initial account to get a non-zero root
        let a0 = Account::with_balance(Address([0u8; 20]), 100);
        store.put_account(&a0).unwrap();
        let root_before = store.state_root();

        // Prepare a batch with a new state change
        let addr = Address([1u8; 20]);
        let mut state = pichain_types::account::AccountState::new();
        state.balance = 999_999;
        state.nonce = 5;
        let block = pichain_types::Block::genesis(31415, 1000);
        let (_batch, new_state_root, _pending_jmt) = store
            .prepare_block_batch(1, &block, &[], &[(addr, &state)], &[])
            .unwrap();

        // The returned state_root should differ from root_before
        assert_ne!(
            new_state_root, root_before,
            "the proposed state root should reflect the new state change"
        );

        // But the in-memory JMT must NOT have been updated yet
        assert_eq!(
            store.state_root(),
            root_before,
            "prepare_block_batch must not mutate the in-memory JMT (Fix STOR-223)"
        );

        // If we intentionally do NOT call commit_block_batch, the JMT stays clean
        assert_eq!(
            store.state_root(),
            root_before,
            "JMT remains unchanged when batch is never committed"
        );
    }

    /// Integration test: sub-executor state (tokens, DEX, NFTs, launchpad, contracts)
    /// is persisted atomically via a WriteBatch and can be read back from individual stores.
    /// Validates the batch_put_* / get_* roundtrip path used by node_state.rs to persist
    /// block execution results.
    #[test]
    fn sub_executor_state_persisted_atomically() {
        use pichain_types::dex::LiquidityPool;
        use pichain_types::launchpad::{LaunchId, LaunchState, LaunchType, TokenLaunch};
        use pichain_types::nft::{CollectionId, Nft, NftAttribute, NftCollection, NftId};
        use pichain_types::token::{MintId, TokenAccount, TokenMint};
        use std::collections::HashMap;

        let (db, _temp_dir) = PiChainDB::open_temp().unwrap();
        let mut batch = db.new_batch();

        let creator = Address([1u8; 20]);
        let owner = Address([2u8; 20]);

        // --- Token mint ---
        let mint_id = MintId::derive(&creator, 0);
        let mint = TokenMint::new(
            mint_id,
            "TestCoin".to_string(),
            "TST".to_string(),
            9,
            1_000_000_000,
            creator,
            String::new(),
            0,
        );
        let token_store = crate::TokenStore::new(&db);
        token_store.batch_put_mint(&mut batch, &mint).unwrap();

        // --- Token account ---
        let mut token_account = TokenAccount::new(owner, mint_id);
        token_account.balance = 42_000;
        token_store
            .batch_put_token_account(&mut batch, &token_account)
            .unwrap();

        // --- Liquidity pool ---
        let mint_b = MintId::derive(&creator, 1);
        let mut pool = LiquidityPool::new(MintId::ZERO, mint_b, creator, 0);
        pool.reserve_a = 1_000_000;
        pool.reserve_b = 2_000_000;
        pool.lp_supply = 1_414_213;
        let pool_id = pool.id;
        let dex_store = crate::DexStore::new(&db);
        dex_store.batch_put_pool(&mut batch, &pool).unwrap();

        // --- LP balance ---
        dex_store
            .batch_put_lp_balance(&mut batch, &pool_id, &owner, 999_000)
            .unwrap();

        // --- NFT collection ---
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
        let nft_store = crate::NftStore::new(&db);
        nft_store
            .batch_put_collection(&mut batch, &collection)
            .unwrap();

        // --- NFT ---
        let nft_id = NftId::derive(&collection_id, 0);
        let nft = Nft {
            id: nft_id,
            collection: collection_id,
            index: 0,
            owner,
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
        nft_store.batch_put_nft(&mut batch, &nft).unwrap();

        // --- Token launch ---
        let launch_mint = MintId::derive(&creator, 2);
        let launch_id = LaunchId::from_mint(&launch_mint);
        let launch = TokenLaunch {
            id: launch_id,
            mint: launch_mint,
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
            token_decimals: 0,
        };
        let launchpad_store = crate::LaunchpadStore::new(&db);
        launchpad_store
            .batch_put_launch(&mut batch, &launch)
            .unwrap();

        // --- Contract storage ---
        let contract_addr = Address([3u8; 20]);
        let storage_key = b"counter";
        let storage_value = 42u64.to_le_bytes();
        let contract_store = crate::ContractStorageStore::new(&db);
        contract_store.batch_put(&mut batch, &contract_addr, storage_key, &storage_value);

        // --- Atomic commit ---
        db.write_batch_sync(batch).unwrap();

        // --- Read back and verify each entity ---

        // Token mint
        let loaded_mint = token_store
            .get_mint(&mint_id)
            .unwrap()
            .expect("token mint should be persisted");
        assert_eq!(loaded_mint.name, "TestCoin");
        assert_eq!(loaded_mint.symbol, "TST");
        assert_eq!(loaded_mint.decimals, 9);
        assert_eq!(loaded_mint.max_supply, 1_000_000_000);

        // Token account
        let loaded_account = token_store
            .get_token_account(&owner, &mint_id)
            .unwrap()
            .expect("token account should be persisted");
        assert_eq!(loaded_account.balance, 42_000);
        assert_eq!(loaded_account.owner, owner);

        // Liquidity pool
        let loaded_pool = dex_store
            .get_pool(&pool_id)
            .unwrap()
            .expect("liquidity pool should be persisted");
        assert_eq!(loaded_pool.reserve_a, 1_000_000);
        assert_eq!(loaded_pool.reserve_b, 2_000_000);
        assert_eq!(loaded_pool.lp_supply, 1_414_213);

        // LP balance
        let loaded_lp = dex_store.get_lp_balance(&pool_id, &owner).unwrap();
        assert_eq!(loaded_lp, 999_000);

        // NFT collection
        let loaded_collection = nft_store
            .get_collection(&collection_id)
            .unwrap()
            .expect("NFT collection should be persisted");
        assert_eq!(loaded_collection.name, "TestNFTs");
        assert_eq!(loaded_collection.max_supply, 100);
        assert_eq!(loaded_collection.royalty_bps, 500);

        // NFT
        let loaded_nft = nft_store
            .get_nft(&nft_id)
            .unwrap()
            .expect("NFT should be persisted");
        assert_eq!(loaded_nft.name, "Test NFT #0");
        assert_eq!(loaded_nft.owner, owner);
        assert!(loaded_nft.listed);
        assert_eq!(loaded_nft.listed_price, 1_000_000_000);
        assert_eq!(loaded_nft.attributes.len(), 1);
        assert_eq!(loaded_nft.attributes[0].value, "Legendary");

        // Token launch
        let loaded_launch = launchpad_store
            .get_launch(&launch_id)
            .unwrap()
            .expect("token launch should be persisted");
        assert_eq!(loaded_launch.mint, launch_mint);
        assert_eq!(loaded_launch.tokens_for_sale, 1_000_000);
        assert_eq!(loaded_launch.state, LaunchState::Active);
        assert_eq!(loaded_launch.created_at_ms, 12345);

        // Contract storage
        let loaded_storage = contract_store
            .get(&contract_addr, storage_key)
            .unwrap()
            .expect("contract storage should be persisted");
        assert_eq!(loaded_storage, storage_value);
    }
}
