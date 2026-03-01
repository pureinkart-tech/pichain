//! Fuzz target for SignedTransaction deserialization.
//!
//! Feeds arbitrary bytes into both JSON and raw deserialization paths to find
//! panics, hangs, or excessive memory allocation in transaction parsing.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pichain_types::SignedTransaction;

fuzz_target!(|data: &[u8]| {
    // Attempt 1: JSON deserialization (the primary wire format).
    // This exercises serde_json parsing of TransactionData, TransactionKind
    // (all 30+ variants), signature bytes, and public key recovery.
    let _ = serde_json::from_slice::<SignedTransaction>(data);

    // Attempt 2: Interpret the data as a length-prefixed message, similar to
    // bincode-style framing that a P2P layer might use.
    // Layout: [4-byte little-endian length] [JSON payload]
    if data.len() >= 4 {
        let len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if len <= data.len().saturating_sub(4) {
            let payload = &data[4..4 + len];
            let _ = serde_json::from_slice::<SignedTransaction>(payload);
        }
    }

    // Attempt 3: Try interpreting the first half as a truncated transaction
    // (simulates partial network reads or corrupted wire data).
    if data.len() > 16 {
        let half = &data[..data.len() / 2];
        let _ = serde_json::from_slice::<SignedTransaction>(half);
    }
});
