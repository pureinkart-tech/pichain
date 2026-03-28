//! PIChain Miner — compute PI digits and earn mining rewards.
//!
//! This tool connects to a running PIChain node via RPC, queries the
//! current mining frontier, computes new PI hex digits using the BBP
//! algorithm, and submits mining proof transactions to earn rewards.
//!
//! Usage:
//!   pichain-miner --keypair wallet.json --rpc-url https://pichain.net

use clap::Parser;

use pichain_mining::bbp::BbpComputer;
use pichain_types::transaction::{Transaction, TransactionData, TransactionKind};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{error, info, warn};

/// PIChain Miner — Computing the Infinite.
///
/// Mine PI digits and earn PI tokens! Anyone can mine — from a laptop
/// to a data center. Use --profile to automatically configure for your hardware:
///
///   pichain-miner --keypair wallet.json                  # auto-detect
///   pichain-miner --keypair wallet.json --profile low    # light usage
///   pichain-miner --keypair wallet.json --profile max    # maximum throughput
///
/// Or fine-tune with --threads, --digits-per-batch, --concurrent-batches.
/// Rewards are proportional to digits computed — more power = more PI, but
/// even a Raspberry Pi can earn rewards.
#[derive(Parser, Debug)]
#[command(name = "pichain-miner", version, about)]
struct Args {
    /// RPC endpoint of the PIChain node.
    /// Use https://pichain.net for the public node, or http://127.0.0.1:8314 for a local node.
    #[arg(long, default_value = "https://pichain.net")]
    rpc_url: String,

    /// Path to the keypair file (PQ wallet JSON).
    /// Required for block-pipeline mining (signs transactions locally).
    /// Not needed if --address is used (pool-submit mode).
    #[arg(long)]
    keypair: Option<PathBuf>,

    /// Mine to this address without a wallet file (pool-submit mode).
    /// The node signs on your behalf — no private keys needed on this machine.
    /// Use your PiBot address, desktop app address, or any PIChain address.
    /// Example: --address Pi314abc123...
    #[arg(long)]
    address: Option<String>,

    /// Hardware profile — auto-configures threads, batch size, and concurrency.
    ///
    /// Profiles:
    ///   auto   — optimized for your hardware (default)
    ///   low    — 2 threads, light CPU usage
    ///   normal — half your cores, balanced
    ///   max    — all cores, large batches, maximum throughput
    ///
    /// You can override individual settings after --profile.
    #[arg(long, value_parser = ["auto", "low", "normal", "max"])]
    profile: Option<String>,

    /// Number of hex digits to compute per batch.
    /// More digits per batch = more reward per proof but takes longer.
    /// Overrides the profile setting if both are specified.
    #[arg(long)]
    digits_per_batch: Option<u32>,

    /// Chain ID (314159 for mainnet, 31415 for devnet).
    #[arg(long, default_value = "31415")]
    chain_id: u64,

    /// Interval between mining rounds in seconds (0 = no delay).
    #[arg(long, default_value = "1")]
    interval_secs: u64,

    /// Generate a new keypair and save it (then exit).
    #[arg(long)]
    generate_keypair: bool,

    /// Show wallet info (address, balance) without mining, then exit.
    #[arg(long)]
    info: bool,

    /// Position offset for multi-miner operation.
    /// Each miner should use a different offset (e.g., 0, 1, 2...)
    /// to avoid computing the same range as other miners.
    #[arg(long, default_value = "0")]
    position_offset: u32,

    /// Override: start mining at this exact position (ignores server frontier).
    #[arg(long)]
    start_at: Option<u64>,

    /// Number of CPU threads to use for parallel digit computation.
    /// Overrides the profile setting if both are specified.
    #[arg(long)]
    threads: Option<usize>,

    /// Run multiple concurrent batches (multiplies throughput).
    /// Each batch computes digits_per_batch digits at different positions.
    /// Overrides the profile setting if both are specified.
    #[arg(long)]
    concurrent_batches: Option<u32>,

    /// Limit maximum CPU usage percentage (approximate).
    /// E.g., --max-cpu 50 uses ~half your CPU.
    #[arg(long)]
    max_cpu: Option<u32>,
}

/// Resolved mining configuration after applying profile + overrides.
struct MiningConfig {
    threads: usize,
    digits_per_batch: u32,
    concurrent_batches: u32,
}

impl MiningConfig {
    fn from_args(args: &Args) -> Self {
        let total_cores = num_cpus::get();

        // Start with profile defaults
        let (prof_threads, prof_digits, prof_batches) = match args.profile.as_deref() {
            Some("low") => (2.min(total_cores), 200u32, 1u32),
            Some("normal") => ((total_cores / 2).max(2), 500, 1),
            Some("max") => (total_cores, 5000, 8),
            _ => {
                // Auto-detect optimal settings based on core count
                if total_cores <= 4 {
                    (total_cores, 500, 1)
                } else if total_cores <= 16 {
                    (total_cores, 1000, 2)
                } else {
                    (total_cores, 2000, 4) // Server-class
                }
            }
        };

        // Apply explicit overrides
        let mut threads = args.threads.unwrap_or(prof_threads);
        let digits_per_batch = args.digits_per_batch.unwrap_or(prof_digits);
        let concurrent_batches = args.concurrent_batches.unwrap_or(prof_batches);

        // Apply CPU limit if specified
        if let Some(max_pct) = args.max_cpu {
            let limited = (total_cores as u64 * max_pct as u64 / 100).max(1) as usize;
            threads = threads.min(limited);
        }

        MiningConfig {
            threads,
            digits_per_batch,
            concurrent_batches,
        }
    }
}

