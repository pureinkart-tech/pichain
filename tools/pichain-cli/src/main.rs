//! PIChain CLI — Wallet tools and PI transfers.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pichain-cli")]
#[command(about = "PIChain wallet tools — generate wallets, send PI, check balances")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a new post-quantum wallet (ML-DSA-65 + SLH-DSA-SHAKE-128f).
    Wallet {
        /// Save wallet to this file path.
        #[arg(long, default_value = "wallet.json")]
        output: String,
    },

    /// Verify a wallet file is valid and keys are consistent.
    VerifyWallet {
        /// Path to the wallet JSON file.
        path: String,
    },

    /// Send PI to another address.
    Send {
        /// Path to your wallet file.
        #[arg(long, default_value = "wallet.json")]
        wallet: String,

        /// Recipient address (Pi314... or 40-char hex).
        #[arg(long)]
        to: String,

        /// Amount of PI to send (e.g. 100, 3.14).
        #[arg(long)]
        amount: f64,

        /// RPC URL of the PIChain node.
        #[arg(long, default_value = "https://pichain.net")]
        rpc_url: String,

        /// Chain ID.
        #[arg(long, default_value = "31415")]
        chain_id: u64,
    },

    /// Check balance of an address.
    Balance {
        /// Address to check (Pi314... or 40-char hex).
        address: String,

        /// RPC URL of the PIChain node.
        #[arg(long, default_value = "https://pichain.net")]
        rpc_url: String,
    },

    /// Compute PI hex digits (for testing the mining algorithm).
    ComputePi {
        /// Starting position.
        #[arg(long, default_value = "0")]
        position: u64,
        /// Number of hex digits.
        #[arg(long, default_value = "100")]
        count: u32,
    },

    /// Hash data with Blake3 (PIChain's hash function).
    Hash {
        /// Data to hash.
        data: String,
    },

    /// Show PIChain token supply and emission info.
    Tokenomics,
}

