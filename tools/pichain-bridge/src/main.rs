mod chains;
mod config;
mod deposit_store;
mod pichain_client;
mod relayer;

use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::chains::bitcoin::BitcoinMonitor;
use crate::chains::ethereum::EthereumMonitor;
use crate::chains::solana::SolanaMonitor;
use crate::chains::ChainMonitor;
use crate::config::BridgeConfig;
use crate::deposit_store::DepositStore;
use crate::pichain_client::PiChainClient;
use crate::relayer::Relayer;

#[derive(Parser)]
#[command(name = "pichain-bridge", about = "PIChain cross-chain bridge relayer")]
struct Cli {
    /// Path to bridge configuration file
    #[arg(short, long, default_value = "bridge-config.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    info!(config = %cli.config, "loading bridge configuration");
    let config = BridgeConfig::load(&cli.config)?;

    // Create data directory
    std::fs::create_dir_all(&config.general.data_dir)?;

    // Initialize components
    let client = PiChainClient::new(&config.general.pichain_rpc);
    let store_path = config.general.data_dir.join("deposits.ndjson");
    let store = DepositStore::load(store_path)?;

    // Initialize chain monitors
    let mut monitors: Vec<Box<dyn ChainMonitor>> = Vec::new();

    // Ethereum
    let eth_monitor = EthereumMonitor::new(&config.ethereum).await?;
    if eth_monitor.is_enabled() {
        info!("Ethereum monitor enabled");
    }
    monitors.push(Box::new(eth_monitor));

    // Solana
    let sol_monitor = SolanaMonitor::new(&config.solana);
    if sol_monitor.is_enabled() {
        info!("Solana monitor enabled");
    }
    monitors.push(Box::new(sol_monitor));

    // Bitcoin
    let btc_monitor = BitcoinMonitor::new(&config.bitcoin);
    if btc_monitor.is_enabled() {
        info!("Bitcoin monitor enabled");
    }
    monitors.push(Box::new(btc_monitor));

    // Register custodial addresses with PIChain node
    let _ = client
        .register_addresses(
            &config.ethereum.custodial_address,
            &config.solana.custodial_address,
            &config.bitcoin.custodial_address,
        )
        .await;

    // Start relayer
    let mut relayer = Relayer::new(
        client,
        store,
        monitors,
        config.general.poll_interval_secs,
    );

    info!("=== PIChain Bridge Relayer started ===");
    relayer.run().await
}
