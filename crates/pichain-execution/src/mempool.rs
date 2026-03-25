//! Transaction mempool — manages pending transactions for block production.
//!
//! Features:
//! - Priority queue ordered by fee (max priority fee first)
//! - Per-sender nonce ordering (sequential within sender, parallel across senders)
//! - Eviction of low-fee transactions when pool is full
//! - Duplicate detection via tx hash
//! - Configurable size limits

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use parking_lot::RwLock;
use pichain_crypto::keys::Address;
use pichain_crypto::Hash;
use pichain_types::transaction::SignedTransaction;
use pichain_types::PiAmount;
use std::collections::{BTreeMap, HashMap};
use tracing::{debug, warn};

/// Configuration for the transaction mempool.
#[derive(Clone, Debug)]
pub struct MempoolConfig {
    /// Maximum number of transactions in the pool.
    pub max_transactions: usize,
    /// Maximum number of pending transactions per sender.
    pub max_per_sender: usize,
    /// Minimum base fee to accept a transaction.
    pub min_base_fee: PiAmount,
    /// Maximum transaction age in seconds before expiry.
    pub max_tx_age_secs: u64,
    /// Maximum transaction size in bytes (canonical encoding).
    /// Prevents DoS via oversized code/data fields. Default: 512KB.
    pub max_tx_size_bytes: usize,
    /// Expected chain_id — rejects transactions targeting a different chain.
    /// 0 means no chain_id enforcement (for tests).
    pub chain_id: u64,
}

impl Default for MempoolConfig {
    fn default() -> Self {
        Self {
            max_transactions: 100_000,
            max_per_sender: 1_000,
            min_base_fee: 1,               // 1 base unit minimum
            max_tx_age_secs: 300, // 5 minutes TTL — gives slow networks time to include txs
            max_tx_size_bytes: 512 * 1024, // 512KB max transaction size
            chain_id: 0,          // 0 = no enforcement (tests); set to real chain_id in production
        }
    }
}

/// A pending transaction in the mempool with its priority score.
#[derive(Clone, Debug)]
struct PendingTx {
    tx: SignedTransaction,
    priority_fee: PiAmount,
    inserted_at: std::time::Instant,
}

impl PendingTx {
    fn effective_priority(&self) -> PiAmount {
        self.priority_fee
    }
}

/// Per-sender transaction queue maintaining nonce ordering.
#[derive(Default)]
struct SenderQueue {
    /// Nonce → transaction, ordered by nonce.
    txs: BTreeMap<u64, Hash>,
    /// Next expected nonce (from on-chain state).
    next_nonce: u64,
}

impl SenderQueue {
    fn ready_count(&self) -> usize {
        let mut count = 0;
        let mut expected = self.next_nonce;
        for &nonce in self.txs.keys() {
            if nonce == expected {
                count += 1;
                expected += 1;
            } else {
                break;
            }
        }
        count
    }

    fn ready_hashes(&self) -> Vec<Hash> {
        let mut hashes = Vec::new();
        let mut expected = self.next_nonce;
        for (&nonce, hash) in &self.txs {
            if nonce == expected {
                hashes.push(*hash);
                expected += 1;
            } else {
                break;
            }
        }
        hashes
    }
}

/// Maximum transactions accepted per sender per second.
/// 25 tx/sec allows high-frequency dApps while preventing spam.
const MAX_SENDER_TX_PER_SECOND: u64 = 25;

/// Thread-safe transaction mempool.
pub struct TransactionPool {
    /// All transactions indexed by hash.
    transactions: DashMap<Hash, PendingTx>,
    /// Per-sender transaction queues.
    sender_queues: DashMap<Address, SenderQueue>,
    /// Set of known transaction hashes (for dedup).
    known_hashes: DashMap<Hash, ()>,
    /// Priority index: (priority_fee, tx_hash) for ordering.
    /// Protected by RwLock since BTreeMap isn't concurrent.
    priority_index: RwLock<BTreeMap<(PiAmount, Hash), ()>>,
    /// Per-sender rate limiting: (count_this_second, window_start).
    sender_rate: DashMap<Address, (u64, std::time::Instant)>,
    /// Configuration.
    config: MempoolConfig,
}

impl TransactionPool {
    /// Create a new transaction pool with default configuration.
    pub fn new() -> Self {
        Self::with_config(MempoolConfig::default())
    }

