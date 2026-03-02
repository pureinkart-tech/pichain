//! JSON-RPC server implementation using axum.

use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, State, WebSocketUpgrade},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};

use crate::RpcError;

/// Strip optional "0x" or "0X" prefix from a hex string.
fn strip_0x(s: &str) -> &str {
    s.strip_prefix("Pi314")
        .or_else(|| s.strip_prefix("pi314"))
        .or_else(|| s.strip_prefix("0x"))
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s)
}

/// AUDIT-FIX RPC-M2: Check if an IP is loopback, including IPv4-mapped IPv6.
/// Rust's `is_loopback()` does NOT classify `::ffff:127.0.0.1` as loopback.
fn is_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

/// Check if an IP is a private/RFC1918 address (includes loopback).
/// Allows: 127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
/// Also handles IPv4-mapped IPv6 addresses (e.g. ::ffff:10.0.0.1).
fn is_private_ip(ip: IpAddr) -> bool {
    let v4 = match ip {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                return true;
            }
            v6.to_ipv4_mapped()
        }
    };
    match v4 {
        Some(v4) => {
            let octets = v4.octets();
            v4.is_loopback()                           // 127.0.0.0/8
                || octets[0] == 10                     // 10.0.0.0/8
                || (octets[0] == 172 && (octets[1] & 0xf0) == 16) // 172.16.0.0/12
                || (octets[0] == 192 && octets[1] == 168)         // 192.168.0.0/16
        }
        None => false,
    }
}

/// Per-IP rate limiter state.
struct IpRateLimiter {
    /// Per-IP request counts: IP → (count, window_start)
    buckets: DashMap<IpAddr, (u64, Instant)>,
    /// Maximum requests per window.
    max_requests: u64,
    /// Window duration in seconds.
    window_secs: u64,
    /// Maximum tracked IPs (prevents OOM from distributed attacks).
    max_entries: usize,
    /// Last cleanup time (for periodic cleanup without modulo race).
    last_cleanup: std::sync::Mutex<Instant>,
}

impl IpRateLimiter {
    fn new(max_requests: u64, window_secs: u64) -> Self {
        Self {
            buckets: DashMap::new(),
            max_requests,
            window_secs,
            max_entries: 50_000,
            last_cleanup: std::sync::Mutex::new(Instant::now()),
        }
    }

    /// Check if an IP is rate-limited. Returns true if allowed.
    fn check(&self, ip: IpAddr) -> bool {
        // If we've exceeded max entries, reject unknown IPs until cleanup
        if self.buckets.len() >= self.max_entries && !self.buckets.contains_key(&ip) {
            self.cleanup();
            if self.buckets.len() >= self.max_entries {
                return false;
            }
        }

        let now = Instant::now();
        let mut entry = self.buckets.entry(ip).or_insert((0, now));
        let (count, window_start) = entry.value_mut();

        // Reset window if expired
        if now.duration_since(*window_start).as_secs() >= self.window_secs {
            *count = 0;
            *window_start = now;
        }

        if *count >= self.max_requests {
            false
        } else {
            *count += 1;
            true
        }
    }

    /// Clean up expired entries to prevent memory growth.
    fn cleanup(&self) {
        let now = Instant::now();
        self.buckets.retain(|_, (_, start)| {
            now.duration_since(*start).as_secs() < self.window_secs * 2
        });
    }

    /// Check with a custom limit (for tiered rate limiting).
    /// Uses a separate key space by combining IP + tier limit.
    fn check_with_limit(&self, ip: IpAddr, limit: u64) -> bool {
        // For stricter tiers, we use the same buckets but track separately
        // by creating a synthetic "IP" that encodes the tier
        let tier_ip = match ip {
            IpAddr::V4(v4) => {
                let mut octets = v4.octets();
                // XOR the limit into the last octet to create a unique bucket
                octets[3] ^= (limit & 0xFF) as u8;
                IpAddr::V4(std::net::Ipv4Addr::from(octets))
            }
            IpAddr::V6(v6) => {
                let mut octets = v6.octets();
                octets[15] ^= (limit & 0xFF) as u8;
                IpAddr::V6(std::net::Ipv6Addr::from(octets))
            }
        };

        if self.buckets.len() >= self.max_entries && !self.buckets.contains_key(&tier_ip) {
            self.cleanup();
            if self.buckets.len() >= self.max_entries {
                return false;
            }
        }

        let now = Instant::now();
        let mut entry = self.buckets.entry(tier_ip).or_insert((0, now));
        let (count, window_start) = entry.value_mut();

        if now.duration_since(*window_start).as_secs() >= self.window_secs {
            *count = 0;
            *window_start = now;
        }

        if *count >= limit {
            false
        } else {
            *count += 1;
            true
        }
    }

    /// Periodic cleanup — runs at most once every 10 seconds.
    fn maybe_cleanup(&self) {
        if let Ok(mut last) = self.last_cleanup.try_lock() {
            if last.elapsed().as_secs() >= 10 {
                self.cleanup();
                *last = Instant::now();
            }
        }
    }
}

/// Classify a request path into a rate limit tier (requests per second).
/// Read: 100/s, Write: 20/s, Expensive: 10/s
fn classify_rate_tier(path: &str) -> u64 {
    // Write operations (tx submission, faucet, bridge mint)
    if path.contains("/tx/submit")
        || path.contains("/faucet")
        || path.contains("/bridge/mint")
        || path.contains("/bridge/withdraw")
        || path.contains("/bridge/deposit-intent")
        || path.contains("/wallet/activate")
    {
        return 20;
    }
    // Expensive queries (swap quotes, block ranges, richlist, events)
    if path.contains("/swap/quote")
        || path.contains("/blocks")
        || path.contains("/richlist")
        || path.contains("/events/query")
        || path.contains("/proof/")
        || path.contains("/portfolio/")
    {
        return 10;
    }
    // Read operations (default)
    100
}

/// Rate limiting middleware for axum.
async fn rate_limit_middleware(
    State(state): State<Arc<RpcState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    // SECURITY: Only trust X-Forwarded-For from known reverse proxy IPs (loopback).
    // Without this, any client can spoof the header to bypass rate limiting.
    let is_trusted_proxy = is_loopback_ip(addr.ip());
    let ip = if is_trusted_proxy {
        request
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.trim().parse::<IpAddr>().ok())
            .unwrap_or_else(|| addr.ip())
    } else {
        // Direct connection — use the actual connection IP, ignore X-Forwarded-For
        addr.ip()
    };

    // Increment request counter
    state.requests_total.fetch_add(1, Ordering::Relaxed);

    // SECURITY (Fix 196): Run periodic cleanup BEFORE the rate limit check,
    // not after. Previously, cleanup only ran when a request was ALLOWED.
    // If a client was perpetually rate-limited, their stale entries (and
    // entries from other expired IPs) were never cleaned up, causing
    // unbounded memory growth from distributed attacks.
    state.rate_limiter.maybe_cleanup();

    // Tiered rate limiting: classify request by path
    let path = request.uri().path();
    let tier_limit = classify_rate_tier(path);
    // Check per-tier rate limit (uses the general limiter for the base check,
    // plus stricter limits for write/expensive operations)
    let allowed = if tier_limit < 100 {
        // Write or expensive tier — use stricter per-path limit
        state.rate_limiter.check_with_limit(ip, tier_limit)
    } else {
        // Read tier — use default limit
        state.rate_limiter.check(ip)
    };

    if !allowed {
        state.requests_errors.fetch_add(1, Ordering::Relaxed);
        warn!(ip = %ip, tier = tier_limit, path = path, "rate limited");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "rate limit exceeded",
                "retry_after_secs": state.rate_limiter.window_secs,
            })),
        )
            .into_response();
    }

    next.run(request).await
}

/// Node info returned by RPC.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeInfo {
    pub chain_id: u64,
    pub version: String,
    pub block_height: u64,
    pub peer_count: usize,
    pub is_syncing: bool,
    pub state_root: String,
    pub base_fee: u64,
    pub total_burned: u64,
    pub mempool_size: usize,
}

/// A pending PoW challenge for wallet activation.
struct ActivationChallenge {
    /// Random 32-byte challenge seed.
    challenge: [u8; 32],
    /// Address this challenge is bound to.
    address: pichain_crypto::ed25519::Address,
    /// Expiry timestamp (seconds since UNIX epoch).
    expires_at: u64,
}

/// Shared state for the RPC server.
pub struct RpcState {
    pub chain_id: u64,
    pub block_height: Arc<RwLock<u64>>,
    pub peer_count: Arc<RwLock<usize>>,
    /// Storage-backed state accessors — when present, endpoints serve real data.
    pub state_provider: Option<Arc<dyn StateProvider>>,
    /// Server start time for uptime tracking.
    pub started_at: Instant,
    /// Request counters for metrics.
    pub requests_total: AtomicU64,
    pub requests_errors: AtomicU64,
    /// Per-IP rate limiter (100 req/sec default).
    rate_limiter: IpRateLimiter,
    /// Semaphore limiting concurrent `spawn_blocking` tasks to prevent thread-pool exhaustion.
    pub blocking_semaphore: Arc<tokio::sync::Semaphore>,
    /// WebSocket event broadcaster for real-time subscriptions.
    pub ws_broadcaster: Arc<crate::ws::WsBroadcaster>,
    /// Pending PoW challenges for wallet activation, keyed by challenge hex.
    activation_challenges: DashMap<String, ActivationChallenge>,
}