fn parse_address(input: &str) -> Result<[u8; 20], String> {
    let hex_str = input.trim().strip_prefix("Pi314").unwrap_or(input.trim());
    if hex_str.len() != 40 {
        return Err(format!(
            "address must be 40 hex chars, got {}",
            hex_str.len()
        ));
    }
    let bytes = hex::decode(hex_str).map_err(|e| format!("invalid hex: {e}"))?;
    let mut arr = [0u8; 20];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

fn load_pq_wallet(path: &str) -> Result<pichain_crypto::PqKeypair, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read wallet '{}': {e}", path))?;
    let export: pichain_crypto::pq_wallet::PqWalletExport =
        serde_json::from_str(&contents).map_err(|e| format!("invalid wallet JSON: {e}"))?;
    pichain_crypto::restore_pq_wallet(&export)
        .map_err(|e| format!("failed to restore PQ keypair: {e}"))
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Wallet { output } => {
            use std::path::Path;

            let path = Path::new(&output);
            if path.exists() {
                eprintln!(
                    "ERROR: File '{}' already exists. Remove it or choose a different path.",
                    output
                );
                std::process::exit(1);
            }

            println!("Generating post-quantum keypair (ML-DSA-65 + SLH-DSA-SHAKE-128f)...");
            println!("This may take a moment (SLH-DSA key generation is compute-intensive).\n");

            let (kp, export) = pichain_crypto::generate_pq_wallet();
            let json = serde_json::to_string_pretty(&export).expect("serialization failed");
            std::fs::write(path, &json).expect("failed to write wallet file");

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }

            println!("=== New PIChain Wallet ===");
            println!("Address:         {}", kp.address());
            println!("Saved to:        {}", output);
            println!("\nQuantum-safe (ML-DSA-65 + SLH-DSA-SHAKE-128f).");
            println!(
                "IMPORTANT: Keep '{}' safe. Your keys cannot be recovered.",
                output
            );
        }

        Commands::VerifyWallet { path } => {
            let contents = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                eprintln!("Failed to read '{}': {}", path, e);
                std::process::exit(1);
            });
            let export: pichain_crypto::pq_wallet::PqWalletExport = serde_json::from_str(&contents)
                .unwrap_or_else(|e| {
                    eprintln!("Invalid wallet JSON: {}", e);
                    std::process::exit(1);
                });

            print!("Verifying wallet... ");
            match pichain_crypto::verify_pq_wallet(&export) {
                Ok(()) => {
                    println!("VALID");
                    println!("Address: {}", export.address);
                }
                Err(e) => {
                    println!("INVALID — {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Send {
            wallet,
            to,
            amount,
            rpc_url,
            chain_id,
        } => {
            // Parse recipient address
            let to_bytes = match parse_address(&to) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("ERROR: {e}");
                    std::process::exit(1);
                }
            };
            let recipient = pichain_crypto::keys::Address(to_bytes);

            // Convert PI to base units
            let base_units = (amount * 1_000_000_000.0) as u64;
            if base_units == 0 {
                eprintln!("ERROR: amount must be > 0");
                std::process::exit(1);
            }

            // Load wallet
            println!("Loading wallet from '{}'...", wallet);
            let kp = match load_pq_wallet(&wallet) {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("ERROR: {e}");
                    std::process::exit(1);
                }
            };
            let sender = kp.address();
            println!("Sender:    {}", sender);
            println!("Recipient: Pi314{}", hex::encode(to_bytes));
            println!("Amount:    {} PI ({} base units)", amount, base_units);

            // Get sender nonce from chain
            let client = reqwest::Client::new();
            let sender_hex = hex::encode(sender.0);
            let acct_url = format!("{}/api/v1/account/{}", rpc_url, sender_hex);
            let (nonce, balance) = match client.get(&acct_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    (
                        body["nonce"].as_u64().unwrap_or(0),
                        body["balance"].as_u64().unwrap_or(0),
                    )
                }
                _ => (0, 0),
            };

            // Check balance before submitting
            if balance < base_units {
                eprintln!(
                    "ERROR: insufficient balance: have {:.4} PI, trying to send {:.4} PI",
                    balance as f64 / 1e9,
                    amount
                );
                std::process::exit(1);
            }

            // Build and sign transaction
            let tx_data = pichain_types::TransactionData {
                sender,
                nonce,
                kind: pichain_types::TransactionKind::Transfer {
                    recipient,
                    amount: base_units,
                },
                gas_limit: 21_000,
                max_base_fee: 10_000,
                max_priority_fee: 100,
                chain_id,
            };

            let signed = pichain_types::Transaction::sign_pq(tx_data, &kp);
            let tx_json = serde_json::to_vec(&signed).expect("serialization failed");
            let tx_hex = hex::encode(&tx_json);

            // Submit to node
            println!("\nSubmitting transaction...");
            let submit_url = format!("{}/api/v1/tx/submit", rpc_url);
            match client
                .post(&submit_url)
                .json(&serde_json::json!({"signed_tx_hex": tx_hex}))
                .send()
                .await
            {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    if body["status"] == "pending" {
                        println!("SUCCESS! TX hash: {}", body["tx_hash"]);
                        println!("Sent {} PI to Pi314{}", amount, hex::encode(to_bytes));
                    } else {
                        eprintln!(
                            "REJECTED: {}",
                            body["error"].as_str().unwrap_or("unknown error")
                        );
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Network error: {e}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Balance { address, rpc_url } => {
            let addr_bytes = match parse_address(&address) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("ERROR: {e}");
                    std::process::exit(1);
                }
            };

            let addr_hex = hex::encode(addr_bytes);
            let url = format!("{}/api/v1/account/{}", rpc_url, addr_hex);
            let client = reqwest::Client::new();
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    let balance = body["balance"].as_u64().unwrap_or(0);
                    let nonce = body["nonce"].as_u64().unwrap_or(0);
                    let pi = balance as f64 / 1_000_000_000.0;
                    println!("Address: Pi314{}", addr_hex);
                    println!("Balance: {:.9} PI ({} base units)", pi, balance);
                    println!("Nonce:   {}", nonce);
                }
                Ok(resp) => {
                    eprintln!("Node returned HTTP {}", resp.status());
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Network error: {e}");
                    std::process::exit(1);
                }
            }
        }

        Commands::ComputePi { position, count } => {
            use pichain_mining::bbp::{digits_to_hex_string, BbpComputer};

            println!("Computing {count} hex digits of PI starting at position {position}...\n");
            let start = std::time::Instant::now();
            let digits = BbpComputer::compute_hex_digits(position, count);
            let elapsed = start.elapsed();
            let hex_str = digits_to_hex_string(&digits);
            println!(
                "PI hex [{position}..{}]: {hex_str}",
                position + count as u64
            );
            println!(
                "\nComputed in {:.3}s ({:.0} digits/sec)",
                elapsed.as_secs_f64(),
                count as f64 / elapsed.as_secs_f64()
            );
        }

        Commands::Hash { data } => {
            let hash = pichain_crypto::hash(data.as_bytes());
            println!("Blake3 hash: {hash}");
        }

        Commands::Tokenomics => {
            println!("=== PIChain Tokenomics ===\n");
            println!("Total Supply:     3,141,592,653 PI (FIXED)");
            println!("Base Unit:        1 PI = 1,000,000,000 base units\n");
            println!("Distribution:");
            println!("  85% Mining Pool: 2,670,353,755 PI");
            println!("  10% Staking:       314,159,265 PI");
            println!("   5% Liquidity:     157,079,632 PI\n");
            println!("Fee Structure (4-way pi-native split):");
            println!("  31.41% burned permanently");
            println!("  31.41% to stakers (proportional to stake)");
            println!("  18.59% mining pool replenishment");
            println!("  18.59% block proposer (+ 100% priority tips)\n");
            println!("Emission: pi-smooth decay (half-life = pi years)");

            let calc = pichain_mining::RewardCalculator::new();
            for year in 1..=10 {
                let emission = calc.annual_emission(year);
                let pi = emission / 1_000_000_000;
                println!("  Year {year:>2}: {pi:>12} PI/year");
            }
            println!("\n  50% mined after pi years (~3.14)");
            println!("  99.9% mined after pi decades (~31.4)");
            println!("  Mining never stops (asymptotic)");
        }
    }
}
