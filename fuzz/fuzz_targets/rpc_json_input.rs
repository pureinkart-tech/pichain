//! Fuzz target for RPC JSON request parsing.
//!
//! Exercises the deserialization paths for all RPC request types
//! that accept JSON bodies. Tests that malformed input cannot cause
//! panics, hangs, or excessive memory allocation.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pichain_types::SignedTransaction;

/// Simulates the POST /api/v1/tx/submit endpoint JSON body.
#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct TxSubmitBody {
    transaction: SignedTransaction,
}

/// Simulates POST /api/v1/events/query request body.
#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct EventQuery {
    topic: Option<String>,
    address: Option<String>,
    from_height: Option<u64>,
    to_height: Option<u64>,
    limit: Option<usize>,
}

/// Simulates POST /api/v1/bridge/mint request body.
#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct BridgeMintRequest {
    chain: String,
    tx_hash: String,
    symbol: String,
    recipient: String,
    amount: u64,
}

/// Simulates POST /api/v1/wallet/activate request body.
#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct ActivateRequest {
    address: String,
    challenge_id: String,
    nonce: u64,
}

fuzz_target!(|data: &[u8]| {
    // Exercise all JSON deserialization paths used by RPC handlers

    // 1. Transaction submission (most complex — 30+ variants)
    let _ = serde_json::from_slice::<TxSubmitBody>(data);

    // 2. Event query (topic/address filtering)
    let _ = serde_json::from_slice::<EventQuery>(data);

    // 3. Bridge mint request
    let _ = serde_json::from_slice::<BridgeMintRequest>(data);

    // 4. Wallet activation
    let _ = serde_json::from_slice::<ActivateRequest>(data);

    // 5. Raw string parsing (address hex, tx hash hex)
    if data.len() >= 40 {
        let hex_str = std::str::from_utf8(&data[..40]).unwrap_or("");
        let _ = hex::decode(hex_str);
    }

    // 6. Extremely large JSON arrays (tests allocation limits)
    if data.len() > 4 {
        let _ = serde_json::from_slice::<Vec<SignedTransaction>>(data);
    }
});
