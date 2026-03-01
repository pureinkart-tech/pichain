use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, info, warn};

use super::{ChainMonitor, Deposit};
use crate::config::BitcoinConfig;

/// Monitors Bitcoin (Testnet) for BTC deposits via Blockstream REST API.
/// No bitcoin crate needed — uses plain HTTP.
pub struct BitcoinMonitor {
    client: reqwest::Client,
    api_url: String,
    custodial_address: String,
    confirmations: u64,
    last_seen_txid: Option<String>,
    enabled: bool,
}

#[derive(Deserialize, Debug)]
struct BtcTransaction {
    txid: String,
    status: BtcTxStatus,
    vout: Vec<BtcVout>,
}

#[derive(Deserialize, Debug)]
struct BtcTxStatus {
    confirmed: bool,
    block_height: Option<u64>,
}

#[derive(Deserialize, Debug)]
struct BtcVout {
    scriptpubkey: Option<String>,
    scriptpubkey_address: Option<String>,
    scriptpubkey_asm: Option<String>,
    scriptpubkey_type: Option<String>,
    value: u64,
}

#[derive(Deserialize, Debug)]
struct BlockTip {
    height: u64,
}

impl BitcoinMonitor {
    pub fn new(config: &BitcoinConfig) -> Self {
        if !config.enabled || config.custodial_address.is_empty() {
            return Self {
                client: reqwest::Client::new(),
                api_url: String::new(),
                custodial_address: String::new(),
                confirmations: 6,
                last_seen_txid: None,
                enabled: false,
            };
        }

        info!(
            api = %config.api_url,
            address = %config.custodial_address,
            "Bitcoin monitor initialized"
        );

        Self {
            client: reqwest::Client::new(),
            api_url: config.api_url.trim_end_matches('/').to_string(),
            custodial_address: config.custodial_address.clone(),
            confirmations: config.confirmations,
            last_seen_txid: None,
            enabled: true,
        }
    }

    /// Get the current chain tip height.
    async fn get_tip_height(&self) -> anyhow::Result<u64> {
        let url = format!("{}/blocks/tip/height", self.api_url);
        let resp = self.client.get(&url).send().await?;
        let height: u64 = resp.text().await?.trim().parse()?;
        Ok(height)
    }

    /// Extract PIChain address from OP_RETURN output.
    /// OP_RETURN data is hex-encoded in scriptpubkey after "6a" (OP_RETURN) + push length.
    fn extract_recipient_from_op_return(vouts: &[BtcVout]) -> Option<String> {
        for vout in vouts {
            let script_type = vout.scriptpubkey_type.as_deref().unwrap_or("");
            if script_type != "op_return" {
                continue;
            }

            // Parse OP_RETURN data from scriptpubkey hex
            let script_hex = match &vout.scriptpubkey {
                Some(s) => s,
                None => continue,
            };

            // OP_RETURN format: 6a (OP_RETURN) + push_len + data
            // For 20 bytes of PIChain address: "6a14" + 40 hex chars
            if script_hex.len() < 4 {
                continue;
            }
            if !script_hex.starts_with("6a") {
                continue;
            }

            // Skip OP_RETURN (6a) and push length byte(s)
            let data_start = if script_hex.len() > 4 {
                // Single byte push: 6a + XX (push length) + data
                4
            } else {
                continue;
            };

            let data_hex = &script_hex[data_start..];
            // PIChain address is 20 bytes = 40 hex chars
            if data_hex.len() >= 40 {
                let addr = &data_hex[..40];
                if addr.chars().all(|c| c.is_ascii_hexdigit()) {
                    // Verify it's not all zeros
                    if !addr.chars().all(|c| c == '0') {
                        return Some(addr.to_lowercase());
                    }
                }
            }
        }
        None
    }

    /// Convert satoshi (8 decimals) to PIChain base units (9 decimals).
    /// Multiplies by 10.
    fn satoshi_to_pichain(satoshi: u64) -> u64 {
        satoshi.saturating_mul(10)
    }
}

#[async_trait]
impl ChainMonitor for BitcoinMonitor {
    async fn poll_deposits(&mut self) -> anyhow::Result<Vec<Deposit>> {
        if !self.enabled {
            return Ok(vec![]);
        }

        // Get chain tip height for confirmation counting
        let tip_height = match self.get_tip_height().await {
            Ok(h) => h,
            Err(e) => {
                warn!(error = %e, "failed to get Bitcoin tip height");
                return Ok(vec![]);
            }
        };

        // Get transactions for our custodial address
        let url = format!("{}/address/{}/txs", self.api_url, self.custodial_address);
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "failed to get Bitcoin transactions");
                return Ok(vec![]);
            }
        };

        let txs: Vec<BtcTransaction> = match resp.json().await {
            Ok(t) => t,
            Err(e) => {
                warn!(error = %e, "failed to parse Bitcoin transactions");
                return Ok(vec![]);
            }
        };

        debug!(count = txs.len(), "fetched Bitcoin transactions");

        let mut deposits = Vec::new();

        for tx in &txs {
            // Skip unconfirmed
            if !tx.status.confirmed {
                continue;
            }

            let block_height = match tx.status.block_height {
                Some(h) => h,
                None => continue,
            };

            // Check confirmations
            let confs = tip_height.saturating_sub(block_height) + 1;
            if confs < self.confirmations {
                continue;
            }

            // Calculate total value sent to our custodial address
            let mut total_value: u64 = 0;
            for vout in &tx.vout {
                let addr = vout.scriptpubkey_address.as_deref().unwrap_or("");
                if addr == self.custodial_address {
                    total_value = total_value.saturating_add(vout.value);
                }
            }

            if total_value == 0 {
                continue;
            }

            // Extract PIChain recipient from OP_RETURN
            let recipient = match Self::extract_recipient_from_op_return(&tx.vout) {
                Some(r) => r,
                None => {
                    warn!(
                        txid = %tx.txid,
                        value = total_value,
                        "BTC deposit without OP_RETURN PIChain address — skipping"
                    );
                    continue;
                }
            };

            let amount = Self::satoshi_to_pichain(total_value);

            info!(
                txid = %tx.txid,
                satoshi = total_value,
                amount_pi = amount,
                recipient = %recipient,
                confirmations = confs,
                "detected BTC deposit"
            );

            deposits.push(Deposit {
                chain: "bitcoin".to_string(),
                tx_hash: tx.txid.clone(),
                recipient,
                amount,
                symbol: "WBTC".to_string(),
                confirmations: confs,
            });
        }

        // Track last seen
        if let Some(first) = txs.first() {
            self.last_seen_txid = Some(first.txid.clone());
        }

        Ok(deposits)
    }

    fn chain_name(&self) -> &str {
        "bitcoin"
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}
