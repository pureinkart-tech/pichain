//! Solana-style 3-stage transaction pipeline.
//!
//! Stages run concurrently across blocks for maximum throughput:
//!
//! ```text
//! Block N:   [Fetch+SigVerify] → [Banking] → [Ledger Write]
//! Block N+1:                      [Fetch+SigVerify] → [Banking] → [Ledger Write]
//! ```
//!
//! Stage 1 — **Fetch + SigVerify**: Pull txs from mempool, batch-verify Ed25519 signatures.
//! Stage 2 — **Banking**: Execute transactions via Sealevel scheduler, build block.
//! Stage 3 — **Ledger Write**: Persist block + state (handled externally by NodeState).

use pichain_crypto::keys;
use pichain_types::transaction::SignedTransaction;
use rayon::prelude::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

use crate::block_producer::{BlockProducer, ProducedBlock};
use crate::mempool::TransactionPool;

/// A batch of transactions with verified signatures, ready for block execution.
#[derive(Debug)]
pub struct VerifiedBatch {
    /// Transactions that passed signature verification.
    pub transactions: Vec<SignedTransaction>,
    /// Time spent on signature verification (microseconds).
    pub sig_verify_time_us: u64,
    /// Number of signatures that failed verification.
    pub rejected_count: usize,
}

/// Configuration for the transaction pipeline.
#[derive(Clone, Debug)]
pub struct PipelineConfig {
    /// Target block time in milliseconds.
    pub target_block_time_ms: u64,
    /// Maximum transactions per block.
    pub max_txs_per_block: usize,
    /// Maximum gas per block.
    pub max_gas_per_block: u64,
    /// Validator address.
    pub validator_address: keys::Address,
}

/// Cumulative pipeline metrics.
struct PipelineMetrics {
    total_sigs_verified: u64,
    total_sigs_rejected: u64,
    total_blocks_produced: u64,
}

/// The 3-stage transaction pipeline.
pub struct TransactionPipeline {
    config: PipelineConfig,
    mempool: Arc<TransactionPool>,
}

impl TransactionPipeline {
    pub fn new(
        config: PipelineConfig,
        _executor: Arc<crate::executor::TransactionExecutor>,
        mempool: Arc<TransactionPool>,
    ) -> Self {
        Self { config, mempool }
    }

    /// Run the 3-stage pipeline until shutdown.
    ///
    /// - Spawns the sigverify stage (fetch + batch verify signatures)
    /// - Runs the banking stage inline (execute + build block)
    /// - Sends produced blocks to `block_tx` for external persistence (Stage 3)
    pub async fn run(
        self,
        mut block_producer: BlockProducer,
        mut shutdown: watch::Receiver<bool>,
        block_tx: mpsc::Sender<ProducedBlock>,
    ) {
        let block_interval = Duration::from_millis(self.config.target_block_time_ms);
        let max_txs = self.config.max_txs_per_block;

        // Channel between Stage 1 (sigverify) and Stage 2 (banking)
        let (verified_tx, mut verified_rx) = mpsc::channel::<VerifiedBatch>(4);

        let mempool = self.mempool.clone();
        let mut sigverify_shutdown = shutdown.clone();

        // --- Stage 1: Fetch + SigVerify (async task) ---
        tokio::spawn(async move {
            info!("pipeline stage 1 (fetch+sigverify) started");
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(block_interval) => {
                        let start = Instant::now();

                        // Atomically claim ready transactions from mempool
                        let transactions = mempool.claim_ready_transactions(max_txs);

                        // Batch-verify signatures in parallel
                        let (verified, rejected) = if transactions.is_empty() {
                            (vec![], 0)
                        } else {
                            batch_verify_signatures(&transactions)
                        };

                        let elapsed_us = start.elapsed().as_micros() as u64;
                        let batch = VerifiedBatch {
                            transactions: verified,
                            sig_verify_time_us: elapsed_us,
                            rejected_count: rejected,
                        };

                        if batch.rejected_count > 0 {
                            warn!(
                                rejected = batch.rejected_count,
                                "signatures rejected in batch verify"
                            );
                        }

                        if verified_tx.send(batch).await.is_err() {
                            break;
                        }
                    }
                    _ = sigverify_shutdown.changed() => {
                        info!("pipeline stage 1 shutting down");
                        break;
                    }
                }
            }
        });

        // --- Stage 2: Banking (runs inline — BlockProducer is !Send) ---
        let mut metrics = PipelineMetrics {
            total_sigs_verified: 0,
            total_sigs_rejected: 0,
            total_blocks_produced: 0,
        };

        info!(
            interval_ms = self.config.target_block_time_ms,
            validator = %self.config.validator_address,
            "pipeline stage 2 (banking) started"
        );

        loop {
            tokio::select! {
                batch = verified_rx.recv() => {
                    let Some(batch) = batch else { break };

                    // Update metrics
                    metrics.total_sigs_verified += batch.transactions.len() as u64;
                    metrics.total_sigs_rejected += batch.rejected_count as u64;

                    // Execute + build block from pre-verified transactions
                    let produced = block_producer.produce_block_from_verified(batch.transactions);

                    metrics.total_blocks_produced += 1;

                    if produced.block.header.tx_count > 0 {
                        debug!(
                            height = produced.block.header.height,
                            sig_verify_us = batch.sig_verify_time_us,
                            banking_ms = produced.production_time_ms,
                            total_verified = metrics.total_sigs_verified,
                            total_rejected = metrics.total_sigs_rejected,
                            "pipeline block produced"
                        );
                    }

                    if block_tx.send(produced).await.is_err() {
                        error!("failed to send produced block to persistence");
                        break;
                    }
                }
                _ = shutdown.changed() => {
                    info!(
                        blocks = metrics.total_blocks_produced,
                        sigs_verified = metrics.total_sigs_verified,
                        sigs_rejected = metrics.total_sigs_rejected,
                        "pipeline stage 2 shutting down"
                    );
                    break;
                }
            }
        }
    }
}

