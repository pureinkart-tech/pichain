//! PIChain Signer — lightweight PQ wallet + signing proxy for browser transactions.
//!
//! Two modes:
//! 1. HTTP proxy (default): localhost:8315 server for browser pages
//! 2. Native messaging: stdin/stdout protocol for Chrome/Firefox extension
//!
//! Usage:
//!   pichain-signer --wallet wallet.json                    # HTTP proxy mode
//!   pichain-signer --wallet wallet.json --generate         # Create new PQ wallet
//!   pichain-signer --native-messaging --wallet wallet.json # Extension native host
//!   pichain-signer --install-extension-host                # Register with Chrome/Firefox

mod native_messaging;
mod vault;

use axum::{
    extract::State,
    http::{HeaderValue, Method, StatusCode},
    response::Json,
    routing::{get, post},
    Router,
};
use clap::Parser;
use pichain_crypto::PqKeypair;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::{error, info, warn};

#[derive(Parser, Debug)]
#[command(name = "pichain-signer")]
#[command(about = "PIChain PQ Wallet — sign transactions for the browser")]
struct Args {
    /// Path to the PQ wallet file.
    #[arg(long, default_value = "wallet.json")]
    wallet: String,

    /// Port for the signing proxy (localhost only).
    #[arg(long, default_value = "8315")]
    port: u16,

    /// Chain ID for transaction validation.
    #[arg(long, default_value = "31415")]
    chain_id: u64,

    /// Generate a new PQ wallet and save it (then start serving).
    #[arg(long)]
    generate: bool,

    /// Run as Chrome native messaging host (stdin/stdout protocol).
    /// Used by the PIChain browser extension. Do not use directly.
    #[arg(long, hide = true)]
    native_messaging: bool,

    /// Install the native messaging host manifest for Chrome/Firefox.
    #[arg(long)]
    install_extension_host: bool,

    /// Enable multi-wallet vault mode for custodial services (e.g., trading bots).
    /// Wallets are stored encrypted in --vault-dir, keyed by user ID.
    #[arg(long)]
    vault: bool,

    /// Directory for encrypted vault wallets.
    #[arg(long, default_value = "vault")]
    vault_dir: String,

    /// Master secret file for vault encryption (32 bytes hex).
    /// Generated automatically on first run if it doesn't exist.
    #[arg(long, default_value = "vault-master.key")]
    vault_secret: String,
}

/// Shared state for the signing proxy.
struct ProxyState {
    pq_keypair: PqKeypair,
    address_hex: String,
    chain_id: u64,
    /// Rate limiter: tracks recent sign requests (timestamp in ms).
    sign_timestamps: std::sync::Mutex<Vec<u64>>,
}

/// Max signing requests per minute (prevents DoS via PQ signature spam).
const MAX_SIGNS_PER_MINUTE: usize = 30;

// ─── Handlers ───

async fn get_status(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "connected": true,
        "address": state.address_hex,
        "chain_id": state.chain_id,
        "version": env!("CARGO_PKG_VERSION"),
        "signer": "pichain-signer",
        "crypto": "ML-DSA-65 + SLH-DSA-SHAKE-128f"
    }))
}

async fn get_address(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "address": state.address_hex }))
}

/// Flexible sign request — accepts tx_data with sender as hex string or byte array.
#[derive(serde::Deserialize)]
struct SignRequest {
    tx_data: serde_json::Value,
}

