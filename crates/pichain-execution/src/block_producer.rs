//! Block production pipeline — assembles transactions from the mempool into blocks.
//!
//! The block producer:
//! 1. Pulls ready transactions from the mempool
//! 2. Executes them via Block-STM parallel execution
//! 3. Builds a new block with state root, tx root, receipts root
//! 4. Applies state changes to storage
//! 5. Emits the block for consensus/propagation

use pichain_crypto::keys::Address;
use pichain_crypto::poseidon::PoseidonHash;
use pichain_crypto::Hash;
use pichain_types::block::{Block, BlockHeader};
use pichain_types::{PiAmount, EPOCH_LENGTH, TARGET_BLOCK_TIME_MS};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info};

use crate::executor::{ExecutionResult, TransactionExecutor};
use crate::mempool::TransactionPool;

/// Configuration for the block producer.
#[derive(Clone, Debug)]
pub struct BlockProducerConfig {
    /// Maximum transactions per block.
    pub max_txs_per_block: usize,
    /// Maximum gas per block.
    pub max_gas_per_block: u64,
    /// Target block time in milliseconds.
    pub target_block_time_ms: u64,
    /// Validator address (block proposer).
    pub validator_address: Address,
}

impl Default for BlockProducerConfig {
    fn default() -> Self {
        Self {
            max_txs_per_block: 50_000,      // 50K txs/block (Solana-competitive)
            max_gas_per_block: 500_000_000, // 500M gas units
            target_block_time_ms: TARGET_BLOCK_TIME_MS,
            validator_address: Address::ZERO,
        }
    }
}

/// Result of producing a block.
#[derive(Clone, Debug)]
pub struct ProducedBlock {
    /// The produced block.
    pub block: Block,
    /// Execution results for each transaction.
    pub execution_results: Vec<ExecutionResult>,
    /// Total PI burned in this block.
    pub total_burned: PiAmount,
    /// Total new PI minted in this block (mining rewards — inflationary).
    pub total_minted: PiAmount,
    /// Total fee reward credited to the block proposer (priority + staker share).
    pub proposer_reward: PiAmount,
    /// Total miner fee income flowing back to mining pool.
    pub total_miner_fee: PiAmount,
    /// Block production time in milliseconds.
    pub production_time_ms: u64,
}

/// Block producer — assembles and executes blocks from the mempool.
pub struct BlockProducer {
    config: BlockProducerConfig,
    executor: Arc<TransactionExecutor>,
    mempool: Arc<TransactionPool>,
    /// Chain ID (included in every block header for replay protection).
    chain_id: u64,
    /// Current block height.
    current_height: u64,
    /// Current epoch.
    current_epoch: u64,
    /// Current consensus round.
    current_round: u64,
    /// Parent block hash.
    parent_hash: Hash,
    /// Current base fee (EIP-1559).
    current_base_fee: PiAmount,
    /// Total PI burned across all blocks.
    total_burned: PiAmount,
    /// TPS tracking: total transactions processed.
    total_txs: u64,
    /// TPS tracking: timestamp of first block.
    first_block_time: Option<Instant>,
    /// Peak TPS observed in a single block.
    peak_tps: f64,
    /// Timestamp of the last produced block (for monotonicity enforcement).
    last_timestamp_ms: u64,
}

impl BlockProducer {
    /// Create a new block producer.
    pub fn new(
        config: BlockProducerConfig,
        executor: Arc<TransactionExecutor>,
        mempool: Arc<TransactionPool>,
        chain_id: u64,
    ) -> Self {
        Self {
            config,
            executor,
            mempool,
            chain_id,
            current_height: 0,
            current_epoch: 0,
            current_round: 0,
            parent_hash: Hash::ZERO,
            current_base_fee: 1_000, // Initial base fee: 1000 base units
            total_burned: 0,
            total_txs: 0,
            first_block_time: None,
            peak_tps: 0.0,
            last_timestamp_ms: 0,
        }
    }

    /// Update the parent hash after the real state root is known.
    /// Called by the persist loop after persist_block computes the JMT state root.
    /// The block producer initially uses PoseidonHash::ZERO for state_root, so the
    /// block hash (and thus parent_hash) must be corrected once the real root is known.
    pub fn set_parent_hash(&mut self, parent_hash: Hash) {
        self.parent_hash = parent_hash;
    }