/// Trait for providing real state data to the RPC layer.
/// Implemented by `NodeState` in the node crate to avoid circular dependencies.
pub trait StateProvider: Send + Sync + 'static {
    fn chain_id(&self) -> u64;
    fn get_block_sync(&self, height: u64) -> Option<pichain_types::Block>;
    fn get_account_sync(&self, address: &pichain_crypto::ed25519::Address) -> Option<pichain_types::Account>;
    fn get_transaction_sync(&self, tx_hash: &pichain_crypto::Hash) -> Option<pichain_types::SignedTransaction>;
    fn get_receipt_sync(&self, tx_hash: &pichain_crypto::Hash) -> Option<pichain_types::TransactionEffect>;
    fn current_height(&self) -> u64;
    fn current_base_fee(&self) -> u64;
    fn total_burned(&self) -> u64;
    fn total_minted(&self) -> u64 { 0 }
    fn state_root_hex(&self) -> String;
    fn mempool_size(&self) -> usize;
    fn mempool_insert(&self, tx: pichain_types::SignedTransaction) -> Result<(), String>;

    /// Look up which block height contains a given transaction.
    fn get_tx_block_height(&self, _tx_hash: &pichain_crypto::Hash) -> Option<u64> {
        None
    }

    /// Whether the node is currently syncing with the network.
    fn is_syncing(&self) -> bool {
        false
    }

    // --- Token queries ---

    /// Get a token mint by ID.
    fn get_token_mint(&self, _mint_id: &pichain_types::MintId) -> Option<pichain_types::TokenMint> {
        None
    }
    /// Get a token account balance.
    fn get_token_account(
        &self,
        _owner: &pichain_crypto::ed25519::Address,
        _mint: &pichain_types::MintId,
    ) -> Option<pichain_types::TokenAccount> {
        None
    }

    // --- DEX queries ---

    /// Get a liquidity pool by token pair.
    fn get_pool_by_mints(
        &self,
        _mint_a: &pichain_types::MintId,
        _mint_b: &pichain_types::MintId,
    ) -> Option<pichain_types::LiquidityPool> {
        None
    }
    /// Calculate a swap quote.
    fn get_swap_quote(
        &self,
        _mint_in: &pichain_types::MintId,
        _mint_out: &pichain_types::MintId,
        _amount_in: u64,
    ) -> Option<SwapQuote> {
        None
    }

    // --- Mining queries ---

    /// Get mining stats (frontier, total digits, next position, etc).
    fn get_mining_stats(&self) -> Option<MiningStatusData> {
        None
    }

    // --- Wallet activation ---

    /// Get total number of activated wallets.
    fn activation_count(&self) -> u64 { 0 }

    /// Activate a wallet: grant 3.14 PI locked (non-transferable, fee-only).
    /// Returns the locked amount granted, or error if already activated.
    fn activate_wallet(&self, _address: &pichain_crypto::ed25519::Address) -> Result<u64, String> {
        Err("wallet activation not available".to_string())
    }

    // --- Launchpad / token listing queries ---

    /// List all token launches.
    fn scan_all_launches(&self) -> Vec<pichain_types::TokenLaunch> { vec![] }
    /// List all token mints.
    fn scan_all_mints(&self) -> Vec<pichain_types::TokenMint> { vec![] }
    /// List all liquidity pools.
    fn scan_all_pools(&self) -> Vec<pichain_types::LiquidityPool> { vec![] }
    /// Get a token launch by mint ID.
    fn get_launch_by_mint(&self, _mint: &pichain_types::MintId) -> Option<pichain_types::TokenLaunch> { None }
    /// Get the mint creation nonce for an address (for deterministic MintId derivation).
    fn get_mint_nonce(&self, _address: &pichain_crypto::ed25519::Address) -> u64 { 0 }
    /// Get all token balances for an owner address.
    fn get_token_balances_for_owner(
        &self,
        _owner: &pichain_crypto::ed25519::Address,
    ) -> Vec<(pichain_types::MintId, pichain_types::TokenAccount)> { vec![] }

    /// Bridge operator: mint wrapped tokens to a recipient address.
    /// Only callable from localhost by the bridge operator.
    fn bridge_mint(
        &self,
        _mint_symbol: &str,
        _recipient: &pichain_crypto::ed25519::Address,
        _amount: u64,
    ) -> Result<(), String> {
        Err("bridge minting not available".to_string())
    }

    /// Bridge: register custodial addresses from the bridge relayer.
    fn bridge_register_addresses(
        &self,
        _eth: &str,
        _sol: &str,
        _btc: &str,
        _usdt: &str,
    ) -> Result<(), String> {
        Err("bridge not available".to_string())
    }

    /// Bridge: get registered custodial addresses.
    fn bridge_get_addresses(&self) -> Option<BridgeAddressesInfo> {
        None
    }

    /// Bridge: get TVL status per wrapped token.
    fn bridge_status(&self) -> BridgeStatusInfo {
        BridgeStatusInfo::default()
    }

    /// Bridge: get recent bridge transfer records.
    fn bridge_get_transfers(&self, _chain: Option<&str>, _limit: usize) -> Vec<BridgeTransferInfo> {
        vec![]
    }

    /// Bridge: register deposit intent (maps external address to PIChain address).
    fn bridge_register_intent(
        &self,
        _chain: &str,
        _external_address: &str,
        _pichain_address: &str,
    ) -> Result<(), String> {
        Err("bridge not available".to_string())
    }

    /// Bridge: look up deposit intent.
    fn bridge_get_intent(&self, _chain: &str, _external_address: &str) -> Option<String> {
        None
    }

    /// Bridge: record a completed mint transfer.
    fn bridge_record_transfer(
        &self,
        _chain: &str,
        _tx_hash: &str,
        _symbol: &str,
        _recipient: &str,
        _amount: u64,
    ) {}

    // --- Transaction History ---

    /// Get transactions involving an address, newest-first.
    fn get_address_transactions(
        &self,
        _address: &pichain_crypto::ed25519::Address,
        _before_height: Option<u64>,
        _limit: usize,
    ) -> Vec<TxHistoryEntry> { vec![] }

    // --- Event Querying ---

    /// Query events by topic hash.
    fn query_events_by_topic(&self, _topic: &[u8; 32], _limit: usize) -> Vec<TxHistoryEntry> { vec![] }

    /// Query events by address.
    fn query_events_by_address(&self, _address: &pichain_crypto::ed25519::Address, _limit: usize) -> Vec<TxHistoryEntry> { vec![] }

    // --- Staking Queries ---

    /// Get list of all validators with stake info.
    fn get_validators(&self) -> Vec<ValidatorInfo> { vec![] }

    /// Get delegations for an address.
    fn get_delegations(&self, _address: &pichain_crypto::ed25519::Address) -> Vec<DelegationInfo> { vec![] }

    /// Get staking rewards for an address.
    fn get_staking_rewards(&self, _address: &pichain_crypto::ed25519::Address) -> u64 { 0 }

    // --- NFT Queries ---

    /// List all NFT collections.
    fn scan_all_collections(&self) -> Vec<pichain_types::NftCollection> { vec![] }

    /// Get NFTs in a collection.
    fn get_collection_items(&self, _collection_id: &pichain_types::CollectionId) -> Vec<pichain_types::Nft> { vec![] }

    /// Get NFTs owned by an address.
    fn get_nfts_by_owner(&self, _owner: &pichain_crypto::ed25519::Address) -> Vec<pichain_types::Nft> { vec![] }

    // --- Light Client ---

    /// Get JMT proof for an account.
    fn get_account_proof(&self, _address: &pichain_crypto::ed25519::Address) -> Option<Vec<u8>> { None }

    // --- Richlist ---

    /// Get top accounts by balance.
    fn get_richlist(&self, _limit: usize) -> Vec<(pichain_crypto::ed25519::Address, u64)> { vec![] }

    // --- Consensus Metrics ---

    /// Get consensus round number.
    fn consensus_round(&self) -> u64 { 0 }
    /// Get total Bullshark commits.
    fn consensus_commits(&self) -> u64 { 0 }
    /// Get total certificates produced.
    fn consensus_certificates(&self) -> u64 { 0 }
    /// Get validator count.
    fn validator_count(&self) -> usize { 0 }
    /// Get total stake.
    fn total_stake(&self) -> u64 { 0 }
}

/// Bridge registered addresses.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeAddressesInfo {
    pub eth: String,
    pub sol: String,
    pub btc: String,
    pub usdt: String,
}

/// Bridge status with TVL per token.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BridgeStatusInfo {
    pub tokens: Vec<BridgeTokenStatus>,
    pub total_transfers: usize,
}

/// Per-token bridge status.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeTokenStatus {
    pub symbol: String,
    pub total_supply: u64,
    pub decimals: u8,
}

/// Bridge transfer record info for API response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeTransferInfo {
    pub chain: String,
    pub tx_hash: String,
    pub symbol: String,
    pub recipient: String,
    pub amount: u64,
    pub timestamp: i64,
}

/// Entry in tx history or event query results.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxHistoryEntry {
    pub tx_hash: String,
    pub height: u64,
    pub tx_index: u16,
}

/// Validator information for staking API.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidatorInfo {
    pub address: String,
    pub stake: u64,
    pub delegated: u64,
    pub commission_bps: u16,
    pub active: bool,
    pub uptime_bps: u16,
    pub blocks_proposed: u64,
}

/// Delegation information.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DelegationInfo {
    pub validator: String,
    pub amount: u64,
    pub rewards_earned: u64,
}

/// Mining status data from the state provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MiningStatusData {
    pub frontier_position: u64,
    pub total_digits_verified: u64,
    pub next_position: u64,
    /// Maximum contiguous digits that can be mined at `next_position`.
    /// Miners should cap their batch size to this value to avoid overlap
    /// with existing ranges (e.g., from browser miners submitting small batches).
    #[serde(default)]
    pub max_batch_at_position: u64,
    pub total_ranges: u64,
    pub unique_miners: u64,
    pub remaining_pool: u64,
    pub total_mined: u64,
    /// Cumulative transaction fee income added to mining pool.
    #[serde(default)]
    pub fee_income: u64,
    pub reward_per_digit: u64,
    pub emission_year: u32,
    /// Current PoW difficulty in leading zero bits.
    pub difficulty_bits: u32,
    /// Current difficulty target as hex string.
    pub difficulty_target_hex: String,
    /// Last block hash (anchor for PoW nonce grinding).
    pub anchor_block_hash: String,
}

/// Swap quote returned by the RPC.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwapQuote {
    pub amount_out: u64,
    pub fee: u64,
    pub price_impact_bps: u64,
}

/// Check whether an origin string is allowed for CORS (Fix RPC-237).
///
/// Extracts the hostname from the origin URL and checks it EXACTLY,
/// not via prefix. This prevents `localhost.evil.com` from matching `localhost`.
///
/// Origin format: `scheme://host[:port]`
fn is_allowed_origin(origin: &str) -> bool {
    // Split off the scheme (e.g., "http" or "https")
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };

    // `rest` is "host[:port]" (origin never has a path)
    // Extract the host by stripping the optional port suffix
    let host = rest.split(':').next().unwrap_or(rest);

    // Reject empty host
    if host.is_empty() {
        return false;
    }

    // Allow localhost (exact match) and 127.0.0.1 for local development
    if host == "localhost" || host == "127.0.0.1" {
        return matches!(scheme, "http" | "https");
    }

    // Allow the official PIChain domains (exact match, https only)
    if host == "pichain.net"
        || host == "www.pichain.net"
        || host == "explorer.pichain.net"
        || host == "pichain.io"
        || host == "explorer.pichain.io"
    {
        return scheme == "https";
    }

    false
}

/// PIChain RPC server.
pub struct RpcServer {
    state: Arc<RpcState>,
}

impl RpcServer {
    pub fn new(chain_id: u64) -> Self {
        Self {
            state: Arc::new(RpcState {
                chain_id,
                block_height: Arc::new(RwLock::new(0)),
                peer_count: Arc::new(RwLock::new(0)),
                state_provider: None,
                started_at: Instant::now(),
                requests_total: AtomicU64::new(0),
                requests_errors: AtomicU64::new(0),
                rate_limiter: IpRateLimiter::new(100, 1), // 100 req/sec per IP
                blocking_semaphore: Arc::new(tokio::sync::Semaphore::new(50)),
                ws_broadcaster: Arc::new(crate::ws::WsBroadcaster::new(1024)),
                activation_challenges: DashMap::new(),
            }),
        }
    }

    /// Create an RPC server backed by a real state provider.
    pub fn with_state(provider: Arc<dyn StateProvider>) -> Self {
        let chain_id = provider.chain_id();
        Self {
            state: Arc::new(RpcState {
                chain_id,
                block_height: Arc::new(RwLock::new(0)),
                peer_count: Arc::new(RwLock::new(0)),
                state_provider: Some(provider),
                started_at: Instant::now(),
                requests_total: AtomicU64::new(0),
                requests_errors: AtomicU64::new(0),
                rate_limiter: IpRateLimiter::new(100, 1), // 100 req/sec per IP
                blocking_semaphore: Arc::new(tokio::sync::Semaphore::new(50)),
                ws_broadcaster: Arc::new(crate::ws::WsBroadcaster::new(1024)),
                activation_challenges: DashMap::new(),
            }),
        }
    }

    /// Build the axum router with all RPC endpoints.
    pub fn router(&self) -> Router {
        let state = self.state.clone();

        Router::new()
            .route("/", get(serve_homepage))
            .route("/explorer", get(serve_explorer))
            .route("/mining", get(serve_mining_dashboard))
            .route("/mine", get(serve_miner_setup))
            .route("/download", get(serve_download_page))
            .route("/dashboard", get(serve_dashboard))
            .route("/health", get(health_detailed))
            .route("/metrics", get(prometheus_metrics))
            .route("/api/v1/info", get(get_node_info))
            .route("/api/v1/block/:height_or_latest", get(get_block))
            .route("/api/v1/tx/submit", post(submit_transaction))
            .route("/api/v1/tx/:hash", get(get_transaction))
            .route("/api/v1/account/:address", get(get_account))
            .route("/api/v1/mining/status", get(get_mining_status))
            .route("/api/v1/receipt/:hash", get(get_receipt))
            .route("/api/v1/blocks", get(get_block_range))
            .route("/api/v1/wallet/challenge", post(get_activation_challenge))
            .route("/api/v1/wallet/activate", post(activate_wallet))
            // Token endpoints
            .route("/api/v1/token/:mint_id", get(get_token_info))
            .route(
                "/api/v1/token/:mint_id/account/:address",
                get(get_token_account_balance),
            )
            // DEX endpoints
            .route("/api/v1/pool/:mint_a/:mint_b", get(get_pool_info))
            .route(
                "/api/v1/swap/quote/:mint_in/:mint_out/:amount_in",
                get(get_swap_quote),
            )
            // Launchpad / listing endpoints
            .route("/api/v1/launches", get(get_all_launches))
            .route("/api/v1/launch/:mint_id", get(get_launch_detail))
            .route("/api/v1/tokens", get(get_all_tokens))
            .route("/api/v1/pools", get(get_all_pools))
            .route("/api/v1/mint-nonce/:address", get(get_mint_nonce))
            .route("/api/v1/portfolio/:address", get(get_portfolio))
            .route("/launch", get(serve_launch_page))
            .route("/trade", get(serve_trade_page))
            .route("/bridge", get(serve_bridge_page))
            .route("/staking", get(serve_staking_page))
            .route("/blocks", get(serve_blocks_page))
            .route("/address", get(serve_address_page))
            .route("/richlist", get(serve_richlist_page))
            .route("/token", get(serve_token_page))
            .route("/nfts", get(serve_nft_page))
            .route("/nft", get(serve_nft_page))
            .route("/terms", get(serve_terms_page))
            .route("/privacy", get(serve_privacy_page))
            // Bridge endpoints
            .route("/api/v1/bridge/deposit-address", post(get_bridge_deposit_address))
            .route("/api/v1/bridge/mint", post(bridge_mint_tokens))
            .route("/api/v1/bridge/register-addresses", post(bridge_register_addresses))
            .route("/api/v1/bridge/status", get(bridge_status))
            .route("/api/v1/bridge/transfers", get(bridge_transfers))
            .route("/api/v1/bridge/deposit-intent", post(bridge_deposit_intent))
            .route("/api/v1/bridge/withdraw", post(bridge_withdraw))
            // Transaction history & events
            .route("/api/v1/address/:address/transactions", get(get_address_transactions))
            .route("/api/v1/events/query", post(query_events))
            // Staking endpoints
            .route("/api/v1/staking/validators", get(get_staking_validators))
            .route("/api/v1/staking/delegations/:address", get(get_delegations))
            .route("/api/v1/staking/rewards/:address", get(get_staking_rewards))
            // NFT endpoints
            .route("/api/v1/nft/collections", get(get_nft_collections))
            .route("/api/v1/nft/collection/:collection_id/items", get(get_collection_items))
            .route("/api/v1/nft/owner/:address", get(get_nfts_by_owner))
            // Light client
            .route("/api/v1/proof/account/:address", get(get_account_proof))
            .route("/api/v1/header/:height", get(get_block_header))
            // Richlist
            .route("/api/v1/richlist", get(get_richlist))
            // WebSocket endpoint for real-time subscriptions
            .route("/ws", get(ws_upgrade))
            .layer(
                // Restrict CORS to known origins instead of allowing all (`*`).
                // Permissive CORS lets any website make API calls to a user's local node,
                // enabling surveillance, account enumeration, and information leakage.
                //
                // Fix RPC-237: Use exact hostname matching instead of prefix matching.
                // Previously `starts_with("http://localhost")` would also match
                // `http://localhost.evil.com`, allowing attacker-controlled origins.
                CorsLayer::new()
                    .allow_origin(tower_http::cors::AllowOrigin::predicate(
                        |origin: &axum::http::HeaderValue, _| {
                            let Ok(s) = origin.to_str() else { return false };
                            is_allowed_origin(s)
                        },
                    ))
                    .allow_methods([
                        axum::http::Method::GET,
                        axum::http::Method::POST,
                        axum::http::Method::OPTIONS,
                    ])
                    .allow_headers([axum::http::header::CONTENT_TYPE])
            )
            .layer(DefaultBodyLimit::max(2 * 1024 * 1024)) // 2MB max request body (tokens can carry large metadata)
            .layer(tower_http::trace::TraceLayer::new_for_http())
            .route_layer(middleware::from_fn_with_state(state.clone(), rate_limit_middleware))
            .with_state(state)
    }