/// Wallet file format (PQ wallets).
#[derive(Serialize, Deserialize)]
struct WalletFile {
    /// Legacy field (ignored, kept for file compat).
    #[serde(skip_serializing_if = "Option::is_none")]
    secret_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,
    /// PQ wallet version (present in PQ wallet files).
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<u32>,
    /// PQ wallet crypto version.
    #[serde(skip_serializing_if = "Option::is_none")]
    crypto_version: Option<u32>,
    /// ML-DSA secret key (PQ wallets).
    #[serde(skip_serializing_if = "Option::is_none")]
    ml_dsa_secret_key: Option<String>,
    /// ML-DSA public key (PQ wallets).
    #[serde(skip_serializing_if = "Option::is_none")]
    ml_dsa_public_key: Option<String>,
    /// SLH-DSA secret key (PQ wallets).
    #[serde(skip_serializing_if = "Option::is_none")]
    slh_dsa_secret_key: Option<String>,
    /// SLH-DSA public key (PQ wallets).
    #[serde(skip_serializing_if = "Option::is_none")]
    slh_dsa_public_key: Option<String>,
}

/// Loaded PQ wallet.
#[allow(dead_code)]
struct LoadedWallet {
    pq_keypair: pichain_crypto::PqKeypair,
}

impl LoadedWallet {
    fn address(&self) -> pichain_crypto::keys::Address {
        self.pq_keypair.address()
    }
}

/// Mining status response from the RPC.
#[derive(Deserialize, Debug)]
struct MiningStatus {
    frontier_position: u64,
    total_digits_verified: u64,
    next_position: u64,
    /// Maximum contiguous digits mineable at next_position before hitting
    /// an existing range. Miners should cap batch size to this value.
    #[serde(default = "default_max_batch")]
    max_batch_at_position: u64,
    #[serde(default)]
    reward_per_digit: u64,
    #[serde(default)]
    difficulty_bits: u32,
    #[serde(default)]
    difficulty_target_hex: String,
    #[serde(default)]
    anchor_block_hash: String,
    #[serde(default)]
    #[allow(dead_code)]
    unique_miners: u64,
    /// Minimum digits per proof at current frontier.
    #[serde(default)]
    min_batch_size: u32,
    /// Maximum position allowed (frontier + max_frontier_distance).
    #[serde(default)]
    max_allowed_position: u64,
    /// Current frontier bonus multiplier at next_position.
    #[serde(default)]
    frontier_bonus_at_next: String,
}

fn default_max_batch() -> u64 {
    u64::MAX
}

/// Slot assignment response from the RPC.
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct SlotResponse {
    address: String,
    slot_index: u32,
    recommended_position: u64,
    active_miners: usize,
    slot_range_size: u64,
}

/// Transaction submission response.
#[derive(Deserialize, Debug)]
struct SubmitResponse {
    tx_hash: String,
    status: String,
    error: Option<String>,
}

/// Account info response.
#[derive(Deserialize, Debug)]
struct AccountResponse {
    balance: u64,
    nonce: u64,
    found: bool,
    #[serde(default)]
    locked_balance: Option<u64>,
}

fn load_wallet(path: &PathBuf) -> anyhow::Result<LoadedWallet> {
    let contents = std::fs::read_to_string(path)?;
    let wallet: WalletFile = serde_json::from_str(&contents)?;

    // Require PQ wallet
    if wallet.ml_dsa_secret_key.is_none() {
        anyhow::bail!(
            "Invalid wallet file — missing PQ key fields.\n\
             Generate a new wallet with: pichain-miner --keypair wallet.json --generate-keypair"
        );
    }

    info!("Loading post-quantum wallet (ML-DSA-65 + SLH-DSA-SHAKE-128f)");
    let export = pichain_crypto::pq_wallet::PqWalletExport {
        version: wallet.version.unwrap_or(1),
        crypto_version: wallet.crypto_version.unwrap_or(1),
        address: wallet.address.unwrap_or_default(),
        ml_dsa_secret_key: wallet.ml_dsa_secret_key.unwrap_or_default(),
        ml_dsa_public_key: wallet.ml_dsa_public_key.unwrap_or_default(),
        slh_dsa_secret_key: wallet.slh_dsa_secret_key.unwrap_or_default(),
        slh_dsa_public_key: wallet.slh_dsa_public_key.unwrap_or_default(),
    };
    let pq_keypair = pichain_crypto::restore_pq_wallet(&export)
        .map_err(|e| anyhow::anyhow!("Failed to restore PQ wallet: {e}"))?;
    Ok(LoadedWallet { pq_keypair })
}

/// Try to detect pichain-signer on localhost:8315.
async fn try_detect_signer() -> Option<(pichain_crypto::keys::Address, String)> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .ok()?;
    let resp = client
        .get("http://127.0.0.1:8315/status")
        .send()
        .await
        .ok()?;
    let data: serde_json::Value = resp.json().await.ok()?;
    if data["connected"].as_bool() != Some(true) {
        return None;
    }
    let addr_str = data["address"].as_str()?;
    let mut hex = addr_str.to_string();
    if hex.starts_with("Pi314") {
        hex = hex[5..].to_string();
    }
    let addr_bytes = hex::decode(&hex).ok()?;
    if addr_bytes.len() != 20 {
        return None;
    }
    let mut arr = [0u8; 20];
    arr.copy_from_slice(&addr_bytes);
    Some((pichain_crypto::keys::Address(arr), hex.to_lowercase()))
}