    /// Set the starting state (after loading from storage).
    pub fn set_state(
        &mut self,
        height: u64,
        parent_hash: Hash,
        base_fee: PiAmount,
        last_timestamp_ms: u64,
    ) {
        self.current_height = height;
        self.parent_hash = parent_hash;
        self.current_base_fee = base_fee;
        self.current_epoch = height / EPOCH_LENGTH;
        self.current_round = height;
        self.last_timestamp_ms = last_timestamp_ms;
    }

    /// Produce a single block from pending transactions (pulls from mempool).
    pub fn produce_block(&mut self) -> ProducedBlock {
        let transactions = self
            .mempool
            .get_ready_transactions(self.config.max_txs_per_block);
        self.produce_block_from_verified(transactions)
    }

    /// Produce a block from pre-fetched, pre-verified transactions.
    /// Used by the pipeline (Stage 2 — Banking) where signatures are already verified.
    pub fn produce_block_from_verified(
        &mut self,
        transactions: Vec<pichain_types::transaction::SignedTransaction>,
    ) -> ProducedBlock {
        let start = Instant::now();

        // Enforce block gas limit — only include transactions that fit within the gas budget.
        let mut transactions = {
            let mut budget = self.config.max_gas_per_block;
            let mut selected = Vec::with_capacity(transactions.len());
            for tx in transactions {
                let gas = tx.estimated_gas();
                if gas <= budget {
                    budget = budget.saturating_sub(gas);
                    selected.push(tx);
                }
                // Skip transactions that would exceed the block gas limit
            }
            selected
        };
        let initial_tx_count = transactions.len();

        // 1. Compute block timestamp BEFORE execution for determinism.
        // All state-creating operations (token creation, pool creation, etc.)
        // will use this timestamp instead of wall-clock time.
        let wall_clock_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        // Enforce strict monotonicity: each block must have a timestamp > previous.
        // If clock drifts backward, advance by 1ms from last block.
        let timestamp_ms = wall_clock_ms.max(self.last_timestamp_ms.saturating_add(1));
        self.executor.set_block_timestamp(timestamp_ms);
        self.executor
            .set_block_height(self.current_height.saturating_add(1));
        // Snapshot DEX pool reserves BEFORE executing any transactions.
        // Price impact is measured against these snapshots to prevent
        // intra-block sandwich attacks from manipulating the impact threshold.
        self.executor.snapshot_dex_reserves();

        debug!(
            height = self.current_height + 1,
            tx_count = initial_tx_count,
            base_fee = self.current_base_fee,
            "producing block"
        );

        // R36-FIX: Snapshot mining processor state BEFORE execution so we can
        // restore it if gas-trimming requires re-execution. Without this, mining
        // proofs registered during the first pass would be rejected as "already
        // computed" during re-execution.
        let mining_snapshot = self.executor.snapshot_mining_processor();

        // 2. Execute transactions via Block-STM
        let execution_results = if transactions.is_empty() {
            vec![]
        } else {
            self.executor
                .execute_block(&transactions, self.current_base_fee)
        };

        // 3. Calculate block-level metrics
        let mut total_gas_used: u64 = execution_results
            .iter()
            .map(|r| r.effect.gas_used)
            .fold(0u64, |acc, v| acc.saturating_add(v));

        // Post-execution gas limit enforcement — trim transactions from the end
        // if actual gas exceeds budget (transactions may use more gas than estimated).
        let mut execution_results = execution_results;
        if total_gas_used > self.config.max_gas_per_block {
            error!(
                total_gas_used,
                max = self.config.max_gas_per_block,
                "BLOCK GAS LIMIT EXCEEDED — trimming transactions to fit"
            );
            // Trim from the end until gas fits within budget
            let mut trimmed_results = Vec::new();
            while total_gas_used > self.config.max_gas_per_block && !execution_results.is_empty() {
                let removed = execution_results.pop().unwrap();
                transactions.pop();
                total_gas_used = total_gas_used.saturating_sub(removed.effect.gas_used);
                trimmed_results.push(removed);
            }
            // R30-FIX: Re-execute retained transactions to get clean sub-executor state.
            // The R29 fix only evicted PI state_cache entries for trimmed txs, but
            // sub-executor state (tokens, DEX, NFTs, launchpad) from trimmed transactions
            // was left behind. The safest fix: evict state_cache for ALL executed
            // transactions (both retained and trimmed), then re-execute only the
            // retained transactions from scratch so sub-executor state is consistent.
            for result in execution_results.iter().chain(trimmed_results.iter()) {
                for addr in result.state_changes.keys() {
                    self.executor.evict_state_cache(addr);
                }
            }
            // Reset anti-concentration tracker so the re-execution doesn't double-count.
            self.executor.rebuild_staking_totals();
            // R35-FIX: Clear sub-executor state (tokens, DEX pools, NFTs, launchpad,
            // WASM contracts) from the first execution. Without this, retained
            // transactions would see stale/duplicate objects from the trimmed pass.
            self.executor.clear_sub_executors();
            // R36-FIX: Restore mining processor to pre-execution snapshot so mining
            // proofs in the retained set aren't rejected as "already computed".
            self.executor.restore_mining_processor(mining_snapshot);
            // Re-snapshot DEX reserves so price impact checks use clean baseline.
            self.executor.snapshot_dex_reserves();
            // Re-execute only the retained transactions with clean state
            execution_results = self
                .executor
                .execute_block(&transactions, self.current_base_fee);
            total_gas_used = execution_results
                .iter()
                .map(|r| r.effect.gas_used)
                .fold(0u64, |acc, v| acc.saturating_add(v));
        }
        let tx_count = transactions.len();
        let total_burned: PiAmount = execution_results
            .iter()
            .map(|r| r.pi_burned)
            .fold(0u64, |acc, v| acc.saturating_add(v));
        let total_minted: PiAmount = execution_results
            .iter()
            .map(|r| r.pi_minted)
            .fold(0u64, |acc, v| acc.saturating_add(v));
        let total_proposer_reward: PiAmount = execution_results
            .iter()
            .map(|r| r.proposer_reward)
            .fold(0u64, |acc, r| acc.saturating_add(r));
        let total_miner_fee: PiAmount = execution_results
            .iter()
            .map(|r| r.miner_fee)
            .fold(0u64, |acc, v| acc.saturating_add(v));

        // Proposer fee credit is handled by the caller (main.rs persist task)
        // to match the follower path and ensure identical state transitions.

        // Feed miner fees back into the mining pool for perpetual sustainability
        if total_miner_fee > 0 {
            self.executor
                .mining_processor()
                .lock()
                .add_fee_income(total_miner_fee);
        }

        // Drip staking rewards from unmined emission recycling to proposer
        {
            let staking_drip = self.executor.mining_processor().lock().drain_staking_drip();
            if staking_drip > 0 {
                let proposer = self.config.validator_address;
                self.executor.credit_account(proposer, staking_drip);
            }
        }

        // Gap prevention is handled at the miner level:
        // 1. local_position advances only by actually submitted batch count (no skipping)
        // 2. Slot endpoint returns gap_fill_position to redirect miners to fill any gaps
        // No server-side BBP computation during block production — it blocks the RPC thread.

        // 4. Compute transaction root (Merkle root of tx hashes)
        let tx_hashes: Vec<Hash> = transactions.iter().map(|tx| tx.hash()).collect();
        let tx_root = if tx_hashes.is_empty() {
            Hash::ZERO
        } else {
            compute_merkle_root(&tx_hashes)
        };

        // 5. Compute state root (would come from JMT in full implementation)
        let state_root = PoseidonHash::ZERO; // Updated by JMT after state application

        // 6. Compute receipts root
        let receipt_hashes: Vec<Hash> = execution_results
            .iter()
            .map(|r| pichain_crypto::hash(&serde_json::to_vec(&r.effect).unwrap_or_default()))
            .collect();
        let receipts_root = if receipt_hashes.is_empty() {
            Hash::ZERO
        } else {
            compute_merkle_root(&receipt_hashes)
        };

        // 7. Build the block header
        let new_height = self.current_height.saturating_add(1);
        let new_epoch = new_height / EPOCH_LENGTH;

        let header = BlockHeader {
            chain_id: self.chain_id,
            height: new_height,
            epoch: new_epoch,
            round: self.current_round.saturating_add(1),
            parent_hash: self.parent_hash,
            tx_root,
            state_root,
            receipts_root,
            proposer: self.config.validator_address,
            timestamp_ms,
            gas_used: total_gas_used,
            base_fee: self.current_base_fee,
            tx_count: tx_count.min(u32::MAX as usize) as u32,
            pi_burned: total_burned,
            pi_miner_fee: total_miner_fee,
        };

        let block = Block {
            header,
            transactions: transactions.clone(),
            proposer_pubkey: vec![], // Signed by caller after production
            proposer_sig: vec![],
        };
        let block_hash = block.hash();

        // 8. Update sender nonces in the mempool
        let mut sender_nonces: HashMap<Address, u64> = HashMap::new();
        for result in &execution_results {
            for (addr, state) in &result.state_changes {
                sender_nonces.insert(*addr, state.nonce);
            }
        }
        self.mempool.remove_committed(&tx_hashes, &sender_nonces);

        // 9. Update block producer state
        self.parent_hash = block_hash;
        self.current_height = new_height;
        self.current_epoch = new_epoch;
        self.current_round = self.current_round.saturating_add(1);
        self.total_burned = self.total_burned.saturating_add(total_burned);
        self.last_timestamp_ms = timestamp_ms;

        // 10. TPS tracking
        self.total_txs = self.total_txs.saturating_add(tx_count as u64);
        if self.first_block_time.is_none() && tx_count > 0 {
            self.first_block_time = Some(Instant::now());
        }
        let production_time = start.elapsed();
        let block_tps = if production_time.as_millis() > 0 {
            tx_count as f64 / production_time.as_secs_f64()
        } else if tx_count > 0 {
            tx_count as f64 / 0.001 // sub-ms block
        } else {
            0.0
        };
        if block_tps > self.peak_tps {
            self.peak_tps = block_tps;
        }

        // 11. Adjust base fee for next block (EIP-1559)
        let gas_target = self.config.max_gas_per_block / 2; // Target 50% utilization
        self.current_base_fee = block.header.next_base_fee(gas_target);

        if tx_count > 0 {
            info!(
                height = new_height,
                tx_count,
                gas_used = total_gas_used,
                burned = total_burned,
                base_fee = self.current_base_fee,
                time_ms = production_time.as_millis() as u64,
                tps = format!("{:.0}", block_tps),
                peak_tps = format!("{:.0}", self.peak_tps),
                "block produced"
            );
        } else {
            debug!(
                height = new_height,
                base_fee = self.current_base_fee,
                time_ms = production_time.as_millis() as u64,
                "empty block produced"
            );
        }

        ProducedBlock {
            block,
            execution_results,
            total_burned,
            total_minted,
            proposer_reward: total_proposer_reward,
            total_miner_fee,
            production_time_ms: production_time.as_millis() as u64,
        }
    }

