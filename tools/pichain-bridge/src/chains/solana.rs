use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::{ChainMonitor, Deposit};
use crate::config::ChainConfig;

/// Monitors Solana (Devnet) for SOL deposits via JSON-RPC (no solana-sdk dependency).
pub struct SolanaMonitor {
    client: reqwest::Client,
    rpc_url: String,
    custodial_address: String,
    confirmations: u64,
    last_signature: Option<String>,
    enabled: bool,
}

// JSON-RPC request/response types

#[derive(Serialize)]
struct RpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Deserialize, Debug)]
struct RpcError {
    message: String,
}

#[derive(Deserialize, Debug)]
struct SignatureInfo {
    signature: String,
    #[allow(dead_code)]
    slot: Option<u64>,
    err: Option<serde_json::Value>,
    memo: Option<String>,
    #[serde(rename = "confirmationStatus")]
    confirmation_status: Option<String>,
}

#[derive(Deserialize, Debug)]
struct TransactionResponse {
    #[allow(dead_code)]
    slot: u64,
    meta: Option<TransactionMeta>,
    transaction: Option<TransactionData>,
}

#[derive(Deserialize, Debug)]
struct TransactionMeta {
    #[serde(rename = "preBalances")]
    pre_balances: Vec<u64>,
    #[serde(rename = "postBalances")]
    post_balances: Vec<u64>,
    err: Option<serde_json::Value>,
    #[serde(rename = "logMessages")]
    log_messages: Option<Vec<String>>,
}

#[derive(Deserialize, Debug)]
struct TransactionData {
    message: TransactionMessage,
    #[allow(dead_code)]
    signatures: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct TransactionMessage {
    #[serde(rename = "accountKeys")]
    account_keys: Vec<String>,
}

impl SolanaMonitor {
    pub fn new(config: &ChainConfig) -> Self {
        if !config.enabled || config.rpc_url.is_empty() {
            return Self {
                client: reqwest::Client::new(),
                rpc_url: String::new(),
                custodial_address: String::new(),
                confirmations: 32,
                last_signature: None,
                enabled: false,
            };
        }

        info!(
            rpc = %config.rpc_url,
            address = %config.custodial_address,
            "Solana monitor initialized"
        );

        Self {
            client: reqwest::Client::new(),
            rpc_url: config.rpc_url.clone(),
            custodial_address: config.custodial_address.clone(),
            confirmations: config.confirmations,
            last_signature: None,
            enabled: true,
        }
    }

    async fn rpc_call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<T> {
        let req = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: method.to_string(),
            params,
        };

        let resp = self.client.post(&self.rpc_url).json(&req).send().await?;

        let body: RpcResponse<T> = resp.json().await?;
        if let Some(err) = body.error {
            anyhow::bail!("Solana RPC error: {}", err.message);
        }
        body.result
            .ok_or_else(|| anyhow::anyhow!("null RPC result"))
    }

    /// Extract PIChain address from memo instruction in Solana transaction.
    /// The memo should be a 40-char hex string (PIChain address).
    fn extract_recipient_from_memo(memo: &str) -> Option<String> {
        let cleaned = memo.trim();
        // Memo might have prefix like "[1] " from Solana memo program
        let addr = if let Some(pos) = cleaned.rfind(' ') {
            &cleaned[pos + 1..]
        } else {
            cleaned
        };
        // Validate: 40 hex chars
        let addr = addr.strip_prefix("0x").unwrap_or(addr);
        if addr.len() == 40 && addr.chars().all(|c| c.is_ascii_hexdigit()) {
            Some(addr.to_lowercase())
        } else {
            None
        }
    }
}

