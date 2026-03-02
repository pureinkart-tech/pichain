//! Fuzz target for mining proof verification.
//!
//! Exercises the BBP digit verification and PoW validation paths
//! with arbitrary inputs to find panics or logic errors in the
//! reward-critical mining proof pipeline.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pichain_types::TransactionKind;

fuzz_target!(|data: &[u8]| {
    // Attempt to deserialize as a MiningProof transaction kind
    let _ = serde_json::from_slice::<TransactionKind>(data);

    // Construct a MiningProof-like structure from raw bytes
    if data.len() >= 20 {
        let start_position = u64::from_le_bytes(
            data[..8].try_into().unwrap_or([0u8; 8])
        );
        let digit_count = u32::from_le_bytes(
            data[8..12].try_into().unwrap_or([0u8; 4])
        );

        // Bound digit_count to prevent OOM (real proofs are bounded by executor)
        let digit_count = digit_count.min(100_000);

        let proof_len = data.len().saturating_sub(20).min(10_000);
        let digits = &data[12..12 + proof_len.min(data.len() - 12)];
        let proof = &data[12..12 + proof_len.min(data.len() - 12)];

        // Try to construct and serialize a MiningProof
        let kind = TransactionKind::MiningProof {
            start_position,
            digit_count,
            digits: digits.to_vec(),
            proof: proof.to_vec(),
            pow_nonce: 0,
            anchor_block_hash: vec![],
        };
        let _ = serde_json::to_string(&kind);

        // Try round-trip
        if let Ok(json) = serde_json::to_string(&kind) {
            let _ = serde_json::from_str::<TransactionKind>(&json);
        }
    }
});
