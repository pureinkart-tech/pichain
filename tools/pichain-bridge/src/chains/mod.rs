pub mod bitcoin;
pub mod ethereum;
pub mod solana;

use async_trait::async_trait;

/// A deposit detected on an external chain.
#[derive(Debug, Clone)]
pub struct Deposit {
    /// External chain name (e.g., "ethereum", "solana", "bitcoin")
    pub chain: String,
    /// Transaction hash on the external chain
    pub tx_hash: String,
    /// PIChain recipient address (40 hex chars, no 0x prefix)
    pub recipient: String,
    /// Amount in PIChain base units (9 decimals)
    pub amount: u64,
    /// Wrapped token symbol to mint (e.g., "WETH", "WSOL", "WBTC", "WUSDT")
    pub symbol: String,
    #[allow(dead_code)]
    /// Number of confirmations on the external chain
    pub confirmations: u64,
}

/// Trait for monitoring an external chain for deposits.
#[async_trait]
pub trait ChainMonitor: Send {
    /// Poll for new confirmed deposits since last check.
    async fn poll_deposits(&mut self) -> anyhow::Result<Vec<Deposit>>;

    /// Chain name for logging.
    fn chain_name(&self) -> &str;

    /// Whether this monitor is enabled.
    fn is_enabled(&self) -> bool;
}