    /// Create a new transaction pool with custom configuration.
    pub fn with_config(config: MempoolConfig) -> Self {
        Self {
            transactions: DashMap::new(),
            sender_queues: DashMap::new(),
            known_hashes: DashMap::new(),
            priority_index: RwLock::new(BTreeMap::new()),
            sender_rate: DashMap::new(),
            config,
        }
    }

    /// Insert a transaction into the pool.
    /// Returns Ok(tx_hash) if accepted, Err if rejected.
    pub fn insert(&self, tx: SignedTransaction) -> Result<Hash, MempoolError> {
        // Signature verification — reject forged transactions at ingress
        if tx.verify().is_err() {
            return Err(MempoolError::InvalidSignature);
        }

        // Chain ID enforcement — reject transactions targeting a different chain.
        // chain_id=0 is explicitly disallowed in production to prevent cross-chain
        // replay attacks where the same signed tx is valid on multiple networks.
        if tx.data.chain_id == 0 {
            return Err(MempoolError::WrongChainId {
                expected: self.config.chain_id,
                got: 0,
            });
        }
        if self.config.chain_id != 0 && tx.data.chain_id != self.config.chain_id {
            return Err(MempoolError::WrongChainId {
                expected: self.config.chain_id,
                got: tx.data.chain_id,
            });
        }

        // Transaction size check — prevent DoS via oversized code/data fields
        let tx_size = tx.data.canonical_bytes().len();
        if tx_size > self.config.max_tx_size_bytes {
            return Err(MempoolError::TransactionTooLarge {
                size: tx_size,
                limit: self.config.max_tx_size_bytes,
            });
        }

        let tx_hash = tx.hash();

        // Atomic dedup check — prevents wasting CPU on duplicate signature verification
        match self.known_hashes.entry(tx_hash) {
            Entry::Occupied(_) => return Err(MempoolError::DuplicateTransaction(tx_hash)),
            Entry::Vacant(v) => {
                v.insert(());
            }
        }

        // Fee check
        if tx.data.max_base_fee < self.config.min_base_fee {
            return Err(MempoolError::FeeTooLow {
                provided: tx.data.max_base_fee,
                minimum: self.config.min_base_fee,
            });
        }

        // Per-sender limit check
        let sender = tx.data.sender;
        let sender_count = self
            .sender_queues
            .get(&sender)
            .map(|q| q.txs.len())
            .unwrap_or(0);
        if sender_count >= self.config.max_per_sender {
            return Err(MempoolError::SenderQueueFull {
                sender,
                limit: self.config.max_per_sender,
            });
        }

        // Per-sender rate limiting — prevent burst flooding
        {
            let now = std::time::Instant::now();
            let mut entry = self.sender_rate.entry(sender).or_insert((0, now));
            let (count, window_start) = entry.value_mut();
            if now.duration_since(*window_start).as_secs() >= 1 {
                *count = 0;
                *window_start = now;
            }
            if *count >= MAX_SENDER_TX_PER_SECOND {
                return Err(MempoolError::SenderQueueFull {
                    sender,
                    limit: MAX_SENDER_TX_PER_SECOND as usize,
                });
            }
            *count += 1;
        }

        // Pool size check — evict lowest priority if full
        if self.transactions.len() >= self.config.max_transactions {
            self.evict_lowest_priority();
        }

        let priority_fee = tx.data.max_priority_fee;
        let nonce = tx.data.nonce;

        // SECURITY: Reject transactions with excessive nonce gaps to prevent DoS.
        // An attacker can submit nonce 10000 to block all future transactions from
        // an address until the gap is filled. Cap the gap at 64 transactions ahead
        // of the sender's current next_nonce.
        const MAX_NONCE_GAP: u64 = 64;
        {
            let next_nonce = self
                .sender_queues
                .get(&sender)
                .map(|q| q.next_nonce)
                .unwrap_or(0);
            if nonce > next_nonce.saturating_add(MAX_NONCE_GAP) {
                // Remove from known_hashes since we're rejecting
                self.known_hashes.remove(&tx_hash);
                return Err(MempoolError::NonceTooFar {
                    nonce,
                    expected: next_nonce,
                    max_gap: MAX_NONCE_GAP,
                });
            }
        }

        let pending = PendingTx {
            tx,
            priority_fee,
            inserted_at: std::time::Instant::now(),
        };

        // Insert into all indices (known_hashes already reserved in dedup check above)
        self.transactions.insert(tx_hash, pending);

        // Update sender queue
        self.sender_queues
            .entry(sender)
            .or_default()
            .txs
            .insert(nonce, tx_hash);

        // Update priority index
        {
            let mut idx = self.priority_index.write();
            idx.insert((priority_fee, tx_hash), ());
        }

        debug!(%tx_hash, %sender, nonce, priority_fee, "transaction added to mempool");
        Ok(tx_hash)
    }