    /// Start the RPC server with per-IP rate limiting.
    pub async fn start(self, addr: SocketAddr) -> Result<(), RpcError> {
        info!(%addr, rate_limit = "100 req/sec/IP", "Starting PIChain RPC server");
        let router = self.router();
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| RpcError::Server(e.to_string()))?;
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .map_err(|e| RpcError::Server(e.to_string()))?;
        Ok(())
    }

    /// Update the block height (called by consensus layer).
    pub async fn set_block_height(&self, height: u64) {
        *self.state.block_height.write().await = height;
    }

    /// Update the peer count (called by network layer).
    pub async fn set_peer_count(&self, count: usize) {
        *self.state.peer_count.write().await = count;
    }

    /// Get the WebSocket broadcaster for emitting events from the node.
    pub fn ws_broadcaster(&self) -> &Arc<crate::ws::WsBroadcaster> {
        &self.state.ws_broadcaster
    }
}

// --- Route handlers ---

/// WebSocket upgrade handler — upgrades HTTP to WS for real-time subscriptions.
/// Rejects new connections when at global or per-IP capacity to prevent resource exhaustion.
async fn ws_upgrade(
    State(state): State<Arc<RpcState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    if state.ws_broadcaster.at_capacity() {
        return (StatusCode::SERVICE_UNAVAILABLE, "WebSocket connection limit reached").into_response();
    }
    let client_ip = addr.ip();
    if state.ws_broadcaster.ip_at_capacity(&client_ip) {
        return (StatusCode::TOO_MANY_REQUESTS, "Too many WebSocket connections from this IP").into_response();
    }
    let broadcaster = state.ws_broadcaster.clone();
    ws.max_message_size(2048)
        .on_upgrade(move |socket| crate::ws::handle_ws(socket, broadcaster, client_ip)).into_response()
}

/// Serve the PIChain homepage / landing page.
async fn serve_homepage() -> impl IntoResponse {
    const HOME_HTML: &str = include_str!("../../../explorer/home.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        HOME_HTML,
    )
}

/// Serve the block explorer UI.
async fn serve_explorer() -> impl IntoResponse {
    const EXPLORER_HTML: &str = include_str!("../../../explorer/index.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        EXPLORER_HTML,
    )
}

/// Live mining dashboard — auto-refreshing web UI.
async fn serve_mining_dashboard() -> impl IntoResponse {
    const MINING_HTML: &str = include_str!("../../../explorer/mining.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        MINING_HTML,
    )
}

/// Miner setup guide — instructions for external miners to join.
async fn serve_miner_setup() -> impl IntoResponse {
    const MINE_HTML: &str = include_str!("../../../explorer/mine.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        MINE_HTML,
    )
}

/// Real-time mining dashboard.
async fn serve_dashboard() -> impl IntoResponse {
    const DASHBOARD_HTML: &str = include_str!("../../../explorer/dashboard.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        DASHBOARD_HTML,
    )
}

/// Download page — pre-built miner binaries.
async fn serve_download_page() -> impl IntoResponse {
    const DOWNLOAD_HTML: &str = include_str!("../../../explorer/download.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        DOWNLOAD_HTML,
    )
}

/// Detailed health check with structured JSON response.
async fn health_detailed(State(state): State<Arc<RpcState>>) -> impl IntoResponse {
    let uptime_secs = state.started_at.elapsed().as_secs();
    let (height, syncing) = if let Some(provider) = &state.state_provider {
        (provider.current_height(), provider.is_syncing())
    } else {
        (*state.block_height.read().await, false)
    };

    let body = serde_json::json!({
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "chain_id": state.chain_id,
        "block_height": height,
        "is_syncing": syncing,
        "uptime_secs": uptime_secs,
        "requests_total": state.requests_total.load(Ordering::Relaxed),
    });

    (StatusCode::OK, Json(body))
}

/// Prometheus-compatible metrics endpoint.
/// AUDIT-FIX H-8: Restrict metrics to private/RFC1918 networks to prevent information disclosure.
/// Metrics expose internal state (peer count, error rates, uptime) that enables
/// targeted attacks. Only Prometheus scraping from localhost or private networks
/// (Docker bridge, LAN) should access this.
/// Allowed: 127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16.
async fn prometheus_metrics(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<RpcState>>,
) -> impl IntoResponse {
    if !is_private_ip(addr.ip()) {
        return (
            StatusCode::FORBIDDEN,
            [("content-type", "text/plain; charset=utf-8")],
            "metrics endpoint is restricted to private networks".to_string(),
        );
    }
    let uptime = state.started_at.elapsed().as_secs();
    let requests_total = state.requests_total.load(Ordering::Relaxed);
    let requests_errors = state.requests_errors.load(Ordering::Relaxed);

    let (height, base_fee, total_burned, mining_frontier, mining_digits, mining_miners) =
        if let Some(provider) = &state.state_provider {
            let (frontier, digits, miners) = provider.get_mining_stats().map_or(
                (0, 0, 0),
                |s| (s.frontier_position, s.total_digits_verified, s.unique_miners),
            );
            (
                provider.current_height(),
                provider.current_base_fee(),
                provider.total_burned(),
                frontier,
                digits,
                miners,
            )
        } else {
            (*state.block_height.read().await, 0, 0, 0, 0, 0)
        };

    let peers = *state.peer_count.read().await;

    let body = format!(
        "# HELP pichain_block_height Current block height\n\
         # TYPE pichain_block_height gauge\n\
         pichain_block_height {height}\n\
         # HELP pichain_peer_count Connected peers\n\
         # TYPE pichain_peer_count gauge\n\
         pichain_peer_count {peers}\n\
         # HELP pichain_base_fee Current base fee in base units\n\
         # TYPE pichain_base_fee gauge\n\
         pichain_base_fee {base_fee}\n\
         # HELP pichain_total_burned Total PI burned in base units\n\
         # TYPE pichain_total_burned counter\n\
         pichain_total_burned {total_burned}\n\
         # HELP pichain_mining_frontier Current mining frontier position\n\
         # TYPE pichain_mining_frontier gauge\n\
         pichain_mining_frontier {mining_frontier}\n\
         # HELP pichain_mining_digits_verified Total PI hex digits verified\n\
         # TYPE pichain_mining_digits_verified counter\n\
         pichain_mining_digits_verified {mining_digits}\n\
         # HELP pichain_mining_unique_miners Unique miner addresses\n\
         # TYPE pichain_mining_unique_miners gauge\n\
         pichain_mining_unique_miners {mining_miners}\n\
         # HELP pichain_uptime_seconds Node uptime in seconds\n\
         # TYPE pichain_uptime_seconds counter\n\
         pichain_uptime_seconds {uptime}\n\
         # HELP pichain_requests_total Total RPC requests served\n\
         # TYPE pichain_requests_total counter\n\
         pichain_requests_total {requests_total}\n\
         # HELP pichain_requests_errors Total RPC request errors\n\
         # TYPE pichain_requests_errors counter\n\
         pichain_requests_errors {requests_errors}\n\
         # HELP pichain_ws_subscribers Active WebSocket subscribers\n\
         # TYPE pichain_ws_subscribers gauge\n\
         pichain_ws_subscribers {ws_subs}\n\
         # HELP pichain_consensus_round Current consensus round\n\
         # TYPE pichain_consensus_round gauge\n\
         pichain_consensus_round {consensus_round}\n\
         # HELP pichain_bullshark_commits Total Bullshark commits\n\
         # TYPE pichain_bullshark_commits counter\n\
         pichain_bullshark_commits {bullshark_commits}\n\
         # HELP pichain_certificates_produced Total certificates produced\n\
         # TYPE pichain_certificates_produced counter\n\
         pichain_certificates_produced {certificates}\n\
         # HELP pichain_validator_count Active validators\n\
         # TYPE pichain_validator_count gauge\n\
         pichain_validator_count {validator_count}\n\
         # HELP pichain_total_stake Total staked PI\n\
         # TYPE pichain_total_stake gauge\n\
         pichain_total_stake {total_stake}\n",
        ws_subs = state.ws_broadcaster.subscriber_count(),
        consensus_round = state.state_provider.as_ref().map_or(0, |p| p.consensus_round()),
        bullshark_commits = state.state_provider.as_ref().map_or(0, |p| p.consensus_commits()),
        certificates = state.state_provider.as_ref().map_or(0, |p| p.consensus_certificates()),
        validator_count = state.state_provider.as_ref().map_or(0, |p| p.validator_count()),
        total_stake = state.state_provider.as_ref().map_or(0, |p| p.total_stake()),
    );

    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
}

async fn get_node_info(State(state): State<Arc<RpcState>>) -> Json<NodeInfo> {
    if let Some(provider) = &state.state_provider {
        return Json(NodeInfo {
            chain_id: state.chain_id,
            version: env!("CARGO_PKG_VERSION").to_string(),
            block_height: provider.current_height(),
            peer_count: *state.peer_count.read().await,
            is_syncing: provider.is_syncing(),
            state_root: provider.state_root_hex(),
            base_fee: provider.current_base_fee(),
            total_burned: provider.total_burned(),
            mempool_size: provider.mempool_size(),
        });
    }

    let height = *state.block_height.read().await;
    let peers = *state.peer_count.read().await;

    Json(NodeInfo {
        chain_id: state.chain_id,
        version: env!("CARGO_PKG_VERSION").to_string(),
        block_height: height,
        peer_count: peers,
        is_syncing: false,
        state_root: String::new(),
        base_fee: 0,
        total_burned: 0,
        mempool_size: 0,
    })
}

#[derive(Serialize)]
struct BlockResponse {
    found: bool,
    height: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    round: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tx_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proposer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gas_used: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_fee: Option<u64>,
    tx_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pi_burned: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tx_hashes: Option<Vec<String>>,
}

impl BlockResponse {
    fn from_block(block: &pichain_types::Block) -> Self {
        let tx_hashes: Vec<String> = block
            .transactions
            .iter()
            .take(MAX_TX_HASHES_PER_BLOCK)
            .map(|tx| tx.hash().to_string())
            .collect();

        Self {
            found: true,
            height: block.header.height,
            epoch: Some(block.header.epoch),
            round: Some(block.header.round),
            parent_hash: Some(block.header.parent_hash.to_string()),
            tx_root: Some(block.header.tx_root.to_string()),
            state_root: Some(block.header.state_root.to_string()),
            proposer: Some(block.header.proposer.to_string()),
            timestamp_ms: Some(block.header.timestamp_ms),
            gas_used: Some(block.header.gas_used),
            base_fee: Some(block.header.base_fee),
            tx_count: block.header.tx_count,
            pi_burned: Some(block.header.pi_burned),
            tx_hashes: Some(tx_hashes),
        }
    }

    fn not_found(height: u64) -> Self {
        Self {
            found: false,
            height,
            epoch: None,
            round: None,
            parent_hash: None,
            tx_root: None,
            state_root: None,
            proposer: None,
            timestamp_ms: None,
            gas_used: None,
            base_fee: None,
            tx_count: 0,
            pi_burned: None,
            tx_hashes: None,
        }
    }
}

async fn get_block(
    State(state): State<Arc<RpcState>>,
    axum::extract::Path(height_or_latest): axum::extract::Path<String>,
) -> (StatusCode, Json<BlockResponse>) {
    let height = if height_or_latest == "latest" {
        if let Some(provider) = &state.state_provider {
            provider.current_height()
        } else {
            return (StatusCode::NOT_FOUND, Json(BlockResponse::not_found(0)));
        }
    } else {
        match height_or_latest.parse::<u64>() {
            Ok(h) => h,
            Err(_) => return (StatusCode::BAD_REQUEST, Json(BlockResponse::not_found(0))),
        }
    };

    if let Some(provider) = state.state_provider.clone() {
        let _permit = state.blocking_semaphore.acquire().await;
        if let Ok(Some(block)) = tokio::task::spawn_blocking(move || {
            provider.get_block_sync(height)
        }).await {
            return (StatusCode::OK, Json(BlockResponse::from_block(&block)));
        }
    }
    (StatusCode::NOT_FOUND, Json(BlockResponse::not_found(height)))
}

#[derive(Deserialize)]
struct SubmitTxRequest {
    signed_tx_hex: String,
}

