//! Chrome Native Messaging Protocol for PIChain Signer.
//!
//! Chrome extensions communicate with native apps via stdin/stdout using
//! a simple protocol: 4-byte little-endian length prefix followed by JSON.
//!
//! This module implements the protocol so the browser extension can use
//! pichain-signer for PQ transaction signing without an HTTP server.

use pichain_crypto::PqKeypair;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// Maximum message size (1MB — Chrome's limit)
const MAX_MSG_SIZE: u32 = 1024 * 1024;

#[derive(Deserialize)]
struct NativeRequest {
    id: Option<u64>,
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    tx_data: Option<serde_json::Value>,
    #[serde(default)]
    password: Option<String>,
}

#[derive(Serialize)]
struct NativeResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unlocked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signed_tx: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

/// Read a native messaging message from stdin.
fn read_message() -> Result<NativeRequest, String> {
    let mut stdin = std::io::stdin().lock();

    // Read 4-byte length prefix (little-endian)
    let mut len_bytes = [0u8; 4];
    stdin
        .read_exact(&mut len_bytes)
        .map_err(|e| format!("stdin read error: {e}"))?;
    let len = u32::from_le_bytes(len_bytes);

    if len == 0 || len > MAX_MSG_SIZE {
        return Err(format!("invalid message length: {len}"));
    }

    // Read JSON body
    let mut buf = vec![0u8; len as usize];
    stdin
        .read_exact(&mut buf)
        .map_err(|e| format!("stdin body read error: {e}"))?;

    serde_json::from_slice(&buf).map_err(|e| format!("invalid JSON: {e}"))
}

/// Write a native messaging response to stdout.
fn write_response(resp: &NativeResponse) {
    let json = match serde_json::to_vec(resp) {
        Ok(j) => j,
        Err(_) => br#"{"error":"internal serialization error"}"#.to_vec(),
    };
    let len = json.len() as u32;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&len.to_le_bytes()).ok();
    stdout.write_all(&json).ok();
    stdout.flush().ok();
}

/// Run the native messaging loop.
///
/// This replaces the HTTP server when pichain-signer is invoked as a
/// Chrome native messaging host. It reads requests from stdin and
/// writes responses to stdout using the 4-byte length-prefix protocol.
pub fn run_native_messaging(pq_keypair: &PqKeypair, chain_id: u64) {
    let address_hex = hex::encode(pq_keypair.address().0);
    let mut msg_timestamps: Vec<u64> = Vec::new();
    const MAX_MSGS_PER_SECOND: usize = 10;

    loop {
        let req = match read_message() {
            Ok(r) => r,
            Err(_) => break, // stdin closed — extension disconnected
        };

        // Rate limit: max 10 messages/second to prevent DoS
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        msg_timestamps.retain(|&t| now_ms.saturating_sub(t) < 1000);
        if msg_timestamps.len() >= MAX_MSGS_PER_SECOND {
            write_response(&NativeResponse {
                id: req.id,
                address: None,
                connected: None,
                unlocked: None,
                ok: None,
                signed_tx: None,
                tx_hash: None,
                error: Some("rate limit exceeded".to_string()),
                version: None,
            });
            continue;
        }
        msg_timestamps.push(now_ms);

        let resp = match req.msg_type.as_str() {
            "status" => NativeResponse {
                id: req.id,
                address: Some(address_hex.clone()),
                connected: Some(true),
                unlocked: Some(true), // pichain-signer is always unlocked (keys in memory)
                ok: None,
                signed_tx: None,
                tx_hash: None,
                error: None,
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            },

            "sign" => match handle_sign(&pq_keypair, &address_hex, chain_id, req.tx_data) {
                Ok((signed_tx, tx_hash)) => NativeResponse {
                    id: req.id,
                    address: None,
                    connected: None,
                    unlocked: None,
                    ok: Some(true),
                    signed_tx: Some(signed_tx),
                    tx_hash: Some(tx_hash),
                    error: None,
                    version: None,
                },
                Err(e) => NativeResponse {
                    id: req.id,
                    address: None,
                    connected: None,
                    unlocked: None,
                    ok: Some(false),
                    signed_tx: None,
                    tx_hash: None,
                    error: Some(e),
                    version: None,
                },
            },

            "ping" => NativeResponse {
                id: req.id,
                address: None,
                connected: Some(true),
                unlocked: None,
                ok: Some(true),
                signed_tx: None,
                tx_hash: None,
                error: None,
                version: None,
            },

            other => NativeResponse {
                id: req.id,
                address: None,
                connected: None,
                unlocked: None,
                ok: None,
                signed_tx: None,
                tx_hash: None,
                error: Some(format!("unknown message type: {other}")),
                version: None,
            },
        };

        write_response(&resp);
    }
}

fn handle_sign(
    pq_keypair: &PqKeypair,
    address_hex: &str,
    chain_id: u64,
    tx_data_value: Option<serde_json::Value>,
) -> Result<(serde_json::Value, String), String> {
    let mut tx_value = tx_data_value.ok_or("missing tx_data")?;

    // Handle sender as hex string (from browser)
    if let Some(sender_str) = tx_value.get("sender").and_then(|s| s.as_str()) {
        let sender_hex = sender_str.strip_prefix("0x").unwrap_or(sender_str);
        if sender_hex != address_hex {
            return Err(format!(
                "sender {} does not match wallet {}",
                sender_hex, address_hex
            ));
        }
        let bytes = hex::decode(sender_hex).map_err(|e| format!("bad sender hex: {e}"))?;
        if bytes.len() != 20 {
            return Err("sender must be 20 bytes".to_string());
        }
        tx_value["sender"] = serde_json::json!(bytes);
    }

    super::convert_hex_strings_to_arrays(&mut tx_value);

    let tx_data: pichain_types::transaction::TransactionData =
        serde_json::from_value(tx_value).map_err(|e| format!("invalid tx_data: {e}"))?;

    if tx_data.chain_id != chain_id && tx_data.chain_id != 0 {
        return Err(format!(
            "chain_id {} does not match {}",
            tx_data.chain_id, chain_id
        ));
    }

    let signed = pichain_types::transaction::Transaction::sign_pq(tx_data, pq_keypair);
    let tx_hash = hex::encode(signed.hash().as_bytes());
    let signed_json = serde_json::to_value(&signed).map_err(|e| format!("serialize error: {e}"))?;

    Ok((signed_json, tx_hash))
}