async fn sign_transaction(
    State(state): State<Arc<ProxyState>>,
    Json(req): Json<SignRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    // Rate limiting: max 30 signs per minute
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut timestamps = state
            .sign_timestamps
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        timestamps.retain(|&t| now.saturating_sub(t) < 60_000);
        if timestamps.len() >= MAX_SIGNS_PER_MINUTE {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({"error": "rate limit exceeded (max 30 signs/minute)"})),
            );
        }
        timestamps.push(now);
    }

    // Parse the tx_data, handling sender as hex string (from browser) or byte array
    let mut tx_value = req.tx_data;

    // If sender is a hex string, convert to the byte array format TransactionData expects
    if let Some(sender_str) = tx_value.get("sender").and_then(|s| s.as_str()) {
        let sender_hex = sender_str.strip_prefix("0x").unwrap_or(sender_str);
        // Verify sender matches our wallet
        if sender_hex != state.address_hex {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": format!("sender {} does not match wallet {}", sender_hex, state.address_hex)
                })),
            );
        }
        // Convert hex to byte array for serde
        match hex::decode(sender_hex) {
            Ok(bytes) if bytes.len() == 20 => {
                let arr: Vec<u8> = bytes;
                tx_value["sender"] = serde_json::json!(arr);
            }
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "invalid sender address" })),
                )
            }
        }
    }

    // Convert all hex string fields that look like addresses (40 hex chars = 20 bytes)
    // or mint IDs (64 hex chars = 32 bytes) to byte arrays for serde deserialization.
    // Browser JS sends everything as hex strings; Rust expects [u8; N] arrays.
    convert_hex_strings_to_arrays(&mut tx_value);

    // Deserialize into TransactionData
    let tx_data: pichain_types::transaction::TransactionData =
        match serde_json::from_value(tx_value) {
            Ok(td) => td,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": format!("invalid tx_data: {e}") })),
                )
            }
        };

    // Verify sender matches wallet
    let sender_hex = hex::encode(tx_data.sender.0);
    if sender_hex != state.address_hex {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!("sender {} does not match wallet {}", sender_hex, state.address_hex)
            })),
        );
    }

    // Verify chain_id
    if tx_data.chain_id != state.chain_id && tx_data.chain_id != 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("chain_id {} does not match {}", tx_data.chain_id, state.chain_id)
            })),
        );
    }

    info!(
        sender = %sender_hex,
        nonce = tx_data.nonce,
        kind = ?std::mem::discriminant(&tx_data.kind),
        "signing transaction"
    );

    // Sign with PQ dual signatures (ML-DSA-65 + SLH-DSA-SHAKE-128f)
    let signed = pichain_types::transaction::Transaction::sign_pq(tx_data, &state.pq_keypair);

    match serde_json::to_value(&signed) {
        Ok(v) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "signed_tx": v,
                "tx_hash": hex::encode(signed.hash().as_bytes()),
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("serialization failed: {e}") })),
        ),
    }
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

    // Generate wallet if requested
    if args.generate {
        let path = std::path::Path::new(&args.wallet);
        if path.exists() {
            anyhow::bail!("wallet file '{}' already exists", args.wallet);
        }
        println!("Generating post-quantum wallet...");
        let (_kp, export) = pichain_crypto::generate_pq_wallet();
        let json = serde_json::to_string_pretty(&export)?;
        // Set permissions BEFORE writing to prevent race condition
        // (create empty file with 0o600, then write content)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(path, "")?; // create empty
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::write(path, &json)?;
        #[cfg(not(unix))]
        {
            // On Windows, warn about manual permission setting
            eprintln!("WARNING: Set file permissions manually — right-click → Properties → Security → restrict to your user only");
        }
        println!("Wallet saved to: {}", args.wallet);
        println!("Address: {}", export.address);
        eprintln!();
        eprintln!("  SECURITY: This wallet file is NOT encrypted.");
        eprintln!("  Protect it like a password — do not share or leave unattended.");
        eprintln!("  For encrypted wallets, use the GUI miner or vault mode.");
        eprintln!();
    }

    // Check wallet exists — helpful message if not
    if !std::path::Path::new(&args.wallet).exists() && !args.generate {
        eprintln!();
        eprintln!("  No wallet found at '{}'", args.wallet);
        eprintln!();
        eprintln!("  To create a new wallet:");
        eprintln!("    pichain-signer --wallet wallet.json --generate");
        eprintln!();
        eprintln!("  Then start the signing proxy:");
        eprintln!("    pichain-signer --wallet wallet.json");
        eprintln!();
        #[cfg(target_os = "windows")]
        {
            eprintln!("Press Enter to close...");
            let _ = std::io::stdin().read_line(&mut String::new());
        }
        std::process::exit(1);
    }

    // Load wallet
    let contents = std::fs::read_to_string(&args.wallet)
        .map_err(|e| anyhow::anyhow!("Failed to read wallet '{}': {e}", args.wallet))?;
    let export: pichain_crypto::pq_wallet::PqWalletExport =
        serde_json::from_str(&contents).map_err(|e| anyhow::anyhow!("Invalid wallet JSON: {e}"))?;
    let pq_keypair = pichain_crypto::restore_pq_wallet(&export)
        .map_err(|e| anyhow::anyhow!("Failed to restore PQ wallet: {e}"))?;

    let address = pq_keypair.address();
    let address_hex = hex::encode(address.0);

    info!(%address, "PQ wallet loaded (ML-DSA-65 + SLH-DSA-SHAKE-128f)");

    // Install extension native messaging host manifest
    if args.install_extension_host {
        install_native_host_manifest(&args.wallet)?;
        println!("Native messaging host registered for Chrome/Firefox.");
        println!("You can now use the PIChain browser extension.");
        return Ok(());
    }

    // Native messaging mode (called by browser extension)
    if args.native_messaging {
        native_messaging::run_native_messaging(&pq_keypair, args.chain_id);
        return Ok(());
    }

    let state = Arc::new(ProxyState {
        pq_keypair,
        address_hex: address_hex.clone(),
        chain_id: args.chain_id,
        sign_timestamps: std::sync::Mutex::new(Vec::new()),
    });

    // CORS: allow pichain.net and localhost origins
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
            let s = origin.to_str().unwrap_or("").to_ascii_lowercase();
            // Extract host from origin (scheme://host[:port])
            // Handle IPv6 bracket notation: http://[::1]:8315
            let after_scheme = s.split("://").nth(1).unwrap_or("");
            let host = if after_scheme.starts_with('[') {
                // IPv6: extract between brackets
                after_scheme
                    .split(']')
                    .next()
                    .unwrap_or("")
                    .strip_prefix('[')
                    .unwrap_or("")
            } else {
                after_scheme.split(':').next().unwrap_or("")
            };
            // Explicit domain whitelist — no wildcard subdomains
            matches!(
                host,
                "pichain.net"
                    | "www.pichain.net"
                    | "app.pichain.net"
                    | "explorer.pichain.net"
                    | "pichain.io"
                    | "www.pichain.io"
                    | "app.pichain.io"
                    | "explorer.pichain.io"
                    | "localhost"
                    | "127.0.0.1"
                    | "::1"
            )
        }))
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([axum::http::header::CONTENT_TYPE]);

    let mut app = Router::new()
        .route("/status", get(get_status))
        .route("/address", get(get_address))
        .route("/sign", post(sign_transaction))
        .with_state(state);

    // Mount vault routes if vault mode is enabled
    if args.vault {
        let master_secret = load_or_create_master_secret(&args.vault_secret)?;
        let vault_state = Arc::new(vault::VaultState::new(
            std::path::PathBuf::from(&args.vault_dir),
            master_secret,
            args.chain_id,
        ));
        let vault_router = Router::new()
            .route("/vault/create", post(vault::vault_create))
            .route("/vault/address", post(vault::vault_address))
            .route("/vault/sign", post(vault::vault_sign))
            .route("/vault/export", post(vault::vault_export))
            .with_state(vault_state);
        app = app.merge(vault_router);
        info!(vault_dir = %args.vault_dir, "vault mode enabled — multi-wallet custodial signing");
    }

    let app = app.layer(cors);

    // Bind to localhost ONLY — never expose signing to the network
    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));

    println!();
    println!("  ╔══════════════════════════════════════════════╗");
    println!("  ║         PIChain Wallet & Signing Proxy       ║");
    println!("  ╠══════════════════════════════════════════════╣");
    println!("  ║  Address: {}  ║", &address_hex[..20]);
    println!(
        "  ║  Signing: http://127.0.0.1:{}              ║",
        args.port
    );
    println!("  ║  Crypto:  ML-DSA-65 + SLH-DSA-SHAKE-128f    ║");
    println!("  ║                                              ║");
    println!("  ║  Open pichain.net to use DEX, staking, etc.  ║");
    println!("  ║  This app signs transactions for the browser ║");
    println!("  ╚══════════════════════════════════════════════╝");
    println!();

    info!(%addr, "signing proxy listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

/// Install the native messaging host manifest for Chrome and Firefox.
///
/// Creates the JSON manifest file that tells the browser where to find
/// the pichain-signer binary for native messaging communication.
fn install_native_host_manifest(wallet_path: &str) -> anyhow::Result<()> {
    let exe_path = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("cannot determine executable path: {e}"))?;
    let exe_str = exe_path.to_string_lossy().to_string();

    let manifest = serde_json::json!({
        "name": "net.pichain.signer",
        "description": "PIChain Post-Quantum Transaction Signer",
        "path": exe_str,
        "type": "stdio",
        "allowed_origins": [
            "chrome-extension://*/"
        ],
        "allowed_extensions": [
            "pichain-wallet@pichain.net"
        ]
    });

    let json = serde_json::to_string_pretty(&manifest)?;

    // Chrome paths
    #[cfg(target_os = "linux")]
    {
        let chrome_dir = dirs_or_home().join(".config/google-chrome/NativeMessagingHosts");
        let chromium_dir = dirs_or_home().join(".config/chromium/NativeMessagingHosts");
        let firefox_dir = dirs_or_home().join(".mozilla/native-messaging-hosts");

        for dir in [&chrome_dir, &chromium_dir, &firefox_dir] {
            std::fs::create_dir_all(dir)?;
            let manifest_path = dir.join("net.pichain.signer.json");
            std::fs::write(&manifest_path, &json)?;
            println!("  Installed: {}", manifest_path.display());
        }
    }

    #[cfg(target_os = "macos")]
    {
        let chrome_dir =
            dirs_or_home().join("Library/Application Support/Google/Chrome/NativeMessagingHosts");
        let firefox_dir =
            dirs_or_home().join("Library/Application Support/Mozilla/NativeMessagingHosts");

        for dir in [&chrome_dir, &firefox_dir] {
            std::fs::create_dir_all(dir)?;
            let manifest_path = dir.join("net.pichain.signer.json");
            std::fs::write(&manifest_path, &json)?;
            println!("  Installed: {}", manifest_path.display());
        }
    }

    #[cfg(target_os = "windows")]
    {
        // On Windows, the manifest path is stored in the registry
        let dir = dirs_or_home().join("AppData/Local/PIChain");
        std::fs::create_dir_all(&dir)?;
        let manifest_path = dir.join("net.pichain.signer.json");
        std::fs::write(&manifest_path, &json)?;
        println!("  Installed: {}", manifest_path.display());
        println!("  NOTE: On Windows, you may also need to add a registry key:");
        println!("  HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\net.pichain.signer");
        println!("  with default value: {}", manifest_path.display());
    }

    Ok(())
}

