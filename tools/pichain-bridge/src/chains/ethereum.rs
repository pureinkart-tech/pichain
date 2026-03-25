use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::{ChainMonitor, Deposit};
use crate::config::ChainConfig;

/// Monitors Ethereum (Sepolia testnet) for ETH deposits via raw JSON-RPC.
/// No alloy dependency — uses plain reqwest HTTP calls.
pub struct EthereumMonitor {
    client: reqwest::Client,
    rpc_url: String,
    custodial_address: String,
    confirmations: u64,
    last_processed_block: u64,
    enabled: bool,
}

#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize, Debug)]
struct JsonRpcError {
    message: String,
}

#[derive(Deserialize, Debug)]
struct EthBlock {
    #[allow(dead_code)]
    number: Option<String>,
    transactions: Vec<EthTransaction>,
}

#[derive(Deserialize, Debug)]
struct EthTransaction {
    hash: String,
    from: Option<String>,
    to: Option<String>,
    value: String,
    input: String,
}

impl EthereumMonitor {
    pub async fn new(config: &ChainConfig) -> anyhow::Result<Self> {
        if !config.enabled || config.rpc_url.is_empty() {
            return Ok(Self {
                client: reqwest::Client::new(),
                rpc_url: String::new(),
                custodial_address: String::new(),
                confirmations: 12,
                last_processed_block: 0,
                enabled: false,
            });
        }

        let client = reqwest::Client::new();

        // Get current block number to start from
        let current_block = Self::get_block_number_with(&client, &config.rpc_url)
            .await
            .unwrap_or(0);
        let start_block = current_block.saturating_sub(config.confirmations);

        let custodial = config.custodial_address.to_lowercase();
        let custodial = custodial
            .strip_prefix("0x")
            .unwrap_or(&custodial)
            .to_string();

        info!(
            rpc = %config.rpc_url,
            address = %config.custodial_address,
            start_block = start_block,
            "Ethereum monitor initialized"
        );

        Ok(Self {
            client,
            rpc_url: config.rpc_url.clone(),
            custodial_address: custodial,
            confirmations: config.confirmations,
            last_processed_block: start_block,
            enabled: true,
        })
    }

    async fn rpc_call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<T> {
        Self::rpc_call_with(&self.client, &self.rpc_url, method, params).await
    }

    async fn rpc_call_with<T: serde::de::DeserializeOwned>(
        client: &reqwest::Client,
        rpc_url: &str,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<T> {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: method.to_string(),
            params,
        };

        let resp = client.post(rpc_url).json(&req).send().await?;
        let body: JsonRpcResponse<T> = resp.json().await?;

        if let Some(err) = body.error {
            anyhow::bail!("ETH RPC error: {}", err.message);
        }
        body.result
            .ok_or_else(|| anyhow::anyhow!("null RPC result"))
    }

    async fn get_block_number_with(client: &reqwest::Client, rpc_url: &str) -> anyhow::Result<u64> {
        let hex_str: String =
            Self::rpc_call_with(client, rpc_url, "eth_blockNumber", serde_json::json!([])).await?;
        parse_hex_u64(&hex_str)
    }

    async fn get_block_number(&self) -> anyhow::Result<u64> {
        Self::get_block_number_with(&self.client, &self.rpc_url).await
    }

    /// Extract PIChain recipient from transaction calldata.
    /// Expected format: first 20 bytes of calldata = PIChain address.
    fn extract_recipient(input: &str) -> Option<String> {
        let input = input.strip_prefix("0x").unwrap_or(input);
        if input.len() < 40 {
            return None;
        }
        let addr = &input[..40];
        if addr.chars().all(|c| c == '0') {
            return None;
        }
        if addr.chars().all(|c| c.is_ascii_hexdigit()) {
            Some(addr.to_lowercase())
        } else {
            None
        }
    }

    /// Convert ETH wei (18 decimals) to PIChain base units (9 decimals).
    /// Divides by 10^9.
    fn wei_to_pichain(wei: u128) -> u64 {
        let divisor: u128 = 1_000_000_000; // 10^9
        let result = wei / divisor;
        if result > u64::MAX as u128 {
            u64::MAX
        } else {
            result as u64
        }
    }
}

/// Parse "0x..." hex string to u64.
fn parse_hex_u64(s: &str) -> anyhow::Result<u64> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    Ok(u64::from_str_radix(s, 16)?)
}

/// Parse "0x..." hex string to u128 (for wei values).
fn parse_hex_u128(s: &str) -> anyhow::Result<u128> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    Ok(u128::from_str_radix(s, 16)?)
}

#[async_trait]
impl ChainMonitor for EthereumMonitor {
    async fn poll_deposits(&mut self) -> anyhow::Result<Vec<Deposit>> {
        if !self.enabled {
            return Ok(vec![]);
        }

        let latest = match self.get_block_number().await {
            Ok(n) => n,
            Err(e) => {
                warn!(error = %e, "failed to get Ethereum block number");
                return Ok(vec![]);
            }
        };

        let safe_block = latest.saturating_sub(self.confirmations);
        if safe_block <= self.last_processed_block {
            return Ok(vec![]);
        }

        let mut deposits = Vec::new();
        let from_block = self.last_processed_block + 1;
        let to_block = safe_block.min(from_block + 100); // Max 100 blocks per poll

        debug!(from = from_block, to = to_block, "scanning Ethereum blocks");

        for block_num in from_block..=to_block {
            let block_hex = format!("0x{:x}", block_num);
            let block: EthBlock = match self
                .rpc_call("eth_getBlockByNumber", serde_json::json!([block_hex, true]))
                .await
            {
                Ok(b) => b,
                Err(e) => {
                    warn!(block = block_num, error = %e, "failed to get block");
                    break;
                }
            };

            for tx in &block.transactions {
                // Check if this transaction sends to our custodial address
                let to_addr = match &tx.to {
                    Some(addr) => addr.strip_prefix("0x").unwrap_or(addr).to_lowercase(),
                    None => continue,
                };

                if to_addr != self.custodial_address {
                    continue;
                }

                let value = match parse_hex_u128(&tx.value) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                if value == 0 {
                    continue;
                }

                // Extract PIChain recipient from calldata
                let recipient = match Self::extract_recipient(&tx.input) {
                    Some(r) => r,
                    None => {
                        warn!(
                            tx_hash = %tx.hash,
                            "ETH deposit to custodial address without PIChain recipient in calldata — skipping"
                        );
                        continue;
                    }
                };

                let amount = Self::wei_to_pichain(value);
                if amount == 0 {
                    continue;
                }

                let confs = latest.saturating_sub(block_num);
                let from_addr = tx.from.as_deref().unwrap_or("unknown");

                info!(
                    tx_hash = %tx.hash,
                    from = from_addr,
                    amount_wei = %value,
                    amount_pi = amount,
                    recipient = %recipient,
                    confirmations = confs,
                    "detected ETH deposit"
                );

                deposits.push(Deposit {
                    chain: "ethereum".to_string(),
                    tx_hash: tx.hash.clone(),
                    recipient,
                    amount,
                    symbol: "WETH".to_string(),
                    confirmations: confs,
                });
            }
        }

        self.last_processed_block = to_block;
        Ok(deposits)
    }

    fn chain_name(&self) -> &str {
        "ethereum"
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}