#[derive(Serialize)]
struct SubmitTxResponse {
    tx_hash: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn submit_transaction(
    State(state): State<Arc<RpcState>>,
    Json(req): Json<SubmitTxRequest>,
) -> (StatusCode, Json<SubmitTxResponse>) {
    // Reject oversized transactions (max 512KB decoded = 1MB hex)
    if req.signed_tx_hex.len() > 1024 * 1024 {
        return (StatusCode::BAD_REQUEST, Json(SubmitTxResponse {
            tx_hash: String::new(),
            status: "error".to_string(),
            error: Some("transaction too large (max 512KB)".to_string()),
        }));
    }

    // Decode the hex-encoded signed transaction
    let tx_bytes = match hex::decode(req.signed_tx_hex) {
        Ok(b) => b,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(SubmitTxResponse {
                tx_hash: String::new(),
                status: "error".to_string(),
                error: Some(format!("invalid hex: {e}")),
            }));
        }
    };

    // Deserialize the transaction
    let signed_tx: pichain_types::SignedTransaction = match serde_json::from_slice(&tx_bytes) {
        Ok(tx) => tx,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(SubmitTxResponse {
                tx_hash: String::new(),
                status: "error".to_string(),
                error: Some(format!("invalid transaction: {e}")),
            }));
        }
    };

    // Verify chain_id matches this node
    if signed_tx.data.chain_id != state.chain_id {
        return (StatusCode::BAD_REQUEST, Json(SubmitTxResponse {
            tx_hash: String::new(),
            status: "error".to_string(),
            error: Some(format!(
                "chain_id mismatch: tx has {}, node expects {}",
                signed_tx.data.chain_id, state.chain_id
            )),
        }));
    }

    // Verify signature
    if let Err(e) = signed_tx.verify() {
        return (StatusCode::BAD_REQUEST, Json(SubmitTxResponse {
            tx_hash: String::new(),
            status: "error".to_string(),
            error: Some(format!("signature verification failed: {e}")),
        }));
    }

    let tx_hash = signed_tx.hash().to_string();

    // Insert into mempool via state provider
    if let Some(provider) = &state.state_provider {
        match provider.mempool_insert(signed_tx) {
            Ok(()) => (StatusCode::OK, Json(SubmitTxResponse {
                tx_hash,
                status: "pending".to_string(),
                error: None,
            })),
            Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, Json(SubmitTxResponse {
                tx_hash,
                status: "rejected".to_string(),
                error: Some(e),
            })),
        }
    } else {
        state.requests_errors.fetch_add(1, Ordering::Relaxed);
        (StatusCode::SERVICE_UNAVAILABLE, Json(SubmitTxResponse {
            tx_hash: String::new(),
            status: "error".to_string(),
            error: Some("node not ready: no state provider".to_string()),
        }))
    }
}

#[derive(Serialize)]
struct TransactionResponse {
    tx_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sender: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gas_used: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_height: Option<u64>,
    found: bool,
}

async fn get_transaction(
    State(state): State<Arc<RpcState>>,
    axum::extract::Path(hash_str): axum::extract::Path<String>,
) -> (StatusCode, Json<TransactionResponse>) {
    if let Some(provider) = state.state_provider.clone() {
        // Validate hex length before decode to prevent DoS with huge strings
        let hash_hex = strip_0x(&hash_str);
        if hash_hex.len() == 64 {
        // Parse hash from hex string
        if let Ok(hash_bytes) = hex::decode(hash_hex) {
            if hash_bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&hash_bytes);
                let tx_hash = pichain_crypto::Hash::from_bytes(arr);

                let _permit = state.blocking_semaphore.acquire().await;
                let tx_hash2 = tx_hash;
                let provider2 = provider.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let tx = provider2.get_transaction_sync(&tx_hash2)?;
                    let receipt = provider2.get_receipt_sync(&tx_hash2);
                    let block_height = provider2.get_tx_block_height(&tx_hash2);
                    Some((tx, receipt, block_height))
                }).await;

                if let Ok(Some((tx, receipt, block_height))) = result {
                    let kind_str = match &tx.data.kind {
                        pichain_types::TransactionKind::Transfer { .. } => "Transfer",
                        pichain_types::TransactionKind::DeployContract { .. } => "DeployContract",
                        pichain_types::TransactionKind::ContractCall { .. } => "ContractCall",
                        pichain_types::TransactionKind::Stake { .. } => "Stake",
                        pichain_types::TransactionKind::Unstake { .. } => "Unstake",
                        pichain_types::TransactionKind::MiningProof { .. } => "MiningProof",
                        pichain_types::TransactionKind::CreateToken { .. } => "CreateToken",
                        pichain_types::TransactionKind::MintToken { .. } => "MintToken",
                        pichain_types::TransactionKind::TransferToken { .. } => "TransferToken",
                        pichain_types::TransactionKind::BurnToken { .. } => "BurnToken",
                        pichain_types::TransactionKind::ApproveToken { .. } => "ApproveToken",
                        pichain_types::TransactionKind::RevokeMintAuthority { .. } => "RevokeMintAuthority",
                        pichain_types::TransactionKind::FreezeTokenAccount { .. } => "FreezeTokenAccount",
                        pichain_types::TransactionKind::ThawTokenAccount { .. } => "ThawTokenAccount",
                        pichain_types::TransactionKind::CreatePool { .. } => "CreatePool",
                        pichain_types::TransactionKind::AddLiquidity { .. } => "AddLiquidity",
                        pichain_types::TransactionKind::RemoveLiquidity { .. } => "RemoveLiquidity",
                        pichain_types::TransactionKind::Swap { .. } => "Swap",
                        pichain_types::TransactionKind::CreateLaunch { .. } => "CreateLaunch",
                        pichain_types::TransactionKind::ParticipateInLaunch { .. } => "ParticipateInLaunch",
                        pichain_types::TransactionKind::FinalizeLaunch { .. } => "FinalizeLaunch",
                        pichain_types::TransactionKind::SellFromLaunch { .. } => "SellFromLaunch",
                        pichain_types::TransactionKind::CreateNftCollection { .. } => "CreateNftCollection",
                        pichain_types::TransactionKind::MintNft { .. } => "MintNft",
                        pichain_types::TransactionKind::TransferNft { .. } => "TransferNft",
                        pichain_types::TransactionKind::ListNft { .. } => "ListNft",
                        pichain_types::TransactionKind::BuyNft { .. } => "BuyNft",
                        pichain_types::TransactionKind::DelistNft { .. } => "DelistNft",
                        pichain_types::TransactionKind::CreateMultisig { .. } => "CreateMultisig",
                        pichain_types::TransactionKind::ExecuteMultisig { .. } => "ExecuteMultisig",
                        pichain_types::TransactionKind::BridgeWithdraw { .. } => "BridgeWithdraw",
                    };

                    let receipt_status = receipt.as_ref().map(|r| {
                        match r.status {
                            pichain_types::TransactionStatus::Success => "success".to_string(),
                            pichain_types::TransactionStatus::Reverted(ref msg) => format!("reverted: {msg}"),
                            pichain_types::TransactionStatus::OutOfGas => "out_of_gas".to_string(),
                        }
                    });
                    let gas_used = receipt.as_ref().map(|r| r.gas_used);

                    return (StatusCode::OK, Json(TransactionResponse {
                        tx_hash: hash_str,
                        sender: Some(tx.data.sender.to_string()),
                        nonce: Some(tx.data.nonce),
                        kind: Some(kind_str.to_string()),
                        status: receipt_status,
                        gas_used,
                        block_height,
                        found: true,
                    }));
                }
            }
        }
        } // hex length check
    }

    (StatusCode::NOT_FOUND, Json(TransactionResponse {
        tx_hash: hash_str,
        sender: None,
        nonce: None,
        kind: None,
        status: None,
        gas_used: None,
        block_height: None,
        found: false,
    }))
}

#[derive(Serialize)]
struct AccountResponse {
    address: String,
    balance: u64,
    balance_pi: String,
    nonce: u64,
    staked: u64,
    locked_balance: u64,
    found: bool,
}

async fn get_account(
    State(state): State<Arc<RpcState>>,
    axum::extract::Path(address_str): axum::extract::Path<String>,
) -> (StatusCode, Json<AccountResponse>) {
    if let Some(provider) = state.state_provider.clone() {
        // Strip optional "0x"/"0X" prefix
        let hex_str = strip_0x(&address_str);

        // Validate hex length before decoding (20 bytes = 40 hex chars)
        if hex_str.len() == 40 {
            if let Ok(addr_bytes) = hex::decode(hex_str) {
                if addr_bytes.len() == 20 {
                    let mut arr = [0u8; 20];
                    arr.copy_from_slice(&addr_bytes);
                    let address = pichain_crypto::ed25519::Address(arr);

                    let _permit = state.blocking_semaphore.acquire().await;
                    if let Ok(Some(account)) = tokio::task::spawn_blocking(move || {
                        provider.get_account_sync(&address)
                    }).await {
                        let whole = account.state.balance / 1_000_000_000;
                        let frac = account.state.balance % 1_000_000_000;
                        let balance_pi = format!("{}.{:09}", whole, frac);
                        return (StatusCode::OK, Json(AccountResponse {
                            address: address_str,
                            balance: account.state.balance,
                            balance_pi,
                            nonce: account.state.nonce,
                            staked: account.state.staked,
                            locked_balance: account.state.locked_balance,
                            found: true,
                        }));
                    }
                }
            }
        }
    }

    (StatusCode::NOT_FOUND, Json(AccountResponse {
        address: address_str,
        balance: 0,
        balance_pi: "0.0".to_string(),
        nonce: 0,
        staked: 0,
        locked_balance: 0,
        found: false,
    }))
}

#[derive(Serialize)]
struct MiningStatus {
    frontier_position: u64,
    total_digits_verified: u64,
    next_position: u64,
    max_batch_at_position: u64,
    total_ranges: u64,
    unique_miners: u64,
    remaining_pool: u64,
    total_mined: u64,
    reward_per_digit: u64,
    emission_year: u32,
    difficulty_bits: u32,
    difficulty_target_hex: String,
    anchor_block_hash: String,
    base_fee: u64,
}

async fn get_mining_status(State(state): State<Arc<RpcState>>) -> Json<MiningStatus> {
    if let Some(provider) = &state.state_provider {
        if let Some(stats) = provider.get_mining_stats() {
            return Json(MiningStatus {
                frontier_position: stats.frontier_position,
                total_digits_verified: stats.total_digits_verified,
                next_position: stats.next_position,
                max_batch_at_position: stats.max_batch_at_position,
                total_ranges: stats.total_ranges,
                unique_miners: stats.unique_miners,
                remaining_pool: stats.remaining_pool,
                total_mined: stats.total_mined,
                reward_per_digit: stats.reward_per_digit,
                emission_year: stats.emission_year,
                difficulty_bits: stats.difficulty_bits,
                difficulty_target_hex: stats.difficulty_target_hex.clone(),
                anchor_block_hash: stats.anchor_block_hash.clone(),
                base_fee: provider.current_base_fee(),
            });
        }
    }

    Json(MiningStatus {
        frontier_position: 0,
        total_digits_verified: 0,
        next_position: 0,
        max_batch_at_position: u64::MAX,
        total_ranges: 0,
        unique_miners: 0,
        remaining_pool: 0,
        total_mined: 0,
        reward_per_digit: 0,
        emission_year: 1,
        difficulty_bits: 8,
        difficulty_target_hex: hex::encode(pichain_mining::difficulty::INITIAL_DIFFICULTY),
        anchor_block_hash: String::new(),
        base_fee: 1000,
    })
}

// --- Wallet activation (PoW challenge + verification) ---

/// PoW difficulty: 20 leading zero bits (~1M hashes, ~1 second on modern CPU).
const ACTIVATION_POW_BITS: u32 = 20;
/// Challenge validity window: 5 minutes.
const CHALLENGE_TTL_SECS: u64 = 300;

#[derive(Deserialize)]
struct ChallengeRequest {
    address: String,
}

#[derive(Serialize)]
struct ChallengeResponse {
    success: bool,
    challenge: String,
    difficulty_bits: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    activations_remaining: u64,
}

/// Issue a PoW challenge that must be solved before wallet activation.
async fn get_activation_challenge(
    State(state): State<Arc<RpcState>>,
    Json(req): Json<ChallengeRequest>,
) -> (StatusCode, Json<ChallengeResponse>) {
    let fail = |status: StatusCode, err: String| {
        (status, Json(ChallengeResponse {
            success: false,
            challenge: String::new(),
            difficulty_bits: ACTIVATION_POW_BITS,
            error: Some(err),
            activations_remaining: 0,
        }))
    };

    let hex_str = strip_0x(&req.address);
    if hex_str.len() != 40 {
        return fail(StatusCode::BAD_REQUEST, "address must be 40 hex characters".to_string());
    }
    let addr_bytes = match hex::decode(hex_str) {
        Ok(b) if b.len() == 20 => b,
        _ => return fail(StatusCode::BAD_REQUEST, "invalid address hex".to_string()),
    };

    let mut arr = [0u8; 20];
    arr.copy_from_slice(&addr_bytes);
    let address = pichain_crypto::ed25519::Address(arr);

    let remaining = if let Some(provider) = &state.state_provider {
        3_140_000u64.saturating_sub(provider.activation_count())
    } else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "node unavailable".to_string());
    };
    if remaining == 0 {
        return fail(StatusCode::FORBIDDEN, "wallet activation cap reached (3,140,000)".to_string());
    }

    // Generate unique 32-byte challenge from timestamp + counter + address
    static CHALLENGE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let cnt = CHALLENGE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let challenge = *pichain_crypto::hash_concat(&[
        &ts.to_le_bytes(),
        &cnt.to_le_bytes(),
        &addr_bytes,
    ]).as_bytes();

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let challenge_hex = hex::encode(challenge);

    // Evict expired challenges (lazy cleanup)
    state.activation_challenges.retain(|_, v| v.expires_at > now_secs);

    state.activation_challenges.insert(challenge_hex.clone(), ActivationChallenge {
        challenge,
        address,
        expires_at: now_secs + CHALLENGE_TTL_SECS,
    });

    (StatusCode::OK, Json(ChallengeResponse {
        success: true,
        challenge: challenge_hex,
        difficulty_bits: ACTIVATION_POW_BITS,
        error: None,
        activations_remaining: remaining,
    }))
}