/// Load or generate the vault master secret.
fn load_or_create_master_secret(path: &str) -> anyhow::Result<[u8; 32]> {
    let path = std::path::Path::new(path);
    if path.exists() {
        let hex_str = std::fs::read_to_string(path)?;
        let bytes = hex::decode(hex_str.trim())?;
        if bytes.len() != 32 {
            anyhow::bail!("master secret must be 32 bytes (got {})", bytes.len());
        }
        let mut secret = [0u8; 32];
        secret.copy_from_slice(&bytes);
        info!("vault master secret loaded from {}", path.display());
        Ok(secret)
    } else {
        // Generate new master secret
        let mut secret = [0u8; 32];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut secret);
        std::fs::write(path, hex::encode(secret))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        info!(
            "vault master secret generated and saved to {}",
            path.display()
        );
        println!(
            "WARNING: Keep {} safe! It encrypts all vault wallets.",
            path.display()
        );
        Ok(secret)
    }
}

fn dirs_or_home() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Recursively convert hex string values to byte arrays in JSON.
/// Handles addresses (40 hex chars → 20 bytes) and mint IDs (64 hex chars → 32 bytes).
/// This bridges the gap between browser JS (which sends hex strings) and
/// Rust serde (which expects [u8; N] arrays for Address/MintId types).
/// Field names that should have hex strings converted to byte arrays.
/// Only these fields are converted — prevents unintended conversion of
/// string fields like token names, metadata URIs, etc.
const HEX_CONVERTIBLE_FIELDS: &[&str] = &[
    "sender",
    "recipient",
    "validator",
    "contract",
    "multisig_address",
    "mint",
    "mint_a",
    "mint_b",
    "mint_in",
    "mint_out",
    "target",
    "nft_id",
    "collection_id",
    "match_id",
    "delegate",
    "anchor_block_hash",
];

