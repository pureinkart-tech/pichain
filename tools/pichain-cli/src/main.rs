//! PIChain CLI — Developer tools for the PIChain ecosystem.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pichain-cli")]
#[command(about = "PIChain developer tools")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a new post-quantum wallet (ML-DSA-65 + SLH-DSA-SHAKE-128f).
    /// All PIChain wallets use dual post-quantum signatures for quantum resistance.
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

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Wallet { output } => {
            use std::path::Path;

            let path = Path::new(&output);
            if path.exists() {
                eprintln!("ERROR: File '{}' already exists. Remove it or choose a different path.", output);
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
            println!("Crypto:          Post-Quantum (ML-DSA-65 + SLH-DSA-SHAKE-128f)");
            println!("ML-DSA PK:       {} bytes (lattice-based)", pichain_crypto::pq::ML_DSA_PK_BYTES);
            println!("SLH-DSA PK:      {} bytes (hash-based)", pichain_crypto::pq::SLH_DSA_PK_BYTES);
            println!("Saved to:        {}", output);
            println!("\nBoth signatures must verify for every transaction.");
            println!("Quantum computers cannot break this wallet.\n");
            println!("IMPORTANT: Keep '{}' safe. Your keys cannot be recovered.", output);
        }

        Commands::VerifyWallet { path } => {
            let contents = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| { eprintln!("Failed to read '{}': {}", path, e); std::process::exit(1); });
            let export: pichain_crypto::pq_wallet::PqWalletExport = serde_json::from_str(&contents)
                .unwrap_or_else(|e| { eprintln!("Invalid wallet JSON: {}", e); std::process::exit(1); });

            print!("Verifying wallet... ");
            match pichain_crypto::verify_pq_wallet(&export) {
                Ok(()) => {
                    println!("VALID");
                    println!("Address:     {}", export.address);
                    println!("ML-DSA key:  {} bytes", hex::decode(&export.ml_dsa_public_key).map(|b| b.len()).unwrap_or(0));
                    println!("SLH-DSA key: {} bytes", hex::decode(&export.slh_dsa_public_key).map(|b| b.len()).unwrap_or(0));
                }
                Err(e) => {
                    println!("INVALID — {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::ComputePi { position, count } => {
            use pichain_mining::bbp::{BbpComputer, digits_to_hex_string};

            println!("Computing {count} hex digits of PI starting at position {position}...\n");

            let start = std::time::Instant::now();
            let digits = BbpComputer::compute_hex_digits(position, count);
            let elapsed = start.elapsed();

            let hex_str = digits_to_hex_string(&digits);
            println!("PI hex [{position}..{}]: {hex_str}", position + count as u64);
            println!("\nComputed in {:.3}s ({:.0} digits/sec)",
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
            println!("Total Supply:     3,141,592,653 PI (FIXED, NON-MINTABLE)");
            println!("Base Unit:        1 PI = 1,000,000,000 base units\n");
            println!("Distribution:");
            println!("  85% Mining Pool (Proof of Useful Work): 2,670,353,755 PI");
            println!("  10% Staking Rewards:                      314,159,265 PI");
            println!("   5% Initial Liquidity:                    157,079,632 PI\n");
            println!("Fee Structure (EIP-1559 + Burn):");
            println!("  25% of base fee → BURNED PERMANENTLY");
            println!("  25% of base fee → Mining pool replenishment");
            println!("  50% of base fee → Stakers / block proposer");
            println!("  100% of priority fee → Block producer\n");
            println!("Quantum Resistance:");
            println!("  Signatures: ML-DSA-65 + SLH-DSA-SHAKE-128f (dual PQ)");
            println!("  Addresses:  Blake3 XOR SHA3-256 quantum-safe hash");
            println!("  Mining:     VDF + sqrt scaling + progressive strengthening\n");

            let calc = pichain_mining::RewardCalculator::new();
            println!("Mining Emission Schedule (2pi% geometric decay):");
            for year in 1..=7 {
                let emission = calc.annual_emission(year);
                let pi = emission / 1_000_000_000;
                println!("  Year {year}: {pi:>15} PI");
            }
        }
    }
}