/// Verify PQ signatures in parallel using rayon.
///
/// Splits transactions into chunks and verifies each chunk's dual PQ signatures
/// (ML-DSA-65 + SLH-DSA-SHAKE-128f) in parallel. Invalid signatures are filtered out.
///
/// Returns `(valid_transactions, rejected_count)`.
pub fn batch_verify_signatures(
    transactions: &[SignedTransaction],
) -> (Vec<SignedTransaction>, usize) {
    if transactions.is_empty() {
        return (vec![], 0);
    }

    // For small batches, verify individually
    if transactions.len() <= 4 {
        let mut valid = Vec::with_capacity(transactions.len());
        let mut rejected = 0;
        for tx in transactions {
            if tx.verify().is_ok() {
                valid.push(tx.clone());
            } else {
                rejected += 1;
            }
        }
        return (valid, rejected);
    }

    // Parallel PQ verification: split into chunks
    const CHUNK_SIZE: usize = 64;

    let chunk_results: Vec<Vec<SignedTransaction>> = transactions
        .par_chunks(CHUNK_SIZE)
        .map(|chunk| {
            // Verify each transaction's dual PQ signatures individually
            chunk
                .iter()
                .filter(|tx| tx.verify().is_ok())
                .cloned()
                .collect()
        })
        .collect();

    let valid: Vec<SignedTransaction> = chunk_results.into_iter().flatten().collect();
    let rejected = transactions.len() - valid.len();
    (valid, rejected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::PqKeypair;
    use pichain_types::transaction::Transaction;

    fn make_signed_tx(sender: &PqKeypair, nonce: u64) -> SignedTransaction {
        let recipient = PqKeypair::generate();
        let data =
            Transaction::transfer(sender.address(), nonce, recipient.address(), 1_000_000, 1);
        Transaction::sign_pq(data, sender)
    }

    #[test]
    fn batch_sig_verify_valid() {
        let sender = PqKeypair::generate();
        let txs: Vec<SignedTransaction> = (0..100).map(|i| make_signed_tx(&sender, i)).collect();

        let (valid, rejected) = batch_verify_signatures(&txs);
        assert_eq!(valid.len(), 100);
        assert_eq!(rejected, 0);
    }

    #[test]
    fn batch_sig_verify_rejects_bad() {
        let sender = PqKeypair::generate();
        let other = PqKeypair::generate();
        let mut txs: Vec<SignedTransaction> = (0..10).map(|i| make_signed_tx(&sender, i)).collect();

        // Corrupt PQ signatures on 3 transactions
        for i in [2, 5, 8] {
            txs[i].pq_signature = Some(other.sign(b"wrong message"));
        }

        let (valid, rejected) = batch_verify_signatures(&txs);
        assert_eq!(valid.len(), 7);
        assert_eq!(rejected, 3);
    }

    #[test]
    fn batch_sig_verify_empty() {
        let (valid, rejected) = batch_verify_signatures(&[]);
        assert_eq!(valid.len(), 0);
        assert_eq!(rejected, 0);
    }

    #[test]
    fn batch_sig_verify_single() {
        let sender = PqKeypair::generate();
        let txs = vec![make_signed_tx(&sender, 0)];

        let (valid, rejected) = batch_verify_signatures(&txs);
        assert_eq!(valid.len(), 1);
        assert_eq!(rejected, 0);
    }

    #[test]
    fn batch_sig_verify_all_invalid() {
        let sender = PqKeypair::generate();
        let other = PqKeypair::generate();
        let mut txs: Vec<SignedTransaction> = (0..5).map(|i| make_signed_tx(&sender, i)).collect();

        // Corrupt all PQ signatures
        for tx in &mut txs {
            tx.pq_signature = Some(other.sign(b"wrong"));
        }

        let (valid, rejected) = batch_verify_signatures(&txs);
        assert_eq!(valid.len(), 0);
        assert_eq!(rejected, 5);
    }
}