/// Max recursion depth for JSON traversal (prevents stack exhaustion on crafted input).
const MAX_HEX_CONVERT_DEPTH: usize = 8;

fn convert_hex_strings_to_arrays(value: &mut serde_json::Value) {
    convert_hex_recursive(value, 0);
}

fn convert_hex_recursive(value: &mut serde_json::Value, depth: usize) {
    if depth > MAX_HEX_CONVERT_DEPTH {
        return; // Stop recursion — crafted deep nesting is rejected
    }
    match value {
        serde_json::Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if HEX_CONVERTIBLE_FIELDS.contains(&key.as_str()) {
                    if let serde_json::Value::String(s) = v {
                        let trimmed = s.strip_prefix("0x").unwrap_or(s);
                        let trimmed = trimmed.strip_prefix("Pi314").unwrap_or(trimmed);
                        if (trimmed.len() == 40 || trimmed.len() == 64)
                            && trimmed.chars().all(|c| c.is_ascii_hexdigit())
                        {
                            if let Ok(bytes) = hex::decode(trimmed) {
                                *v = serde_json::json!(bytes);
                            }
                        }
                    }
                } else {
                    convert_hex_recursive(v, depth + 1);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                convert_hex_recursive(v, depth + 1);
            }
        }
        _ => {}
    }
}