    /// Remove a transaction from the pool (e.g., after inclusion in a block).
    pub fn remove(&self, tx_hash: &Hash) -> Option<SignedTransaction> {
        let pending = self.transactions.remove(tx_hash)?;
        let (_, pending) = pending;

        let sender = pending.tx.data.sender;
        let nonce = pending.tx.data.nonce;

        // Remove from sender queue
        if let Some(mut queue) = self.sender_queues.get_mut(&sender) {
            queue.txs.remove(&nonce);
            if queue.txs.is_empty() {
                drop(queue);
                self.sender_queues.remove(&sender);
            }
        }

        // Remove from priority index
        {
            let mut idx = self.priority_index.write();
            idx.remove(&(pending.priority_fee, *tx_hash));
        }

        // Clean up known_hashes to prevent unbounded memory growth
        self.known_hashes.remove(tx_hash);

        Some(pending.tx)
    }

    /// Remove all transactions included in a block and update sender nonces.
    pub fn remove_committed(&self, tx_hashes: &[Hash], sender_nonces: &HashMap<Address, u64>) {
        for hash in tx_hashes {
            self.remove(hash);
        }

        // Update next expected nonces for senders.
        // Use or_default() to CREATE the SenderQueue if it was deleted by
        // claim_ready_transactions. Without this, the nonce update is silently
        // lost, causing the next transaction for this sender to appear "not ready"
        // (because set_sender_nonce reads a stale value from storage).
        for (sender, &new_nonce) in sender_nonces {
            let mut entry = self.sender_queues.entry(*sender).or_default();
            // Only advance, never go backwards — prevents stale updates
            if new_nonce > entry.next_nonce {
                entry.next_nonce = new_nonce;
            }
            // Remove any transactions with nonce < new_nonce (stale)
            let stale: Vec<u64> = entry
                .txs
                .keys()
                .take_while(|&&n| n < new_nonce)
                .copied()
                .collect();
            for nonce in stale {
                if let Some(hash) = entry.txs.remove(&nonce) {
                    let priority_fee = self.transactions.get(&hash).map(|p| p.priority_fee);
                    self.transactions.remove(&hash);
                    self.known_hashes.remove(&hash);
                    if let Some(fee) = priority_fee {
                        let mut idx = self.priority_index.write();
                        idx.remove(&(fee, hash));
                    }
                }
            }
        }
    }

    /// Atomically claim transactions for block production.
    /// Claimed transactions are removed from the ready set and cannot be
    /// double-claimed by a concurrent pipeline stage.
    pub fn claim_ready_transactions(&self, max: usize) -> Vec<SignedTransaction> {
        let txs = self.get_ready_transactions(max);
        if txs.is_empty() {
            return txs;
        }

        // Track the highest nonce claimed per sender so we can advance next_nonce.
        // Without this, the sender queue is deleted (when empty) but next_nonce
        // is never updated, causing future transactions at nonce N+1 to appear
        // "not ready" because the queue still expects nonce N (or 0 for a
        // freshly-created default queue).
        let mut sender_max_nonce: std::collections::HashMap<pichain_crypto::keys::Address, u64> =
            std::collections::HashMap::new();
        for tx in &txs {
            let entry = sender_max_nonce.entry(tx.data.sender).or_insert(0);
            *entry = (*entry).max(tx.data.nonce);
        }

        // Remove claimed transactions from the pool to prevent double-processing
        for tx in &txs {
            let hash = tx.hash();
            self.remove(&hash);
        }

        // Advance next_nonce for each sender past the claimed transactions.
        // Re-create the sender queue entry if remove() deleted it (queue was empty).
        for (sender, max_nonce) in sender_max_nonce {
            let new_next = max_nonce.saturating_add(1);
            let mut entry = self.sender_queues.entry(sender).or_default();
            entry.next_nonce = entry.next_nonce.max(new_next);
        }

        txs
    }

