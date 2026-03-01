use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// HTTP client for calling the PIChain node's bridge mint endpoint.
pub struct PiChainClient {
    base_url: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct MintRequest {
    symbol: String,
    recipient: String,
    amount: u64,
    chain: String,
    tx_hash: String,
}

#[derive(Deserialize, Debug)]
struct MintResponse {
    success: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Serialize)]
struct RegisterAddressesRequest {
    eth: String,
    sol: String,
    btc: String,
    usdt: String,
}

#[derive(Deserialize, Debug)]
pub struct HealthResponse {
    #[serde(default)]
    pub height: u64,
}

impl PiChainClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Mint wrapped tokens to a PIChain address.
    /// Calls POST /api/v1/bridge/mint (localhost-only).
    pub async fn mint(
        &self,
        symbol: &str,
        recipient: &str,
        amount: u64,
        chain: &str,
        tx_hash: &str,
    ) -> Result<()> {
        let url = format!("{}/api/v1/bridge/mint", self.base_url);
        let req = MintRequest {
            symbol: symbol.to_string(),
            recipient: recipient.to_string(),
            amount,
            chain: chain.to_string(),
            tx_hash: tx_hash.to_string(),
        };

        let resp = self
            .client
            .post(&url)
            .json(&req)
            .send()
            .await
            .context("failed to call bridge/mint")?;

        let status = resp.status();
        let body: MintResponse = resp.json().await.context("failed to parse mint response")?;

        if body.success {
            info!(
                symbol = symbol,
                recipient = recipient,
                amount = amount,
                "minted wrapped tokens"
            );
            Ok(())
        } else {
            let err = body.error.unwrap_or_else(|| format!("HTTP {}", status));
            warn!(error = %err, "bridge mint failed");
            anyhow::bail!("bridge mint failed: {}", err)
        }
    }

    /// Register custodial addresses with the PIChain node.
    /// Calls POST /api/v1/bridge/register-addresses (localhost-only).
    pub async fn register_addresses(
        &self,
        eth: &str,
        sol: &str,
        btc: &str,
    ) -> Result<()> {
        let url = format!("{}/api/v1/bridge/register-addresses", self.base_url);
        let req = RegisterAddressesRequest {
            eth: eth.to_string(),
            sol: sol.to_string(),
            btc: btc.to_string(),
            usdt: eth.to_string(), // USDT uses same ETH address
        };

        let resp = self
            .client
            .post(&url)
            .json(&req)
            .send()
            .await
            .context("failed to register addresses")?;

        if resp.status().is_success() {
            info!("registered custodial addresses with PIChain node");
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            warn!(body = %body, "address registration failed (endpoint may not exist yet)");
            // Non-fatal: the endpoint might not exist yet
            Ok(())
        }
    }

    /// Check if the PIChain node is reachable.
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/api/v1/mining/status", self.base_url);
        match self.client.get(&url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }
}