#[async_trait]
impl ChainMonitor for SolanaMonitor {
    async fn poll_deposits(&mut self) -> anyhow::Result<Vec<Deposit>> {
        if !self.enabled {
            return Ok(vec![]);
        }

        // Get recent signatures for our custodial address
        let mut params = serde_json::json!([
            self.custodial_address,
            {
                "limit": 50,
                "commitment": "confirmed"
            }
        ]);

        // If we have a last signature, only get newer ones
        if let Some(ref sig) = self.last_signature {
            params[1]["until"] = serde_json::json!(sig);
        }

        let signatures: Vec<SignatureInfo> =
            match self.rpc_call("getSignaturesForAddress", params).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, "failed to get Solana signatures");
                    return Ok(vec![]);
                }
            };

        if signatures.is_empty() {
            return Ok(vec![]);
        }

        debug!(count = signatures.len(), "found Solana signatures");

        let mut deposits = Vec::new();

        // Process signatures (they come newest-first, so iterate in reverse)
        for sig_info in signatures.iter().rev() {
            // Skip failed transactions
            if sig_info.err.is_some() {
                continue;
            }

            // Only process finalized/confirmed
            let status = sig_info.confirmation_status.as_deref().unwrap_or("unknown");
            if status != "finalized" && status != "confirmed" {
                continue;
            }

            // Get full transaction details
            let tx_params = serde_json::json!([
                sig_info.signature,
                {
                    "encoding": "json",
                    "commitment": "confirmed",
                    "maxSupportedTransactionVersion": 0
                }
            ]);

            let tx: TransactionResponse = match self.rpc_call("getTransaction", tx_params).await {
                Ok(t) => t,
                Err(e) => {
                    warn!(sig = %sig_info.signature, error = %e, "failed to get transaction");
                    continue;
                }
            };

            let meta = match &tx.meta {
                Some(m) if m.err.is_none() => m,
                _ => continue,
            };

            let tx_data = match &tx.transaction {
                Some(t) => t,
                None => continue,
            };

            // Find our custodial address in account keys
            let our_index = tx_data
                .message
                .account_keys
                .iter()
                .position(|k| k == &self.custodial_address);

            let our_index = match our_index {
                Some(i) => i,
                None => continue,
            };

            // Calculate SOL received (post - pre balance)
            if our_index >= meta.pre_balances.len() || our_index >= meta.post_balances.len() {
                continue;
            }

            let pre = meta.pre_balances[our_index];
            let post = meta.post_balances[our_index];
            if post <= pre {
                continue; // No deposit (sent or no change)
            }

            let lamports = post - pre;
            // SOL has 9 decimals, same as PIChain — no conversion needed
            let amount = lamports;

            if amount == 0 {
                continue;
            }

            // Extract PIChain recipient from memo
            let recipient = sig_info
                .memo
                .as_deref()
                .and_then(Self::extract_recipient_from_memo);

            let recipient = match recipient {
                Some(r) => r,
                None => {
                    // Check log messages for memo
                    let from_logs = meta.log_messages.as_ref().and_then(|logs| {
                        logs.iter()
                            .find(|l| l.contains("Memo") || l.contains("memo"))
                            .and_then(|l| {
                                // Extract hex address from log
                                let parts: Vec<&str> = l.split_whitespace().collect();
                                parts.last().and_then(|s| {
                                    let s = s.trim_matches('"');
                                    Self::extract_recipient_from_memo(s)
                                })
                            })
                    });

                    match from_logs {
                        Some(r) => r,
                        None => {
                            warn!(
                                sig = %sig_info.signature,
                                lamports = lamports,
                                "SOL deposit without PIChain recipient memo — skipping"
                            );
                            continue;
                        }
                    }
                }
            };

            info!(
                sig = %sig_info.signature,
                lamports = lamports,
                amount_pi = amount,
                recipient = %recipient,
                "detected SOL deposit"
            );

            deposits.push(Deposit {
                chain: "solana".to_string(),
                tx_hash: sig_info.signature.clone(),
                recipient,
                amount,
                symbol: "WSOL".to_string(),
                confirmations: self.confirmations,
            });
        }

        // Update last signature to newest
        if let Some(newest) = signatures.first() {
            self.last_signature = Some(newest.signature.clone());
        }

        Ok(deposits)
    }

    fn chain_name(&self) -> &str {
        "solana"
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}
