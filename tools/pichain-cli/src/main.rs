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
    /// Generate a new wallet (Ed25519 keypair).
    Wallet {
        /// Display the secret key (DANGEROUS — keep it safe!).
        #[arg(long)]
        show_secret: bool,
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
        Commands::Wallet { show_secret } => {
            let kp = pichain_crypto::Keypair::generate();
            println!("=== New PIChain Wallet ===");
            println!("Address:    {}", kp.address());
            println!("Public Key: {}", kp.public);
            if show_secret {
                println!("WARNING: Keep this secret key safe. Never share it.");
                println!("Secret Key: {}", hex::encode(kp.secret.to_bytes()));
            } else {
                println!("Secret Key: [hidden — use --show-secret to display]");
            }
            println!("\nStore your secret key safely. It cannot be recovered.");
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
            println!("  40% Community & Mining Pool:    1,256,637,061 PI");
            println!("  20% Validator Rewards Reserve:    628,318,530 PI");
            println!("  15% Foundation & Development:    471,238,897 PI");
            println!("  10% Ecosystem Grants Fund:       314,159,265 PI");
            println!("   8% Team & Early Contributors:   251,327,412 PI");
            println!("   5% Public Sale / IDO:           157,079,632 PI");
            println!("   2% Strategic Partners:           62,831,853 PI\n");
            println!("Fee Structure (EIP-1559 + Burn):");
            println!("  25% of base fee → BURNED PERMANENTLY");
            println!("  65% of base fee → Distributed to stakers");
            println!("  10% of base fee → Protocol treasury");
            println!("  100% of priority fee → Block producer\n");

            let calc = pichain_mining::RewardCalculator::new();
            println!("Mining Emission Schedule:");
            for year in 1..=7 {
                let emission = calc.annual_emission(year);
                let pi = emission / 1_000_000_000;
                println!("  Year {year}: {pi:>15} PI");
            }
        }
    }
}
