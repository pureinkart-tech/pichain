//! PIChain Node — the main entry point.
//!
//! Ties together all components: crypto, storage, networking,
//! consensus, execution, mining, and RPC.

mod node_state;

use clap::{Parser, Subcommand};
use node_state::NodeState;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

/// Validator key file persisted to {data_dir}/validator.key.
///
/// Contains three key types:
/// - `p2p_secret`: libp2p peer identity (ed25519, required by libp2p protocol)
/// - `bls_secret`: BLS12-381 for consensus attestations
/// - `pq_wallet`: PQ keypair (ML-DSA-65 + SLH-DSA) for block signing + validator address
///
/// When libp2p adds PQ identity support, `p2p_secret` can be migrated.
#[derive(Serialize, Deserialize)]
struct ValidatorKeyFile {
    /// libp2p P2P identity key (network routing only).
    /// Uses ed25519 because libp2p requires it.
    #[serde(alias = "ed25519_secret")]
    p2p_secret: String,
    bls_secret: String,
    /// PQ wallet export (ML-DSA-65 + SLH-DSA) for block signing.
    /// Generated automatically on first run if missing.
    #[serde(default)]
    pq_wallet: Option<pichain_crypto::pq_wallet::PqWalletExport>,
}

#[derive(Parser)]
#[command(name = "pichain")]
#[command(about = "PIChain — The Deflationary High-Performance Blockchain")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Node configuration — loadable from TOML or CLI flags.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeConfig {
    /// Data directory for blockchain storage.
    pub data_dir: String,
    /// RPC server listen address.
    pub rpc_addr: String,
    /// P2P listen address.
    pub p2p_addr: String,
    /// Chain ID (314159 for mainnet, 31415 for devnet).
    pub chain_id: u64,
    /// Bootstrap peers (multiaddr format).
    #[serde(default)]
    pub bootstrap_peers: Vec<String>,
    /// Maximum mempool size.
    pub max_mempool_size: usize,
    /// Validator self-stake in PI (registered at startup).
    pub validator_stake_pi: u64,
    /// Log level (trace, debug, info, warn, error).
    pub log_level: String,
    /// RPC-only mode: sync blocks from peers but don't produce blocks.
    /// Used for public-facing API nodes separated from the validator.
    #[serde(default)]
    pub rpc_only: bool,
    /// Additional genesis validators (for multi-validator mode).
    /// Empty means single-node mode (backward compatible).
    #[serde(default)]
    pub genesis_validators: Vec<GenesisValidatorConfig>,
}

/// Configuration for an additional genesis validator.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenesisValidatorConfig {
    /// Hex-encoded 20-byte validator address.
    pub address: String,
    /// Validator stake in PI.
    pub stake_pi: u64,
    /// Hex-encoded 48-byte BLS public key.
    pub bls_public_key: String,
    /// Hex-encoded 96-byte BLS proof-of-possession.
    /// Required for multi-validator mode to prevent rogue-key attacks.
    #[serde(default)]
    pub bls_pop: Option<String>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            data_dir: "./pichain-data".to_string(),
            rpc_addr: "127.0.0.1:8314".to_string(),
            p2p_addr: "/ip4/0.0.0.0/tcp/9314".to_string(),
            chain_id: 31415,
            bootstrap_peers: Vec::new(),
            max_mempool_size: 100_000,
            validator_stake_pi: 100_000,
            log_level: "info".to_string(),
            rpc_only: false,
            genesis_validators: Vec::new(),
        }
    }
}

impl NodeConfig {
    fn load_from_file(path: &PathBuf) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: NodeConfig = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Generate a default config file.
    fn generate_default(path: &PathBuf) -> anyhow::Result<()> {
        let config = Self::default();
        let toml_str = toml::to_string_pretty(&config)?;
        std::fs::write(path, toml_str)?;
        Ok(())
    }