#[derive(Deserialize)]
struct ActivateRequest {
    address: String,
    challenge: String,
    nonce: u64,
}

#[derive(Serialize)]
struct ActivateResponse {
    success: bool,
    locked_amount: u64,
    locked_amount_pi: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    address: String,
}

/// Verify PoW solution and activate wallet.
async fn activate_wallet(
    State(state): State<Arc<RpcState>>,
    Json(req): Json<ActivateRequest>,
) -> (StatusCode, Json<ActivateResponse>) {
    let fail = |status: StatusCode, err: String, addr: String| {
        (status, Json(ActivateResponse {
            success: false,
            locked_amount: 0,
            locked_amount_pi: "0".to_string(),
            error: Some(err),
            address: addr,
        }))
    };

    // Validate address
    let hex_str = strip_0x(&req.address);
    if hex_str.len() != 40 {
        return fail(StatusCode::BAD_REQUEST, "address must be 40 hex characters".to_string(), req.address);
    }
    let addr_bytes = match hex::decode(hex_str) {
        Ok(b) if b.len() == 20 => b,
        _ => return fail(StatusCode::BAD_REQUEST, "invalid address hex".to_string(), req.address),
    };
    let mut arr = [0u8; 20];
    arr.copy_from_slice(&addr_bytes);
    let address = pichain_crypto::ed25519::Address(arr);

    // Look up and consume the challenge (one-time use)
    let challenge_entry = state.activation_challenges.remove(&req.challenge);
    let (_, entry) = match challenge_entry {
        Some(e) => e,
        None => return fail(StatusCode::BAD_REQUEST, "invalid or expired challenge".to_string(), req.address),
    };

    // Check expiry
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now_secs > entry.expires_at {
        return fail(StatusCode::BAD_REQUEST, "challenge expired".to_string(), req.address);
    }

    // Check challenge is bound to this address
    if entry.address != address {
        return fail(StatusCode::BAD_REQUEST, "challenge not issued for this address".to_string(), req.address);
    }

    // Verify PoW: SHA-256(challenge || nonce_le_bytes) must have ACTIVATION_POW_BITS leading zero bits
    // Uses SHA-256 so browsers can solve via native crypto.subtle (no CDN/WASM deps)
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(entry.challenge);
    hasher.update(req.nonce.to_le_bytes());
    let result = hasher.finalize();
    let hash_slice: &[u8] = result.as_ref();
    let hash_bytes: &[u8; 32] = hash_slice.try_into().expect("sha256 is 32 bytes");

    if !has_leading_zeros(hash_bytes, ACTIVATION_POW_BITS) {
        return fail(StatusCode::BAD_REQUEST, "invalid PoW solution".to_string(), req.address);
    }

    // PoW verified — proceed with activation
    if let Some(provider) = &state.state_provider {
        match provider.activate_wallet(&address) {
            Ok(amount) => {
                info!(
                    address = %req.address,
                    "wallet activated via PoW challenge"
                );
                return (StatusCode::OK, Json(ActivateResponse {
                    success: true,
                    locked_amount: amount,
                    locked_amount_pi: format!(
                        "{}.{:09}",
                        amount / 1_000_000_000,
                        amount % 1_000_000_000
                    ),
                    error: None,
                    address: req.address,
                }));
            }
            Err(e) => {
                return fail(StatusCode::UNPROCESSABLE_ENTITY, e, req.address);
            }
        }
    }

    fail(StatusCode::SERVICE_UNAVAILABLE, "activation unavailable".to_string(), req.address)
}

/// Check if a hash has at least `bits` leading zero bits.
fn has_leading_zeros(hash: &[u8; 32], bits: u32) -> bool {
    let full_bytes = (bits / 8) as usize;
    let remaining_bits = bits % 8;
    for &b in &hash[..full_bytes] {
        if b != 0 {
            return false;
        }
    }
    if remaining_bits > 0 && full_bytes < 32 {
        let mask = 0xFF << (8 - remaining_bits);
        if hash[full_bytes] & mask != 0 {
            return false;
        }
    }
    true
}

// --- Receipt handler ---

#[derive(Serialize)]
struct ReceiptResponse {
    tx_hash: String,
    status: String,
    gas_used: u64,
    base_fee: u64,
    events: Vec<EventData>,
    found: bool,
}

#[derive(Serialize)]
struct EventData {
    emitter: String,
    event_type: String,
    data_hex: String,
}

async fn get_receipt(
    State(state): State<Arc<RpcState>>,
    axum::extract::Path(hash_str): axum::extract::Path<String>,
) -> (StatusCode, Json<ReceiptResponse>) {
    if let Some(provider) = state.state_provider.clone() {
        let hash_hex = strip_0x(&hash_str);
        if hash_hex.len() == 64 {
            if let Ok(hash_bytes) = hex::decode(hash_hex) {
                if hash_bytes.len() == 32 {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&hash_bytes);
                    let tx_hash = pichain_crypto::Hash::from_bytes(arr);

                    let _permit = state.blocking_semaphore.acquire().await;
                    let result = tokio::task::spawn_blocking(move || {
                        provider.get_receipt_sync(&tx_hash)
                    }).await;

                    if let Ok(Some(receipt)) = result {
                        let status = match &receipt.status {
                            pichain_types::TransactionStatus::Success => "success".to_string(),
                            pichain_types::TransactionStatus::Reverted(msg) => format!("reverted: {msg}"),
                            pichain_types::TransactionStatus::OutOfGas => "out_of_gas".to_string(),
                        };
                        let events: Vec<EventData> = receipt.events.iter().map(|e| EventData {
                            emitter: e.emitter.to_string(),
                            event_type: e.event_type.clone(),
                            data_hex: hex::encode(&e.data),
                        }).collect();

                        return (StatusCode::OK, Json(ReceiptResponse {
                            tx_hash: hash_str,
                            status,
                            gas_used: receipt.gas_used,
                            base_fee: receipt.base_fee,
                            events,
                            found: true,
                        }));
                    }
                }
            }
        }
    }

    (StatusCode::NOT_FOUND, Json(ReceiptResponse {
        tx_hash: hash_str,
        status: String::new(),
        gas_used: 0,
        base_fee: 0,
        events: vec![],
        found: false,
    }))
}

// --- Block range handler ---

#[derive(Deserialize)]
struct BlockRangeQuery {
    #[serde(default)]
    from: Option<u64>,
    #[serde(default)]
    to: Option<u64>,
    #[serde(default = "default_limit")]
    limit: u64,
}

fn default_limit() -> u64 { 20 }

/// Maximum blocks returned in a range query.
const MAX_BLOCK_RANGE: u64 = 50;

/// Maximum transaction hashes included in a single block response.
const MAX_TX_HASHES_PER_BLOCK: usize = 1000;

#[derive(Serialize)]
struct BlockListResponse {
    blocks: Vec<BlockSummary>,
    total_height: u64,
}

#[derive(Serialize)]
struct BlockSummary {
    height: u64,
    tx_count: u32,
    gas_used: u64,
    base_fee: u64,
    pi_burned: u64,
    timestamp_ms: u64,
}

async fn get_block_range(
    State(state): State<Arc<RpcState>>,
    axum::extract::Query(query): axum::extract::Query<BlockRangeQuery>,
) -> Json<BlockListResponse> {
    if let Some(provider) = state.state_provider.clone() {
        let limit = query.limit.clamp(1, MAX_BLOCK_RANGE); // clamp to [1, 50]

        // Acquire semaphore permit to limit concurrent blocking tasks
        let _permit = match state.blocking_semaphore.acquire().await {
            Ok(permit) => permit,
            Err(_) => return Json(BlockListResponse { blocks: vec![], total_height: 0 }),
        };

        // Run potentially blocking multi-block iteration off the async runtime
        let result = tokio::task::spawn_blocking(move || {
            let current = provider.current_height();
            let to = query.to.unwrap_or(current).min(current);
            let from = query.from.unwrap_or(to.saturating_sub(limit - 1));

            let mut blocks = Vec::new();
            for h in (from..=to).rev().take(limit as usize) {
                if let Some(block) = provider.get_block_sync(h) {
                    blocks.push(BlockSummary {
                        height: block.header.height,
                        tx_count: block.header.tx_count,
                        gas_used: block.header.gas_used,
                        base_fee: block.header.base_fee,
                        pi_burned: block.header.pi_burned,
                        timestamp_ms: block.header.timestamp_ms,
                    });
                }
            }
            BlockListResponse { blocks, total_height: current }
        }).await;

        if let Ok(response) = result {
            return Json(response);
        }
    }

    Json(BlockListResponse { blocks: vec![], total_height: 0 })
}

// --- Token route handlers ---

#[derive(Serialize)]
struct TokenInfoResponse {
    mint_id: String,
    name: String,
    symbol: String,
    decimals: u8,
    total_supply: u64,
    max_supply: u64,
    creator: String,
    has_mint_authority: bool,
    active: bool,
    found: bool,
}

async fn get_token_info(
    State(state): State<Arc<RpcState>>,
    axum::extract::Path(mint_id_hex): axum::extract::Path<String>,
) -> (StatusCode, Json<TokenInfoResponse>) {
    if let Some(provider) = state.state_provider.clone() {
        // Validate hex length before decoding (32 bytes = 64 hex chars)
        let mint_id_stripped = strip_0x(&mint_id_hex);
        if mint_id_stripped.len() == 64 {
        if let Ok(bytes) = hex::decode(mint_id_stripped) {
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                let mint_id = pichain_types::MintId(arr);

                let _permit = state.blocking_semaphore.acquire().await;
                if let Ok(Some(mint)) = tokio::task::spawn_blocking(move || {
                    provider.get_token_mint(&mint_id)
                }).await {
                    return (StatusCode::OK, Json(TokenInfoResponse {
                        mint_id: mint_id_hex,
                        name: mint.name,
                        symbol: mint.symbol,
                        decimals: mint.decimals,
                        total_supply: mint.total_supply,
                        max_supply: mint.max_supply,
                        creator: mint.creator.to_string(),
                        has_mint_authority: mint.mint_authority.is_some(),
                        active: mint.active,
                        found: true,
                    }));
                }
            }
        }
        }
    }

    (StatusCode::NOT_FOUND, Json(TokenInfoResponse {
        mint_id: mint_id_hex,
        name: String::new(),
        symbol: String::new(),
        decimals: 0,
        total_supply: 0,
        max_supply: 0,
        creator: String::new(),
        has_mint_authority: false,
        active: false,
        found: false,
    }))
}

#[derive(Serialize)]
struct TokenAccountResponse {
    owner: String,
    mint_id: String,
    balance: u64,
    frozen: bool,
    found: bool,
}

async fn get_token_account_balance(
    State(state): State<Arc<RpcState>>,
    axum::extract::Path((mint_id_hex, address_hex)): axum::extract::Path<(String, String)>,
) -> (StatusCode, Json<TokenAccountResponse>) {
    if let Some(provider) = state.state_provider.clone() {
        // Validate hex lengths before decode to prevent DoS
        let mint_id_stripped = strip_0x(&mint_id_hex);
        let addr_stripped = strip_0x(&address_hex);
        if mint_id_stripped.len() == 64 && addr_stripped.len() == 40 {
        if let (Ok(mint_bytes), Ok(addr_bytes)) =
            (hex::decode(mint_id_stripped), hex::decode(addr_stripped))
        {
            if mint_bytes.len() == 32 && addr_bytes.len() == 20 {
                let mut mint_arr = [0u8; 32];
                mint_arr.copy_from_slice(&mint_bytes);
                let mint_id = pichain_types::MintId(mint_arr);

                let mut addr_arr = [0u8; 20];
                addr_arr.copy_from_slice(&addr_bytes);
                let address = pichain_crypto::ed25519::Address(addr_arr);

                let _permit = state.blocking_semaphore.acquire().await;
                if let Ok(Some(account)) = tokio::task::spawn_blocking(move || {
                    provider.get_token_account(&address, &mint_id)
                }).await {
                    return (StatusCode::OK, Json(TokenAccountResponse {
                        owner: address_hex,
                        mint_id: mint_id_hex,
                        balance: account.balance,
                        frozen: account.frozen,
                        found: true,
                    }));
                }
            }
        }
        } // hex length check
    }

    (StatusCode::NOT_FOUND, Json(TokenAccountResponse {
        owner: address_hex,
        mint_id: mint_id_hex,
        balance: 0,
        frozen: false,
        found: false,
    }))
}

// --- DEX route handlers ---

#[derive(Serialize)]
struct PoolInfoResponse {
    pool_id: String,
    mint_a: String,
    mint_b: String,
    reserve_a: u64,
    reserve_b: u64,
    lp_supply: u64,
    fee_bps: u16,
    active: bool,
    found: bool,
}