    /// Run the block production loop.
    /// Produces blocks at the target block time interval.
    pub async fn run(
        &mut self,
        mut shutdown: watch::Receiver<bool>,
        block_tx: mpsc::Sender<ProducedBlock>,
    ) {
        let block_interval = Duration::from_millis(self.config.target_block_time_ms);

        info!(
            interval_ms = self.config.target_block_time_ms,
            validator = %self.config.validator_address,
            "block production loop started"
        );

        loop {
            tokio::select! {
                _ = tokio::time::sleep(block_interval) => {
                    // Only produce blocks if there are pending transactions
                    // or if we need to produce empty blocks for liveness
                    let produced = self.produce_block();

                    if let Err(e) = block_tx.send(produced).await {
                        error!("failed to send produced block: {e}");
                        break;
                    }
                }
                _ = shutdown.changed() => {
                    info!("block production loop shutting down");
                    break;
                }
            }
        }
    }

    /// Get the current block height.
    pub fn current_height(&self) -> u64 {
        self.current_height
    }

    /// Get the current base fee.
    pub fn current_base_fee(&self) -> PiAmount {
        self.current_base_fee
    }

    /// Get total PI burned.
    pub fn total_burned(&self) -> PiAmount {
        self.total_burned
    }

    /// Get the average TPS since the first transaction.
    pub fn average_tps(&self) -> f64 {
        if let Some(first_time) = self.first_block_time {
            let elapsed = first_time.elapsed().as_secs_f64();
            if elapsed > 0.0 {
                return self.total_txs as f64 / elapsed;
            }
        }
        0.0
    }

