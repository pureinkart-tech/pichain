//! Fuzz target for Block deserialization.
//!
//! Feeds arbitrary bytes into JSON deserialization of Block to find panics,
//! hangs, or excessive memory allocation in block parsing. Blocks contain
//! a BlockHeader and a Vec<SignedTransaction>, so this exercises nested
//! deserialization of the full block structure.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pichain_types::Block;

fuzz_target!(|data: &[u8]| {
    // Attempt 1: Direct JSON deserialization of a full Block.
    // This exercises BlockHeader fields (chain_id, height, epoch, round,
    // parent_hash, tx_root, state_root, receipts_root, proposer, timestamp,
    // gas_used, base_fee, tx_count, pi_burned, pi_miner_fee) as well as
    // the embedded Vec<SignedTransaction>.
    let _ = serde_json::from_slice::<Block>(data);

    // Attempt 2: Try deserializing just the header portion.
    // A malformed block might have a valid header but corrupted transactions.
    let _ = serde_json::from_slice::<pichain_types::BlockHeader>(data);

    // Attempt 3: Length-prefixed framing (simulates network wire format).
    // Layout: [4-byte little-endian length] [JSON payload]
    if data.len() >= 4 {
        let len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if len <= data.len().saturating_sub(4) {
            let payload = &data[4..4 + len];
            let _ = serde_json::from_slice::<Block>(payload);
        }
    }
});