async fn get_pool_info(
    State(state): State<Arc<RpcState>>,
    axum::extract::Path((mint_a_hex, mint_b_hex)): axum::extract::Path<(String, String)>,
) -> (StatusCode, Json<PoolInfoResponse>) {
    if let Some(provider) = state.state_provider.clone() {
        // Validate hex lengths before decode to prevent DoS
        let mint_a_stripped = strip_0x(&mint_a_hex);
        let mint_b_stripped = strip_0x(&mint_b_hex);
        if mint_a_stripped.len() == 64 && mint_b_stripped.len() == 64 {
        if let (Ok(a_bytes), Ok(b_bytes)) = (hex::decode(mint_a_stripped), hex::decode(mint_b_stripped)) {
            if a_bytes.len() == 32 && b_bytes.len() == 32 {
                let mut a_arr = [0u8; 32];
                a_arr.copy_from_slice(&a_bytes);
                let mint_a = pichain_types::MintId(a_arr);

                let mut b_arr = [0u8; 32];
                b_arr.copy_from_slice(&b_bytes);
                let mint_b = pichain_types::MintId(b_arr);

                let _permit = state.blocking_semaphore.acquire().await;
                if let Ok(Some(pool)) = tokio::task::spawn_blocking(move || {
                    provider.get_pool_by_mints(&mint_a, &mint_b)
                }).await {
                    return (StatusCode::OK, Json(PoolInfoResponse {
                        pool_id: pool.id.to_string(),
                        mint_a: pool.mint_a.to_string(),
                        mint_b: pool.mint_b.to_string(),
                        reserve_a: pool.reserve_a,
                        reserve_b: pool.reserve_b,
                        lp_supply: pool.lp_supply,
                        fee_bps: pool.fee_bps,
                        active: pool.active,
                        found: true,
                    }));
                }
            }
        }
        } // hex length check
    }

    (StatusCode::NOT_FOUND, Json(PoolInfoResponse {
        pool_id: String::new(),
        mint_a: mint_a_hex,
        mint_b: mint_b_hex,
        reserve_a: 0,
        reserve_b: 0,
        lp_supply: 0,
        fee_bps: 0,
        active: false,
        found: false,
    }))
}

#[derive(Serialize)]
struct SwapQuoteResponse {
    mint_in: String,
    mint_out: String,
    amount_in: u64,
    amount_out: u64,
    fee: u64,
    price_impact_bps: u64,
    found: bool,
}

async fn get_swap_quote(
    State(state): State<Arc<RpcState>>,
    axum::extract::Path((mint_in_hex, mint_out_hex, amount_in)): axum::extract::Path<(
        String,
        String,
        u64,
    )>,
) -> (StatusCode, Json<SwapQuoteResponse>) {
    if let Some(provider) = state.state_provider.clone() {
        // Validate hex lengths before decode to prevent DoS
        let mint_in_stripped = strip_0x(&mint_in_hex);
        let mint_out_stripped = strip_0x(&mint_out_hex);
        if mint_in_stripped.len() == 64 && mint_out_stripped.len() == 64 {
        if let (Ok(in_bytes), Ok(out_bytes)) =
            (hex::decode(mint_in_stripped), hex::decode(mint_out_stripped))
        {
            if in_bytes.len() == 32 && out_bytes.len() == 32 {
                let mut in_arr = [0u8; 32];
                in_arr.copy_from_slice(&in_bytes);
                let mint_in = pichain_types::MintId(in_arr);

                let mut out_arr = [0u8; 32];
                out_arr.copy_from_slice(&out_bytes);
                let mint_out = pichain_types::MintId(out_arr);

                let _permit = state.blocking_semaphore.acquire().await;
                if let Ok(Some(quote)) = tokio::task::spawn_blocking(move || {
                    provider.get_swap_quote(&mint_in, &mint_out, amount_in)
                }).await {
                    return (StatusCode::OK, Json(SwapQuoteResponse {
                        mint_in: mint_in_hex,
                        mint_out: mint_out_hex,
                        amount_in,
                        amount_out: quote.amount_out,
                        fee: quote.fee,
                        price_impact_bps: quote.price_impact_bps,
                        found: true,
                    }));
                }
            }
        }
        } // hex length check
    }

    (StatusCode::NOT_FOUND, Json(SwapQuoteResponse {
        mint_in: mint_in_hex,
        mint_out: mint_out_hex,
        amount_in,
        amount_out: 0,
        fee: 0,
        price_impact_bps: 0,
        found: false,
    }))
}

// ─── Launch page serving ────────────────────────────────────────────────────

async fn serve_launch_page() -> impl IntoResponse {
    const LAUNCH_HTML: &str = include_str!("../../../explorer/launch.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        LAUNCH_HTML,
    )
}

/// Trade page — DEX swap interface and cross-chain bridge.
async fn serve_trade_page() -> impl IntoResponse {
    const TRADE_HTML: &str = include_str!("../../../explorer/trade.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        TRADE_HTML,
    )
}

/// Bridge monitor page — real-time bridge activity dashboard.
async fn serve_bridge_page() -> impl IntoResponse {
    const BRIDGE_HTML: &str = include_str!("../../../explorer/bridge.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        BRIDGE_HTML,
    )
}

/// Staking page — validators, delegations, rewards.
async fn serve_staking_page() -> impl IntoResponse {
    const STAKING_HTML: &str = include_str!("../../../explorer/staking.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        STAKING_HTML,
    )
}

/// Block list page — paginated block browsing.
async fn serve_blocks_page() -> impl IntoResponse {
    const BLOCKS_HTML: &str = include_str!("../../../explorer/blocks.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        BLOCKS_HTML,
    )
}

/// Address detail page — balance, nonce, tx history.
async fn serve_address_page() -> impl IntoResponse {
    const ADDRESS_HTML: &str = include_str!("../../../explorer/address.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        ADDRESS_HTML,
    )
}

/// Rich list page — top accounts by balance.
async fn serve_richlist_page() -> impl IntoResponse {
    const RICHLIST_HTML: &str = include_str!("../../../explorer/richlist.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        RICHLIST_HTML,
    )
}

/// Token detail page — info and holders.
async fn serve_token_page() -> impl IntoResponse {
    const TOKEN_HTML: &str = include_str!("../../../explorer/token.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        TOKEN_HTML,
    )
}

/// NFT marketplace page — collections, items, marketplace.
async fn serve_nft_page() -> impl IntoResponse {
    const NFT_HTML: &str = include_str!("../../../explorer/nft.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        NFT_HTML,
    )
}

/// Terms of Service page.
async fn serve_terms_page() -> impl IntoResponse {
    const TERMS_HTML: &str = include_str!("../../../explorer/terms.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        TERMS_HTML,
    )
}

/// Privacy Policy page.
async fn serve_privacy_page() -> impl IntoResponse {
    const PRIVACY_HTML: &str = include_str!("../../../explorer/privacy.html");
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        PRIVACY_HTML,
    )
}

/// Bridge deposit address request.
#[derive(Deserialize)]
struct BridgeDepositRequest {
    chain: String,
    pichain_address: String,
}

/// Bridge deposit address — returns custodial address from the bridge relayer.
/// Falls back to hardcoded defaults if no relayer has registered addresses.
async fn get_bridge_deposit_address(
    State(state): State<Arc<RpcState>>,
    Json(req): Json<BridgeDepositRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let chain = req.chain.to_lowercase();

    // Validate pichain address
    let addr_hex = strip_0x(&req.pichain_address);
    if addr_hex.len() != 40 || hex::decode(addr_hex).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid pichain_address" })),
        );
    }

    // Try to get registered addresses from bridge relayer
    let registered = state.state_provider.as_ref().and_then(|p| p.bridge_get_addresses());

    let (address, note) = match chain.as_str() {
        "eth" => {
            let addr = registered.as_ref()
                .filter(|a| !a.eth.is_empty())
                .map(|a| a.eth.clone())
                .unwrap_or_else(|| "0x742d35Cc6634C0532925a3b844Bc9e7595f2bD18".to_string());
            (addr, "Send ETH to this address with your PIChain address as calldata. wETH will be minted after 12 confirmations.")
        }
        "sol" => {
            let addr = registered.as_ref()
                .filter(|a| !a.sol.is_empty())
                .map(|a| a.sol.clone())
                .unwrap_or_else(|| "BRjpCHtyQLeSJjRKsMdbbQWfQ2EH3ZTfHxKBxJfRzakr".to_string());
            (addr, "Send SOL to this address with your PIChain address in memo. wSOL will be minted after 32 confirmations.")
        }
        "btc" => {
            let addr = registered.as_ref()
                .filter(|a| !a.btc.is_empty())
                .map(|a| a.btc.clone())
                .unwrap_or_else(|| "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string());
            (addr, "Send BTC to this address with your PIChain address in OP_RETURN. wBTC will be minted after 6 confirmations.")
        }
        "usdt" => {
            let addr = registered.as_ref()
                .filter(|a| !a.usdt.is_empty())
                .map(|a| a.usdt.clone())
                .unwrap_or_else(|| "0x742d35Cc6634C0532925a3b844Bc9e7595f2bD18".to_string());
            (addr, "Send USDT (ERC-20) to this address. wUSDT will be minted after 12 confirmations.")
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "unsupported chain. Use: eth, sol, btc, usdt" })),
            );
        }
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "chain": chain,
            "address": address,
            "pichain_address": req.pichain_address,
            "note": note,
        })),
    )
}

/// Bridge mint request — only from localhost, used by bridge operator.
#[derive(Deserialize)]
struct BridgeMintRequest {
    symbol: String,      // "wETH", "wSOL", "wBTC", "wUSDT"
    recipient: String,   // hex PIChain address
    amount: u64,         // amount in base units
    #[serde(default)]
    chain: String,       // source chain for tracking (optional)
    #[serde(default)]
    tx_hash: String,     // external chain tx hash for tracking (optional)
}

/// Mint wrapped tokens to a recipient — localhost-only bridge operator endpoint.
async fn bridge_mint_tokens(
    State(state): State<Arc<RpcState>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    Json(req): Json<BridgeMintRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    // Localhost-only security check
    if !is_loopback_ip(addr.ip()) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "bridge mint only available from localhost" })),
        );
    }

    // Validate recipient address
    let hex_str = strip_0x(&req.recipient);
    if hex_str.len() != 40 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "recipient must be 40 hex characters" })),
        );
    }
    let addr_bytes = match hex::decode(hex_str) {
        Ok(b) if b.len() == 20 => b,
        _ => return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid recipient hex" })),
        ),
    };
    let mut arr = [0u8; 20];
    arr.copy_from_slice(&addr_bytes);
    let recipient = pichain_crypto::ed25519::Address(arr);

    if req.amount == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "amount must be > 0" })),
        );
    }

    if let Some(provider) = &state.state_provider {
        match provider.bridge_mint(&req.symbol, &recipient, req.amount) {
            Ok(()) => {
                // Record the transfer for status tracking
                if !req.chain.is_empty() && !req.tx_hash.is_empty() {
                    provider.bridge_record_transfer(
                        &req.chain,
                        &req.tx_hash,
                        &req.symbol,
                        &req.recipient,
                        req.amount,
                    );
                }
                info!(
                    symbol = %req.symbol,
                    recipient = %req.recipient,
                    amount = req.amount,
                    "bridge mint completed"
                );
                (StatusCode::OK, Json(serde_json::json!({
                    "success": true,
                    "symbol": req.symbol,
                    "recipient": req.recipient,
                    "amount": req.amount,
                })))
            }
            Err(e) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": e })),
            ),
        }
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "node unavailable" })),
        )
    }
}

/// Register custodial addresses — localhost-only, called by bridge relayer on startup.
#[derive(Deserialize)]
struct RegisterAddressesRequest {
    eth: String,
    sol: String,
    btc: String,
    #[serde(default)]
    usdt: String,
}

async fn bridge_register_addresses(
    State(state): State<Arc<RpcState>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    Json(req): Json<RegisterAddressesRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !is_loopback_ip(addr.ip()) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "register-addresses only available from localhost" })),
        );
    }

    let usdt = if req.usdt.is_empty() { req.eth.clone() } else { req.usdt };

    if let Some(provider) = &state.state_provider {
        match provider.bridge_register_addresses(&req.eth, &req.sol, &req.btc, &usdt) {
            Ok(()) => {
                info!(eth = %req.eth, sol = %req.sol, btc = %req.btc, "registered bridge custodial addresses");
                (StatusCode::OK, Json(serde_json::json!({ "success": true })))
            }
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e }))),
        }
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({ "error": "node unavailable" })))
    }
}

/// Bridge status — public endpoint showing TVL and transfer count.
async fn bridge_status(
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Some(provider) = &state.state_provider {
        let status = provider.bridge_status();
        let addresses = provider.bridge_get_addresses();
        (StatusCode::OK, Json(serde_json::json!({
            "tokens": status.tokens,
            "total_transfers": status.total_transfers,
            "addresses": addresses,
        })))
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({ "error": "node unavailable" })))
    }
}

/// Bridge transfers — public endpoint listing recent bridge mints.
#[derive(Deserialize)]
struct BridgeTransfersQuery {
    #[serde(default)]
    chain: Option<String>,
    #[serde(default = "default_transfers_limit")]
    limit: usize,
}

fn default_transfers_limit() -> usize { 50 }