/// Display wallet info: address, balance, and usage tips.
async fn show_wallet_info(
    rpc_url: &str,
    address_hex: &str,
    address: &pichain_crypto::keys::Address,
    is_pool_mode: bool,
) {
    let display_addr = format!("{}", address);
    eprintln!();
    eprintln!("  ╔══════════════════════════════════════════════════════════╗");
    eprintln!("  ║               PIChain Wallet Info                        ║");
    eprintln!("  ╠══════════════════════════════════════════════════════════╣");
    eprintln!("  ║                                                          ║");
    eprintln!("  ║  Address: {:<46} ║", display_addr);

    // Fetch balance
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    match client
        .get(format!("{}/api/v1/account/{}", rpc_url, address_hex))
        .send()
        .await
    {
        Ok(resp) => {
            if let Ok(acct) = resp.json::<AccountResponse>().await {
                let balance_pi = acct.balance as f64 / 1_000_000_000.0;
                let locked_pi = acct.locked_balance.unwrap_or(0) as f64 / 1_000_000_000.0;
                eprintln!("  ║  Balance: {:<46} ║", format!("{:.4} PI", balance_pi));
                if locked_pi > 0.0 {
                    eprintln!(
                        "  ║  Locked:  {:<46} ║",
                        format!("{:.4} PI (gas fees)", locked_pi)
                    );
                }
                eprintln!("  ║  Nonce:   {:<46} ║", acct.nonce);
            } else {
                eprintln!("  ║  Balance: (could not fetch)                              ║");
            }
        }
        Err(_) => {
            eprintln!("  ║  Balance: (could not reach node)                         ║");
        }
    }

    eprintln!("  ║                                                          ║");
    eprintln!("  ╠══════════════════════════════════════════════════════════╣");
    eprintln!("  ║  HOW TO ACCESS YOUR PI                                  ║");
    eprintln!("  ╠══════════════════════════════════════════════════════════╣");
    eprintln!("  ║                                                          ║");
    eprintln!("  ║  Website:                                                ║");
    eprintln!("  ║    Go to https://pichain.net → any tab → Connect Wallet  ║");
    eprintln!("  ║    Enter your address (above) in \"View Only\" mode       ║");
    eprintln!("  ║                                                          ║");
    eprintln!("  ║  Check balance:                                          ║");
    eprintln!("  ║    pichain-cli balance {}  ║", display_addr);
    eprintln!("  ║                                                          ║");
    if !is_pool_mode {
        eprintln!("  ║  Send PI:                                                ║");
        eprintln!("  ║    pichain-cli send --to Pi314<recipient> --amount 10     ║");
        eprintln!("  ║                                                          ║");
    }
    eprintln!("  ║  PiBot (Telegram):                                      ║");
    eprintln!("  ║    For easier wallet management, mine to your PiBot      ║");
    eprintln!("  ║    address instead:                                      ║");
    eprintln!("  ║    1. Open @PiChainTradeBot on Telegram                  ║");
    eprintln!("  ║    2. Type /wallet to see your PiBot address             ║");
    eprintln!("  ║    3. Restart miner with:                                ║");
    eprintln!("  ║       pichain-miner --address <your_pibot_address>       ║");
    eprintln!("  ║                                                          ║");
    eprintln!("  ╚══════════════════════════════════════════════════════════╝");
    eprintln!();
}

/// On Windows, pause so the user can read the output before the terminal closes.
/// Does nothing on Unix (terminal stays open).
fn wait_for_key_on_windows() {
    #[cfg(target_os = "windows")]
    {
        eprintln!("Press Enter to close...");
        let _ = std::io::stdin().read_line(&mut String::new());
    }
}

