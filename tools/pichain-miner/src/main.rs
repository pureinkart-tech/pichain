//! PIChain Miner — compute PI digits and earn mining rewards.
//!
//! This tool connects to a running PIChain node via RPC, queries the
//! current mining frontier, computes new PI hex digits using the BBP
//! algorithm, and submits mining proof transactions to earn rewards.
//!
//! Usage:
//!   pichain-miner --keypair wallet.json --rpc-url https://pichain.net

use clap::Parser;
use pichain_crypto::PqKeypair;
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

    /// Path to the keypair file (hex-encoded secret key).
    /// File should contain a JSON object: {"secret_key": "hex..."}
    #[arg(long)]
    keypair: PathBuf,

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

    println!("=== New PIChain Mining Wallet ===");
    println!("Address:    {}", kp.address());
    println!("Crypto:     Post-Quantum (ML-DSA-65 + SLH-DSA-SHAKE-128f)");
    println!("Saved to:   {}", path.display());
    println!("\nQuantum computers cannot break this wallet.");
    println!("Keep this file safe. Your keys cannot be recovered.");
    eprintln!("\n  SECURITY: Wallet file is NOT encrypted. Protect it like a password.");
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
        let result = generate_and_save_keypair(&args.keypair);
        if result.is_err() {
            eprintln!("\nError: {}", result.as_ref().unwrap_err());
            wait_for_key_on_windows();
        }
        return result;
    }

    // Check if wallet file exists — give helpful message if not
    if !std::path::Path::new(&args.keypair.to_string_lossy().to_string()).exists() {
        eprintln!();
        eprintln!("  ╔══════════════════════════════════════════════╗");
        eprintln!("  ║         PIChain Miner — Getting Started      ║");
        eprintln!("  ╠══════════════════════════════════════════════╣");
        eprintln!("  ║                                              ║");
        eprintln!("  ║  No wallet found at '{}'", args.keypair.display());
        eprintln!("  ║                                              ║");
        eprintln!("  ║  Step 1: Open a terminal (PowerShell/CMD)    ║");
        eprintln!("  ║  Step 2: Navigate to this folder             ║");
        eprintln!("  ║  Step 3: Run:                                ║");
        eprintln!("  ║                                              ║");
        #[cfg(target_os = "windows")]
        eprintln!("  ║  .\\pichain-miner-windows-x86_64.exe `        ║");
        #[cfg(not(target_os = "windows"))]
        eprintln!("  ║  ./pichain-miner \\                           ║");
        eprintln!("  ║    --keypair wallet.json --generate-keypair  ║");
        eprintln!("  ║                                              ║");
        eprintln!("  ║  Then run the miner:                         ║");
        #[cfg(target_os = "windows")]
        eprintln!("  ║  .\\pichain-miner-windows-x86_64.exe `        ║");
        #[cfg(not(target_os = "windows"))]
        eprintln!("  ║  ./pichain-miner \\                           ║");
        eprintln!("  ║    --keypair wallet.json                     ║");
        eprintln!("  ║                                              ║");
        eprintln!("  ╚══════════════════════════════════════════════╝");
        eprintln!();
        wait_for_key_on_windows();
        std::process::exit(1);
    }

    // Load PQ wallet
    let loaded = match load_wallet(&args.keypair) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("\nError loading wallet: {e}");
            wait_for_key_on_windows();
            return Err(e);
        }
    };
    let address = loaded.address();
    let address_hex = hex::encode(address.0);
    info!(%address, "Post-quantum wallet loaded (ML-DSA-65 + SLH-DSA-SHAKE-128f)");
    info!(%address, "Miner wallet loaded");

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();
    let mut local_nonce: Option<u64> = None;
    let mut local_position: Option<u64> = None;

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
    let batch_size = config.digits_per_batch;
    let batch_count = config.concurrent_batches;

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
        digits_per_batch = batch_size,
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

        // Enforce server-reported minimum batch size (frontier-scaled)
        let effective_min_batch = if mining_status.min_batch_size > 0 {
            (batch_size).max(mining_status.min_batch_size)
        } else {
            batch_size
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
            // Always start from server's next_position (first unmined gap from frontier).
            // Use local_position only to avoid re-mining the same gap within a round
            // (server won't update until proof is in a block), but cap how far ahead.
            let server_pos = mining_status.next_position;
            let pos = if let Some(local_pos) = local_position {
                // Use local tracking but never go too far ahead of frontier
                let capped = local_pos.min(server_pos.saturating_add(max_local_ahead));
                capped.max(server_pos) // Never go behind server
            } else {
                server_pos
            };

            // Enforce max_allowed_position from frontier distance limit
            let pos = if mining_status.max_allowed_position > 0 {
                pos.min(mining_status.max_allowed_position)
            } else {
                pos
            };

            // If we've hit the cap (too far ahead of frontier), wait for
            // frontier to catch up instead of wasting compute on positions
            // that may never advance the frontier.
            if local_position.is_some() && pos >= server_pos.saturating_add(max_local_ahead) {
                info!(
                    local_pos = ?local_position,
                    frontier = mining_status.frontier_position,
                    server_next = server_pos,
                    max_ahead = max_local_ahead,
                    "Waiting for frontier to catch up (proofs pending in blocks)..."
                );
                local_position = None; // Reset to re-query next round
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
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
            batch_size = effective_batch_size,
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

        // When the gap is smaller than batch_size * batch_count, reduce to 1 batch
        // to avoid submitting at positions that overlap with existing ranges.
        let effective_batch_count = if effective_batch_size < batch_size {
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
                gas_limit: 200_000u64
                    .saturating_add((batch_digit_count as u64).saturating_mul(100)),
                max_base_fee: 1_000,
                max_priority_fee: 100,
                chain_id: args.chain_id,
            };

            let signed = Transaction::sign_pq(tx_data, &loaded.pq_keypair);
            let tx_hex = hex::encode(serde_json::to_vec(&signed)?);

            let submit_body = serde_json::json!({
                "signed_tx_hex": tx_hex,
            });

            proofs_submitted += 1;

            match client
                .post(format!("{}/api/v1/tx/submit", args.rpc_url))
                .json(&submit_body)
                .send()
                .await
            {
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
                                "Mining proof submitted successfully"
                            );
                            current_nonce = current_nonce.saturating_add(1);
                        } else {
                            error!(
                                status = %result.status,
                                error = ?result.error,
                                position = batch_pos,
                                "Mining proof rejected"
                            );
                            local_nonce = None;
                            local_position = None;
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Failed to parse submit response: {e}");
                        local_nonce = None;
                        local_position = None;
                        break;
                    }
                },
                Err(e) => {
                    error!("Failed to submit transaction: {e}");
                    local_nonce = None;
                    local_position = None;
                    break;
                }
            }
        }

        // Update local nonce if all batches succeeded
        if local_nonce.is_some() || current_nonce > nonce {
            local_nonce = Some(current_nonce);
        }

        // Advance local position past the batches we just submitted.
        // This prevents re-computing the same ranges on the next round
        // before the server has registered our proofs.
        if current_nonce > nonce {
            let total_digits_this_round =
                effective_batch_count as u64 * effective_batch_size as u64;
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
    use std::io::Write;

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