    /// Get transactions ready for inclusion in a block, ordered by priority fee.
    /// Only returns transactions with sequential nonces from the sender's current nonce.
    /// Expired transactions are lazily cleaned up during this call.
    pub fn get_ready_transactions(&self, max_count: usize) -> Vec<SignedTransaction> {
        // Lazily expire old transactions
        self.expire_old_transactions();

        // Collect all ready transactions across all senders
        let mut ready: Vec<(PiAmount, Hash)> = Vec::new();

        for entry in self.sender_queues.iter() {
            let queue = entry.value();
            for hash in queue.ready_hashes() {
                if let Some(pending) = self.transactions.get(&hash) {
                    ready.push((pending.effective_priority(), hash));
                }
            }
        }

        // Sort by priority fee (highest first)
        ready.sort_by(|a, b| b.0.cmp(&a.0));

        // Take up to max_count
        ready
            .into_iter()
            .take(max_count)
            .filter_map(|(_, hash)| self.transactions.get(&hash).map(|p| p.tx.clone()))
            .collect()
    }

    /// Update the expected nonce for a sender (from on-chain state).
    /// Uses max() to avoid overwriting a fresher nonce set by remove_committed
    /// with a stale value from storage that hasn't been persisted yet.
    pub fn set_sender_nonce(&self, sender: Address, nonce: u64) {
        let mut entry = self.sender_queues.entry(sender).or_default();
        entry.next_nonce = entry.next_nonce.max(nonce);
    }

    /// Get the number of transactions in the pool.
    pub fn len(&self) -> usize {
        self.transactions.len()
    }

    /// Check if the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    /// Get the number of pending transactions for a specific sender.
    pub fn sender_count(&self, sender: &Address) -> usize {
        self.sender_queues
            .get(sender)
            .map(|q| q.txs.len())
            .unwrap_or(0)
    }

    /// Check if a transaction is in the pool.
    pub fn contains(&self, tx_hash: &Hash) -> bool {
        self.transactions.contains_key(tx_hash)
    }

    /// Get a transaction by hash.
    pub fn get(&self, tx_hash: &Hash) -> Option<SignedTransaction> {
        self.transactions.get(tx_hash).map(|p| p.tx.clone())
    }

    /// Evict the lowest-priority transaction from the pool.
    fn evict_lowest_priority(&self) {
        let mut idx = self.priority_index.write();
        if let Some((&(priority, hash), _)) = idx.iter().next() {
            idx.remove(&(priority, hash));
            drop(idx);

            if let Some((_, pending)) = self.transactions.remove(&hash) {
                let sender = pending.tx.data.sender;
                let nonce = pending.tx.data.nonce;
                if let Some(mut queue) = self.sender_queues.get_mut(&sender) {
                    queue.txs.remove(&nonce);
                }
                self.known_hashes.remove(&hash);
                warn!(%hash, priority, "evicted low-priority transaction from mempool");
            }
        }
    }

    /// Expire transactions older than `max_tx_age_secs`.
    /// Returns the number of expired transactions removed.
    pub fn expire_old_transactions(&self) -> usize {
        let now = std::time::Instant::now();
        let max_age = std::time::Duration::from_secs(self.config.max_tx_age_secs);
        let mut expired_hashes = Vec::new();

        for entry in self.transactions.iter() {
            if now.duration_since(entry.value().inserted_at) > max_age {
                expired_hashes.push(*entry.key());
            }
        }

        let count = expired_hashes.len();
        for hash in &expired_hashes {
            self.remove(hash);
        }

        if count > 0 {
            debug!(expired = count, "expired old transactions from mempool");
        }
        count
    }

    /// Get pool statistics.
    pub fn stats(&self) -> MempoolStats {
        let tx_count = self.transactions.len();
        let sender_count = self.sender_queues.len();

        let mut total_ready = 0;
        for entry in self.sender_queues.iter() {
            total_ready += entry.value().ready_count();
        }

        MempoolStats {
            total_transactions: tx_count,
            ready_transactions: total_ready,
            unique_senders: sender_count,
            pool_utilization: tx_count as f64 / self.config.max_transactions as f64,
        }
    }
}

impl Default for TransactionPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Mempool statistics.
#[derive(Clone, Debug)]
pub struct MempoolStats {
    pub total_transactions: usize,
    pub ready_transactions: usize,
    pub unique_senders: usize,
    pub pool_utilization: f64,
}