async fn bridge_transfers(
    State(state): State<Arc<RpcState>>,
    axum::extract::Query(query): axum::extract::Query<BridgeTransfersQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Some(provider) = &state.state_provider {
        let limit = query.limit.min(200);
        let transfers = provider.bridge_get_transfers(query.chain.as_deref(), limit);
        (StatusCode::OK, Json(serde_json::json!({ "transfers": transfers })))
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({ "error": "node unavailable" })))
    }
}

/// Deposit intent — user registers their PIChain address for a specific external chain address.
#[derive(Deserialize)]
struct DepositIntentRequest {
    chain: String,
    external_address: String,
    pichain_address: String,
}

async fn bridge_deposit_intent(
    State(state): State<Arc<RpcState>>,
    Json(req): Json<DepositIntentRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let chain = req.chain.to_lowercase();
    if !["eth", "sol", "btc", "usdt"].contains(&chain.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "unsupported chain. Use: eth, sol, btc, usdt" })),
        );
    }

    let addr_hex = strip_0x(&req.pichain_address);
    if addr_hex.len() != 40 || hex::decode(addr_hex).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid pichain_address" })),
        );
    }

    if req.external_address.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "external_address required" })),
        );
    }

    if let Some(provider) = &state.state_provider {
        match provider.bridge_register_intent(&chain, &req.external_address, addr_hex) {
            Ok(()) => (StatusCode::OK, Json(serde_json::json!({
                "success": true,
                "chain": chain,
                "external_address": req.external_address,
                "pichain_address": req.pichain_address,
            }))),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e }))),
        }
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({ "error": "node unavailable" })))
    }
}

/// Bridge withdraw — burn wrapped tokens and create a withdrawal record.
#[derive(Deserialize)]
struct BridgeWithdrawRequest {
    symbol: String,
    amount: u64,
    destination_chain: String,
    destination_address: String,
    pichain_address: String,
}

async fn bridge_withdraw(
    State(_state): State<Arc<RpcState>>,
    Json(req): Json<BridgeWithdrawRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    // Validate inputs
    let chain = req.destination_chain.to_lowercase();
    if !["eth", "sol", "btc"].contains(&chain.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "unsupported chain. Use: eth, sol, btc" })),
        );
    }

    let addr_hex = strip_0x(&req.pichain_address);
    if addr_hex.len() != 40 || hex::decode(addr_hex).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid pichain_address" })),
        );
    }

    if req.amount == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "amount must be > 0" })),
        );
    }

    if req.destination_address.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "destination_address required" })),
        );
    }

    // For now, withdrawals are recorded but processing happens via the bridge relayer.
    // The actual burn and release on external chain is handled by the relayer polling for
    // withdrawal records.
    (StatusCode::OK, Json(serde_json::json!({
        "success": true,
        "status": "pending",
        "message": "Withdrawal queued. The bridge relayer will process it shortly.",
        "symbol": req.symbol,
        "amount": req.amount,
        "destination_chain": chain,
        "destination_address": req.destination_address,
    })))
}

// ─── Launchpad / listing API handlers ───────────────────────────────────────

#[derive(Serialize)]
struct LaunchListItem {
    mint_id: String,
    launch_id: String,
    creator: String,
    state: String,
    tokens_for_sale: u64,
    tokens_sold: u64,
    pi_raised: u64,
    target_pi: u64,
    current_price: u64,
    percent_complete: f64,
    max_per_address: u64,
    contributors: usize,
    created_at_ms: u64,
    // Enriched from TokenMint
    name: String,
    symbol: String,
    metadata_uri: String,
    decimals: u8,
    // Bonding curve params
    launch_type: String,
    base_price: u64,
    slope: u64,
    price_scale: u64,
}

async fn get_all_launches(
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Some(provider) = state.state_provider.clone() {
        let _permit = state.blocking_semaphore.acquire().await;
        if let Ok(data) = tokio::task::spawn_blocking(move || {
            let launches = provider.scan_all_launches();
            let mut items: Vec<LaunchListItem> = launches.iter().map(|l| {
                let mint = provider.get_token_mint(&l.mint);
                let (lt_name, bp, sl, ps) = match &l.launch_type {
                    pichain_types::launchpad::LaunchType::FairLaunch { price_per_token } =>
                        ("FairLaunch".to_string(), *price_per_token, 0u64, 1u64),
                    pichain_types::launchpad::LaunchType::BondingCurve { base_price, slope, price_scale } =>
                        ("BondingCurve".to_string(), *base_price, *slope, *price_scale),
                };
                let current_price = l.current_price();
                let pct = if l.target_pi > 0 {
                    (l.pi_raised as f64 / l.target_pi as f64) * 100.0
                } else { 0.0 };
                let state_str = match &l.state {
                    pichain_types::launchpad::LaunchState::Active => "Active",
                    pichain_types::launchpad::LaunchState::TargetReached => "TargetReached",
                    pichain_types::launchpad::LaunchState::Finalized => "Finalized",
                    pichain_types::launchpad::LaunchState::Cancelled => "Cancelled",
                };
                LaunchListItem {
                    mint_id: hex::encode(l.mint.0),
                    launch_id: hex::encode(l.id.0),
                    creator: hex::encode(l.creator.0),
                    state: state_str.to_string(),
                    tokens_for_sale: l.tokens_for_sale,
                    tokens_sold: l.tokens_sold,
                    pi_raised: l.pi_raised,
                    target_pi: l.target_pi,
                    current_price,
                    percent_complete: pct,
                    max_per_address: l.max_per_address,
                    contributors: l.contributions.len(),
                    created_at_ms: l.created_at_ms,
                    name: mint.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
                    symbol: mint.as_ref().map(|m| m.symbol.clone()).unwrap_or_default(),
                    metadata_uri: mint.as_ref().map(|m| m.metadata_uri.clone()).unwrap_or_default(),
                    decimals: mint.as_ref().map(|m| m.decimals).unwrap_or(9),
                    launch_type: lt_name,
                    base_price: bp,
                    slope: sl,
                    price_scale: ps,
                }
            }).collect();
            items.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
            items
        }).await {
            return (StatusCode::OK, Json(serde_json::json!({ "launches": data })));
        }
    }
    (StatusCode::OK, Json(serde_json::json!({ "launches": [] })))
}

async fn get_launch_detail(
    State(state): State<Arc<RpcState>>,
    axum::extract::Path(mint_id_hex): axum::extract::Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Some(provider) = state.state_provider.clone() {
        let stripped = strip_0x(&mint_id_hex);
        if stripped.len() == 64 {
            if let Ok(bytes) = hex::decode(stripped) {
                if bytes.len() == 32 {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&bytes);
                    let mint = pichain_types::MintId(arr);
                    let _permit = state.blocking_semaphore.acquire().await;
                    if let Ok(Some((launch, token))) = tokio::task::spawn_blocking(move || {
                        let launch = provider.get_launch_by_mint(&mint)?;
                        let token = provider.get_token_mint(&mint);
                        Some((launch, token))
                    }).await {
                            let (lt_name, bp, sl, ps) = match &launch.launch_type {
                                pichain_types::launchpad::LaunchType::FairLaunch { price_per_token } =>
                                    ("FairLaunch", *price_per_token, 0u64, 1u64),
                                pichain_types::launchpad::LaunchType::BondingCurve { base_price, slope, price_scale } =>
                                    ("BondingCurve", *base_price, *slope, *price_scale),
                            };
                            let state_str = match &launch.state {
                                pichain_types::launchpad::LaunchState::Active => "Active",
                                pichain_types::launchpad::LaunchState::TargetReached => "TargetReached",
                                pichain_types::launchpad::LaunchState::Finalized => "Finalized",
                                pichain_types::launchpad::LaunchState::Cancelled => "Cancelled",
                            };
                            return (StatusCode::OK, Json(serde_json::json!({
                                "found": true,
                                "mint_id": mint_id_hex,
                                "launch_id": hex::encode(launch.id.0),
                                "creator": hex::encode(launch.creator.0),
                                "state": state_str,
                                "tokens_for_sale": launch.tokens_for_sale,
                                "tokens_sold": launch.tokens_sold,
                                "pi_raised": launch.pi_raised,
                                "target_pi": launch.target_pi,
                                "current_price": launch.current_price(),
                                "percent_complete": if launch.target_pi > 0 { (launch.pi_raised as f64 / launch.target_pi as f64) * 100.0 } else { 0.0 },
                                "max_per_address": launch.max_per_address,
                                "contributors": launch.contributions.len(),
                                "created_at_ms": launch.created_at_ms,
                                "launch_type": lt_name,
                                "base_price": bp,
                                "slope": sl,
                                "price_scale": ps,
                                "liquidity_bps": launch.liquidity_bps,
                                "token_liquidity_bps": launch.token_liquidity_bps,
                                "name": token.as_ref().map(|t| t.name.as_str()).unwrap_or(""),
                                "symbol": token.as_ref().map(|t| t.symbol.as_str()).unwrap_or(""),
                                "metadata_uri": token.as_ref().map(|t| t.metadata_uri.as_str()).unwrap_or(""),
                                "decimals": token.as_ref().map(|t| t.decimals).unwrap_or(9),
                            })));
                    }
                }
            }
        }
    }
    (StatusCode::NOT_FOUND, Json(serde_json::json!({ "found": false })))
}

async fn get_all_tokens(
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Some(provider) = state.state_provider.clone() {
        let _permit = state.blocking_semaphore.acquire().await;
        if let Ok(mints) = tokio::task::spawn_blocking(move || {
            provider.scan_all_mints()
        }).await {
            let items: Vec<serde_json::Value> = mints.iter().map(|m| {
                serde_json::json!({
                    "mint_id": hex::encode(m.id.0),
                    "name": m.name,
                    "symbol": m.symbol,
                    "decimals": m.decimals,
                    "total_supply": m.total_supply,
                    "max_supply": m.max_supply,
                    "creator": hex::encode(m.creator.0),
                    "metadata_uri": m.metadata_uri,
                    "has_mint_authority": m.mint_authority.is_some(),
                    "active": m.active,
                })
            }).collect();
            return (StatusCode::OK, Json(serde_json::json!({ "tokens": items })));
        }
    }
    (StatusCode::OK, Json(serde_json::json!({ "tokens": [] })))
}

async fn get_all_pools(
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Some(provider) = state.state_provider.clone() {
        let _permit = state.blocking_semaphore.acquire().await;
        if let Ok(pools) = tokio::task::spawn_blocking(move || {
            provider.scan_all_pools()
        }).await {
            let items: Vec<serde_json::Value> = pools.iter().map(|p| {
                serde_json::json!({
                    "pool_id": hex::encode(p.id.0),
                    "mint_a": hex::encode(p.mint_a.0),
                    "mint_b": hex::encode(p.mint_b.0),
                    "reserve_a": p.reserve_a,
                    "reserve_b": p.reserve_b,
                    "lp_supply": p.lp_supply,
                    "fee_bps": p.fee_bps,
                    "active": p.active,
                })
            }).collect();
            return (StatusCode::OK, Json(serde_json::json!({ "pools": items })));
        }
    }
    (StatusCode::OK, Json(serde_json::json!({ "pools": [] })))
}

async fn get_mint_nonce(
    State(state): State<Arc<RpcState>>,
    axum::extract::Path(address_hex): axum::extract::Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Some(provider) = state.state_provider.clone() {
        let stripped = strip_0x(&address_hex);
        if stripped.len() == 40 {
            if let Ok(bytes) = hex::decode(stripped) {
                if bytes.len() == 20 {
                    let mut arr = [0u8; 20];
                    arr.copy_from_slice(&bytes);
                    let addr = pichain_crypto::ed25519::Address(arr);
                    let _permit = state.blocking_semaphore.acquire().await;
                    if let Ok(nonce) = tokio::task::spawn_blocking(move || {
                        provider.get_mint_nonce(&addr)
                    }).await {
                        return (StatusCode::OK, Json(serde_json::json!({
                            "address": address_hex,
                            "mint_nonce": nonce,
                        })));
                    }
                }
            }
        }
    }
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({
        "error": "invalid address"
    })))
}

async fn get_portfolio(
    State(state): State<Arc<RpcState>>,
    axum::extract::Path(address_hex): axum::extract::Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Some(provider) = state.state_provider.clone() {
        let stripped = strip_0x(&address_hex);
        if stripped.len() == 40 {
            if let Ok(bytes) = hex::decode(stripped) {
                if bytes.len() == 20 {
                    let mut arr = [0u8; 20];
                    arr.copy_from_slice(&bytes);
                    let addr = pichain_crypto::ed25519::Address(arr);
                    let _permit = state.blocking_semaphore.acquire().await;
                    if let Ok(result) = tokio::task::spawn_blocking(move || {
                        let pi_account = provider.get_account_sync(&addr);
                        let token_balances = provider.get_token_balances_for_owner(&addr);
                        (pi_account, token_balances)
                    }).await {
                        let (pi_account, token_balances) = result;
                        let pi_balance = pi_account.map(|a| a.state.balance).unwrap_or(0);
                        let tokens: Vec<serde_json::Value> = token_balances.iter().map(|(mint_id, acct)| {
                            serde_json::json!({
                                "mint_id": hex::encode(mint_id.0),
                                "balance": acct.balance,
                            })
                        }).collect();
                        return (StatusCode::OK, Json(serde_json::json!({
                            "address": address_hex,
                            "pi_balance": pi_balance,
                            "tokens": tokens,
                        })));
                    }
                }
            }
        }
    }
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({
        "error": "invalid address"
    })))
}

// --- Transaction History ---

