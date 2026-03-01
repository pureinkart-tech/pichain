use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct BridgeConfig {
    pub general: GeneralConfig,
    #[serde(default)]
    pub ethereum: ChainConfig,
    #[serde(default)]
    pub solana: ChainConfig,
    #[serde(default)]
    pub bitcoin: BitcoinConfig,
}

#[derive(Debug, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_pichain_rpc")]
    pub pichain_rpc: String,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
pub struct ChainConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub rpc_url: String,
    #[serde(default)]
    pub custodial_address: String,
    #[serde(default = "default_eth_confirmations")]
    pub confirmations: u64,
}

#[derive(Debug, Default, Deserialize)]
pub struct BitcoinConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_btc_api")]
    pub api_url: String,
    #[serde(default)]
    pub custodial_address: String,
    #[serde(default = "default_btc_confirmations")]
    pub confirmations: u64,
}

fn default_pichain_rpc() -> String {
    "http://127.0.0.1:8314".to_string()
}

fn default_poll_interval() -> u64 {
    15
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("./bridge-data")
}

fn default_eth_confirmations() -> u64 {
    12
}

fn default_btc_api() -> String {
    "https://blockstream.info/testnet/api".to_string()
}

fn default_btc_confirmations() -> u64 {
    6
}

impl BridgeConfig {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: BridgeConfig = toml::from_str(&content)?;
        Ok(config)
    }
}
