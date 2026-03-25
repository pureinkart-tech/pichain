//! Block-Level STARK Signature Aggregation for PIChain.
//!
//! Provides the framework for compressing N post-quantum signatures per block
//! into a single STARK proof (~200KB) that proves all signatures verified correctly.
//!
//! # Problem
//!
//! Dual PQ signatures are ~22KB per transaction. At 50,000 txs/block,
//! that's ~1.1GB of signature data per block — unsustainable.
//!
//! # Solution
//!
//! After a block is finalized, generate a STARK proof that:
//! "All N signatures in this block verified correctly against their respective
//! public keys and transaction data."
//!
//! Individual signatures can then be pruned after a retention window.
//! Light clients only need the STARK proof.
//!
//! # Implementation status
//!
//! This module provides the interface and data structures. The actual STARK proof
//! generation requires a ZK proof system (e.g., RISC Zero, Winterfell, or Plonky3).
//! These will be integrated as external dependencies when the PQ signature system
//! is activated on the network.
//!
//! # Why STARKs (not SNARKs)
//!
//! - STARKs use FRI commitment (hash-based) — quantum-safe
//! - SNARKs (Groth16, PLONK) use elliptic curve pairings — broken by Shor's
//! - STARKs require no trusted setup
//! - Proof size is larger (~200KB vs ~200 bytes) but acceptable for block-level aggregation

use serde::{Deserialize, Serialize};

/// Block signature aggregation proof.
///
/// Proves that all transactions in a block had valid PQ signatures
/// without storing the individual signatures long-term.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockAggregationProof {
    /// Block height this proof covers.
    pub block_height: u64,
    /// Block hash this proof covers.
    pub block_hash: [u8; 32],
    /// Number of transactions whose signatures were verified.
    pub tx_count: u32,
    /// Commitment to the set of transaction hashes (Merkle root).
    pub tx_commitment: [u8; 32],
    /// The STARK proof data.
    /// When proof generation is not available, this is empty (signatures retained).
    pub proof_data: Vec<u8>,
    /// Whether this proof has been verified.
    pub verified: bool,
    /// Proof system identifier.
    pub proof_system: ProofSystem,
}

/// Supported proof systems for signature aggregation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofSystem {
    /// No proof — individual signatures are retained.
    None,
    /// RISC Zero zkVM STARK proof.
    RiscZero,
    /// Winterfell STARK proof.
    Winterfell,
    /// Plonky3 STARK proof.
    Plonky3,
}

/// Input to the STARK proof circuit.
///
/// This structure defines what the ZK circuit proves:
/// "For each (tx_data, pq_public_keys, pq_dual_signature) triple in this block,
/// both ML-DSA and SLH-DSA signature verification succeeded."
#[derive(Clone, Debug)]
pub struct AggregationCircuitInput {
    /// Transaction data hashes (canonical_bytes → Blake3 hash).
    pub tx_data_hashes: Vec<[u8; 32]>,
    /// ML-DSA public keys (one per transaction).
    pub ml_dsa_pks: Vec<Vec<u8>>,
    /// SLH-DSA public keys (one per transaction).
    pub slh_dsa_pks: Vec<Vec<u8>>,
    /// ML-DSA signatures (one per transaction).
    pub ml_dsa_sigs: Vec<Vec<u8>>,
    /// SLH-DSA signatures (one per transaction).
    pub slh_dsa_sigs: Vec<Vec<u8>>,
}

/// Result of attempting to generate an aggregation proof.
#[derive(Clone, Debug)]
pub enum AggregationResult {
    /// Proof generated successfully.
    Success(BlockAggregationProof),
    /// Proof generation not available (no prover configured).
    /// Signatures should be retained in the block.
    NotAvailable,
    /// Proof generation failed.
    Failed(String),
}

/// Trait for STARK proof generators.
///
/// Implement this trait to plug in a specific ZK proof system
/// (RISC Zero, Winterfell, Plonky3, etc.).
pub trait StarkProver: Send + Sync {
    /// Generate a STARK proof that all signatures in the input verified correctly.
    fn prove(&self, input: &AggregationCircuitInput) -> Result<Vec<u8>, String>;

    /// Verify a previously generated STARK proof.
    fn verify(&self, proof: &[u8], tx_commitment: &[u8; 32], tx_count: u32) -> Result<bool, String>;

    /// Name of this proof system.
    fn system(&self) -> ProofSystem;
}

/// Placeholder prover that signals proof generation is not yet available.
///
/// When this is the active prover, blocks retain individual signatures
/// rather than generating aggregation proofs. This allows the system to
/// operate correctly before a ZK proof system is integrated.
pub struct PlaceholderProver;

impl StarkProver for PlaceholderProver {
    fn prove(&self, _input: &AggregationCircuitInput) -> Result<Vec<u8>, String> {
        Err("STARK prover not configured — signatures will be retained".into())
    }