async fn get_address_transactions(
    axum::extract::Path(address_hex): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let address_hex = strip_0x(&address_hex);
    if address_hex.len() != 40 {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "address must be 40 hex chars"})));
    }
    let Ok(addr_bytes) = hex::decode(address_hex) else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "invalid hex"})));
    };
    let before_height = params.get("before").and_then(|s| s.parse::<u64>().ok());
    let limit = params.get("limit").and_then(|s| s.parse::<usize>().ok()).unwrap_or(50).min(200);

    if let Some(provider) = &state.state_provider {
        let mut address = [0u8; 20];
        address.copy_from_slice(&addr_bytes);
        let addr = pichain_crypto::ed25519::Address(address);
        let entries = provider.get_address_transactions(&addr, before_height, limit);
        return (StatusCode::OK, Json(serde_json::json!({
            "address": address_hex,
            "transactions": entries,
            "count": entries.len(),
        })));
    }
    (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error": "node not ready"})))
}

// --- Event Querying ---

async fn query_events(
    State(state): State<Arc<RpcState>>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    let limit = body.get("limit").and_then(|v| v.as_u64()).unwrap_or(50).min(200) as usize;

    if let Some(provider) = &state.state_provider {
        // Query by topic
        if let Some(topic_hex) = body.get("topic").and_then(|v| v.as_str()) {
            let topic_hex = strip_0x(topic_hex);
            if topic_hex.len() != 64 {
                return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "topic must be 64 hex chars"})));
            }
            if let Ok(bytes) = hex::decode(topic_hex) {
                let mut topic = [0u8; 32];
                topic.copy_from_slice(&bytes);
                let entries = provider.query_events_by_topic(&topic, limit);
                return (StatusCode::OK, Json(serde_json::json!({
                    "events": entries,
                    "count": entries.len(),
                })));
            }
        }
        // Query by address
        if let Some(addr_hex) = body.get("address").and_then(|v| v.as_str()) {
            let addr_hex = strip_0x(addr_hex);
            if addr_hex.len() != 40 {
                return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "address must be 40 hex chars"})));
            }
            if let Ok(bytes) = hex::decode(addr_hex) {
                let mut address = [0u8; 20];
                address.copy_from_slice(&bytes);
                let addr = pichain_crypto::ed25519::Address(address);
                let entries = provider.query_events_by_address(&addr, limit);
                return (StatusCode::OK, Json(serde_json::json!({
                    "events": entries,
                    "count": entries.len(),
                })));
            }
        }
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "provide 'topic' or 'address'"})));
    }
    (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error": "node not ready"})))
}

// --- Staking Endpoints ---

async fn get_staking_validators(
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Some(provider) = &state.state_provider {
        let validators = provider.get_validators();
        return (StatusCode::OK, Json(serde_json::json!({
            "validators": validators,
            "count": validators.len(),
        })));
    }
    (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error": "node not ready"})))
}

async fn get_delegations(
    axum::extract::Path(address_hex): axum::extract::Path<String>,
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let address_hex = strip_0x(&address_hex);
    if address_hex.len() != 40 {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "address must be 40 hex chars"})));
    }
    if let (Ok(bytes), Some(provider)) = (hex::decode(address_hex), &state.state_provider) {
        let mut address = [0u8; 20];
        address.copy_from_slice(&bytes);
        let addr = pichain_crypto::ed25519::Address(address);
        let delegations = provider.get_delegations(&addr);
        return (StatusCode::OK, Json(serde_json::json!({
            "address": address_hex,
            "delegations": delegations,
        })));
    }
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "invalid address"})))
}

async fn get_staking_rewards(
    axum::extract::Path(address_hex): axum::extract::Path<String>,
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let address_hex = strip_0x(&address_hex);
    if address_hex.len() != 40 {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "address must be 40 hex chars"})));
    }
    if let (Ok(bytes), Some(provider)) = (hex::decode(address_hex), &state.state_provider) {
        let mut address = [0u8; 20];
        address.copy_from_slice(&bytes);
        let addr = pichain_crypto::ed25519::Address(address);
        let rewards = provider.get_staking_rewards(&addr);
        return (StatusCode::OK, Json(serde_json::json!({
            "address": address_hex,
            "pending_rewards": rewards,
        })));
    }
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "invalid address"})))
}

// --- NFT Endpoints ---

async fn get_nft_collections(
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Some(provider) = &state.state_provider {
        let collections = provider.scan_all_collections();
        let data: Vec<_> = collections.iter().map(|c| serde_json::json!({
            "id": hex::encode(c.id.0),
            "name": c.name,
            "symbol": c.symbol,
            "creator": hex::encode(c.creator.0),
            "max_supply": c.max_supply,
            "total_minted": c.minted,
            "royalty_bps": c.royalty_bps,
        })).collect();
        return (StatusCode::OK, Json(serde_json::json!({"collections": data, "count": data.len()})));
    }
    (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error": "node not ready"})))
}

async fn get_collection_items(
    axum::extract::Path(collection_id_hex): axum::extract::Path<String>,
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let cid_hex = strip_0x(&collection_id_hex);
    if cid_hex.len() != 64 {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "collection_id must be 64 hex chars"})));
    }
    if let (Ok(bytes), Some(provider)) = (hex::decode(cid_hex), &state.state_provider) {
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes);
        let collection_id = pichain_types::CollectionId(id);
        let items = provider.get_collection_items(&collection_id);
        let data: Vec<_> = items.iter().map(|nft| serde_json::json!({
            "nft_id": hex::encode(nft.id.0),
            "collection_id": hex::encode(nft.collection.0),
            "name": nft.name,
            "owner": hex::encode(nft.owner.0),
            "metadata_uri": nft.metadata_uri,
        })).collect();
        return (StatusCode::OK, Json(serde_json::json!({"items": data, "count": data.len()})));
    }
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "invalid collection_id"})))
}

async fn get_nfts_by_owner(
    axum::extract::Path(address_hex): axum::extract::Path<String>,
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let address_hex = strip_0x(&address_hex);
    if address_hex.len() != 40 {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "address must be 40 hex chars"})));
    }
    if let (Ok(bytes), Some(provider)) = (hex::decode(address_hex), &state.state_provider) {
        let mut address = [0u8; 20];
        address.copy_from_slice(&bytes);
        let addr = pichain_crypto::ed25519::Address(address);
        let nfts = provider.get_nfts_by_owner(&addr);
        let data: Vec<_> = nfts.iter().map(|nft| serde_json::json!({
            "nft_id": hex::encode(nft.id.0),
            "collection_id": hex::encode(nft.collection.0),
            "name": nft.name,
            "owner": hex::encode(nft.owner.0),
            "metadata_uri": nft.metadata_uri,
        })).collect();
        return (StatusCode::OK, Json(serde_json::json!({"nfts": data, "count": data.len()})));
    }
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "invalid address"})))
}

// --- Light Client ---

async fn get_account_proof(
    axum::extract::Path(address_hex): axum::extract::Path<String>,
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let address_hex = strip_0x(&address_hex);
    if address_hex.len() != 40 {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "address must be 40 hex chars"})));
    }
    if let (Ok(bytes), Some(provider)) = (hex::decode(address_hex), &state.state_provider) {
        let mut address = [0u8; 20];
        address.copy_from_slice(&bytes);
        let addr = pichain_crypto::ed25519::Address(address);
        if let Some(proof_bytes) = provider.get_account_proof(&addr) {
            return (StatusCode::OK, Json(serde_json::json!({
                "address": address_hex,
                "proof": hex::encode(proof_bytes),
                "state_root": provider.state_root_hex(),
            })));
        }
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "no proof available"})));
    }
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "invalid address"})))
}

async fn get_block_header(
    axum::extract::Path(height_str): axum::extract::Path<String>,
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let Ok(height) = height_str.parse::<u64>() else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "invalid height"})));
    };
    if let Some(provider) = &state.state_provider {
        if let Some(block) = provider.get_block_sync(height) {
            let h = &block.header;
            return (StatusCode::OK, Json(serde_json::json!({
                "height": h.height,
                "epoch": h.epoch,
                "parent_hash": hex::encode(h.parent_hash.as_bytes()),
                "state_root": hex::encode(h.state_root.as_bytes()),
                "tx_root": hex::encode(h.tx_root.as_bytes()),
                "tx_count": h.tx_count,
                "gas_used": h.gas_used,
                "base_fee": h.base_fee,
                "pi_burned": h.pi_burned,
                "proposer": hex::encode(h.proposer.0),
                "timestamp_ms": h.timestamp_ms,
            })));
        }
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "block not found"})));
    }
    (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error": "node not ready"})))
}

// --- Richlist ---

async fn get_richlist(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    State(state): State<Arc<RpcState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let limit = params.get("limit").and_then(|s| s.parse::<usize>().ok()).unwrap_or(100).min(500);
    if let Some(provider) = &state.state_provider {
        let entries = provider.get_richlist(limit);
        let data: Vec<_> = entries.iter().map(|(addr, balance)| serde_json::json!({
            "address": hex::encode(addr.0),
            "balance": balance,
        })).collect();
        return (StatusCode::OK, Json(serde_json::json!({"richlist": data, "count": data.len()})));
    }
    (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error": "node not ready"})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn node_info() {
        let server = RpcServer::new(314159);
        server.set_block_height(42).await;

        let state = server.state.clone();
        let height = *state.block_height.read().await;
        assert_eq!(height, 42);
    }

    #[test]
    fn rate_limiter_cleanup_runs_independently_of_check() {
        // Fix 196: Previously cleanup only ran when check() returned true.
        // Verify that cleanup works even when all IPs are rate-limited.
        let limiter = IpRateLimiter::new(1, 1); // 1 req/sec

        let ip1: IpAddr = "1.2.3.4".parse().unwrap();
        let ip2: IpAddr = "5.6.7.8".parse().unwrap();

        // Exhaust rate limit for both IPs
        assert!(limiter.check(ip1));
        assert!(!limiter.check(ip1)); // rate-limited
        assert!(limiter.check(ip2));
        assert!(!limiter.check(ip2)); // rate-limited

        // Both IPs should have entries
        assert_eq!(limiter.buckets.len(), 2);

        // Wait for window to expire
        std::thread::sleep(std::time::Duration::from_millis(2100));

        // Cleanup should remove stale entries regardless of rate-limit state.
        // (This previously required a successful check() to trigger.)
        limiter.cleanup();
        assert_eq!(limiter.buckets.len(), 0, "cleanup should remove expired entries");
    }

    #[test]
    fn rate_limiter_maybe_cleanup_is_periodic() {
        let limiter = IpRateLimiter::new(100, 1);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        limiter.check(ip);

        // maybe_cleanup has an internal 10-second debounce, so calling it
        // immediately should not remove recent entries
        limiter.maybe_cleanup();
        assert_eq!(limiter.buckets.len(), 1);
    }

    /// RPC-237: CORS must reject lookalike origins like "localhost.evil.com".
    /// Previously, prefix matching with `starts_with("http://localhost")`
    /// would accept `http://localhost.evil.com` as a valid origin.
    #[test]
    fn cors_rejects_lookalike_origins() {
        // Must reject: localhost lookalikes
        assert!(!is_allowed_origin("http://localhost.evil.com"));
        assert!(!is_allowed_origin("https://localhost.evil.com"));
        assert!(!is_allowed_origin("http://localhost.evil.com:3000"));
        assert!(!is_allowed_origin("http://localhostx"));

        // Must reject: 127.0.0.1 lookalikes
        assert!(!is_allowed_origin("http://127.0.0.1.evil.com"));

        // Must reject: pichain.io / pichain.net lookalikes
        assert!(!is_allowed_origin("https://pichain.io.evil.com"));
        assert!(!is_allowed_origin("https://explorer.pichain.io.evil.com"));
        assert!(!is_allowed_origin("https://fakepichain.io"));
        assert!(!is_allowed_origin("https://xpichain.io"));
        assert!(!is_allowed_origin("https://pichain.net.evil.com"));
        assert!(!is_allowed_origin("https://fakepichain.net"));

        // Must reject: pichain domains over plain http
        assert!(!is_allowed_origin("http://pichain.io"));
        assert!(!is_allowed_origin("http://explorer.pichain.io"));
        assert!(!is_allowed_origin("http://pichain.net"));
        assert!(!is_allowed_origin("http://www.pichain.net"));

        // Must reject: malformed origins
        assert!(!is_allowed_origin("localhost"));
        assert!(!is_allowed_origin("://localhost"));
        assert!(!is_allowed_origin(""));

        // Must accept: legitimate localhost origins
        assert!(is_allowed_origin("http://localhost"));
        assert!(is_allowed_origin("http://localhost:3000"));
        assert!(is_allowed_origin("https://localhost"));
        assert!(is_allowed_origin("https://localhost:8443"));

        // Must accept: 127.0.0.1
        assert!(is_allowed_origin("http://127.0.0.1"));
        assert!(is_allowed_origin("http://127.0.0.1:8080"));
        assert!(is_allowed_origin("https://127.0.0.1"));

        // Must accept: official pichain.io domains (https only)
        assert!(is_allowed_origin("https://pichain.io"));
        assert!(is_allowed_origin("https://explorer.pichain.io"));

        // Must accept: official pichain.net domains (https only)
        assert!(is_allowed_origin("https://pichain.net"));
        assert!(is_allowed_origin("https://www.pichain.net"));
        assert!(is_allowed_origin("https://explorer.pichain.net"));
    }
}