fn generate_and_save_keypair(path: &PathBuf) -> anyhow::Result<()> {
    if path.exists() {
        anyhow::bail!(
            "wallet file already exists at '{}'. Remove it manually or choose a different path.",
            path.display()
        );
    }

    println!("Generating post-quantum keypair (ML-DSA-65 + SLH-DSA-SHAKE-128f)...");
    println!("This may take a moment.\n");

    let (kp, export) = pichain_crypto::generate_pq_wallet();
    let json = serde_json::to_string_pretty(&export)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, "")?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::write(path, &json)?;

    let addr = kp.address();
    println!("=== New PIChain Mining Wallet ===");
    println!("Address:    {}", addr);
    println!("Crypto:     Post-Quantum (ML-DSA-65 + SLH-DSA-SHAKE-128f)");
    println!("Saved to:   {}", path.display());
    println!();
    println!("Quantum computers cannot break this wallet.");
    println!("Keep this file safe. Your keys cannot be recovered.");
    eprintln!();
    eprintln!("  SECURITY: Wallet file is NOT encrypted. Protect it like a password.");
    eprintln!();
    eprintln!("  NEXT STEPS:");
    eprintln!(
        "    Start mining:    pichain-miner --keypair {}",
        path.display()
    );
    eprintln!(
        "    Check balance:   pichain-miner --keypair {} --info",
        path.display()
    );
    eprintln!(
        "    View on web:     https://pichain.net → Connect Wallet → enter {}",
        addr
    );
    eprintln!();
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    // Handle keypair generation
    if args.generate_keypair {
        let keypair_path = args
            .keypair
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("wallet.json"));
        let result = generate_and_save_keypair(&keypair_path);
        if let Err(ref e) = result {
            eprintln!("\nError: {}", e);
            wait_for_key_on_windows();
        }
        return result;
    }

    // Determine mining mode: --address (pool-submit) or --keypair (block pipeline)
    let use_pool_submit;
    let address: pichain_crypto::keys::Address;
    let address_hex: String;
    let loaded: Option<LoadedWallet>;

    if let Some(ref addr_str) = args.address {
        // Pool-submit mode: mine to any address, no wallet needed
        let mut hex = addr_str.clone();
        if hex.starts_with("Pi314") {
            hex = hex[5..].to_string();
        }
        if hex.starts_with("pi314") {
            hex = hex[5..].to_string();
        }
        let addr_bytes = hex::decode(&hex).map_err(|e| anyhow::anyhow!("invalid address: {e}"))?;
        if addr_bytes.len() != 20 {
            anyhow::bail!(
                "address must be 20 bytes (40 hex chars), got {}",
                addr_bytes.len()
            );
        }
        let mut arr = [0u8; 20];
        arr.copy_from_slice(&addr_bytes);
        address = pichain_crypto::keys::Address(arr);
        address_hex = hex.to_lowercase();
        loaded = None;
        use_pool_submit = true;
        info!(%address, "Pool-submit mode — mining to address (no wallet file needed)");
    } else if let Some(ref keypair_path) = args.keypair {
        // Block pipeline mode: sign transactions locally
        if !keypair_path.exists() {
            eprintln!();
            eprintln!("  ╔══════════════════════════════════════════════════════╗");
            eprintln!("  ║           PIChain Miner — Getting Started            ║");
            eprintln!("  ╠══════════════════════════════════════════════════════╣");
            eprintln!("  ║                                                      ║");
            eprintln!("  ║  No wallet found at '{}'", keypair_path.display());
            eprintln!("  ║                                                      ║");
            eprintln!("  ║  Option A: Create a new wallet                       ║");
            eprintln!("  ║    pichain-miner --keypair wallet.json \\              ║");
            eprintln!("  ║      --generate-keypair                              ║");
            eprintln!("  ║                                                      ║");
            eprintln!("  ║  Option B: Mine to an existing address (no wallet)   ║");
            eprintln!("  ║    pichain-miner --address Pi314your_address_here     ║");
            eprintln!("  ║                                                      ║");
            eprintln!("  ║  Use your PiBot, desktop app, or any PIChain address ║");
            eprintln!("  ║  with --address to start mining immediately.         ║");
            eprintln!("  ║                                                      ║");
            eprintln!("  ╚══════════════════════════════════════════════════════╝");
            eprintln!();
            wait_for_key_on_windows();
            std::process::exit(1);
        }
        let w = match load_wallet(keypair_path) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("\nError loading wallet: {e}");
                wait_for_key_on_windows();
                return Err(e);
            }
        };
        address = w.address();
        address_hex = hex::encode(address.0);
        loaded = Some(w);
        use_pool_submit = false;
        info!(%address, "Post-quantum wallet loaded (ML-DSA-65 + SLH-DSA-SHAKE-128f)");
    } else {
        // No --keypair or --address: try auto-detect from pichain-signer on localhost:8315
        let signer_addr = try_detect_signer().await;

        if let Some((detected_address, detected_hex)) = signer_addr {
            info!(%detected_address, "Auto-detected PIChain Signer on localhost:8315");
            address = detected_address;
            address_hex = detected_hex;
            loaded = None;
            use_pool_submit = true;
        } else {
            eprintln!();
            eprintln!("  ╔══════════════════════════════════════════════════════════╗");
            eprintln!("  ║             PIChain Miner — Getting Started              ║");
            eprintln!("  ╠══════════════════════════════════════════════════════════╣");
            eprintln!("  ║                                                          ║");
            eprintln!("  ║  Easiest: Mine to your PiBot wallet                      ║");
            eprintln!("  ║    1. Open @PiChainTradeBot on Telegram                  ║");
            eprintln!("  ║    2. Type /wallet to see your address                   ║");
            eprintln!("  ║    3. Run:                                               ║");
            eprintln!("  ║       pichain-miner --address Pi314<your_address>        ║");
            eprintln!("  ║                                                          ║");
            eprintln!("  ║  Or: Mine with a local wallet file                       ║");
            eprintln!("  ║    pichain-miner --keypair wallet.json --generate-keypair ║");
            eprintln!("  ║    pichain-miner --keypair wallet.json                   ║");
            eprintln!("  ║                                                          ║");
            eprintln!("  ║  Or: Run PIChain Signer / desktop app first              ║");
            eprintln!("  ║    The miner will auto-detect it on localhost:8315        ║");
            eprintln!("  ║                                                          ║");
            eprintln!("  ╚══════════════════════════════════════════════════════════╝");
            eprintln!();
            wait_for_key_on_windows();
            std::process::exit(1);
        }
    }
    // Handle --info: show wallet details and exit
    if args.info {
        show_wallet_info(&args.rpc_url, &address_hex, &address, use_pool_submit).await;
        wait_for_key_on_windows();
        return Ok(());
    }

    // Show wallet summary at startup
    {
        let display_addr = format!("{}", address);
        eprintln!();
        eprintln!("  ┌─────────────────────────────────────────────────────┐");
        eprintln!("  │  Wallet: {:<43}│", display_addr);

        // Quick balance check
        let quick_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        if let Ok(resp) = quick_client
            .get(format!("{}/api/v1/account/{}", args.rpc_url, address_hex))
            .send()
            .await
        {
            if let Ok(acct) = resp.json::<AccountResponse>().await {
                let bal = acct.balance as f64 / 1_000_000_000.0;
                eprintln!("  │  Balance: {:<43}│", format!("{:.4} PI", bal));
            }
        }
        if use_pool_submit {
            eprintln!("  │  Mode:    {:<43}│", "Pool-submit (no wallet file)");
        } else {
            eprintln!("  │  Mode:    {:<43}│", "Block pipeline (local signing)");
        }
        eprintln!("  │                                                     │");
        eprintln!("  │  Tip: Run with --info for wallet details & how to   │");
        eprintln!("  │       access your PI on the web or via PiBot.       │");
        eprintln!("  └─────────────────────────────────────────────────────┘");
        eprintln!();
    }

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();
    let mut local_nonce: Option<u64> = None;
    let mut local_position: Option<u64> = None;
    let mut last_gap_filled: Option<u64> = None;

    // Graceful shutdown via Ctrl+C
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
        shutdown_clone.store(true, Ordering::SeqCst);
    });

    // SECURITY: Warn if RPC URL uses HTTP (unencrypted — signed transactions visible to network)
    if args.rpc_url.starts_with("http://")
        && !args.rpc_url.contains("127.0.0.1")
        && !args.rpc_url.contains("localhost")
    {
        eprintln!();
        eprintln!("  ⚠ WARNING: RPC URL uses HTTP (unencrypted).");
        eprintln!("  Signed transactions may be visible to network observers.");
        eprintln!("  Use https:// for production mining.");
        eprintln!();
    }

    // Resolve mining configuration from profile + overrides
    let config = MiningConfig::from_args(&args);
    let num_threads = config.threads;
    let base_batch_size = config.digits_per_batch;
    let batch_count = config.concurrent_batches;

    // Adaptive batch sizing: scale down when getting 0-reward overlaps,
    // scale up when earning rewards. Smaller batches = faster submission
    // = less chance of overlap with other miners.

    // Configure rayon thread pool
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global()
        .ok(); // Ignore if already initialized

    let profile_name = args.profile.as_deref().unwrap_or("auto");
    let total_cores = num_cpus::get();

    // CPU usage warning
    let cpu_pct = if total_cores > 0 {
        num_threads * 100 / total_cores
    } else {
        100
    };
    eprintln!();
    eprintln!("  ====================== WARNING ======================");
    eprintln!("  Mining uses significant CPU resources.");
    eprintln!(
        "  Profile: {profile_name} — {num_threads} / {total_cores} CPU threads (~{cpu_pct}% CPU)"
    );
    if cpu_pct > 75 {
        eprintln!("  This will heavily load your system and may cause");
        eprintln!("  slowdowns, high temperatures, and increased power");
        eprintln!("  usage. Make sure your cooling is adequate.");
    }
    eprintln!("  Use --profile low for lighter CPU usage.");
    eprintln!("  Use --max-cpu 50 to limit to ~50% CPU.");
    eprintln!("  Press Ctrl+C at any time to stop mining.");
    eprintln!("  By continuing, you accept responsibility for any");
    eprintln!("  impact on your system's performance or hardware.");
    eprintln!("  =====================================================");
    eprintln!();

    info!(
        rpc = %args.rpc_url,
        profile = profile_name,
        digits_per_batch = base_batch_size,
        chain_id = args.chain_id,
        position_offset = args.position_offset,
        threads = num_threads,
        concurrent_batches = batch_count,
        "PIChain Miner starting — Computing the Infinite ({} CPU threads, profile: {})",
        num_threads,
        profile_name,
    );

    // Mining stats
    let session_start = std::time::Instant::now();
    let mut total_digits_computed: u64 = 0;
    let mut proofs_submitted: u64 = 0;
    let mut proofs_accepted: u64 = 0;
    let mut loop_count: u64 = 0;

    // Mining loop
    loop {
        // Check for shutdown
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        // 1. Query mining status to get next position
        let mining_status = match client
            .get(format!("{}/api/v1/mining/status", args.rpc_url))
            .send()
            .await
        {
            Ok(resp) => match resp.json::<MiningStatus>().await {
                Ok(status) => status,
                Err(e) => {
                    warn!("Failed to parse mining status: {e}");
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
            },
            Err(e) => {
                warn!("Failed to query mining status: {e}");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        // Use configured batch size, enforce server minimum
        // Adaptive batch: query ACTIVE miners from slot endpoint (not historical unique_miners)
        let server_min = mining_status.min_batch_size.max(10);
        let active_miners = match client
            .get(format!(
                "{}/api/v1/mining/slot/{}",
                args.rpc_url, address_hex
            ))
            .send()
            .await
        {
            Ok(resp) => {
                let slot: serde_json::Value = resp.json().await.unwrap_or_default();
                slot["active_miners"].as_u64().unwrap_or(1) as u32
            }
            _ => 1,
        };
        // Scale: 1 miner=full, 2-5=half, 6-20=quarter, 21+=minimum
        let effective_min_batch = if active_miners <= 1 {
            base_batch_size.max(server_min)
        } else if active_miners <= 5 {
            (base_batch_size / 2).max(server_min)
        } else if active_miners <= 20 {
            (base_batch_size / 4).max(server_min)
        } else {
            server_min
        };

        // Calculate position and effective batch size.
        //
        // FRONTIER-FIRST MINING: Always mine at or very near the server's
        // next_position (first gap from frontier). Local tracking prevents
        // re-mining the same gap within a round, but is capped to prevent
        // racing far ahead of the frontier.
        let max_local_ahead = effective_min_batch as u64 * batch_count as u64 * 5;

        let (position, effective_batch_size) = if let Some(start) = args.start_at {
            // Manual override: mine sequentially from a fixed starting point
            let pos = start.saturating_add(loop_count.saturating_mul(effective_min_batch as u64));
            (pos, effective_min_batch)
        } else {
            // Query our assigned slot for a position unique to this miner.
            // The slot system gives each miner a non-overlapping range based on
            // next_uncomputed_position + (slot_index * slot_range_size).
            // Use max(slot_position, local_position) so we never go backwards
            // even if other miners have computed positions ahead of us.
            let (slot_pos, gap_fill_pos) = match client
                .get(format!(
                    "{}/api/v1/mining/slot/{}",
                    args.rpc_url, address_hex
                ))
                .send()
                .await
            {
                Ok(resp) => {
                    let slot: serde_json::Value = resp.json().await.unwrap_or_default();
                    let pos = slot["recommended_position"]
                        .as_u64()
                        .unwrap_or(mining_status.next_position);
                    let gap = slot["gap_fill_position"].as_u64();
                    (pos, gap)
                }
                _ => (mining_status.next_position, None),
            };

            // If the server detected a gap in the digit registry, fill it first.
            // Use the gap position DIRECTLY — don't apply max() which would skip past it.
            // Skip if we already filled this exact gap (server hasn't processed it yet).
            let filling_gap = if let Some(gap_pos) = gap_fill_pos {
                if last_gap_filled == Some(gap_pos) {
                    // Already filled this gap, waiting for server to process it
                    false
                } else if local_position.is_none_or(|lp| lp > gap_pos) {
                    info!(
                        gap_position = gap_pos,
                        local_pos = ?local_position,
                        "Filling gap to advance frontier"
                    );
                    last_gap_filled = Some(gap_pos);
                    true
                } else {
                    false
                }
            } else {
                // No gap — clear the tracker
                last_gap_filled = None;
                false
            };

            let pos = if filling_gap {
                // Go directly to the gap — don't max() with slot_pos
                gap_fill_pos.unwrap()
            } else if let Some(local_pos) = local_position {
                // Normal: take the FURTHER of local tracking and server recommendation
                local_pos.max(slot_pos)
            } else {
                slot_pos
            };

            // Cap at max_allowed_position
            let pos = pos.min(mining_status.max_allowed_position.max(pos));

            // Enforce max_allowed_position from frontier distance limit
            let pos = if mining_status.max_allowed_position > 0 {
                pos.min(mining_status.max_allowed_position)
            } else {
                pos
            };

            // If we've hit the cap (too far ahead of frontier), wait for
            // frontier to catch up instead of wasting compute on positions
            // that may never advance the frontier.
            if local_position.is_some() && pos >= slot_pos.saturating_add(max_local_ahead) {
                info!(
                    local_pos = ?local_position,
                    frontier = mining_status.frontier_position,
                    server_next = slot_pos,
                    max_ahead = max_local_ahead,
                    "Waiting for frontier to catch up (proofs pending in blocks)..."
                );
                // DON'T reset local_position — keep it so we don't re-mine same range
                // Just wait for blocks to catch up
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            }

            // Cap batch size to available gap at this position
            let max_gap = mining_status.max_batch_at_position;
            let min_required = mining_status.min_batch_size.max(10) as u64;
            let effective = if max_gap < min_required {
                warn!(
                    gap = max_gap,
                    min_required = min_required,
                    position = pos,
                    "Gap too small for minimum batch size, waiting..."
                );
                local_position = None;
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                continue;
            } else if max_gap < effective_min_batch as u64 {
                // Gap is smaller than configured batch but >= min_required — use gap size
                info!(
                    configured = effective_min_batch,
                    available_gap = max_gap,
                    "Capping batch size to fit available gap"
                );
                max_gap as u32
            } else {
                effective_min_batch
            };
            (pos, effective)
        };

        // Parse difficulty target and anchor block hash
        let difficulty_target: [u8; 32] = if !mining_status.difficulty_target_hex.is_empty() {
            let bytes = hex::decode(&mining_status.difficulty_target_hex).unwrap_or_default();
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                arr
            } else {
                warn!("RPC returned invalid difficulty target — using default. Verify your --rpc-url is correct.");
                pichain_mining::difficulty::INITIAL_DIFFICULTY
            }
        } else {
            pichain_mining::difficulty::INITIAL_DIFFICULTY
        };

        let anchor_block_hash: [u8; 32] = if !mining_status.anchor_block_hash.is_empty() {
            let bytes = hex::decode(&mining_status.anchor_block_hash).unwrap_or_default();
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                arr
            } else {
                [0u8; 32]
            }
        } else {
            [0u8; 32]
        };

        info!(
            position,
            frontier = mining_status.frontier_position,
            total_verified = mining_status.total_digits_verified,
            reward_per_digit = mining_status.reward_per_digit,
            difficulty_bits = mining_status.difficulty_bits,
            bonus = %mining_status.frontier_bonus_at_next,
            base_batch_size = effective_batch_size,
            "Mining round {}",
            loop_count + 1,
        );

        // 2. Get account nonce (and check balance)
        let nonce = if let Some(n) = local_nonce {
            n
        } else {
            match client
                .get(format!("{}/api/v1/account/{}", args.rpc_url, address_hex))
                .send()
                .await
            {
                Ok(resp) => match resp.json::<AccountResponse>().await {
                    Ok(acct) if acct.found => {
                        // Check balance covers gas (locked_balance counts — it's for fees)
                        let estimated_gas = 200_000 + effective_batch_size as u64 * 100;
                        let min_cost = estimated_gas * 1_100; // base_fee + priority_fee estimate
                        let effective_balance = acct.balance + acct.locked_balance.unwrap_or(0);
                        if effective_balance < min_cost {
                            warn!(
                                balance = acct.balance,
                                locked = acct.locked_balance.unwrap_or(0),
                                needed = min_cost,
                                "Insufficient balance for gas, waiting..."
                            );
                            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                            continue;
                        }
                        acct.nonce
                    }
                    _ => 0,
                },
                Err(_) => 0,
            }
        };

        // 3. Compute PI digits (parallel across CPU cores)
        let compute_start = std::time::Instant::now();

        // When the gap is smaller than base_batch_size * batch_count, reduce to 1 batch
        // to avoid submitting at positions that overlap with existing ranges.
        let effective_batch_count = if effective_batch_size < base_batch_size {
            1 // Gap is constrained — only submit one batch at the gap
        } else {
            batch_count
        };

        // Compute all concurrent batches in parallel using rayon
        let batches: Vec<(u64, u32, Vec<u8>)> = if effective_batch_count == 1 {
            // Single batch — parallelize within the batch
            let digits = BbpComputer::compute_hex_digits_parallel(position, effective_batch_size);
            vec![(position, effective_batch_size, digits)]
        } else {
            // Multiple concurrent batches at different positions using rayon
            use rayon::prelude::*;
            (0..effective_batch_count)
                .into_par_iter()
                .map(|i| {
                    let batch_pos = position
                        .saturating_add((i as u64).saturating_mul(effective_batch_size as u64));
                    let digits =
                        BbpComputer::compute_hex_digits_parallel(batch_pos, effective_batch_size);
                    (batch_pos, effective_batch_size, digits)
                })
                .collect()
        };
        let compute_time = compute_start.elapsed();

        let total_batch_digits = effective_batch_count as u64 * effective_batch_size as u64;
        let digits_per_sec = if compute_time.as_millis() > 0 {
            total_batch_digits as f64 / compute_time.as_secs_f64()
        } else {
            0.0
        };

        info!(
            digits = total_batch_digits,
            batches = effective_batch_count,
            position,
            elapsed_ms = compute_time.as_millis(),
            digits_per_sec = format!("{:.0}", digits_per_sec),
            "PI digits computed ({} threads)",
            num_threads,
        );

        // 4. Find PoW nonce and submit all batches
        let mut current_nonce = nonce;
        for (batch_pos, batch_digit_count, digits) in batches {
            // Find a PoW nonce that meets the difficulty target
            // Scale attempts to difficulty: need ~2^bits attempts on average.
            // Use 8x expected attempts for high success probability.
            let nonce_start = std::time::Instant::now();
            let max_nonce_attempts = if mining_status.difficulty_bits < 60 {
                8u64.saturating_mul(
                    1u64.checked_shl(mining_status.difficulty_bits)
                        .unwrap_or(u64::MAX),
                )
                .max(10_000_000) // floor: 10M
            } else {
                u64::MAX // extremely high difficulty, search until found
            };
            let pow_nonce = pichain_mining::find_nonce_parallel(
                &digits,
                &anchor_block_hash,
                &difficulty_target,
                max_nonce_attempts,
                &address.0,
            );

            let pn = match pow_nonce {
                Some(pn) => {
                    info!(
                        pow_nonce = pn,
                        nonce_time_ms = nonce_start.elapsed().as_millis(),
                        difficulty_bits = mining_status.difficulty_bits,
                        "PoW nonce found"
                    );
                    pn
                }
                None => {
                    warn!(
                        max_attempts = max_nonce_attempts,
                        difficulty_bits = mining_status.difficulty_bits,
                        elapsed_ms = nonce_start.elapsed().as_millis(),
                        "Failed to find PoW nonce after {} attempts, skipping batch",
                        max_nonce_attempts,
                    );
                    continue;
                }
            };

            // Submit proof — either via block pipeline (signed) or pool-submit (address only)
            let (submit_url, submit_body) = if use_pool_submit {
                // Pool-submit: node signs on our behalf
                (
                    format!("{}/api/v1/mining/pool-submit", args.rpc_url),
                    serde_json::json!({
                        "miner_address": address_hex,
                        "start_position": batch_pos,
                        "digit_count": batch_digit_count,
                        "digits": hex::encode(&digits),
                        "pow_nonce": pn,
                        "anchor_block_hash": hex::encode(anchor_block_hash),
                    }),
                )
            } else {
                // Block pipeline: sign locally with PQ keypair
                let tx_data = TransactionData {
                    sender: address,
                    nonce: current_nonce,
                    kind: TransactionKind::MiningProof {
                        start_position: batch_pos,
                        digit_count: batch_digit_count,
                        digits,
                        proof: vec![],
                        pow_nonce: pn,
                        anchor_block_hash: anchor_block_hash.to_vec(),
                    },
                    gas_limit: 0,
                    max_base_fee: 0,
                    max_priority_fee: 0,
                    chain_id: args.chain_id,
                };
                let signed = Transaction::sign_pq(tx_data, &loaded.as_ref().unwrap().pq_keypair);
                let tx_hex = hex::encode(serde_json::to_vec(&signed)?);
                (
                    format!("{}/api/v1/tx/submit", args.rpc_url),
                    serde_json::json!({ "signed_tx_hex": tx_hex }),
                )
            };

            proofs_submitted += 1;

            match client.post(&submit_url).json(&submit_body).send().await {
                Ok(resp) => match resp.json::<SubmitResponse>().await {
                    Ok(result) => {
                        if result.status == "pending" {
                            proofs_accepted += 1;
                            total_digits_computed += batch_digit_count as u64;
                            info!(
                                tx_hash = %result.tx_hash,
                                position = batch_pos,
                                digits = batch_digit_count,
                                total_digits = total_digits_computed,
                                accepted = proofs_accepted,
                                batch = effective_min_batch,
                                "Mining proof submitted successfully"
                            );
                            // Show balance every 10 proofs
                            if proofs_accepted.is_multiple_of(10) {
                                if let Ok(bal_resp) = client
                                    .get(format!("{}/api/v1/account/{}", args.rpc_url, address_hex))
                                    .send()
                                    .await
                                {
                                    if let Ok(acct) = bal_resp.json::<AccountResponse>().await {
                                        let bal = acct.balance as f64 / 1_000_000_000.0;
                                        info!(
                                            balance = format!("{:.4} PI", bal),
                                            proofs = proofs_accepted,
                                            "Wallet balance update"
                                        );
                                    }
                                }
                            }
                            current_nonce = current_nonce.saturating_add(1);
                            // CRITICAL: advance local position past what we just submitted
                            // so we don't re-mine the same range while waiting for block inclusion
                            local_position =
                                Some(batch_pos.saturating_add(batch_digit_count as u64));
                        } else {
                            error!(
                                status = %result.status,
                                error = ?result.error,
                                position = batch_pos,
                                "Mining proof rejected"
                            );
                            local_nonce = None;
                            // DON'T reset local_position — keep it at the last successful
                            // batch so next round starts at the gap, not jumping past it
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Failed to parse submit response: {e}");
                        local_nonce = None;
                        // Keep local_position so next round retries from the gap
                        break;
                    }
                },
                Err(e) => {
                    error!("Failed to submit transaction: {e}");
                    local_nonce = None;
                    // Keep local_position so next round retries from the gap
                    break;
                }
            }
        }

        // Update local nonce if all batches succeeded
        if local_nonce.is_some() || current_nonce > nonce {
            local_nonce = Some(current_nonce);
        }

        // Advance local position past the batches we actually submitted successfully.
        // CRITICAL: Only count ACTUALLY SUBMITTED batches, not planned batch count.
        // Using planned count causes gaps when a middle batch fails — local_position
        // jumps past the failed batch, leaving a permanent hole in the digit registry.
        if current_nonce > nonce {
            let actually_submitted = current_nonce.saturating_sub(nonce);
            let total_digits_this_round =
                actually_submitted.saturating_mul(effective_batch_size as u64);
            local_position = Some(position.saturating_add(total_digits_this_round));
        }

        loop_count += 1;

        // 6. Wait before next round
        if args.interval_secs > 0 {
            tokio::time::sleep(tokio::time::Duration::from_secs(args.interval_secs)).await;
        }
    }

    // Print summary on shutdown
    let elapsed = session_start.elapsed();
    println!("\n=== PIChain Mining Summary ===");
    println!("  Miner address:         {}", address);
    println!("  CPU threads:           {}", num_threads);
    println!("  Concurrent batches:    {}", batch_count);
    println!("  Total digits computed:  {}", total_digits_computed);
    println!("  Proofs submitted:       {}", proofs_submitted);
    println!("  Proofs accepted:        {}", proofs_accepted);
    println!("  Session time:           {:.1}s", elapsed.as_secs_f64());
    if elapsed.as_secs() > 0 {
        println!(
            "  Digits per second:      {:.1}",
            total_digits_computed as f64 / elapsed.as_secs_f64()
        );
    }
    println!("==============================");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_wallet_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invalid.json");

        // Create an invalid wallet file (no PQ keys)
        let invalid = serde_json::json!({
            "address": "0000000000000000000000000000000000000000"
        });
        std::fs::write(&path, serde_json::to_string(&invalid).unwrap()).unwrap();

        // Loading should FAIL — missing PQ key fields
        let result = load_wallet(&path);
        assert!(result.is_err(), "invalid wallet should be rejected");
        let err = result.err().unwrap().to_string();
        assert!(
            err.contains("missing PQ key fields"),
            "expected rejection message, got: {err}"
        );
    }

    #[test]
    fn generate_and_load_pq_keypair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pq-wallet.json");

        // Generate PQ wallet
        let (_kp, export) = pichain_crypto::generate_pq_wallet();
        let json = serde_json::to_string_pretty(&export).unwrap();
        std::fs::write(&path, &json).unwrap();

        // Load PQ wallet
        let loaded = load_wallet(&path).unwrap();
        assert_eq!(format!("{}", loaded.address()), export.address);
    }

    #[test]
    fn compute_digits_for_proof() {
        let digits = BbpComputer::compute_hex_digits(0, 200);
        assert_eq!(digits.len(), 200);

        // Verify first few digits match known PI hex
        assert_eq!(digits[0], 0x2); // PI hex starts with 2
        assert_eq!(digits[1], 0x4);
        assert_eq!(digits[2], 0x3);
        assert_eq!(digits[3], 0xF);
    }
}