    /// Validate configuration at startup. Returns Err for fatal misconfigurations.
    fn validate(&self) -> anyhow::Result<()> {
        if self.chain_id == 0 {
            anyhow::bail!("chain_id must be non-zero");
        }
        if self.max_mempool_size == 0 {
            anyhow::bail!("max_mempool_size must be > 0");
        }
        if self.validator_stake_pi == 0 && !self.rpc_only {
            anyhow::bail!("validator_stake_pi must be > 0");
        }
        if self.rpc_addr.parse::<SocketAddr>().is_err() {
            anyhow::bail!("rpc_addr '{}' is not a valid socket address", self.rpc_addr);
        }
        if self.data_dir.is_empty() {
            anyhow::bail!("data_dir must not be empty");
        }
        // Validate genesis validators hex encoding
        for (i, gv) in self.genesis_validators.iter().enumerate() {
            if hex::decode(&gv.address).is_err() {
                anyhow::bail!("genesis_validators[{}].address is not valid hex", i);
            }
            if hex::decode(&gv.bls_public_key).is_err() {
                anyhow::bail!("genesis_validators[{}].bls_public_key is not valid hex", i);
            }
        }
        Ok(())
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Start the PIChain node.
    Run {
        /// Data directory for blockchain storage.
        #[arg(long, default_value = "./pichain-data")]
        data_dir: String,

        /// RPC server listen address.
        #[arg(long, default_value = "127.0.0.1:8314")]
        rpc_addr: String,

        /// P2P listen address.
        #[arg(long, default_value = "/ip4/0.0.0.0/tcp/9314")]
        p2p_addr: String,

        /// Chain ID (314159 for mainnet, 31415 for devnet).
        #[arg(long, default_value = "31415")]
        chain_id: u64,

        /// Path to TOML config file (overrides default settings).
        #[arg(long)]
        config: Option<PathBuf>,

        /// RPC-only mode: sync blocks from peers, don't produce blocks.
        /// Use this for public-facing API nodes separated from the validator.
        #[arg(long)]
        rpc_only: bool,
    },

    /// Initialize a new PIChain data directory.
    Init {
        /// Data directory path.
        #[arg(long, default_value = "./pichain-data")]
        data_dir: String,

        /// Chain ID.
        #[arg(long, default_value = "31415")]
        chain_id: u64,
    },

    /// Generate a new validator keypair.
    Keygen,

    /// Show node information.
    Info {
        /// RPC endpoint to query.
        #[arg(long, default_value = "http://127.0.0.1:8314")]
        rpc_url: String,
    },

    /// Mine PI digits (Proof of Useful Work).
    Mine {
        /// Starting digit position.
        #[arg(long, default_value = "0")]
        start_position: u64,

        /// Number of digits to compute per batch.
        #[arg(long, default_value = "1000")]
        batch_size: u32,

        /// Continuously mine (loop after each batch).
        #[arg(long)]
        continuous: bool,
    },

    /// Show blockchain status.
    Status {
        /// Data directory path.
        #[arg(long, default_value = "./pichain-data")]
        data_dir: String,
    },

    /// Generate a default TOML configuration file.
    GenerateConfig {
        /// Output path for the config file.
        #[arg(long, default_value = "./pichain.toml")]
        output: PathBuf,
    },

    /// Run the genesis ceremony — create a mainnet genesis with real addresses.
    GenesisCeremony {
        /// Path to genesis config JSON file. If --generate-template is set, this is the output.
        #[arg(long, default_value = "./genesis.json")]
        genesis_file: PathBuf,

        /// Generate a template genesis config with placeholder addresses.
        #[arg(long)]
        generate_template: bool,

        /// Verify an existing genesis config file (check supply, addresses, hash).
        #[arg(long)]
        verify: bool,

        /// Sign the genesis file with this validator's key (proves agreement).
        /// Requires --key-file to point to a validator.key file.
        #[arg(long)]
        sign: bool,

        /// Path to validator.key file (for --sign mode).
        #[arg(long)]
        key_file: Option<PathBuf>,

        /// Initialize data directory from a finalized genesis file.
        #[arg(long)]
        data_dir: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load config early so we can use log_level from it
    let cli = Cli::parse();

    // Determine log level: RUST_LOG env > config file > default "info"
    let (cfg, data_dir, rpc_addr, p2p_addr, chain_id) = match &cli.command {
        Commands::Run {
            data_dir,
            rpc_addr,
            p2p_addr,
            chain_id,
            config,
            rpc_only,
        } => {
            let mut cfg = if let Some(ref config_path) = config {
                match NodeConfig::load_from_file(config_path) {
                    Ok(c) => c,
                    Err(e) => {
                        // FATAL: If the operator explicitly passes a config file, it MUST
                        // parse correctly. Silently falling back to defaults could cause
                        // the node to run with the wrong chain_id, wrong genesis validators,
                        // wrong listen addresses, etc. — all of which are dangerous.
                        eprintln!(
                            "FATAL: failed to parse config file '{}': {e}\n\
                             The node will NOT start with default settings when a config file \
                             is explicitly provided.\n\
                             Fix the config file or remove --config to use defaults.",
                            config_path.display()
                        );
                        std::process::exit(1);
                    }
                }
            } else {
                NodeConfig::default()
            };
            // LIMITATION: These comparisons against hardcoded default values are fragile.
            // If the default_value in #[arg(...)] above changes, these must be updated too.
            // CLI args override config file values. If the CLI arg equals its default,
            // we assume it wasn't explicitly set and defer to the config file.
            let d = if *data_dir != "./pichain-data" {
                data_dir.clone()
            } else {
                cfg.data_dir.clone()
            };
            let r = if *rpc_addr != "127.0.0.1:8314" {
                rpc_addr.clone()
            } else {
                cfg.rpc_addr.clone()
            };
            let p = if *p2p_addr != "/ip4/0.0.0.0/tcp/9314" {
                p2p_addr.clone()
            } else {
                cfg.p2p_addr.clone()
            };
            let c = if *chain_id != 31415 {
                *chain_id
            } else {
                cfg.chain_id
            };
            // --rpc-only flag overrides config (CLI flag = true wins)
            if *rpc_only {
                cfg.rpc_only = true;
            }
            (Some(cfg), Some(d), Some(r), Some(p), Some(c))
        }
        _ => (None, None, None, None, None),
    };

    // Initialize tracing — use config log_level if RUST_LOG is not set
    let log_level = cfg.as_ref().map(|c| c.log_level.as_str()).unwrap_or("info");
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level)),
        )
        .init();

    match cli.command {
        Commands::Run { config, .. } => {
            let cfg = cfg.ok_or_else(|| {
                anyhow::anyhow!("node configuration not loaded — provide --config")
            })?;
            let data_dir = data_dir.ok_or_else(|| anyhow::anyhow!("data_dir not configured"))?;
            let rpc_addr = rpc_addr.ok_or_else(|| anyhow::anyhow!("rpc_addr not configured"))?;
            let p2p_addr = p2p_addr.ok_or_else(|| anyhow::anyhow!("p2p_addr not configured"))?;
            let chain_id = chain_id.ok_or_else(|| anyhow::anyhow!("chain_id not configured"))?;

            if config.is_some() {
                info!(
                    "Loaded config (log_level={}, mempool={})",
                    cfg.log_level, cfg.max_mempool_size
                );
            }

            // Validate configuration before starting
            cfg.validate()?;

            let is_rpc_only = cfg.rpc_only;
            if is_rpc_only {
                info!("Starting PIChain node in RPC-ONLY mode (no block production)");
            } else {
                info!("Starting PIChain node...");
            }
            info!(
                chain_id,
                data_dir,
                rpc_addr,
                p2p_addr,
                rpc_only = is_rpc_only
            );

            // --- 1. Initialize Storage ---
            info!("Opening database at {data_dir}");
            let db = match pichain_storage::PiChainDB::open(&data_dir) {
                Ok(db) => db,
                Err(e) => {
                    warn!("Database open failed ({e}), attempting repair...");
                    pichain_storage::PiChainDB::repair(&data_dir)?;
                    info!("Repair complete, retrying open...");
                    pichain_storage::PiChainDB::open(&data_dir)?
                }
            };
            pichain_storage::migration::migrate_to_latest(&db)?;
            let state_store = pichain_storage::StateStore::new(db);

            // --- 2. Initialize Execution Engine ---
            let executor = Arc::new(pichain_execution::TransactionExecutor::new(chain_id));
            info!("Execution engine initialized (Block-STM parallel, chain_id={chain_id})");

            // --- 3. Initialize Transaction Pool (Mempool) ---
            // PQ signature enforcement: ALWAYS enabled.
            // All transactions require PQ signatures (ML-DSA-65 + SLH-DSA).
            // Browser-based signing uses the PQ wallet connector.
            let mempool_config = pichain_execution::MempoolConfig {
                max_transactions: cfg.max_mempool_size,
                chain_id,
                ..Default::default()
            };
            let mempool = Arc::new(pichain_execution::TransactionPool::with_config(
                mempool_config,
            ));
            info!(
                "Transaction mempool initialized (capacity: {})",
                cfg.max_mempool_size
            );

            // --- 5. Create Shared Node State ---
            let mut node_state =
                NodeState::new(state_store, executor.clone(), mempool.clone(), chain_id);
            // Enable mempool WAL for crash recovery (replays pending txs from previous run)
            node_state.enable_mempool_wal(&data_dir);
            // Pool signer will be set after PQ keypair is loaded (step 8b)
            let node_state = Arc::new(node_state);

            // --- 6. Apply Genesis (if first run) ---
            let network_mode = pichain_types::NetworkMode::from_chain_id(chain_id);
            if network_mode.is_mainnet() && !is_rpc_only {
                // Mainnet safety: require at least 4 validators for BFT safety (3f+1)
                if cfg.genesis_validators.len() < 3 {
                    anyhow::bail!(
                        "mainnet requires at least 4 validators (self + 3 genesis_validators) \
                         for BFT safety. Got {} genesis_validators.",
                        cfg.genesis_validators.len()
                    );
                }
                info!("MAINNET mode — strict validation enabled");
            } else {
                info!(mode = ?network_mode, "network mode");
            }
            let genesis = if chain_id == 314159 {
                pichain_types::GenesisConfig::pichain_mainnet()
            } else {
                pichain_types::GenesisConfig::devnet()
            };
            // Validate genesis supply integrity before applying
            genesis
                .validate()
                .map_err(|e| anyhow::anyhow!("FATAL: genesis config invalid — {e}"))?;
            node_state.apply_genesis(&genesis)?;

            // --- 7. Resume From Last Block ---
            node_state.resume_from_storage()?;

            // --- 7b. Bootstrap bridge wrapped tokens + liquidity pools (idempotent) ---
            node_state.bootstrap_bridge_tokens()?;

            let current_height = node_state.height();
            let last_hash = node_state.last_hash();
            let current_base_fee = node_state.get_base_fee();

            info!(
                height = current_height,
                base_fee = current_base_fee,
                "chain state loaded"
            );

            // --- 8. Load or Generate Persistent Validator Keys ---
            // In RPC-only mode, generate ephemeral keys (needed for P2P peer identity only)
            let key_path = std::path::Path::new(&data_dir).join("validator.key");
            let (validator_key, validator_bls) = if is_rpc_only {
                // Ephemeral keys for P2P — no validator.key file needed
                let ed_kp = if key_path.exists() {
                    let key_data = std::fs::read_to_string(&key_path)?;
                    let keys: ValidatorKeyFile = serde_json::from_str(&key_data)
                        .map_err(|e| anyhow::anyhow!("failed to parse validator.key: {e}"))?;
                    let ed_bytes: [u8; 32] = hex::decode(&keys.p2p_secret)?
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("P2P secret key must be 32 bytes"))?;
                    pichain_crypto::Keypair::from_secret_bytes(&ed_bytes)
                } else {
                    pichain_crypto::Keypair::generate()
                };
                let bls_kp = pichain_crypto::BlsKeypair::generate();
                info!(peer_identity = %ed_kp.address(), "RPC-only mode — no validator key required");
                (ed_kp, bls_kp)
            } else if key_path.exists() {
                // Load existing keys
                let key_data = std::fs::read_to_string(&key_path)?;
                let keys: ValidatorKeyFile = serde_json::from_str(&key_data)
                    .map_err(|e| anyhow::anyhow!("failed to parse validator.key: {e}"))?;
                let ed_bytes: [u8; 32] = hex::decode(&keys.p2p_secret)?
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("P2P secret key must be 32 bytes"))?;
                let bls_bytes: [u8; 32] = hex::decode(&keys.bls_secret)?
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("BLS secret key must be 32 bytes"))?;
                let ed_kp = pichain_crypto::Keypair::from_secret_bytes(&ed_bytes);
                let bls_kp = pichain_crypto::BlsKeypair::from_bytes(&bls_bytes)?;
                info!(address = %ed_kp.address(), "loaded persistent validator keys from {}", key_path.display());
                (ed_kp, bls_kp)
            } else {
                // Generate new keys and persist
                let ed_kp = pichain_crypto::Keypair::generate();
                let bls_kp = pichain_crypto::BlsKeypair::generate();
                let key_file = ValidatorKeyFile {
                    p2p_secret: hex::encode(ed_kp.secret.to_bytes()),
                    bls_secret: hex::encode(bls_kp.secret.to_bytes()),
                    pq_wallet: None, // PQ keypair generated in step 8b
                };
                let json = serde_json::to_string_pretty(&key_file)?;
                let tmp_path = key_path.with_extension("key.tmp");
                std::fs::write(&tmp_path, &json)?;
                std::fs::rename(&tmp_path, &key_path)?;
                // Set restrictive permissions (owner read/write only)
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
                }
                info!(address = %ed_kp.address(), "generated new validator keys → {}", key_path.display());
                (ed_kp, bls_kp)
            };

            // --- 8b. Load or Generate PQ Block-Signing Keypair ---
            let pq_block_signer: pichain_crypto::PqKeypair = {
                // Reload key file to check for pq_wallet field
                let key_data = std::fs::read_to_string(&key_path)?;
                let mut keys: ValidatorKeyFile = serde_json::from_str(&key_data)
                    .map_err(|e| anyhow::anyhow!("failed to parse validator.key: {e}"))?;

                if let Some(ref pq_export) = keys.pq_wallet {
                    // Load existing PQ keypair
                    let kp = pichain_crypto::restore_pq_wallet(pq_export)
                        .map_err(|e| anyhow::anyhow!("failed to restore PQ keypair: {e}"))?;
                    info!(pq_address = %kp.address(), "loaded PQ block-signing keypair");
                    kp
                } else {
                    // Generate new PQ keypair and persist
                    info!("generating PQ block-signing keypair (ML-DSA-65 + SLH-DSA)...");
                    let (kp, export) = pichain_crypto::generate_pq_wallet();
                    keys.pq_wallet = Some(export);
                    let json = serde_json::to_string_pretty(&keys)?;
                    let tmp_path = key_path.with_extension("key.tmp");
                    std::fs::write(&tmp_path, &json)?;
                    std::fs::rename(&tmp_path, &key_path)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
                    }
                    info!(pq_address = %kp.address(), "generated PQ block-signing keypair → {}", key_path.display());
                    kp
                }
            };

            // Set pool signer for browser mining
            node_state.set_pool_signer(pq_block_signer.clone());

            // --- 9. Start P2P Networking ---
            // Resolve seeds: hardcoded mainnet seeds → DNS seeds → config bootstrap_peers
            let resolved_peers =
                pichain_network::PiChainSwarm::resolve_seeds(chain_id, &cfg.bootstrap_peers);
            if !resolved_peers.is_empty() {
                info!(
                    count = resolved_peers.len(),
                    "seed/bootstrap peers resolved"
                );
            }
            let swarm_config = pichain_network::SwarmConfig {
                listen_addr: p2p_addr.clone(),
                bootstrap_peers: resolved_peers,
                identity_secret: Some(validator_key.secret.to_bytes()),
                ..Default::default()
            };
            let mut p2p_swarm = pichain_network::PiChainSwarm::start(swarm_config).await?;
            let peer_id_str = p2p_swarm.peer_id().to_string();
            info!(peer_id = %peer_id_str, "P2P swarm started (GossipSub + Kademlia DHT)");

            // --- 10. Initialize Consensus Engine ---
            // Build validator set: self + additional genesis validators from config
            let self_validator = pichain_consensus::Validator {
                address: pq_block_signer.address(),
                bls_public_key: validator_bls.public.clone(),
                bls_pop: Some(validator_bls.secret.proof_of_possession()),
                stake: cfg.validator_stake_pi * pichain_types::BASE_UNITS_PER_PI,
                uptime_bps: 10000,
                contribution_score: 100,
                active: true,
            };
            let mut validators = vec![self_validator];
            for gv in &cfg.genesis_validators {
                let addr_hex = hex::decode(&gv.address).map_err(|e| {
                    anyhow::anyhow!(
                        "invalid genesis validator address hex '{}': {e}",
                        gv.address
                    )
                })?;
                let addr_bytes: [u8; 20] = addr_hex.try_into().map_err(|v: Vec<u8>| {
                    anyhow::anyhow!(
                        "genesis validator address must be 20 bytes, got {}: {}",
                        v.len(),
                        gv.address
                    )
                })?;
                let addr = pichain_crypto::keys::Address(addr_bytes);
                if addr == pq_block_signer.address() || addr == validator_key.address() {
                    continue; // Skip self (check both PQ and legacy addresses)
                }
                let bls_hex = hex::decode(&gv.bls_public_key).map_err(|e| {
                    anyhow::anyhow!(
                        "invalid genesis validator BLS key hex '{}': {e}",
                        gv.bls_public_key
                    )
                })?;
                let bls_bytes: [u8; 48] = bls_hex.try_into().map_err(|v: Vec<u8>| {
                    anyhow::anyhow!("BLS public key must be 48 bytes, got {}", v.len())
                })?;
                let bls_pk =
                    pichain_crypto::bls::BlsPublicKey::from_bytes(&bls_bytes).map_err(|e| {
                        anyhow::anyhow!("invalid BLS public key for validator {}: {e}", gv.address)
                    })?;
                // Verify BLS proof-of-possession if provided (required for multi-validator security)
                let pop_sig = if let Some(ref pop_hex) = gv.bls_pop {
                    let pop_bytes = hex::decode(pop_hex).map_err(|e| {
                        anyhow::anyhow!("invalid BLS PoP hex for validator {}: {e}", gv.address)
                    })?;
                    let pop_arr: [u8; 96] = pop_bytes.try_into().map_err(|v: Vec<u8>| {
                        anyhow::anyhow!(
                            "BLS PoP must be 96 bytes, got {} for validator {}",
                            v.len(),
                            gv.address
                        )
                    })?;
                    let pop =
                        pichain_crypto::bls::BlsSignature::from_bytes(&pop_arr).map_err(|e| {
                            anyhow::anyhow!("invalid BLS PoP for validator {}: {e}", gv.address)
                        })?;
                    bls_pk.verify_proof_of_possession(&pop)
                        .map_err(|e| anyhow::anyhow!("BLS PoP verification FAILED for validator {} — possible rogue-key attack: {e}", gv.address))?;
                    info!(validator = %gv.address, "BLS proof-of-possession verified");
                    Some(pop)
                } else {
                    anyhow::bail!(
                        "genesis validator {} is missing BLS proof-of-possession (bls_pop field). \
                         BLS PoP is REQUIRED to prevent rogue-key attacks in multi-validator consensus. \
                         Generate it with: pichain keygen",
                        gv.address
                    );
                };
                validators.push(pichain_consensus::Validator {
                    address: addr,
                    bls_public_key: bls_pk,
                    bls_pop: pop_sig,
                    stake: gv.stake_pi * pichain_types::BASE_UNITS_PER_PI,
                    uptime_bps: 10000,
                    contribution_score: 100,
                    active: true,
                });
            }
            if validators.len() > 1 {
                info!(total = validators.len(), "multi-validator consensus mode");
            }
            // BFT safety requires at least 4 validators (3f+1 where f=1).
            // In single-node/dev mode (no genesis_validators), allow 1 validator.
            // In multi-node mode (genesis_validators configured), warn if < 4.
            if validators.len() > 1 && validators.len() < 4 {
                warn!(
                    count = validators.len(),
                    "WARNING: fewer than 4 validators configured — BFT safety requires \
                     at least 4 validators (3f+1) to tolerate 1 Byzantine fault. \
                     With {} validators, ANY single fault halts consensus.",
                    validators.len()
                );
            }
            let validator_set = pichain_consensus::ValidatorSet::new(validators);
            let staking_manager = pichain_consensus::StakingManager::new();
            let consensus_config = pichain_consensus::ConsensusConfig {
                validator_address: pq_block_signer.address(),
                enable_fast_path: true,
                pi_seed: *last_hash.as_bytes(),
                chain_id,
            };
            let consensus_engine = std::sync::Arc::new(parking_lot::Mutex::new(
                pichain_consensus::ConsensusEngine::new(
                    consensus_config,
                    validator_set,
                    staking_manager,
                ),
            ));
            // Resume consensus from persisted height so the engine doesn't
            // try to re-commit blocks that already exist in storage.
            consensus_engine.lock().set_starting_round(current_height);

            // Inject BLS secret key for certificate signing (validators only)
            if !is_rpc_only {
                let bls_sk =
                    pichain_crypto::bls::BlsSecretKey::from_bytes(&validator_bls.secret.to_bytes())
                        .map_err(|e| anyhow::anyhow!("BLS secret key roundtrip failed: {e}"))?;
                consensus_engine.lock().set_bls_secret_key(bls_sk);
            }
            info!(
                address = %pq_block_signer.address(),
                rpc_only = is_rpc_only,
                "consensus engine initialized (Narwhal DAG + Bullshark + Avalanche fast-path)"
            );

            // --- 11. Initialize Transaction Pipeline (validators only) ---
            let pipeline_and_producer = if !is_rpc_only {
                let pipeline_config = pichain_execution::PipelineConfig {
                    target_block_time_ms: pichain_types::TARGET_BLOCK_TIME_MS,
                    max_txs_per_block: 50_000,
                    max_gas_per_block: 500_000_000,
                    validator_address: pq_block_signer.address(),
                };

                let block_config = pichain_execution::BlockProducerConfig {
                    validator_address: pq_block_signer.address(),
                    target_block_time_ms: pichain_types::TARGET_BLOCK_TIME_MS,
                    ..Default::default()
                };
                let mut block_producer = pichain_execution::BlockProducer::new(
                    block_config,
                    executor.clone(),
                    mempool.clone(),
                    chain_id,
                );
                let last_block_ts = node_state.last_block_timestamp_ms();
                block_producer.set_state(
                    current_height,
                    last_hash,
                    current_base_fee,
                    last_block_ts,
                );

                let pipeline = pichain_execution::TransactionPipeline::new(
                    pipeline_config,
                    executor.clone(),
                    mempool.clone(),
                );

                info!(
                    "Transaction pipeline initialized (3-stage: SigVerify → Banking → Ledger, {}ms blocks, height {})",
                    pichain_types::TARGET_BLOCK_TIME_MS,
                    current_height
                );
                Some((pipeline, block_producer))
            } else {
                info!(
                    height = current_height,
                    "RPC-only mode — block production disabled, syncing from peers"
                );
                None
            };

            // --- 12. Start RPC Server (with real state) ---
            let rpc = pichain_rpc::RpcServer::with_state(node_state.clone())
                .with_games_dir(std::path::PathBuf::from("explorer/games"));
            let rpc_addr: SocketAddr = rpc_addr.parse()?;
            info!(%rpc_addr, "Starting RPC server");

            // --- 13. Start Pipeline + Persistence ---
            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            let (block_tx, mut block_rx) = mpsc::channel(100);

            // Pipeline task — runs Stage 1 (SigVerify) + Stage 2 (Banking)
            // Only in validator mode — RPC-only nodes don't produce blocks
            let pipeline_shutdown = shutdown_rx.clone();
            let mut pipeline_handle =
                if let Some((pipeline, block_producer)) = pipeline_and_producer {
                    tokio::spawn(async move {
                        pipeline
                            .run(block_producer, pipeline_shutdown, block_tx)
                            .await;
                    })
                } else {
                    // RPC-only: spawn a no-op task that just waits for shutdown
                    let mut shutdown = pipeline_shutdown;
                    tokio::spawn(async move {
                        let _ = shutdown.changed().await;
                    })
                };

            // Extract P2P broadcaster before moving swarm
            let p2p_broadcaster = p2p_swarm.broadcaster();

            // Block processing task — consensus finality + persist to storage
            let persist_state = node_state.clone();
            let persist_consensus = consensus_engine.clone();
            let ws_broadcaster = rpc.ws_broadcaster().clone();
            let persist_p2p = p2p_broadcaster.clone();
            // Clone the PQ keypair for the persist task (block signing)
            let persist_pq_keypair = pq_block_signer.clone();
            let mut persist_handle = tokio::spawn(async move {
                while let Some(mut produced) = block_rx.recv().await {
                    // Sign the block with our PQ keypair (ML-DSA-65)
                    produced.block.sign_pq(&persist_pq_keypair);
                    let height = produced.block.header.height;
                    let tx_count = produced.block.header.tx_count;
                    let block_hash = produced.block.header.hash();

                    // Propose block through consensus engine (DAG + Bullshark).
                    // Evidence is drained inside the sync block; broadcast happens outside
                    // to avoid holding the !Send MutexGuard across .await points.
                    let evidence_to_broadcast = {
                        let mut engine = persist_consensus.lock();
                        let tx_hashes: Vec<pichain_crypto::Hash> = produced
                            .block
                            .transactions
                            .iter()
                            .map(|tx| tx.hash())
                            .collect();
                        engine.propose_block_at(
                            block_hash,
                            tx_hashes,
                            produced.block.header.timestamp_ms,
                        );

                        // Try to advance consensus and finalize blocks
                        let finalized = engine.try_advance();
                        if !finalized.is_empty() {
                            info!(
                                finalized_count = finalized.len(),
                                round = engine.current_round(),
                                committed_round = engine.committed_round(),
                                "Bullshark consensus advanced"
                            );
                        }

                        // Update PI seed from latest block hash
                        engine.set_pi_seed(*block_hash.as_bytes());

                        // Drain evidence while lock is held (returns Vec)
                        engine.drain_pending_evidence()
                    }; // MutexGuard dropped here

                    // Broadcast equivocation evidence to all peers (async, no lock held)
                    for ev in evidence_to_broadcast {
                        let msg = pichain_network::PeerSyncMessage::EquivocationEvidence {
                            author: ev.author.0,
                            round: ev.round,
                            cert_a: serde_json::to_vec(&ev.cert_a).unwrap_or_default(),
                            cert_b: serde_json::to_vec(&ev.cert_b).unwrap_or_default(),
                        };
                        if let Ok(msg_bytes) = serde_json::to_vec(&msg) {
                            let _ = persist_p2p.broadcast_sync(msg_bytes).await;
                        }
                    }

                    // Credit the proposer with their fee share (18.59% base + priority)
                    // before persist_block, which only handles staker distribution.
                    let proposer_total: u64 = produced.execution_results.iter()
                        .map(|r| r.proposer_reward)
                        .fold(0u64, |a, v| a.saturating_add(v));
                    if proposer_total > 0 {
                        persist_state.executor.credit_account(
                            produced.block.header.proposer,
                            proposer_total,
                        );
                    }

                    // Persist block + state changes to RocksDB
                    let computed_state_root = match persist_state.persist_block(&produced).await {
                        Ok(root) => root,
                        Err(e) => {
                            // R37-FIX: Break instead of continue. Continuing after a persist
                            // failure would orphan all subsequent blocks — the next block's
                            // parent_hash won't match because this block was never persisted,
                            // making all following blocks unrecoverable.
                            error!(height, error = %e, "FATAL: failed to persist block — halting block production");
                            break;
                        }
                    };

                    // R36-FIX: Update the block's state_root with the real value computed
                    // by JMT during persist_block. The block producer sets state_root to
                    // PoseidonHash::ZERO because the JMT root isn't known until after
                    // state changes are applied. Without this, peers receive a block with
                    // ZERO state_root and their state root verification always fails.
                    produced.block.header.state_root = computed_state_root;

                    // Broadcast block to P2P network
                    if let Ok(block_bytes) = serde_json::to_vec(&produced.block) {
                        if let Err(e) = persist_p2p.broadcast_block(block_bytes).await {
                            warn!(height, error = %e, "failed to broadcast block to P2P");
                        }
                    }

                    // Broadcast new block event to WebSocket subscribers
                    ws_broadcaster.broadcast(pichain_rpc::WsEvent::NewBlock {
                        height,
                        tx_count,
                        gas_used: produced.block.header.gas_used,
                        base_fee: produced.block.header.base_fee,
                        pi_burned: produced.block.header.pi_burned,
                        timestamp_ms: produced.block.header.timestamp_ms,
                        proposer: produced.block.header.proposer.to_string(),
                        block_hash: block_hash.to_string(),
                    });

                    // Broadcast individual transaction events + swap events
                    for (i, tx) in produced.block.transactions.iter().enumerate() {
                        let status = produced
                            .execution_results
                            .get(i)
                            .map(|r| match &r.effect.status {
                                pichain_types::TransactionStatus::Success => "success".to_string(),
                                pichain_types::TransactionStatus::Reverted(msg) => {
                                    format!("reverted: {msg}")
                                }
                                pichain_types::TransactionStatus::OutOfGas => {
                                    "out_of_gas".to_string()
                                }
                            })
                            .unwrap_or_else(|| "unknown".to_string());
                        let kind = match &tx.data.kind {
                            pichain_types::TransactionKind::Transfer { .. } => "Transfer",
                            pichain_types::TransactionKind::Stake { .. } => "Stake",
                            pichain_types::TransactionKind::Unstake { .. } => "Unstake",
                            pichain_types::TransactionKind::MiningProof { .. } => "MiningProof",
                            pichain_types::TransactionKind::DeployContract { .. } => {
                                "DeployContract"
                            }
                            pichain_types::TransactionKind::ContractCall { .. } => "ContractCall",
                            _ => "Other",
                        }
                        .to_string();
                        ws_broadcaster.broadcast(pichain_rpc::WsEvent::NewTransaction {
                            tx_hash: tx.hash().to_string(),
                            sender: tx.data.sender.to_string(),
                            kind,
                            block_height: height,
                            status,
                        });

                        // Broadcast swap events for DEX real-time charts
                        if let Some(result) = produced.execution_results.get(i) {
                            for event in &result.effect.events {
                                if event.event_type == "Swap" {
                                    if let Ok(sd) =
                                        serde_json::from_slice::<serde_json::Value>(&event.data)
                                    {
                                        let amount_in = sd["amount_in"].as_u64().unwrap_or(0);
                                        let amount_out = sd["amount_out"].as_u64().unwrap_or(0);
                                        let price = if amount_in > 0 {
                                            amount_out as f64 / amount_in as f64
                                        } else {
                                            0.0
                                        };
                                        ws_broadcaster.broadcast(
                                            pichain_rpc::WsEvent::SwapExecuted {
                                                pool_id: sd["pool"]
                                                    .as_str()
                                                    .unwrap_or("")
                                                    .to_string(),
                                                sender: tx.data.sender.to_string(),
                                                mint_in: sd["mint_in"]
                                                    .as_str()
                                                    .unwrap_or("")
                                                    .to_string(),
                                                mint_out: sd["mint_out"]
                                                    .as_str()
                                                    .unwrap_or("")
                                                    .to_string(),
                                                amount_in,
                                                amount_out,
                                                fee: sd["fee"].as_u64().unwrap_or(0),
                                                price,
                                                timestamp_ms: produced.block.header.timestamp_ms,
                                                block_height: height,
                                            },
                                        );
                                    }
                                }
                                // Broadcast betting match updates
                                if matches!(
                                    event.event_type.as_str(),
                                    "CreateMatch"
                                        | "JoinMatch"
                                        | "StartMatch"
                                        | "ResolveMatch"
                                        | "CancelMatch"
                                ) {
                                    if let Ok(ed) =
                                        serde_json::from_slice::<serde_json::Value>(&event.data)
                                    {
                                        ws_broadcaster.broadcast(
                                            pichain_rpc::WsEvent::MatchUpdate {
                                                match_id: ed["match_id"]
                                                    .as_str()
                                                    .unwrap_or("")
                                                    .to_string(),
                                                event_type: event.event_type.clone(),
                                                state: ed
                                                    .get("state")
                                                    .and_then(|s| s.as_str())
                                                    .unwrap_or("unknown")
                                                    .to_string(),
                                                block_height: height,
                                            },
                                        );
                                    }
                                }
                            }
                        }
                    }

                    if tx_count > 0 {
                        info!(
                            height,
                            txs = tx_count,
                            gas = produced.block.header.gas_used,
                            burned = produced.total_burned,
                            time_ms = produced.production_time_ms,
                            "block finalized and persisted"
                        );
                    }
                }
            });

            // --- P2P Inbound Message Processing Task ---
            // NOTE: This MUST be spawned BEFORE startup sync so that sync
            // responses (ChainTip, BlockData) have a handler to process them.
            let inbound_consensus = consensus_engine.clone();
            let inbound_chain_id = chain_id;
            let inbound_node_state = node_state.clone();
            let inbound_self_address = pq_block_signer.address();
            let inbound_sync_broadcaster = p2p_broadcaster.clone();
            let mut inbound_handle = tokio::spawn(async move {
                loop {
                    match p2p_swarm.recv().await {
                        Some(pichain_network::SwarmMessage::Block(data)) => {
                            match serde_json::from_slice::<pichain_types::Block>(&data) {
                                Ok(block) => {
                                    // SECURITY: Reject blocks from a different chain.
                                    if block.header.chain_id != inbound_chain_id {
                                        warn!(
                                            height = block.header.height,
                                            block_chain_id = block.header.chain_id,
                                            our_chain_id = inbound_chain_id,
                                            "rejected peer block: chain_id mismatch"
                                        );
                                        continue;
                                    }

                                    // SECURITY: Validate all transaction signatures in received blocks.
                                    // Without this, a malicious peer could inject blocks containing
                                    // forged transactions (e.g., unauthorized transfers, fake mining
                                    // proofs) which would be accepted and executed, corrupting state.
                                    let mut all_sigs_valid = true;
                                    for tx in &block.transactions {
                                        if tx.verify().is_err() {
                                            warn!(
                                                height = block.header.height,
                                                tx_hash = %tx.hash(),
                                                "rejected peer block: invalid transaction signature"
                                            );
                                            all_sigs_valid = false;
                                            break;
                                        }
                                    }
                                    if !all_sigs_valid {
                                        continue;
                                    }

                                    // Verify tx_root matches the block's transactions to prevent
                                    // blocks with tampered transaction lists.
                                    let computed_tx_root = block.compute_tx_root();
                                    if computed_tx_root != block.header.tx_root {
                                        warn!(
                                            height = block.header.height,
                                            "rejected peer block: tx_root mismatch"
                                        );
                                        continue;
                                    }

                                    // SECURITY: Reject blocks with timestamps too far in the future.
                                    // Maximum allowed drift: 15 seconds (covers network latency + clock skew).
                                    const MAX_TIMESTAMP_DRIFT_MS: u64 = 15_000;
                                    let now_ms =
                                        chrono::Utc::now().timestamp_millis().max(0) as u64;
                                    if block.header.timestamp_ms
                                        > now_ms.saturating_add(MAX_TIMESTAMP_DRIFT_MS)
                                    {
                                        warn!(
                                            height = block.header.height,
                                            block_ts = block.header.timestamp_ms,
                                            now = now_ms,
                                            "rejected peer block: timestamp too far in the future"
                                        );
                                        continue;
                                    }

                                    // Validate tx_count matches actual transaction count
                                    if block.header.tx_count as usize != block.transactions.len() {
                                        warn!(
                                            height = block.header.height,
                                            header_tx_count = block.header.tx_count,
                                            actual_tx_count = block.transactions.len(),
                                            "rejected peer block: tx_count mismatch"
                                        );
                                        continue;
                                    }

                                    // Validate gas_used is within block gas limit
                                    if block.header.gas_used > 500_000_000 {
                                        warn!(
                                            height = block.header.height,
                                            gas_used = block.header.gas_used,
                                            "rejected peer block: gas_used exceeds block limit"
                                        );
                                        continue;
                                    }

                                    // Validate proposer is not zero address (genesis excluded)
                                    if block.header.height > 0
                                        && block.header.proposer
                                            == pichain_crypto::keys::Address::ZERO
                                    {
                                        warn!(
                                            height = block.header.height,
                                            "rejected peer block: zero proposer address"
                                        );
                                        continue;
                                    }

                                    // SECURITY: Verify the block was actually signed by the claimed proposer.
                                    // Without this, ANY peer could forge blocks with arbitrary proposer addresses.
                                    if let Err(e) = block.verify_proposer_sig() {
                                        warn!(
                                            height = block.header.height,
                                            error = %e,
                                            "rejected peer block: invalid proposer signature"
                                        );
                                        continue;
                                    }

                                    // R30-FIX: Validate block height and parent hash continuity.
                                    // Without this, a malicious peer could send blocks with
                                    // arbitrary heights or parent hashes, causing chain forks
                                    // or corrupting local state.
                                    let expected_height = inbound_node_state.height() + 1;
                                    if block.header.height != expected_height {
                                        warn!(
                                            height = block.header.height,
                                            expected = expected_height,
                                            "rejected peer block: non-sequential height"
                                        );
                                        continue;
                                    }
                                    let local_last_hash = inbound_node_state.last_hash();
                                    if block.header.parent_hash != local_last_hash {
                                        warn!(
                                            height = block.header.height,
                                            "rejected peer block: parent_hash mismatch"
                                        );
                                        continue;
                                    }

                                    // Skip blocks we produced ourselves (already persisted)
                                    if block.header.proposer == inbound_self_address {
                                        debug!(
                                            height = block.header.height,
                                            "skipping own block from P2P"
                                        );
                                        continue;
                                    }

                                    // Execute and persist the peer block
                                    match inbound_node_state.execute_peer_block(&block).await {
                                        Ok(()) => { /* logged inside execute_peer_block */ }
                                        Err(e) => {
                                            warn!(
                                                height = block.header.height,
                                                error = %e,
                                                "failed to execute peer block"
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "failed to deserialize peer block");
                                }
                            }
                        }
                        Some(pichain_network::SwarmMessage::Consensus(data)) => {
                            let evidence_to_broadcast = match serde_json::from_slice::<
                                pichain_consensus::Certificate,
                            >(&data)
                            {
                                Ok(cert) => {
                                    let round = cert.round();
                                    let author = cert.author();
                                    let mut engine = inbound_consensus.lock();
                                    if let Err(e) = engine.receive_certificate(cert) {
                                        debug!(
                                            round,
                                            author = %author,
                                            error = %e,
                                            "rejected peer certificate"
                                        );
                                        Vec::new()
                                    } else {
                                        debug!(round, author = %author, "accepted peer certificate");
                                        let finalized = engine.try_advance();
                                        if !finalized.is_empty() {
                                            info!(
                                                count = finalized.len(),
                                                "consensus advanced from peer certificate"
                                            );
                                        }
                                        engine.drain_pending_evidence()
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "failed to deserialize peer consensus msg");
                                    Vec::new()
                                }
                            }; // MutexGuard dropped here

                            // Broadcast evidence outside lock scope
                            for ev in evidence_to_broadcast {
                                let msg = pichain_network::PeerSyncMessage::EquivocationEvidence {
                                    author: ev.author.0,
                                    round: ev.round,
                                    cert_a: serde_json::to_vec(&ev.cert_a).unwrap_or_default(),
                                    cert_b: serde_json::to_vec(&ev.cert_b).unwrap_or_default(),
                                };
                                if let Ok(msg_bytes) = serde_json::to_vec(&msg) {
                                    let _ =
                                        inbound_sync_broadcaster.broadcast_sync(msg_bytes).await;
                                }
                            }
                        }
                        Some(pichain_network::SwarmMessage::Transaction(data)) => {
                            match serde_json::from_slice::<pichain_types::SignedTransaction>(&data)
                            {
                                Ok(tx) => {
                                    // SECURITY: Verify signature before mempool insertion to prevent
                                    // forged-sender DoS attacks that fill the mempool with invalid txs.
                                    if tx.verify().is_err() {
                                        debug!(tx_hash = %tx.hash(), "rejected peer transaction: invalid signature");
                                        continue;
                                    }
                                    // R37-FIX: Route through mempool_insert() instead of direct
                                    // mempool.insert(). mempool_insert() lazy-loads the sender's
                                    // on-chain nonce and rejects far-future nonce gaps, preventing
                                    // P2P transactions from bypassing nonce validation.
                                    if let Err(e) = inbound_node_state.insert_transaction(tx) {
                                        debug!(error = %e, "peer transaction rejected by mempool");
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "failed to deserialize peer transaction");
                                }
                            }
                        }
                        Some(pichain_network::SwarmMessage::Sync(data)) => {
                            match serde_json::from_slice::<pichain_network::PeerSyncMessage>(&data)
                            {
                                Ok(pichain_network::PeerSyncMessage::GetChainTip) => {
                                    let height = inbound_node_state.height();
                                    let hash = inbound_node_state.last_hash();
                                    let resp = pichain_network::PeerSyncMessage::ChainTip {
                                        height,
                                        block_hash: *hash.as_bytes(),
                                    };
                                    if let Ok(resp_bytes) = serde_json::to_vec(&resp) {
                                        let _ = inbound_sync_broadcaster
                                            .broadcast_sync(resp_bytes)
                                            .await;
                                    }
                                    debug!(height, "responded to GetChainTip request");
                                }
                                Ok(pichain_network::PeerSyncMessage::ChainTip {
                                    height: peer_height,
                                    ..
                                }) => {
                                    let local_height = inbound_node_state.height();
                                    if peer_height > local_height {
                                        info!(
                                            local = local_height,
                                            peer = peer_height,
                                            behind = peer_height - local_height,
                                            "peer is ahead — requesting missing blocks"
                                        );
                                        let req = pichain_network::PeerSyncMessage::GetBlocks {
                                            start_height: local_height + 1,
                                            end_height: peer_height,
                                        };
                                        if let Ok(req_bytes) = serde_json::to_vec(&req) {
                                            let _ = inbound_sync_broadcaster
                                                .broadcast_sync(req_bytes)
                                                .await;
                                        }
                                    }
                                }
                                Ok(pichain_network::PeerSyncMessage::GetBlocks {
                                    start_height,
                                    end_height,
                                }) => {
                                    let local_height = inbound_node_state.height();
                                    // SECURITY: Limit range to prevent sync amplification attacks.
                                    // Since GossipSub broadcasts to ALL peers, each block response
                                    // is an O(N) message. We limit to 20 blocks per request and
                                    // cap total response size to 5MB to prevent bandwidth exhaustion.
                                    const MAX_SYNC_BLOCKS: u64 = 20;
                                    const MAX_SYNC_BYTES: usize = 5 * 1024 * 1024; // 5MB
                                    let safe_end = end_height
                                        .min(local_height)
                                        .min(start_height.saturating_add(MAX_SYNC_BLOCKS));
                                    info!(
                                        start = start_height,
                                        end = safe_end,
                                        "serving blocks to syncing peer"
                                    );
                                    let mut total_bytes = 0usize;
                                    for h in start_height..=safe_end {
                                        if let Some(block) = inbound_node_state.get_block(h) {
                                            if let Ok(block_bytes) = serde_json::to_vec(&block) {
                                                total_bytes =
                                                    total_bytes.saturating_add(block_bytes.len());
                                                if total_bytes > MAX_SYNC_BYTES {
                                                    warn!(height = h, total_bytes, "sync response size limit reached, stopping");
                                                    break;
                                                }
                                                let resp =
                                                    pichain_network::PeerSyncMessage::BlockData {
                                                        height: h,
                                                        data: block_bytes,
                                                    };
                                                if let Ok(resp_bytes) = serde_json::to_vec(&resp) {
                                                    let _ = inbound_sync_broadcaster
                                                        .broadcast_sync(resp_bytes)
                                                        .await;
                                                }
                                            }
                                        }
                                    }
                                }
                                Ok(pichain_network::PeerSyncMessage::BlockData {
                                    height,
                                    data,
                                }) => {
                                    // A peer sent us a block we requested for sync.
                                    // Full validation is applied (same as regular peer blocks)
                                    // to prevent sync-time poisoning attacks.
                                    match serde_json::from_slice::<pichain_types::Block>(&data) {
                                        Ok(block) => {
                                            let expected_height = inbound_node_state.height() + 1;
                                            if height != expected_height
                                                || block.header.height != expected_height
                                            {
                                                debug!(
                                                    height,
                                                    expected = expected_height,
                                                    "sync block out of order, skipping"
                                                );
                                            } else if block.header.chain_id != inbound_chain_id {
                                                warn!(height, "sync block chain_id mismatch");
                                            } else if block.header.proposer == inbound_self_address
                                            {
                                                debug!(height, "skipping own sync block");
                                            } else {
                                                // Validate proposer signature
                                                let mut valid = true;
                                                if let Err(e) = block.verify_proposer_sig() {
                                                    warn!(height, error = %e, "sync block: invalid proposer signature");
                                                    valid = false;
                                                }

                                                // Validate all transaction signatures
                                                if valid {
                                                    for tx in &block.transactions {
                                                        if tx.verify().is_err() {
                                                            warn!(height, tx_hash = %tx.hash(), "sync block: invalid tx signature");
                                                            valid = false;
                                                            break;
                                                        }
                                                    }
                                                }

                                                // Validate tx_root matches transactions
                                                if valid {
                                                    let computed_tx_root = block.compute_tx_root();
                                                    if computed_tx_root != block.header.tx_root {
                                                        warn!(
                                                            height,
                                                            "sync block: tx_root mismatch"
                                                        );
                                                        valid = false;
                                                    }
                                                }

                                                // Validate tx_count matches actual transaction count
                                                if valid
                                                    && block.header.tx_count as usize
                                                        != block.transactions.len()
                                                {
                                                    warn!(height, "sync block: tx_count mismatch");
                                                    valid = false;
                                                }

                                                // Validate gas_used within block limit
                                                if valid && block.header.gas_used > 500_000_000 {
                                                    warn!(
                                                        height,
                                                        "sync block: gas_used exceeds limit"
                                                    );
                                                    valid = false;
                                                }

                                                // Validate parent hash continuity
                                                if valid {
                                                    let local_last_hash =
                                                        inbound_node_state.last_hash();
                                                    if block.header.parent_hash != local_last_hash {
                                                        warn!(
                                                            height,
                                                            "sync block: parent_hash mismatch"
                                                        );
                                                        valid = false;
                                                    }
                                                }

                                                // Validate zero proposer (genesis excluded)
                                                if valid
                                                    && block.header.height > 0
                                                    && block.header.proposer
                                                        == pichain_crypto::keys::Address::ZERO
                                                {
                                                    warn!(
                                                        height,
                                                        "sync block: zero proposer address"
                                                    );
                                                    valid = false;
                                                }

                                                if valid {
                                                    match inbound_node_state
                                                        .execute_peer_block(&block)
                                                        .await
                                                    {
                                                        Ok(()) => {
                                                            info!(height, "sync block applied");
                                                        }
                                                        Err(e) => {
                                                            warn!(height, error = %e, "failed to apply sync block");
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!(height, error = %e, "failed to deserialize sync block");
                                        }
                                    }
                                }
                                Ok(pichain_network::PeerSyncMessage::EquivocationEvidence {
                                    author,
                                    round,
                                    cert_a,
                                    cert_b,
                                }) => {
                                    // Process equivocation evidence received from a peer.
                                    match (
                                        serde_json::from_slice::<pichain_consensus::Certificate>(
                                            &cert_a,
                                        ),
                                        serde_json::from_slice::<pichain_consensus::Certificate>(
                                            &cert_b,
                                        ),
                                    ) {
                                        (Ok(ca), Ok(cb)) => {
                                            let evidence =
                                                pichain_consensus::EquivocationEvidence {
                                                    author: pichain_crypto::keys::Address(
                                                        author,
                                                    ),
                                                    round,
                                                    cert_a: ca,
                                                    cert_b: cb,
                                                };
                                            let mut engine = inbound_consensus.lock();
                                            let slashed = engine.process_remote_evidence(evidence);
                                            if slashed {
                                                warn!(
                                                    validator = ?author,
                                                    round,
                                                    "applied remote equivocation evidence — validator slashed"
                                                );
                                            }
                                        }
                                        _ => {
                                            warn!(round, "failed to deserialize equivocation evidence certificates");
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "failed to deserialize sync message");
                                }
                            }
                        }
                        Some(pichain_network::SwarmMessage::PeerDiscovered(peer_id)) => {
                            info!(%peer_id, "new peer connected");
                        }
                        Some(pichain_network::SwarmMessage::PeerDisconnected(peer_id)) => {
                            info!(%peer_id, "peer disconnected");
                        }
                        Some(_) => {} // MiningProof, etc.
                        None => {
                            warn!("P2P inbound channel closed");
                            break;
                        }
                    }
                }
            });

            // --- Startup Chain Sync ---
            // The inbound handler is now running and can process sync responses.
            // Wait for peer connections, broadcast GetChainTip, then monitor
            // height until sync stabilizes before starting block production.
            {
                let has_bootstrap = !cfg.bootstrap_peers.is_empty();
                if has_bootstrap {
                    info!("waiting for peer connections before starting block production...");
                    let sync_broadcaster = p2p_broadcaster.clone();

                    // Give swarm time to establish connections to bootstrap peers
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

                    // Broadcast GetChainTip to discover peer heights
                    let tip_req = pichain_network::PeerSyncMessage::GetChainTip;
                    if let Ok(msg_bytes) = serde_json::to_vec(&tip_req) {
                        let _ = sync_broadcaster.broadcast_sync(msg_bytes).await;
                    }

                    // Wait for sync to complete. The inbound handler processes ChainTip
                    // responses, issues GetBlocks requests, and applies BlockData.
                    // We monitor node_state.height() until it stabilizes.
                    let initial_height = node_state.height();
                    let mut last_height = initial_height;
                    let mut stable_count = 0u32;
                    for _ in 0..60 {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        let current = node_state.height();
                        if current > last_height {
                            info!(height = current, "syncing blocks...");
                            last_height = current;
                            stable_count = 0;

                            // Re-broadcast GetChainTip to discover if more blocks exist
                            let tip_req = pichain_network::PeerSyncMessage::GetChainTip;
                            if let Ok(msg_bytes) = serde_json::to_vec(&tip_req) {
                                let _ = sync_broadcaster.broadcast_sync(msg_bytes).await;
                            }
                        } else {
                            stable_count += 1;
                            if stable_count >= 5 {
                                if current > initial_height {
                                    info!(
                                        initial = initial_height,
                                        current, "chain sync complete — caught up"
                                    );
                                } else {
                                    info!(height = current, "chain sync complete — already at tip");
                                }
                                break;
                            }
                        }
                    }
                }
            }

            // Refresh state values after potential sync
            let state_root = node_state.state_root();
            let current_height = node_state.height();
            let current_base_fee = node_state.get_base_fee();
            println!();
            println!("  ====================================================");
            println!("   PIChain Node v{}", env!("CARGO_PKG_VERSION"));
            println!("  ====================================================");
            println!("   Chain ID:       {chain_id}");
            println!("   RPC:            http://{rpc_addr}");
            println!("   P2P:            {p2p_addr}");
            println!("   Peer ID:        {peer_id_str}");
            println!("   Validator:      {}", pq_block_signer.address());
            println!("   Block Height:   {current_height}");
            println!("   State Root:     {state_root}");
            println!("   Base Fee:       {current_base_fee}");
            println!(
                "   Block Time:     {}ms",
                pichain_types::TARGET_BLOCK_TIME_MS
            );
            println!("   Mempool:        {} pending", mempool.len());
            println!("  ====================================================");
            println!("   Components:");
            println!("     Storage ......... RocksDB (WAL sync, 512MB cache) + JMT");
            println!("     Consensus ....... Narwhal DAG + Bullshark + Avalanche fast-path");
            println!("     Execution ....... Block-STM parallel + revm EVM");
            println!("     Pipeline ........ 3-stage (SigVerify → Banking → Ledger)");
            println!("     Networking ...... libp2p + GossipSub + Kademlia DHT + Turbine");
            println!("     Mining .......... BBP PI digit computation (PoUW)");
            println!("     Security ........ Rate limiting + sig verify + tx TTL");
            println!("     WebSocket ....... ws://{rpc_addr}/ws (newBlocks, newTransactions, miningStatus)");
            println!("  ====================================================");
            println!("   Press Ctrl+C to shut down gracefully.");
            println!();

            // --- 14. Graceful Shutdown ---
            // Start RPC server with graceful shutdown support
            let shutdown_signal = shutdown_tx.clone();
            let mut rpc_handle = tokio::spawn(async move {
                let router = rpc.router();
                let listener = match tokio::net::TcpListener::bind(rpc_addr).await {
                    Ok(l) => l,
                    Err(e) => {
                        error!("failed to bind RPC address {rpc_addr}: {e}");
                        return;
                    }
                };
                if let Err(e) = axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(async move {
                    let mut rx = shutdown_signal.subscribe();
                    while rx.changed().await.is_ok() {
                        if *rx.borrow() {
                            break;
                        }
                    }
                })
                .await
                {
                    error!("RPC server error: {e}");
                }
            });

            // AUDIT-FIX C-1: View change timeout task.
            // Periodically checks if the consensus round has timed out (leader
            // didn't produce a certificate). If so, advances to the next round.
            // This prevents a single Byzantine leader from stalling the chain.
            let timeout_consensus = consensus_engine.clone();
            let timeout_shutdown = shutdown_rx.clone();
            let mut timeout_handle = tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
                loop {
                    interval.tick().await;
                    if *timeout_shutdown.borrow() {
                        break;
                    }
                    let mut engine = timeout_consensus.lock();
                    if engine.check_round_timeout() {
                        // Timeout occurred — try to advance consensus with whatever
                        // certificates we do have (may commit skipped rounds).
                        let finalized = engine.try_advance();
                        if !finalized.is_empty() {
                            info!(
                                finalized_count = finalized.len(),
                                round = engine.current_round(),
                                "Bullshark commit after view change timeout"
                            );
                        }
                    }
                }
            });

            // Wait for SIGINT (Ctrl+C), SIGTERM, or a critical task panic
            let shutdown_signal = async {
                #[cfg(unix)]
                {
                    let mut sigterm =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                            .expect("failed to register SIGTERM handler");
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {
                            info!("SIGINT received — stopping node gracefully");
                        }
                        _ = sigterm.recv() => {
                            info!("SIGTERM received — stopping node gracefully");
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    tokio::signal::ctrl_c().await.ok();
                    info!("SIGINT received — stopping node gracefully");
                }
            };

            tokio::select! {
                _ = shutdown_signal => {}
                result = &mut pipeline_handle => {
                    match result {
                        Ok(()) => error!("pipeline task exited unexpectedly"),
                        Err(e) => error!("pipeline task panicked: {e}"),
                    }
                }
                result = &mut persist_handle => {
                    match result {
                        Ok(()) => error!("block processing task exited unexpectedly"),
                        Err(e) => error!("block processing task panicked: {e}"),
                    }
                }
                result = &mut rpc_handle => {
                    match result {
                        Ok(()) => error!("RPC server task exited unexpectedly"),
                        Err(e) => error!("RPC server task panicked: {e}"),
                    }
                }
                result = &mut inbound_handle => {
                    match result {
                        Ok(()) => error!("P2P inbound task exited unexpectedly"),
                        Err(e) => error!("P2P inbound task panicked: {e}"),
                    }
                }
                result = &mut timeout_handle => {
                    match result {
                        Ok(()) => error!("timeout checker task exited unexpectedly"),
                        Err(e) => error!("timeout checker task panicked: {e}"),
                    }
                }
            }

            // Signal all tasks to stop
            let _ = shutdown_tx.send(true);

            // R26-FIX: Wait up to 5 seconds for ALL tasks to drain gracefully.
            // Previously only pipeline_handle and persist_handle were awaited,
            // leaving rpc_handle and inbound_handle to be dropped abruptly.
            let drain_timeout = std::time::Duration::from_secs(5);
            let drain_start = std::time::Instant::now();
            tokio::select! {
                _ = async {
                    let _ = pipeline_handle.await;
                    let _ = persist_handle.await;
                    let _ = rpc_handle.await;
                    let _ = inbound_handle.await;
                    let _ = timeout_handle.await;
                } => {
                    info!(
                        elapsed_ms = drain_start.elapsed().as_millis() as u64,
                        "all tasks drained cleanly"
                    );
                }
                _ = tokio::time::sleep(drain_timeout) => {
                    warn!("shutdown timeout — aborting remaining tasks after 5s");
                }
            }
            info!("PIChain node shut down");
        }

        Commands::Init { data_dir, chain_id } => {
            info!("Initializing PIChain data directory: {data_dir}");

            // Create database
            let db = pichain_storage::PiChainDB::open(&data_dir)?;
            let state_store = pichain_storage::StateStore::new(db);
            let executor = Arc::new(pichain_execution::TransactionExecutor::new(chain_id));
            let mempool = Arc::new(pichain_execution::TransactionPool::with_config(
                pichain_execution::MempoolConfig {
                    chain_id,
                    ..Default::default()
                },
            ));

            let node_state = Arc::new(NodeState::new(state_store, executor, mempool, chain_id));

            // Apply genesis
            let genesis = if chain_id == 314159 {
                pichain_types::GenesisConfig::pichain_mainnet()
            } else {
                pichain_types::GenesisConfig::devnet()
            };
            // R32-FIX: Validate genesis before applying
            genesis
                .validate()
                .map_err(|e| anyhow::anyhow!("FATAL: genesis config invalid — {e}"))?;
            node_state.apply_genesis(&genesis)?;

            let state_root = node_state.state_root();
            let total_allocated: u128 = genesis.allocations.iter().map(|a| a.amount as u128).sum();
            let pi_amount = total_allocated / pichain_types::BASE_UNITS_PER_PI as u128;

            println!("PIChain data directory initialized:");
            println!("  Path:        {data_dir}");
            println!("  Chain ID:    {chain_id}");
            println!("  State Root:  {state_root}");
            println!("  Genesis:     block created");
            if chain_id == 314159 {
                println!("  Network:     MAINNET");
                println!("  Supply:      3,141,592,653 PI");
            } else {
                println!("  Network:     DEVNET");
                println!("  Allocated:   {} PI", pi_amount);
            }
            println!("  Allocations:");
            for alloc in &genesis.allocations {
                let pi = alloc.amount / pichain_types::BASE_UNITS_PER_PI;
                println!("    {} — {} PI", alloc.label, pi);
            }
        }

        Commands::Keygen => {
            // Generate P2P identity keypair (libp2p requires ed25519 for peer identity)
            let ed_kp = pichain_crypto::Keypair::generate();
            println!("=== PIChain Validator Keypair ===");
            println!("Address:     {}", ed_kp.address());
            println!("Public Key:  {}", ed_kp.public);
            println!(
                "Secret Key:  {} (KEEP SECRET!)",
                hex::encode(ed_kp.secret.to_bytes())
            );

            // Generate BLS keypair
            let bls_kp = pichain_crypto::BlsKeypair::generate();
            let bls_pop = bls_kp.secret.proof_of_possession();
            println!("\n=== BLS Consensus Key ===");
            println!("BLS Public:  {}", hex::encode(bls_kp.public.to_bytes()));
            println!(
                "BLS Secret:  {} (KEEP SECRET!)",
                hex::encode(bls_kp.secret.to_bytes())
            );
            println!("BLS PoP:     {}", hex::encode(bls_pop.to_bytes()));

            // Compute deterministic peer ID
            let secret_bytes = ed_kp.secret.to_bytes();
            match pichain_network::peer_id_from_ed25519(&secret_bytes) {
                Ok(peer_id) => {
                    println!("\n=== P2P Identity ===");
                    println!("Peer ID:     {peer_id}");
                    println!("Multiaddr:   /ip4/<YOUR_IP>/tcp/9314/p2p/{peer_id}");
                }
                Err(e) => {
                    warn!("could not compute peer ID: {e}");
                }
            }

            // Print TOML snippet for other nodes
            println!("\n=== Config Snippet (for other nodes' pichain.toml) ===");
            println!("[[genesis_validators]]");
            println!("address = \"{}\"", hex::encode(ed_kp.address().0));
            println!("stake_pi = 100000");
            println!(
                "bls_public_key = \"{}\"",
                hex::encode(bls_kp.public.to_bytes())
            );
            println!("bls_pop = \"{}\"", hex::encode(bls_pop.to_bytes()));

            println!("\nSave these keys securely. You will need them to run a validator.");
        }

        Commands::Info { rpc_url } => {
            println!("Querying PIChain node at {rpc_url}...");
            println!("(RPC client integration pending)");
        }

        Commands::Mine {
            start_position,
            batch_size,
            continuous,
        } => {
            use pichain_mining::bbp::{digits_to_hex_string, BbpComputer};

            println!("=== PIChain Miner — Computing the Infinite ===");
            println!("Starting position: {start_position}");
            println!("Batch size: {batch_size} hex digits");
            println!("Continuous: {continuous}\n");

            let mut pos = start_position;
            let mut total_digits = 0u64;
            let mut total_time = std::time::Duration::ZERO;

            loop {
                let start = std::time::Instant::now();
                let digits = BbpComputer::compute_hex_digits(pos, batch_size);
                let elapsed = start.elapsed();

                total_digits += batch_size as u64;
                total_time += elapsed;

                let hex_str = digits_to_hex_string(&digits);
                let rate = batch_size as f64 / elapsed.as_secs_f64();

                println!(
                    "Batch: position {pos}..{} ({batch_size} digits in {:.3}s, {:.0} d/s)",
                    pos + batch_size as u64,
                    elapsed.as_secs_f64(),
                    rate,
                );

                // Show first 64 chars of the batch
                let preview = if hex_str.len() > 64 {
                    &hex_str[..64]
                } else {
                    &hex_str
                };
                println!("  Digits: {preview}...");

                // Create and verify proof
                let proof = pichain_mining::MiningProof::new(
                    pos,
                    digits,
                    pichain_crypto::keys::Address::ZERO,
                    0,
                );
                let verifier = pichain_mining::ProofVerifier::new();
                match verifier.verify(&proof, &[0u8; 32]) {
                    Ok(()) => println!("  Proof: VERIFIED"),
                    Err(e) => println!("  Proof: FAILED - {e}"),
                }

                println!(
                    "  Total: {} digits in {:.1}s ({:.0} avg d/s)\n",
                    total_digits,
                    total_time.as_secs_f64(),
                    total_digits as f64 / total_time.as_secs_f64(),
                );

                if !continuous {
                    break;
                }
                pos += batch_size as u64;
            }
        }

        Commands::GenerateConfig { output } => {
            NodeConfig::generate_default(&output)?;
            println!("Default config written to: {}", output.display());
            println!(
                "Edit the file and pass it with: pichain run --config {}",
                output.display()
            );
        }

        Commands::GenesisCeremony {
            genesis_file,
            generate_template,
            verify,
            sign,
            key_file,
            data_dir,
        } => {
            if generate_template {
                // Generate a template with placeholder addresses
                let genesis = pichain_types::GenesisConfig::pichain_mainnet();
                let json = serde_json::to_string_pretty(&genesis)?;
                std::fs::write(&genesis_file, &json)?;
                println!("=== PIChain Genesis Ceremony ===");
                println!("Template genesis written to: {}", genesis_file.display());
                println!("Genesis hash: {}", genesis.genesis_hash());
                println!();
                println!("Next steps:");
                println!("  1. Edit the file to replace Address::ZERO with real addresses");
                println!("  2. Set timestamp_ms to the genesis ceremony time");
                println!("  3. Add initial validators");
                println!(
                    "  4. Verify:  pichain genesis-ceremony --genesis-file {} --verify",
                    genesis_file.display()
                );
                println!("  5. Sign:    pichain genesis-ceremony --genesis-file {} --sign --key-file validator.key", genesis_file.display());
                println!("  6. Init:    pichain genesis-ceremony --genesis-file {} --data-dir /path/to/data", genesis_file.display());
                println!();
                println!("Allocations:");
                for alloc in &genesis.allocations {
                    let pi = alloc.amount / pichain_types::BASE_UNITS_PER_PI;
                    println!(
                        "  {} — {:>15} PI — addr: {}",
                        alloc.label, pi, alloc.address
                    );
                }
                let total: u128 = genesis.allocations.iter().map(|a| a.amount as u128).sum();
                println!(
                    "  Total: {} PI ({} base units)",
                    total / pichain_types::BASE_UNITS_PER_PI as u128,
                    total
                );
            } else if sign {
                // Sign the genesis file to prove validator agreement
                let key_path = key_file.ok_or_else(|| {
                    anyhow::anyhow!(
                    "--key-file is required for --sign mode. Point it to your validator.key file."
                )
                })?;
                let key_data = std::fs::read_to_string(&key_path)?;
                let keys: ValidatorKeyFile = serde_json::from_str(&key_data)
                    .map_err(|e| anyhow::anyhow!("failed to parse validator.key: {e}"))?;
                let ed_bytes: [u8; 32] = hex::decode(&keys.p2p_secret)?
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("P2P secret key must be 32 bytes"))?;
                let kp = pichain_crypto::Keypair::from_secret_bytes(&ed_bytes);

                let contents = std::fs::read_to_string(&genesis_file)?;
                let genesis: pichain_types::GenesisConfig = serde_json::from_str(&contents)?;
                genesis
                    .validate()
                    .map_err(|e| anyhow::anyhow!("genesis invalid: {e}"))?;

                let genesis_hash = genesis.genesis_hash();
                let mut sign_msg = Vec::new();
                sign_msg.extend_from_slice(b"pichain-genesis-sign:");
                sign_msg.extend_from_slice(genesis_hash.as_bytes());
                let sig = kp.sign(&sign_msg);

                println!("=== PIChain Genesis Signature ===");
                println!("Genesis file:  {}", genesis_file.display());
                println!("Genesis hash:  {genesis_hash}");
                println!("Validator:     {}", kp.address());
                println!("Signature:     {}", hex::encode(sig.to_bytes()));
                println!();
                println!("Share this signature with all other validators.");
                println!(
                    "They can verify with: pichain genesis-ceremony --verify --genesis-file {}",
                    genesis_file.display()
                );
            } else if verify {
                // Verify a genesis file
                let contents = std::fs::read_to_string(&genesis_file)?;
                let genesis: pichain_types::GenesisConfig = serde_json::from_str(&contents)?;

                let genesis_hash = genesis.genesis_hash();

                println!("=== PIChain Genesis Verification ===");
                println!("File: {}", genesis_file.display());
                println!("Chain ID: {}", genesis.chain_id);
                println!("Timestamp: {} ms", genesis.timestamp_ms);
                println!("Genesis Hash: {genesis_hash}");
                println!();

                // Check supply
                let total: u128 = genesis.allocations.iter().map(|a| a.amount as u128).sum();
                let expected = pichain_types::TOTAL_SUPPLY;
                let supply_ok = total == expected;
                println!(
                    "Supply check: {} (total={}, expected={})",
                    if supply_ok { "PASS" } else { "FAIL" },
                    total,
                    expected
                );

                // Check for zero addresses (virtual pools are allowed to use Address::ZERO)
                let mut zero_count = 0;
                for alloc in &genesis.allocations {
                    if alloc.address == pichain_crypto::keys::Address::ZERO
                        && !alloc.virtual_pool
                    {
                        zero_count += 1;
                        println!(
                            "  WARNING: {} has Address::ZERO (placeholder — must be replaced)",
                            alloc.label
                        );
                    } else if alloc.virtual_pool {
                        println!("  OK: {} is virtual (minted over time)", alloc.label);
                    }
                }

                // Check timestamp
                let ts_ok = genesis.timestamp_ms > 0;
                println!(
                    "Timestamp check: {} ({})",
                    if ts_ok { "PASS" } else { "WARN — not set" },
                    genesis.timestamp_ms
                );

                // Check validators
                println!("Validators: {}", genesis.validators.len());
                for v in &genesis.validators {
                    println!(
                        "  {} — {} PI staked — {}",
                        v.name,
                        v.stake / pichain_types::BASE_UNITS_PER_PI,
                        v.address
                    );
                }

                // Check chain ID
                let chain_ok = genesis.chain_id == 314159;
                println!(
                    "Chain ID check: {} (mainnet=314159, got={})",
                    if chain_ok { "PASS" } else { "WARN" },
                    genesis.chain_id
                );

                println!();
                println!("ALL VALIDATORS MUST CONFIRM THIS HASH: {genesis_hash}");
                println!();
                if supply_ok && ts_ok && zero_count == 0 && chain_ok {
                    println!("Genesis file is VALID and ready for launch.");
                } else {
                    println!("Genesis file has ISSUES that must be resolved before launch.");
                    if zero_count > 0 {
                        println!(
                            "  - {} allocation(s) have placeholder Address::ZERO",
                            zero_count
                        );
                    }
                    if !ts_ok {
                        println!("  - Timestamp not set");
                    }
                    if !chain_ok {
                        println!("  - Chain ID is not 314159 (mainnet)");
                    }
                }
            } else if let Some(ref dir) = data_dir {
                // Initialize from custom genesis
                let contents = std::fs::read_to_string(&genesis_file)?;
                let genesis: pichain_types::GenesisConfig = serde_json::from_str(&contents)?;

                // R32-FIX: Use validate() for comprehensive genesis verification
                genesis
                    .validate()
                    .map_err(|e| anyhow::anyhow!("FATAL: genesis config invalid — {e}"))?;

                println!("Initializing from genesis: {}", genesis_file.display());

                let db = pichain_storage::PiChainDB::open(dir)?;
                let state_store = pichain_storage::StateStore::new(db);
                let executor = Arc::new(pichain_execution::TransactionExecutor::new(
                    genesis.chain_id,
                ));
                let mempool = Arc::new(pichain_execution::TransactionPool::with_config(
                    pichain_execution::MempoolConfig {
                        chain_id: genesis.chain_id,
                        ..Default::default()
                    },
                ));

                let node_state = Arc::new(NodeState::new(
                    state_store,
                    executor,
                    mempool,
                    genesis.chain_id,
                ));
                node_state.apply_genesis(&genesis)?;

                let state_root = node_state.state_root();
                let genesis_hash = genesis.genesis_hash();
                println!("  Chain ID:      {}", genesis.chain_id);
                println!("  Genesis Hash:  {genesis_hash}");
                println!("  State Root:    {state_root}");
                println!("  Allocations:   {}", genesis.allocations.len());
                println!("  Validators:    {}", genesis.validators.len());
                println!("  Data Dir:      {dir}");
                println!("  Genesis initialized successfully!");
                println!();
                println!(
                    "  Start the node: pichain run --data-dir {dir} --chain-id {}",
                    genesis.chain_id
                );
            } else {
                println!("Usage:");
                println!("  pichain genesis-ceremony --generate-template          Create template");
                println!("  pichain genesis-ceremony --verify                     Verify genesis");
                println!("  pichain genesis-ceremony --data-dir /path/to/data     Initialize from genesis");
            }
        }

        Commands::Status { data_dir } => {
            let db = pichain_storage::PiChainDB::open(&data_dir)?;
            let state_store = pichain_storage::StateStore::new(db);

            let height = state_store.latest_height()?;

            println!("=== PIChain Status ===");
            println!("Data directory: {data_dir}");
            println!("State root:    {}", state_store.state_root());
            println!("Block height:  {}", height);

            if let Some(last) = state_store.get_block(height)? {
                println!("Last block:");
                println!("  Height:      {}", last.header.height);
                println!("  Epoch:       {}", last.header.epoch);
                println!("  Tx count:    {}", last.header.tx_count);
                println!("  Gas used:    {}", last.header.gas_used);
                println!("  Base fee:    {}", last.header.base_fee);
                println!("  PI burned:   {}", last.header.pi_burned);
                println!("  Proposer:    {}", last.header.proposer);
                println!("  Timestamp:   {}", last.header.timestamp_ms);
            }
        }
    }

    Ok(())
}