    fn verify(&self, _proof: &[u8], _tx_commitment: &[u8; 32], _tx_count: u32) -> Result<bool, String> {
        Err("STARK verifier not configured".into())
    }

    fn system(&self) -> ProofSystem {
        ProofSystem::None
    }
}

/// Compute the transaction commitment (Merkle root of tx hashes).
///
/// This is the public input to the STARK circuit — the verifier checks
/// the proof against this commitment without seeing individual transactions.
pub fn compute_tx_commitment(tx_hashes: &[[u8; 32]]) -> [u8; 32] {
    if tx_hashes.is_empty() {
        return [0u8; 32];
    }
    if tx_hashes.len() == 1 {
        return tx_hashes[0];
    }

    // Simple binary Merkle tree with domain separation
    let mut layer: Vec<[u8; 32]> = tx_hashes.to_vec();
    while layer.len() > 1 {
        let mut next = Vec::with_capacity((layer.len() + 1) / 2);
        for chunk in layer.chunks(2) {
            if chunk.len() == 2 {
                let h = crate::hash_concat(&[
                    &[0x01u8] as &[u8], // internal node domain separator
                    &chunk[0] as &[u8],
                    &chunk[1] as &[u8],
                ]);
                next.push(*h.as_bytes());
            } else {
                // Odd element: hash with distinct domain separator to prevent
                // second-preimage attacks via level confusion.
                let h = crate::hash_concat(&[
                    &[0x02u8] as &[u8], // odd-promotion domain separator
                    &chunk[0] as &[u8],
                ]);
                next.push(*h.as_bytes());
            }
        }
        layer = next;
    }
    layer[0]
}

/// Signature retention policy.
///
/// Defines how long individual PQ signatures are retained before they can be
/// pruned (after a STARK aggregation proof covers them).
#[derive(Clone, Debug)]
pub struct RetentionPolicy {
    /// Number of blocks to retain individual signatures after STARK proof generation.
    /// Full nodes keep sigs for this window; after that, only the STARK proof is needed.
    pub retention_blocks: u64,
    /// Whether to generate STARK proofs automatically.
    pub auto_prove: bool,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            // ~1 epoch of retention before pruning
            retention_blocks: 275_159,
            auto_prove: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tx_commitment_empty() {
        let commitment = compute_tx_commitment(&[]);
        assert_eq!(commitment, [0u8; 32]);
    }

    #[test]
    fn tx_commitment_single() {
        let hash = [42u8; 32];
        let commitment = compute_tx_commitment(&[hash]);
        assert_eq!(commitment, hash);
    }

    #[test]
    fn tx_commitment_deterministic() {
        let hashes = vec![[1u8; 32], [2u8; 32], [3u8; 32]];
        let c1 = compute_tx_commitment(&hashes);
        let c2 = compute_tx_commitment(&hashes);
        assert_eq!(c1, c2);
    }

    #[test]
    fn tx_commitment_order_dependent() {
        let h1 = compute_tx_commitment(&[[1u8; 32], [2u8; 32]]);
        let h2 = compute_tx_commitment(&[[2u8; 32], [1u8; 32]]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn placeholder_prover_returns_error() {
        let prover = PlaceholderProver;
        let input = AggregationCircuitInput {
            tx_data_hashes: vec![],
            ml_dsa_pks: vec![],
            slh_dsa_pks: vec![],
            ml_dsa_sigs: vec![],
            slh_dsa_sigs: vec![],
        };
        assert!(prover.prove(&input).is_err());
        assert_eq!(prover.system(), ProofSystem::None);
    }

    #[test]
    fn tx_commitment_power_of_two() {
        let hashes: Vec<[u8; 32]> = (0..8).map(|i| [i as u8; 32]).collect();
        let commitment = compute_tx_commitment(&hashes);
        assert_ne!(commitment, [0u8; 32]);
    }

    #[test]
    fn tx_commitment_odd_count() {
        let hashes: Vec<[u8; 32]> = (0..5).map(|i| [i as u8; 32]).collect();
        let commitment = compute_tx_commitment(&hashes);
        assert_ne!(commitment, [0u8; 32]);
    }

    #[test]
    fn retention_policy_defaults() {
        let policy = RetentionPolicy::default();
        assert!(policy.retention_blocks > 0);
        assert!(!policy.auto_prove);
    }

    #[test]
    fn block_aggregation_proof_serialization() {
        let proof = BlockAggregationProof {
            block_height: 42,
            block_hash: [1u8; 32],
            tx_count: 100,
            tx_commitment: [2u8; 32],
            proof_data: vec![0xDE, 0xAD],
            verified: false,
            proof_system: ProofSystem::None,
        };
        let json = serde_json::to_string(&proof).unwrap();
        let restored: BlockAggregationProof = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.block_height, 42);
        assert_eq!(restored.tx_count, 100);
        assert_eq!(restored.proof_system, ProofSystem::None);
    }
}