/// Errors from mempool operations.
#[derive(Debug, thiserror::Error)]
pub enum MempoolError {
    #[error("duplicate transaction: {0}")]
    DuplicateTransaction(Hash),
    #[error("fee too low: provided {provided}, minimum {minimum}")]
    FeeTooLow {
        provided: PiAmount,
        minimum: PiAmount,
    },
    #[error("sender queue full for {sender}: limit {limit}")]
    SenderQueueFull { sender: Address, limit: usize },
    #[error("pool full")]
    PoolFull,
    #[error("invalid nonce")]
    InvalidNonce,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("transaction too large: {size} bytes, limit {limit}")]
    TransactionTooLarge { size: usize, limit: usize },
    #[error("wrong chain_id: expected {expected}, got {got}")]
    WrongChainId { expected: u64, got: u64 },
    #[error("nonce too far ahead: nonce {nonce}, expected ~{expected}, max gap {max_gap}")]
    NonceTooFar {
        nonce: u64,
        expected: u64,
        max_gap: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::PqKeypair;
    use pichain_types::transaction::Transaction;

    fn test_config() -> MempoolConfig {
        MempoolConfig {
            chain_id: 31415,
            ..Default::default()
        }
    }

    fn make_tx(sender: &PqKeypair, nonce: u64, priority_fee: PiAmount) -> SignedTransaction {
        let recipient = PqKeypair::generate();
        let mut data = Transaction::transfer(
            sender.address(),
            nonce,
            recipient.address(),
            1_000_000, // 0.001 PI
            31415,
        );
        data.max_priority_fee = priority_fee;
        Transaction::sign_pq(data, sender)
    }

    #[test]
    fn insert_and_retrieve() {
        let pool = TransactionPool::with_config(test_config());
        let sender = PqKeypair::generate();
        let tx = make_tx(&sender, 0, 100);
        let hash = tx.hash();

        pool.insert(tx).unwrap();
        assert_eq!(pool.len(), 1);
        assert!(pool.contains(&hash));
    }

    #[test]
    fn duplicate_rejected() {
        let pool = TransactionPool::with_config(test_config());
        let sender = PqKeypair::generate();
        let tx = make_tx(&sender, 0, 100);

        pool.insert(tx.clone()).unwrap();
        assert!(pool.insert(tx).is_err());
    }

    #[test]
    fn ready_transactions_ordered_by_priority() {
        let pool = TransactionPool::with_config(test_config());

        let s1 = PqKeypair::generate();
        let s2 = PqKeypair::generate();
        let s3 = PqKeypair::generate();

        // Insert with different priorities
        pool.insert(make_tx(&s1, 0, 10)).unwrap();
        pool.insert(make_tx(&s2, 0, 100)).unwrap();
        pool.insert(make_tx(&s3, 0, 50)).unwrap();

        let ready = pool.get_ready_transactions(10);
        assert_eq!(ready.len(), 3);
        // Highest priority first
        assert_eq!(ready[0].data.max_priority_fee, 100);
        assert_eq!(ready[1].data.max_priority_fee, 50);
        assert_eq!(ready[2].data.max_priority_fee, 10);
    }

    #[test]
    fn nonce_ordering_within_sender() {
        let pool = TransactionPool::with_config(test_config());
        let sender = PqKeypair::generate();

        // Insert nonces out of order
        pool.insert(make_tx(&sender, 2, 100)).unwrap();
        pool.insert(make_tx(&sender, 0, 100)).unwrap();
        pool.insert(make_tx(&sender, 1, 100)).unwrap();

        // All three should be ready (sequential from 0)
        let ready = pool.get_ready_transactions(10);
        assert_eq!(ready.len(), 3);
    }

    #[test]
    fn gap_in_nonces_blocks_later_txs() {
        let pool = TransactionPool::with_config(test_config());
        let sender = PqKeypair::generate();

        // Insert nonce 0 and 2 (gap at 1)
        pool.insert(make_tx(&sender, 0, 100)).unwrap();
        pool.insert(make_tx(&sender, 2, 100)).unwrap();

        // Only nonce 0 should be ready
        let ready = pool.get_ready_transactions(10);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].data.nonce, 0);
    }

    #[test]
    fn remove_transaction() {
        let pool = TransactionPool::with_config(test_config());
        let sender = PqKeypair::generate();
        let tx = make_tx(&sender, 0, 100);
        let hash = tx.hash();

        pool.insert(tx).unwrap();
        assert_eq!(pool.len(), 1);

        pool.remove(&hash);
        assert_eq!(pool.len(), 0);
        assert!(!pool.contains(&hash));
    }

    #[test]
    fn per_sender_limit() {
        let config = MempoolConfig {
            max_per_sender: 3,
            ..test_config()
        };
        let pool = TransactionPool::with_config(config);
        let sender = PqKeypair::generate();

        pool.insert(make_tx(&sender, 0, 100)).unwrap();
        pool.insert(make_tx(&sender, 1, 100)).unwrap();
        pool.insert(make_tx(&sender, 2, 100)).unwrap();

        // Fourth should be rejected
        assert!(pool.insert(make_tx(&sender, 3, 100)).is_err());
    }

    #[test]
    fn stats() {
        let pool = TransactionPool::with_config(test_config());
        let s1 = PqKeypair::generate();
        let s2 = PqKeypair::generate();

        pool.insert(make_tx(&s1, 0, 100)).unwrap();
        pool.insert(make_tx(&s1, 1, 50)).unwrap();
        pool.insert(make_tx(&s2, 0, 75)).unwrap();

        let stats = pool.stats();
        assert_eq!(stats.total_transactions, 3);
        assert_eq!(stats.unique_senders, 2);
        assert_eq!(stats.ready_transactions, 3);
    }

    #[test]
    fn eviction_removes_lowest_priority() {
        let config = MempoolConfig {
            max_transactions: 3,
            max_per_sender: 10,
            min_base_fee: 1,
            ..test_config()
        };
        let pool = TransactionPool::with_config(config);

        let s1 = PqKeypair::generate();
        let s2 = PqKeypair::generate();
        let s3 = PqKeypair::generate();
        let s4 = PqKeypair::generate();

        // Fill pool with 3 txs at different priorities
        pool.insert(make_tx(&s1, 0, 10)).unwrap(); // lowest priority
        pool.insert(make_tx(&s2, 0, 50)).unwrap();
        pool.insert(make_tx(&s3, 0, 100)).unwrap();

        // Pool is full (3/3), inserting a 4th should evict the lowest (priority=10)
        pool.insert(make_tx(&s4, 0, 75)).unwrap();
        assert_eq!(pool.len(), 3);

        // The lowest priority (s1, priority=10) should have been evicted
        assert_eq!(pool.sender_count(&s1.address()), 0);
        // Higher priority txs should remain
        assert_eq!(pool.sender_count(&s2.address()), 1);
        assert_eq!(pool.sender_count(&s3.address()), 1);
        assert_eq!(pool.sender_count(&s4.address()), 1);
    }

    #[test]
    fn expire_old_transactions() {
        let config = MempoolConfig {
            max_tx_age_secs: 0, // Expire immediately
            ..test_config()
        };
        let pool = TransactionPool::with_config(config);
        let sender = PqKeypair::generate();

        pool.insert(make_tx(&sender, 0, 100)).unwrap();
        pool.insert(make_tx(&sender, 1, 50)).unwrap();
        assert_eq!(pool.len(), 2);

        // Wait a tiny bit for the 0-second TTL to take effect
        std::thread::sleep(std::time::Duration::from_millis(10));

        let expired = pool.expire_old_transactions();
        assert_eq!(expired, 2);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn get_ready_expires_old_txs() {
        let config = MempoolConfig {
            max_tx_age_secs: 0,
            ..test_config()
        };
        let pool = TransactionPool::with_config(config);
        let sender = PqKeypair::generate();

        pool.insert(make_tx(&sender, 0, 100)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));

        // get_ready_transactions should return empty after expiry
        let ready = pool.get_ready_transactions(10);
        assert!(ready.is_empty());
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn invalid_signature_rejected() {
        let pool = TransactionPool::with_config(test_config());
        let sender = PqKeypair::generate();
        let other = PqKeypair::generate();

        // Create a properly signed transaction, then corrupt the PQ signature
        let mut tx = make_tx(&sender, 0, 100);
        tx.pq_signature = Some(other.sign(b"wrong message"));

        let result = pool.insert(tx);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            MempoolError::InvalidSignature
        ));
        assert_eq!(pool.len(), 0);
    }
}