    /// Get peak TPS observed in any single block.
    pub fn peak_tps(&self) -> f64 {
        self.peak_tps
    }

    /// Get total transactions processed.
    pub fn total_txs(&self) -> u64 {
        self.total_txs
    }
}

/// Compute a binary Merkle root from a list of hashes.
///
/// Uses domain separation to prevent second-preimage attacks:
/// - Leaf nodes are hashed with a 0x00 prefix
/// - Internal nodes are hashed with a 0x01 prefix
/// - Odd elements are promoted without re-hashing (no duplicate-last-tx vulnerability)
fn compute_merkle_root(hashes: &[Hash]) -> Hash {
    if hashes.is_empty() {
        return Hash::ZERO;
    }
    if hashes.len() == 1 {
        // Single leaf: hash with leaf prefix for domain separation
        return pichain_crypto::hash_concat(&[&[0x00], hashes[0].as_bytes()]);
    }

    // Hash each leaf with 0x00 prefix (domain separation: leaf vs internal)
    let mut current_level: Vec<Hash> = hashes
        .iter()
        .map(|h| pichain_crypto::hash_concat(&[&[0x00], h.as_bytes()]))
        .collect();

    while current_level.len() > 1 {
        let mut next_level = Vec::with_capacity(current_level.len().div_ceil(2));
        for chunk in current_level.chunks(2) {
            if chunk.len() == 2 {
                // Internal node: 0x01 prefix
                next_level.push(pichain_crypto::hash_concat(&[
                    &[0x01],
                    chunk[0].as_bytes(),
                    chunk[1].as_bytes(),
                ]));
            } else {
                // Odd element: promote to next level (avoids duplicate-last-tx attack)
                next_level.push(chunk[0]);
            }
        }
        current_level = next_level;
    }
    current_level[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mempool::TransactionPool;
    use pichain_crypto::PqKeypair;
    use pichain_types::account::AccountState;
    use pichain_types::transaction::Transaction;

    fn setup() -> (BlockProducer, PqKeypair) {
        let validator = PqKeypair::generate();
        let executor = Arc::new(TransactionExecutor::new(1));
        let mempool = Arc::new(TransactionPool::with_config(
            crate::mempool::MempoolConfig {
                
                ..Default::default()
            },
        ));

        let config = BlockProducerConfig {
            validator_address: validator.address(),
            ..Default::default()
        };

        let producer = BlockProducer::new(config, executor.clone(), mempool.clone(), 31415);
        (producer, validator)
    }

    #[test]
    fn produce_empty_block() {
        let (mut producer, _) = setup();
        let produced = producer.produce_block();

        assert_eq!(produced.block.header.height, 1);
        assert_eq!(produced.block.header.tx_count, 0);
        assert_eq!(produced.total_burned, 0);
    }

    #[test]
    fn produce_block_with_transactions() {
        let validator = PqKeypair::generate();
        let executor = Arc::new(TransactionExecutor::new(1));
        let mempool = Arc::new(TransactionPool::with_config(
            crate::mempool::MempoolConfig {
                
                ..Default::default()
            },
        ));

        // Fund a sender
        let sender = PqKeypair::generate();
        let recipient = PqKeypair::generate();
        executor.set_account(
            sender.address(),
            AccountState::with_balance(100 * 1_000_000_000),
        );

        // Add transaction to mempool
        let tx_data = Transaction::transfer(
            sender.address(),
            0,
            recipient.address(),
            1_000_000_000, // 1 PI
            1,
        );
        let signed = Transaction::sign_pq(tx_data, &sender);
        mempool.insert(signed).unwrap();

        let config = BlockProducerConfig {
            validator_address: validator.address(),
            ..Default::default()
        };
        let mut producer = BlockProducer::new(config, executor, mempool.clone(), 31415);

        let produced = producer.produce_block();
        assert_eq!(produced.block.header.height, 1);
        assert_eq!(produced.block.header.tx_count, 1);
        assert!(produced.total_burned > 0);

        // Mempool should be empty after block production
        assert_eq!(mempool.len(), 0);
    }

    #[test]
    fn sequential_blocks_increment_height() {
        let (mut producer, _) = setup();

        let b1 = producer.produce_block();
        let b2 = producer.produce_block();
        let b3 = producer.produce_block();

        assert_eq!(b1.block.header.height, 1);
        assert_eq!(b2.block.header.height, 2);
        assert_eq!(b3.block.header.height, 3);

        // Parent hashes should chain
        assert_eq!(b2.block.header.parent_hash, b1.block.hash());
        assert_eq!(b3.block.header.parent_hash, b2.block.hash());
    }

    #[test]
    fn base_fee_adjusts() {
        let validator = PqKeypair::generate();
        let executor = Arc::new(TransactionExecutor::new(1));
        let mempool = Arc::new(TransactionPool::with_config(
            crate::mempool::MempoolConfig {
                
                ..Default::default()
            },
        ));

        let config = BlockProducerConfig {
            validator_address: validator.address(),
            max_gas_per_block: 1_000_000,
            ..Default::default()
        };
        let mut producer = BlockProducer::new(config, executor, mempool, 31415);

        let initial_fee = producer.current_base_fee();
        producer.produce_block(); // Empty block → fee should decrease
        let new_fee = producer.current_base_fee();

        // Base fee should decrease for empty blocks (below target utilization)
        assert!(new_fee <= initial_fee);
    }

    #[test]
    fn merkle_root_computation() {
        let h1 = pichain_crypto::hash(b"tx1");
        let h2 = pichain_crypto::hash(b"tx2");
        let h3 = pichain_crypto::hash(b"tx3");

        let root = compute_merkle_root(&[h1, h2, h3]);
        assert_ne!(root, Hash::ZERO);

        // Same inputs should give same root
        let root2 = compute_merkle_root(&[h1, h2, h3]);
        assert_eq!(root, root2);

        // Different inputs should give different root
        let root3 = compute_merkle_root(&[h1, h3, h2]);
        assert_ne!(root, root3);
    }

    #[test]
    fn block_gas_limit_enforced_by_trimming() {
        let validator = PqKeypair::generate();
        let executor = Arc::new(TransactionExecutor::new(1));
        let mempool = Arc::new(TransactionPool::with_config(
            crate::mempool::MempoolConfig {
                
                ..Default::default()
            },
        ));

        // Create multiple senders with funds
        let senders: Vec<PqKeypair> = (0..5).map(|_| PqKeypair::generate()).collect();
        let recipient = PqKeypair::generate();
        for sender in &senders {
            executor.set_account(
                sender.address(),
                AccountState::with_balance(100 * 1_000_000_000),
            );
        }

        // Add 5 transactions to mempool — each uses ~21,000 gas for transfer
        for sender in senders.iter() {
            let tx_data = Transaction::transfer(
                sender.address(),
                0,
                recipient.address(),
                1_000_000_000, // 1 PI
                1,
            );
            let signed = Transaction::sign_pq(tx_data, sender);
            mempool.insert(signed).unwrap();
        }

        // Set a very low gas limit that can only fit ~2 transactions
        // Each transfer uses 21,000 gas. Set limit to 42,000.
        let config = BlockProducerConfig {
            validator_address: validator.address(),
            max_gas_per_block: 42_000,
            ..Default::default()
        };
        let mut producer = BlockProducer::new(config, executor, mempool.clone(), 1);

        let produced = producer.produce_block();

        // Total gas in the block must not exceed the block gas limit
        assert!(
            produced.block.header.gas_used <= 42_000,
            "Block gas {} should not exceed gas limit 42000",
            produced.block.header.gas_used,
        );
        // Block should have included some transactions (pre-selection filters by estimate)
        // and the final tx_count should match execution_results length
        assert_eq!(
            produced.block.header.tx_count as usize,
            produced.execution_results.len(),
            "tx_count header must match execution_results length"
        );
    }
}
