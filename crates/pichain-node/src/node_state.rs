//! Shared node state — the central coordinator for all PIChain subsystems.
//!
//! `NodeState` holds Arc references to all components so they can be shared
//! across async tasks (block producer, RPC server, networking, etc).
//!
//! Storage and block hash use `parking_lot::RwLock` (non-poisoning) for
//! synchronous access from the RPC layer. Staking and mining use
//! `tokio::sync::RwLock` since they are only accessed from async block
//! processing.

use parking_lot::RwLock;
use pichain_consensus::StakingManager;
use pichain_crypto::keys::Address;
use pichain_crypto::Hash;
use pichain_execution::{ProducedBlock, TransactionExecutor, TransactionPool};
use pichain_rpc::{StateProvider, TxHistoryEntry};
use pichain_storage::StateStore;
use pichain_types::account::Account;
use pichain_types::genesis::GenesisConfig;
use pichain_types::{Block, PiAmount, SignedTransaction};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Shared node state accessible from all subsystems.
pub struct NodeState {
    /// Persistent storage (RocksDB + JMT). Uses std::sync::RwLock because
    /// RocksDB operations are fast and we need sync access from StateProvider.
    pub store: RwLock<StateStore>,
    /// Parallel transaction executor (Block-STM).
    pub executor: Arc<TransactionExecutor>,
    /// Transaction mempool.
    pub mempool: Arc<TransactionPool>,
    /// Staking and slashing manager.
    pub staking: tokio::sync::RwLock<StakingManager>,
    /// Chain ID.
    pub chain_id: u64,
    /// Current block height.
    block_height: AtomicU64,
    /// Last block hash.
    last_block_hash: RwLock<Hash>,
    /// Current base fee.
    base_fee: AtomicU64,
    /// Total PI burned.
    total_burned: AtomicU64,
    /// Total PI minted via mining rewards (inflationary).
    total_minted: AtomicU64,
    /// Timestamp of the last block (for monotonicity enforcement on restart).
    last_block_timestamp_ms: AtomicU64,
    /// Mutex to serialize peer block application. Without this, two sync blocks
    /// arriving concurrently could both see the same height and attempt to apply
    /// at the same slot, causing state corruption or duplicate block insertion.
    peer_block_mutex: tokio::sync::Mutex<()>,
    /// Bridge: registered custodial addresses (set by bridge relayer).
    bridge_addresses: RwLock<BridgeAddresses>,
    /// Bridge: recent transfer records (mint events).
    bridge_transfers: RwLock<Vec<BridgeTransferRecord>>,
    /// Bridge: deposit intents mapping "chain:external_addr" → pichain_addr.
    deposit_intents: RwLock<HashMap<String, String>>,
    /// Mempool WAL (write-ahead log) path for crash recovery.
    /// Pending transactions are written here so they survive node restarts.
    mempool_wal_path: Option<PathBuf>,
    /// PQ keypair for pool mining — signs mining proofs on behalf of browser miners.
    /// The reward goes to the specified miner address, not the node's address.
    pool_signer: RwLock<Option<pichain_crypto::PqKeypair>>,
}

/// Registered custodial addresses from the bridge relayer.
#[derive(Clone, Default)]
pub struct BridgeAddresses {
    pub eth: String,
    pub sol: String,
    pub btc: String,
    pub usdt: String,
}

/// Record of a bridge mint event.
#[derive(Clone)]
pub struct BridgeTransferRecord {
    pub chain: String,
    pub tx_hash: String,
    pub symbol: String,
    pub recipient: String,
    pub amount: u64,
    pub timestamp: i64,
}

impl NodeState {
    /// Create a new NodeState from components.
    pub fn new(
        store: StateStore,
        executor: Arc<TransactionExecutor>,
        mempool: Arc<TransactionPool>,
        chain_id: u64,
    ) -> Self {
        Self {
            store: RwLock::new(store),
            executor,
            mempool,
            staking: tokio::sync::RwLock::new(StakingManager::new()),
            chain_id,
            block_height: AtomicU64::new(0),
            last_block_hash: RwLock::new(Hash::ZERO),
            base_fee: AtomicU64::new(1_000),
            total_burned: AtomicU64::new(0),
            total_minted: AtomicU64::new(0),
            last_block_timestamp_ms: AtomicU64::new(0),
            peer_block_mutex: tokio::sync::Mutex::new(()),
            bridge_addresses: RwLock::new(BridgeAddresses::default()),
            bridge_transfers: RwLock::new(Vec::new()),
            deposit_intents: RwLock::new(HashMap::new()),
            mempool_wal_path: None,
            pool_signer: RwLock::new(None),
        }
    }

    /// Set the PQ keypair used for pool mining (browser miners).
    pub fn set_pool_signer(&self, pq_keypair: pichain_crypto::PqKeypair) {
        *self.pool_signer.write() = Some(pq_keypair);
    }

    /// Set the mempool WAL path and replay any pending transactions from a previous run.
    pub fn enable_mempool_wal(&mut self, data_dir: &str) {
        let path = PathBuf::from(data_dir).join("mempool-wal.ndjson");
        // Replay pending transactions from previous run
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(contents) => {
                    let mut replayed = 0u32;
                    let mut failed = 0u32;
                    for line in contents.lines() {
                        if line.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<SignedTransaction>(line) {
                            Ok(tx) => {
                                let sender = tx.data.sender;
                                // Load account state for nonce validation
                                if let Ok(Some(account)) = self.store.read().get_account(&sender) {
                                    if self.executor.get_account(&sender).is_none() {
                                        self.executor.set_account(sender, account.state.clone());
                                    }
                                    self.mempool.set_sender_nonce(sender, account.state.nonce);
                                    // Skip txs with nonces already confirmed on-chain
                                    if tx.data.nonce < account.state.nonce {
                                        continue;
                                    }
                                }
                                match self.mempool.insert(tx) {
                                    Ok(_) => replayed += 1,
                                    Err(_) => failed += 1,
                                }
                            }
                            Err(_) => failed += 1,
                        }
                    }
                    if replayed > 0 || failed > 0 {
                        info!(
                            replayed,
                            failed, "mempool WAL: replayed pending transactions"
                        );
                    }
                }
                Err(e) => warn!(error = %e, "failed to read mempool WAL"),
            }
            // Clear the WAL after replay (will be rebuilt from current mempool)
            let _ = std::fs::write(&path, "");
        }
        self.mempool_wal_path = Some(path);
    }

    /// Append a transaction to the mempool WAL.
    fn wal_append(&self, tx: &SignedTransaction) {
        if let Some(path) = &self.mempool_wal_path {
            if let Ok(json) = serde_json::to_string(tx) {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let _ = writeln!(f, "{}", json);
                }
            }
        }
    }

    /// Rewrite the WAL with only the currently pending mempool transactions.
    /// Called after each block is persisted to remove confirmed txs.
    pub fn wal_compact(&self) {
        if let Some(path) = &self.mempool_wal_path {
            let pending = self.mempool.get_ready_transactions(100_000);
            let mut contents = String::new();
            for tx in &pending {
                if let Ok(json) = serde_json::to_string(tx) {
                    contents.push_str(&json);
                    contents.push('\n');
                }
            }
            if let Err(e) = std::fs::write(path, &contents) {
                warn!(error = %e, "failed to compact mempool WAL");
            }
        }
    }

    /// Initialize from genesis — load allocations into executor cache and storage.
    pub fn apply_genesis(&self, genesis: &GenesisConfig) -> anyhow::Result<()> {
        let mut store = self.store.write();

        // Check if genesis has already been applied (accounts exist and block 0 exists)
        if store.get_block(0)?.is_some() && store.latest_height()? == 0 {
            // Genesis block exists but height is 0 — reload allocations into executor cache
            info!("Genesis block exists, reloading allocations into executor cache");
            for alloc in &genesis.allocations {
                if alloc.virtual_pool {
                    continue;
                } // skip virtual pools
                if let Some(account) = store.get_account(&alloc.address)? {
                    self.executor.set_account(alloc.address, account.state);
                } else {
                    // Account not in storage yet — apply it
                    let account = Account::with_balance(alloc.address, alloc.amount);
                    self.executor
                        .set_account(alloc.address, account.state.clone());
                    store.put_account(&account)?;
                }
            }
            return Ok(());
        }

        if store.latest_height()? > 0 {
            info!("Chain has advanced past genesis, skipping genesis application");
            // Still load genesis allocation accounts into executor cache so the
            // faucet, community pool, etc. are available for block execution.
            for alloc in &genesis.allocations {
                if alloc.virtual_pool {
                    continue;
                } // skip virtual pools
                if let Some(account) = store.get_account(&alloc.address)? {
                    self.executor.set_account(alloc.address, account.state);
                }
            }
            return Ok(());
        }

        info!(
            allocations = genesis.allocations.len(),
            validators = genesis.validators.len(),
            "applying genesis configuration"
        );

        // Load each allocation into both the executor cache and persistent storage.
        // AUDIT-FIX H-9: Skip virtual_pool allocations — the mining pool is tracked
        // in RewardCalculator and minted over time. Creating a real balance would
        // double-count the supply (real balance + minted rewards).
        for alloc in &genesis.allocations {
            if alloc.virtual_pool {
                debug!(
                    amount = alloc.amount,
                    label = %alloc.label,
                    "skipping virtual pool allocation (tracked in RewardCalculator)"
                );
                continue;
            }

            let account = Account::with_balance(alloc.address, alloc.amount);

            // Load into executor's in-memory cache for Block-STM
            self.executor
                .set_account(alloc.address, account.state.clone());

            // Persist to RocksDB + JMT
            store.put_account(&account)?;

            debug!(
                address = %alloc.address,
                amount = alloc.amount,
                label = %alloc.label,
                "genesis allocation applied"
            );
        }

        // Create genesis block if it doesn't exist
        if store.get_block(0)?.is_none() {
            // Use timestamp from genesis config for determinism; fall back to wall clock
            let timestamp = if genesis.timestamp_ms > 0 {
                genesis.timestamp_ms
            } else {
                chrono::Utc::now().timestamp_millis().max(0) as u64
            };
            let genesis_block = Block::genesis(self.chain_id, timestamp);
            store.put_block(0, &genesis_block)?;
            store.set_latest_height(0)?;
            info!(timestamp, "Genesis block created at height 0");
        }

        info!(
            state_root = %store.state_root(),
            "genesis applied successfully"
        );

        Ok(())
    }

    /// Resume from the last persisted block — reload height, hash, base fee.
    pub fn resume_from_storage(&self) -> anyhow::Result<()> {
        let mut height;
        {
            let mut store = self.store.write();

            height = store.latest_height()?;
            if height == 0 {
                // Check if blocks actually exist beyond genesis (metadata corruption recovery).
                // If block 1 exists but latest_height is 0, the metadata was lost (e.g., WAL TTL).
                // Binary-search for the actual latest height.
                if store.get_block(1)?.is_some() {
                    let recovered = Self::recover_latest_height(&store);
                    if recovered > 0 {
                        warn!(
                            recovered_height = recovered,
                            "latest_height metadata was 0 but blocks exist — recovered via scan"
                        );
                        store.set_latest_height(recovered)?;
                        // Fall through to normal resume path below
                        // (height variable is reassigned)
                    }
                }

                // Re-read after potential recovery
                let h = store.latest_height()?;
                if h == 0 {
                    if store.get_block(0)?.is_some() {
                        let jmt_count = store.rebuild_jmt()?;
                        if jmt_count > 0 {
                            info!(accounts = jmt_count, "JMT rebuilt from genesis state");
                        }
                        info!("Chain at genesis, nothing to resume");
                    }
                    return Ok(());
                }
                // Use recovered height
                height = h;
            }

            // Forward-scan: check if blocks exist beyond latest_height (can happen
            // when the block was committed but metadata update didn't persist).
            let mut scan_h = height + 1;
            while store.get_block(scan_h)?.is_some() {
                scan_h += 1;
            }
            if scan_h - 1 > height {
                let old = height;
                height = scan_h - 1;
                store.set_latest_height(height)?;
                warn!(
                    old_height = old,
                    actual_height = height,
                    "found blocks beyond latest_height — corrected metadata"
                );
            }

            // Rebuild the in-memory JMT from persisted account state FIRST
            // so that state_root() returns the correct value.
            let jmt_count = store.rebuild_jmt()?;

            // Load the latest block to get parent hash and base fee
            let last_block = store.get_block(height)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "block at height {height} not found but latest_height says {height}"
                )
            })?;

            let block_hash = last_block.hash();
            let base_fee = last_block.header.base_fee;
            let last_ts = last_block.header.timestamp_ms;

            // Update node state
            self.block_height.store(height, Ordering::SeqCst);
            *self.last_block_hash.write() = block_hash;
            self.base_fee.store(base_fee, Ordering::SeqCst);
            // R37-FIX: Persist last block timestamp so block producer can enforce
            // monotonicity on restart instead of starting from 0.
            self.last_block_timestamp_ms
                .store(last_ts, Ordering::SeqCst);

            // Reload cumulative total_burned and total_minted
            let db = store.db();
            if let Ok(Some(val)) = db.get_metadata(b"total_burned") {
                if val.len() == 8 {
                    let burned = u64::from_le_bytes(val[..8].try_into().unwrap());
                    self.total_burned.store(burned, Ordering::SeqCst);
                }
            }
            if let Ok(Some(val)) = db.get_metadata(b"total_minted") {
                if val.len() == 8 {
                    let minted = u64::from_le_bytes(val[..8].try_into().unwrap());
                    self.total_minted.store(minted, Ordering::SeqCst);
                }
            }

            info!(
                height,
                block_hash = %block_hash,
                base_fee,
                total_burned = self.total_burned.load(Ordering::SeqCst),
                total_minted = self.total_minted.load(Ordering::SeqCst),
                jmt_accounts = jmt_count,
                state_root = %store.state_root(),
                "resumed from storage (JMT rebuilt)"
            );
        } // Release store write lock before rebuilding mining state

        // Rebuild mining state from block history
        self.rebuild_mining_state(height)?;

        // Reload sub-executor state (tokens, DEX, NFTs, launchpad, contract storage)
        self.reload_sub_executor_state()?;

        // Pre-load ALL staked accounts from storage into the executor cache
        // so that anti-concentration tracking is accurate from the first block.
        // The executor cache is lazy-loaded, so without this scan, non-genesis
        // stakers would be missing from the StakeTracker until their first tx.
        {
            let store = self.store.read();
            let staked_accounts = store.scan_staked_accounts()?;
            let count = staked_accounts.len();
            for account in staked_accounts {
                self.executor.set_account(account.address, account.state);
            }
            if count > 0 {
                info!(
                    staked_accounts = count,
                    "pre-loaded staked accounts for anti-concentration tracking"
                );
            }
        }
        // Rebuild staking concentration totals from loaded account state.
        self.executor.rebuild_staking_totals();

        Ok(())
    }

    /// Binary search for the actual latest block height when metadata is lost.
    /// Assumes block 1 exists. Exponentially probes upward then binary-searches.
    fn recover_latest_height(store: &pichain_storage::StateStore) -> u64 {
        // Exponential probe to find an upper bound
        let mut upper = 1u64;
        while store.get_block(upper).ok().flatten().is_some() {
            if upper > u64::MAX / 2 {
                break;
            }
            upper *= 2;
        }
        // Binary search between upper/2 and upper
        let mut lo = upper / 2;
        let mut hi = upper;
        while lo < hi {
            let mid = lo + (hi - lo + 1) / 2;
            if store.get_block(mid).ok().flatten().is_some() {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    /// Reload all sub-executor state from storage into the in-memory DashMap caches.
    /// Called on startup after resume_from_storage().
    fn reload_sub_executor_state(&self) -> anyhow::Result<()> {
        let store = self.store.read();
        let db = store.db();

        // Token mints
        let token_store = pichain_storage::TokenStore::new(db);
        let mints = token_store.scan_all_mints()?;
        let mint_count = mints.len();
        for mint in mints {
            self.executor.token_executor().load_mint(mint);
        }

        // Token accounts
        let accounts = token_store.scan_all_token_accounts()?;
        let account_count = accounts.len();
        for account in accounts {
            self.executor.token_executor().load_token_account(account);
        }

        // DEX pools
        let dex_store = pichain_storage::DexStore::new(db);
        let pools = dex_store.scan_all_pools()?;
        let pool_count = pools.len();
        for pool in pools {
            self.executor.dex_executor().load_pool(pool);
        }

        // DEX LP balances
        let lp_balances = dex_store.scan_all_lp_balances()?;
        let lp_count = lp_balances.len();
        for (pool_id, owner, balance) in lp_balances {
            self.executor
                .dex_executor()
                .load_lp_balance(pool_id, owner, balance);
        }

        // NFT collections
        let nft_store = pichain_storage::NftStore::new(db);
        let collections = nft_store.scan_all_collections()?;
        let collection_count = collections.len();
        for collection in collections {
            self.executor.nft_executor().load_collection(collection);
        }

        // NFTs
        let nfts = nft_store.scan_all_nfts()?;
        let nft_count = nfts.len();
        for nft in nfts {
            self.executor.nft_executor().load_nft(nft);
        }

        // Launchpad launches
        let launchpad_store = pichain_storage::LaunchpadStore::new(db);
        let launches = launchpad_store.scan_all_launches()?;
        let launch_count = launches.len();
        for launch in launches {
            self.executor.launchpad_executor().load_launch(launch);
        }

        // Betting matches
        let betting_store = pichain_storage::BettingStore::new(db);
        let betting_matches = betting_store.scan_all_matches()?;
        let match_count = betting_matches.len();
        for m in betting_matches {
            self.executor.betting_executor().load_match(m);
        }

        // WASM contract storage
        let contract_store = pichain_storage::ContractStorageStore::new(db);
        let entries = contract_store.scan_all()?;
        let contract_count = entries.len();
        for (contract, key, value) in entries {
            self.executor.load_contract_storage(contract, key, value);
        }

        // Mint nonces (metadata: "mn:" + address → u64)
        let mint_nonce_entries = db.scan_metadata_prefix(b"mn:")?;
        for (key_suffix, value) in &mint_nonce_entries {
            if key_suffix.len() == 20 && value.len() == 8 {
                let mut addr_bytes = [0u8; 20];
                addr_bytes.copy_from_slice(key_suffix);
                let addr = pichain_crypto::keys::Address(addr_bytes);
                let nonce = u64::from_le_bytes(value[..8].try_into().unwrap());
                self.executor.token_executor().load_mint_nonce(addr, nonce);
            }
        }

        // Collection nonces (metadata: "cn:" + address → u64)
        let coll_nonce_entries = db.scan_metadata_prefix(b"cn:")?;
        for (key_suffix, value) in &coll_nonce_entries {
            if key_suffix.len() == 20 && value.len() == 8 {
                let mut addr_bytes = [0u8; 20];
                addr_bytes.copy_from_slice(key_suffix);
                let addr = pichain_crypto::keys::Address(addr_bytes);
                let nonce = u64::from_le_bytes(value[..8].try_into().unwrap());
                self.executor
                    .nft_executor()
                    .load_collection_nonce(addr, nonce);
            }
        }

        if mint_count > 0
            || account_count > 0
            || pool_count > 0
            || collection_count > 0
            || nft_count > 0
            || launch_count > 0
            || contract_count > 0
            || match_count > 0
        {
            info!(
                mints = mint_count,
                token_accounts = account_count,
                pools = pool_count,
                lp_balances = lp_count,
                collections = collection_count,
                nfts = nft_count,
                launches = launch_count,
                betting_matches = match_count,
                contract_entries = contract_count,
                mint_nonces = mint_nonce_entries.len(),
                collection_nonces = coll_nonce_entries.len(),
                "sub-executor state reloaded from storage"
            );
        }

        Ok(())
    }

    /// Bootstrap wrapped tokens and DEX pools for the cross-chain bridge.
    /// Creates wETH, wSOL, wBTC, wUSDT mints with the bridge operator as mint_authority,
    /// then creates PI/wXXX pools seeded with initial liquidity from the 5% reserve.
    /// Idempotent: skips if the first wrapped mint already exists.
    pub fn bootstrap_bridge_tokens(&self) -> anyhow::Result<()> {
        use pichain_types::dex::{isqrt, LiquidityPool, PoolId};
        use pichain_types::token::{token_account_key, MintId, TokenAccount, TokenMint};

        let bridge_operator = GenesisConfig::devnet_bridge_operator_address();
        let liquidity_addr = GenesisConfig::devnet_liquidity_address();

        // Deterministic MintIds for wrapped tokens (using bridge operator + sequential nonces)
        let weth_mint_id = MintId::derive(&bridge_operator, 0);
        let wsol_mint_id = MintId::derive(&bridge_operator, 1);
        let wbtc_mint_id = MintId::derive(&bridge_operator, 2);
        let wusdt_mint_id = MintId::derive(&bridge_operator, 3);

        // Check if already bootstrapped (idempotent)
        if self
            .executor
            .token_executor()
            .get_mint(&weth_mint_id)
            .is_some()
        {
            info!("bridge tokens already bootstrapped, skipping");
            return Ok(());
        }

        info!("bootstrapping bridge wrapped tokens and liquidity pools...");

        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;

        // Define wrapped tokens: (mint_id, name, symbol, decimals, nonce)
        let tokens = [
            (weth_mint_id, "Wrapped Ether", "wETH", 9u8, 0u64),
            (wsol_mint_id, "Wrapped Solana", "wSOL", 9u8, 1u64),
            (wbtc_mint_id, "Wrapped Bitcoin", "wBTC", 9u8, 2u64),
            (wusdt_mint_id, "Wrapped Tether", "wUSDT", 9u8, 3u64),
        ];

        // Pool seeding amounts (in base units, 1 PI = 1_000_000_000):
        // PI/wETH: 25M PI + 12,500 wETH  (1 wETH = 2,000 PI)
        // PI/wSOL: 25M PI + 250,000 wSOL  (1 wSOL = 100 PI)
        // PI/wBTC: 25M PI + 250 wBTC      (1 wBTC = 100,000 PI)
        // PI/wUSDT: 25M PI + 25M wUSDT    (1 wUSDT = 1 PI)
        let base = 1_000_000_000u64; // 1 PI in base units
        let pi_per_pool: u64 = 25_000_000 * base; // 25M PI per pool
        let pool_configs: [(MintId, u64, u64); 4] = [
            (weth_mint_id, pi_per_pool, 12_500 * base),      // wETH
            (wsol_mint_id, pi_per_pool, 250_000 * base),     // wSOL
            (wbtc_mint_id, pi_per_pool, 250 * base),         // wBTC
            (wusdt_mint_id, pi_per_pool, 25_000_000 * base), // wUSDT
        ];
        let total_pi_needed: u64 = pi_per_pool * 4; // 100M PI

        let mut store = self.store.write();

        // 1. Try to debit PI from liquidity reserve; if not found, mint directly
        //    (handles live chains initialized before liquidity reserve was added)
        if let Some(liq_state) = self.executor.get_account(&liquidity_addr) {
            if liq_state.balance >= total_pi_needed {
                let mut liq_acct = Account {
                    address: liquidity_addr,
                    state: liq_state,
                };
                liq_acct.state.balance -= total_pi_needed;
                store.put_account(&liq_acct)?;
                self.executor.set_account(liquidity_addr, liq_acct.state);
                info!(
                    pi_debited = total_pi_needed / base,
                    "debited PI from liquidity reserve for bridge pools"
                );
            } else {
                info!("liquidity reserve insufficient, minting PI for bridge pool seeding");
            }
        } else {
            info!("liquidity reserve not found, minting PI for bridge pool seeding (devnet bootstrap)");
        }

        // 2. Fund bridge operator with gas PI (10 PI for tx fees)
        let gas_amount: u64 = 10 * base;
        let bridge_state = self
            .executor
            .get_account(&bridge_operator)
            .unwrap_or_default();
        let mut bridge_acct = Account {
            address: bridge_operator,
            state: bridge_state,
        };
        bridge_acct.state.balance = bridge_acct
            .state
            .balance
            .checked_add(gas_amount)
            .ok_or_else(|| anyhow::anyhow!("bridge operator balance overflow"))?;
        store.put_account(&bridge_acct)?;
        self.executor
            .set_account(bridge_operator, bridge_acct.state);

        // Now borrow db for token/pool writes
        let db = store.db();
        let token_store = pichain_storage::TokenStore::new(db);
        let dex_store = pichain_storage::DexStore::new(db);

        // 3. Create token mints
        for &(mint_id, name, symbol, decimals, _nonce) in &tokens {
            let mint = TokenMint {
                id: mint_id,
                name: name.to_string(),
                symbol: symbol.to_string(),
                decimals,
                total_supply: 0,
                max_supply: 0, // unlimited — bridge mints on demand
                creator: bridge_operator,
                mint_authority: Some(bridge_operator),
                freeze_authority: None,
                active: true,
                created_at_ms: now_ms,
                metadata_uri: String::new(),
            };
            token_store.put_mint(&mint)?;
            self.executor.token_executor().load_mint(mint);
            info!(symbol, mint_id = %mint_id, "created wrapped token mint");
        }

        // Store mint nonce for bridge operator (4 mints created = nonce 4)
        let nonce_key = [b"mn:".as_slice(), &bridge_operator.0].concat();
        db.put_metadata(&nonce_key, &4u64.to_le_bytes())?;

        // 4. Create pools + seed liquidity
        for (wrapped_mint, pi_amount, token_amount) in pool_configs {
            // Create pool: native PI (MintId::ZERO) vs wrapped token
            let pool_id = PoolId::derive(&MintId::ZERO, &wrapped_mint);
            let (mint_a, mint_b) = if MintId::ZERO <= wrapped_mint {
                (MintId::ZERO, wrapped_mint)
            } else {
                (wrapped_mint, MintId::ZERO)
            };

            // Map amounts to canonical ordering
            let (amount_a, amount_b) = if mint_a == MintId::ZERO {
                (pi_amount, token_amount)
            } else {
                (token_amount, pi_amount)
            };

            // Calculate LP tokens: sqrt(amount_a * amount_b) - MINIMUM_LIQUIDITY
            let product = amount_a as u128 * amount_b as u128;
            let lp_total = isqrt(product);
            let lp_minted = (lp_total - LiquidityPool::MINIMUM_LIQUIDITY as u128) as u64;

            // Mint the wrapped token supply for this pool
            let mut mint = self
                .executor
                .token_executor()
                .get_mint(&wrapped_mint)
                .ok_or_else(|| anyhow::anyhow!("mint not found after creation"))?;
            mint.total_supply = mint
                .total_supply
                .checked_add(token_amount)
                .ok_or_else(|| anyhow::anyhow!("token supply overflow"))?;
            token_store.put_mint(&mint)?;
            self.executor.token_executor().load_mint(mint);

            // Create token account for the pool's wrapped token reserve
            let pool_token_key = token_account_key(&liquidity_addr, &wrapped_mint);
            let pool_token_acct = TokenAccount {
                key: pool_token_key,
                owner: liquidity_addr,
                mint: wrapped_mint,
                balance: token_amount,
                delegate: None,
                delegate_amount: 0,
                frozen: false,
            };
            token_store.put_token_account(&pool_token_acct)?;
            self.executor
                .token_executor()
                .load_token_account(pool_token_acct);

            // Create the pool with seeded reserves
            let pool = LiquidityPool {
                id: pool_id,
                mint_a,
                mint_b,
                reserve_a: amount_a,
                reserve_b: amount_b,
                lp_supply: lp_minted,
                fee_bps: LiquidityPool::DEFAULT_FEE_BPS,
                creator: liquidity_addr,
                active: true,
                created_at_ms: now_ms,
                cumulative_volume_a: 0,
                cumulative_volume_b: 0,
                creator_fee_recipient: None,
                creator_fee_bps: 0,
                created_at_height: 0, // Genesis pools have no cooldown
            };
            dex_store.put_pool(&pool)?;
            self.executor.dex_executor().load_pool(pool);

            // Assign LP tokens to liquidity address
            dex_store.put_lp_balance(&pool_id, &liquidity_addr, lp_minted)?;
            self.executor
                .dex_executor()
                .load_lp_balance(pool_id, liquidity_addr, lp_minted);

            info!(
                pool_id = %pool_id,
                reserve_pi = pi_amount / base,
                reserve_token = token_amount / base,
                lp_minted,
                "created and seeded bridge liquidity pool"
            );
        }

        info!(
            bridge_operator = %bridge_operator,
            total_pi_used = total_pi_needed / base,
            "bridge tokens and pools bootstrapped successfully"
        );

        Ok(())
    }

    /// Rebuild mining state by replaying MiningProof transactions from persisted blocks.
    /// Called on startup to restore the DigitRegistry to its correct state.
    fn rebuild_mining_state(&self, latest_height: u64) -> anyhow::Result<()> {
        let store = self.store.read();
        let mut processor = self.executor.mining_processor().lock();
        let mut replayed = 0u64;

        // Set genesis timestamp BEFORE replaying any proofs.
        // Without this, genesis_timestamp_ms defaults to 0 and
        // year_from_timestamp() always returns year 1, causing perpetual
        // year-1 rewards regardless of actual chain age.
        if let Some(genesis_block) = store.get_block(0)? {
            let genesis_ts = genesis_block.header.timestamp_ms;
            if genesis_ts > 0 {
                processor.set_genesis_timestamp(genesis_ts);
                debug!(
                    genesis_timestamp_ms = genesis_ts,
                    "set genesis timestamp for mining replay"
                );
            } else {
                tracing::warn!("genesis block has timestamp 0 — emission year calculation will default to year 1");
            }
        }

        for height in 0..=latest_height {
            if let Some(block) = store.get_block(height)? {
                processor.set_height(height);
                // AUDIT-FIX Mining-M2: Apply fee income BEFORE replaying this block's
                // mining proofs, so remaining_pool() includes fee income during replay
                // (matching live processing behavior). Without this, proofs that were
                // valid live could fail replay when near the pool cap.
                if block.header.pi_miner_fee > 0 {
                    processor.add_fee_income(block.header.pi_miner_fee);
                }
                for tx in &block.transactions {
                    if let pichain_types::TransactionKind::MiningProof {
                        start_position,
                        digit_count,
                        ref digits,
                        ..
                    } = tx.data.kind
                    {
                        // Only replay if the transaction was actually successful
                        let tx_hash = tx.hash();
                        if let Some(receipt) = store.get_receipt(&tx_hash)? {
                            if receipt.status == pichain_types::TransactionStatus::Success {
                                if let Err(e) = processor.register_historical(
                                    start_position,
                                    digit_count,
                                    digits,
                                    tx.data.sender,
                                    height,
                                    block.header.timestamp_ms,
                                ) {
                                    // Log but don't fail — could be duplicate from replay
                                    tracing::warn!(
                                        height,
                                        start_position,
                                        error = %e,
                                        "failed to replay mining proof (may be expected)"
                                    );
                                } else {
                                    replayed += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        if replayed > 0 {
            let stats = processor.stats();
            info!(
                replayed,
                frontier = stats.frontier_position,
                total_digits = stats.total_digits_verified,
                unique_miners = stats.unique_miners,
                fee_income = stats.fee_income,
                "mining state rebuilt from block history"
            );
        }

        Ok(())
    }

    /// Persist a produced block — write block, transactions, receipts, state changes,
    /// and sub-executor state (tokens, DEX, NFTs, launchpad, contract storage).
    /// Uses atomic WriteBatch with WAL sync to prevent partial writes on crash.
    /// Returns the computed state root from local execution.
    pub async fn persist_block(
        &self,
        produced: &ProducedBlock,
    ) -> anyhow::Result<pichain_crypto::poseidon::PoseidonHash> {
        let state_root = {
            let mut store = self.store.write();
            let block = &produced.block;
            let height = block.header.height;

            // Build tx/receipt pairs and state changes for atomic write
            let mut txs_and_receipts = Vec::new();
            let mut state_changes = Vec::new();

            for (i, tx) in block.transactions.iter().enumerate() {
                let tx_hash = tx.hash();
                let receipt = produced.execution_results.get(i).map(|r| &r.effect);
                txs_and_receipts.push((tx_hash, tx, receipt));

                if let Some(result) = produced.execution_results.get(i) {
                    for (addr, state) in &result.state_changes {
                        state_changes.push((*addr, state));
                    }
                }
            }

            // Phase 1: Prepare the batch (block, txs, receipts, account state, height)
            let (mut batch, computed_state_root, pending_jmt_updates) = store.prepare_block_batch(
                height,
                block,
                &txs_and_receipts
                    .iter()
                    .map(|(h, tx, r)| (*h, *tx, r.as_ref().map(|x| *x)))
                    .collect::<Vec<_>>(),
                &state_changes
                    .iter()
                    .map(|(a, s)| (*a, *s))
                    .collect::<Vec<_>>(),
                &[], // H6: object changes (currently added in Phase 2 via sub-executor batch_put_*)
            )?;

            // Phase 2: Add sub-executor state to the same batch (atomic with block)
            let sub_state = self.executor.snapshot_sub_state();
            let db = store.db();

            // Token mints + accounts
            let token_store = pichain_storage::TokenStore::new(db);
            for mint in sub_state.mints.values() {
                token_store.batch_put_mint(&mut batch, mint)?;
            }
            for account in sub_state.token_accounts.values() {
                token_store.batch_put_token_account(&mut batch, account)?;
            }

            // DEX pools + LP balances
            let dex_store = pichain_storage::DexStore::new(db);
            for pool in sub_state.pools.values() {
                dex_store.batch_put_pool(&mut batch, pool)?;
            }
            for ((pool_id, address), balance) in &sub_state.lp_balances {
                dex_store.batch_put_lp_balance(&mut batch, pool_id, address, *balance)?;
            }

            // Accumulate holder dividend fees from swaps
            {
                let dividend_fees = self.executor.dex_executor().drain_dividend_fees();
                if !dividend_fees.is_empty() {
                    let div_store = pichain_storage::DividendStore::new(db);
                    for (mint_id, fee_amount) in &dividend_fees {
                        let mut pool = div_store.get_pool(mint_id).unwrap_or(None).unwrap_or(
                            pichain_storage::DividendPool {
                                mint_id: *mint_id,
                                total_accumulated: 0,
                                total_claimed: 0,
                                reward_per_share_x1e12: 0,
                                total_supply: 0,
                            },
                        );
                        // Get current total supply for the token
                        let total_supply = self
                            .executor
                            .token_executor()
                            .get_mint(mint_id)
                            .map(|m| m.total_supply)
                            .unwrap_or(0);
                        if total_supply > 0 && *fee_amount > 0 {
                            pool.total_accumulated =
                                pool.total_accumulated.saturating_add(*fee_amount);
                            pool.total_supply = total_supply;
                            // reward_per_share += fee * 1e12 / total_supply
                            let increment = (*fee_amount as u128)
                                .checked_mul(1_000_000_000_000)
                                .unwrap_or(0)
                                / (total_supply as u128);
                            pool.reward_per_share_x1e12 =
                                pool.reward_per_share_x1e12.saturating_add(increment);
                            div_store.put_pool(&pool).ok();
                        }
                    }
                }
            }

            // NFT collections + NFTs
            let nft_store = pichain_storage::NftStore::new(db);
            for collection in sub_state.collections.values() {
                nft_store.batch_put_collection(&mut batch, collection)?;
            }
            for nft in sub_state.nfts.values() {
                nft_store.batch_put_nft(&mut batch, nft)?;
            }

            // Launchpad launches
            let launchpad_store = pichain_storage::LaunchpadStore::new(db);
            for launch in sub_state.launches.values() {
                launchpad_store.batch_put_launch(&mut batch, launch)?;
            }

            // Betting matches
            let betting_store = pichain_storage::BettingStore::new(db);
            for m in sub_state.matches.values() {
                betting_store.batch_put_match(&mut batch, m)?;
            }

            // Match nonces (metadata: "bn:" + address → u64)
            for (addr, nonce) in &sub_state.match_nonces {
                let mut key = Vec::with_capacity(23);
                key.extend_from_slice(b"bn:");
                key.extend_from_slice(&addr.0);
                db.batch_put_metadata(&mut batch, &key, &nonce.to_le_bytes());
            }

            // WASM contract storage
            let contract_store = pichain_storage::ContractStorageStore::new(db);
            for ((contract_addr, storage_key), value) in &sub_state.contract_storage {
                if value.is_empty() {
                    contract_store.batch_delete(&mut batch, contract_addr, storage_key);
                } else {
                    contract_store.batch_put(&mut batch, contract_addr, storage_key, value);
                }
            }

            // Mint nonces (metadata: "mn:" + address → u64)
            for (addr, nonce) in &sub_state.mint_nonces {
                let mut key = Vec::with_capacity(23);
                key.extend_from_slice(b"mn:");
                key.extend_from_slice(&addr.0);
                db.batch_put_metadata(&mut batch, &key, &nonce.to_le_bytes());
            }

            // Collection nonces (metadata: "cn:" + address → u64)
            for (addr, nonce) in &sub_state.collection_nonces {
                let mut key = Vec::with_capacity(23);
                key.extend_from_slice(b"cn:");
                key.extend_from_slice(&addr.0);
                db.batch_put_metadata(&mut batch, &key, &nonce.to_le_bytes());
            }

            // Persist cumulative total_burned and total_minted in the same batch
            let new_total_burned = self
                .total_burned
                .load(Ordering::SeqCst)
                .saturating_add(produced.total_burned);
            let new_total_minted = self
                .total_minted
                .load(Ordering::SeqCst)
                .saturating_add(produced.total_minted);

            // R26-FIX: Supply invariant check BEFORE commit. If total minted would
            // exceed the mining pool, reject the block rather than persisting an
            // invalid state. Previously this check ran after commit, making it a
            // detection-only log that couldn't prevent the violation.
            let mining_pool_128 = pichain_types::TOTAL_SUPPLY * 85 / 100;
            let mining_pool_base = u64::try_from(mining_pool_128).unwrap_or(u64::MAX);
            if new_total_minted > mining_pool_base {
                error!(
                    total_minted = new_total_minted,
                    mining_pool = mining_pool_base,
                    "SUPPLY INVARIANT VIOLATION: block would exceed mining pool — rejecting"
                );
                return Err(anyhow::anyhow!(
                    "supply invariant violation: total_minted {} > mining_pool {}",
                    new_total_minted,
                    mining_pool_base
                ));
            }

            db.batch_put_metadata(&mut batch, b"total_burned", &new_total_burned.to_le_bytes());
            db.batch_put_metadata(&mut batch, b"total_minted", &new_total_minted.to_le_bytes());

            // --- Transaction History + Event Indexing ---
            for (i, tx) in block.transactions.iter().enumerate() {
                let tx_hash = tx.hash();
                let tx_idx = i as u16;
                let sender_bytes = tx.data.sender.0;

                // Index for sender
                db.batch_index_tx_for_address(
                    &mut batch,
                    &sender_bytes,
                    height,
                    tx_idx,
                    tx_hash.as_bytes(),
                );

                // Index for recipient (if applicable)
                if let Some(recipient) = tx.data.kind.recipient_address() {
                    if recipient != tx.data.sender {
                        db.batch_index_tx_for_address(
                            &mut batch,
                            &recipient.0,
                            height,
                            tx_idx,
                            tx_hash.as_bytes(),
                        );
                    }
                }

                // Index events from receipt + record trades
                if let Some(result) = produced.execution_results.get(i) {
                    for (evt_idx, event) in result.effect.events.iter().enumerate() {
                        let global_idx = (i * 256 + evt_idx) as u16;
                        let topic = pichain_crypto::hash(event.event_type.as_bytes());
                        db.batch_index_event_topic(
                            &mut batch,
                            topic.as_bytes(),
                            height,
                            global_idx,
                            tx_hash.as_bytes(),
                        );
                        db.batch_index_event_address(
                            &mut batch,
                            &sender_bytes,
                            height,
                            global_idx,
                            tx_hash.as_bytes(),
                        );

                        // Record swap trades for DEX analytics
                        if event.event_type == "Swap" {
                            if let Ok(swap_data) =
                                serde_json::from_slice::<serde_json::Value>(&event.data)
                            {
                                if let (
                                    Some(pool_hex),
                                    Some(mint_in_hex),
                                    Some(mint_out_hex),
                                    Some(amount_in),
                                    Some(amount_out),
                                    Some(fee),
                                ) = (
                                    swap_data["pool"].as_str(),
                                    swap_data["mint_in"].as_str(),
                                    swap_data["mint_out"].as_str(),
                                    swap_data["amount_in"].as_u64(),
                                    swap_data["amount_out"].as_u64(),
                                    swap_data["fee"].as_u64(),
                                ) {
                                    if let (Ok(pool_bytes), Ok(mint_in_bytes), Ok(mint_out_bytes)) = (
                                        hex::decode(pool_hex),
                                        hex::decode(mint_in_hex),
                                        hex::decode(mint_out_hex),
                                    ) {
                                        if pool_bytes.len() == 32
                                            && mint_in_bytes.len() == 32
                                            && mint_out_bytes.len() == 32
                                        {
                                            let mut pool_arr = [0u8; 32];
                                            pool_arr.copy_from_slice(&pool_bytes);
                                            let mut min_arr = [0u8; 32];
                                            min_arr.copy_from_slice(&mint_in_bytes);
                                            let mut mout_arr = [0u8; 32];
                                            mout_arr.copy_from_slice(&mint_out_bytes);
                                            let trade = pichain_storage::TradeRecord {
                                                pool_id: pichain_types::dex::PoolId(pool_arr),
                                                sender: tx.data.sender,
                                                mint_in: pichain_types::token::MintId(min_arr),
                                                mint_out: pichain_types::token::MintId(mout_arr),
                                                amount_in,
                                                amount_out,
                                                fee,
                                                timestamp_ms: block.header.timestamp_ms,
                                                block_height: height,
                                                tx_hash: *tx_hash.as_bytes(),
                                            };
                                            let _ = dex_store
                                                .batch_record_trade(&mut batch, &trade, global_idx);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Phase 3: Commit the entire batch atomically with WAL sync.
            // The pending JMT updates are applied to the in-memory tree only
            // after the batch succeeds (Fix STOR-223).
            store.commit_block_batch(batch, pending_jmt_updates, height)?;

            // Update in-memory node state
            self.block_height.store(height, Ordering::SeqCst);
            *self.last_block_hash.write() = block.hash();
            self.base_fee.store(block.header.base_fee, Ordering::SeqCst);
            self.total_burned.store(new_total_burned, Ordering::SeqCst);
            self.total_minted.store(new_total_minted, Ordering::SeqCst);

            debug!(
                height,
                tx_count = block.header.tx_count,
                burned = produced.total_burned,
                minted = produced.total_minted,
                "block persisted (atomic + sub-executor state)"
            );

            // Update mempool sender nonces from committed block so that
            // subsequent transactions at the next nonce are considered "ready".
            // claim_ready_transactions already advances next_nonce for the
            // current batch, but this ensures the mempool stays consistent
            // with persisted state (e.g., after restart or WAL replay).
            if block.header.tx_count > 0 {
                let committed_hashes: Vec<pichain_crypto::Hash> =
                    block.transactions.iter().map(|tx| tx.hash()).collect();
                let mut sender_nonces = std::collections::HashMap::new();
                for result in &produced.execution_results {
                    for (addr, state) in &result.state_changes {
                        sender_nonces.insert(*addr, state.nonce);
                    }
                }
                self.mempool
                    .remove_committed(&committed_hashes, &sender_nonces);
            }

            // Compact mempool WAL — remove confirmed transactions
            if block.header.tx_count > 0 {
                self.wal_compact();
            }

            computed_state_root
        }; // Release store lock before acquiring staking lock

        // Distribute staking rewards (65% of fees go to stakers)
        let fee_calculator = pichain_execution::FeeCalculator::new();
        let total_fees: u64 = produced
            .execution_results
            .iter()
            .map(|r| fee_calculator.calculate_fee(r.effect.gas_used, r.effect.base_fee, 0))
            .fold(0u64, |acc, v| acc.saturating_add(v));
        let staker_reward_u128 =
            total_fees as u128 * pichain_types::FEE_STAKER_RATE_BPS as u128 / 10_000;
        let staker_reward = u64::try_from(staker_reward_u128).unwrap_or_else(|_| {
            error!(
                staker_reward_u128,
                "staker reward exceeds u64 — clamping to u64::MAX"
            );
            u64::MAX
        });
        if staker_reward > 0 {
            let mut staking = self.staking.write().await;
            staking.distribute_rewards(staker_reward);
            staking.record_block_proposed(produced.block.header.proposer);
        }

        Ok(state_root)
    }

    /// Get the current state root from JMT.
    pub fn state_root(&self) -> pichain_crypto::poseidon::PoseidonHash {
        self.store.read().state_root()
    }

    /// Insert a transaction into the mempool with full nonce validation.
    /// R37-FIX: Public method so P2P inbound transactions can use the same
    /// nonce gap rejection as RPC transactions, instead of bypassing it via
    /// direct mempool.insert().
    pub fn insert_transaction(&self, tx: pichain_types::SignedTransaction) -> Result<(), String> {
        let sender = tx.data.sender;
        let on_chain_nonce = if let Ok(Some(account)) = self.store.read().get_account(&sender) {
            if self.executor.get_account(&sender).is_none() {
                self.executor.set_account(sender, account.state.clone());
            }
            self.mempool.set_sender_nonce(sender, account.state.nonce);
            account.state.nonce
        } else {
            0
        };
        const MAX_NONCE_GAP: u64 = 1024;
        if tx.data.nonce > on_chain_nonce.saturating_add(MAX_NONCE_GAP) {
            return Err(format!(
                "nonce {} is too far ahead of on-chain nonce {} (max gap: {})",
                tx.data.nonce, on_chain_nonce, MAX_NONCE_GAP
            ));
        }
        self.mempool
            .insert(tx)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Get current block height.
    pub fn height(&self) -> u64 {
        self.block_height.load(Ordering::SeqCst)
    }

    /// Get last block hash.
    pub fn last_hash(&self) -> Hash {
        *self.last_block_hash.read()
    }

    /// Get current base fee.
    pub fn get_base_fee(&self) -> PiAmount {
        self.base_fee.load(Ordering::SeqCst)
    }

    /// Get the last block's timestamp (for block producer monotonicity on restart).
    pub fn last_block_timestamp_ms(&self) -> u64 {
        self.last_block_timestamp_ms.load(Ordering::SeqCst)
    }

    /// Get total burned.
    pub fn get_total_burned(&self) -> PiAmount {
        self.total_burned.load(Ordering::SeqCst)
    }

    /// Get total minted via mining rewards.
    pub fn get_total_minted(&self) -> PiAmount {
        self.total_minted.load(Ordering::SeqCst)
    }

    /// Retrieve a stored block by height (for serving to syncing peers).
    pub fn get_block(&self, height: u64) -> Option<Block> {
        let store = self.store.read();
        store.get_block(height).ok().flatten()
    }

    /// Execute and persist a block received from a peer.
    ///
    /// This is the core of chain sync: when a follower node receives a valid block
    /// from the leader/proposer, it re-executes the transactions against its local
    /// state and persists the results. This ensures the follower independently
    /// validates all state transitions rather than blindly trusting the proposer.
    ///
    /// Returns Ok(()) if the block was successfully executed and persisted,
    /// or Err if execution/persistence failed.
    pub async fn execute_peer_block(&self, block: &Block) -> anyhow::Result<()> {
        // Serialize peer block application to prevent concurrent blocks at the
        // same height from corrupting state. The inbound handler and sync handler
        // can both call this method concurrently.
        let _guard = self.peer_block_mutex.lock().await;

        let height = block.header.height;

        // Re-check height under the mutex — another block may have been applied
        // between the caller's check and acquiring this lock.
        let expected_height = self.height() + 1;
        if height != expected_height {
            return Err(anyhow::anyhow!(
                "peer block height {height} != expected {expected_height} (applied concurrently)"
            ));
        }

        // Load sender accounts into executor cache from storage (they may not
        // be cached yet if this node just restarted or hasn't seen these accounts).
        {
            let store = self.store.read();
            for tx in &block.transactions {
                let sender = tx.data.sender;
                if self.executor.get_account(&sender).is_none() {
                    if let Ok(Some(account)) = store.get_account(&sender) {
                        self.executor.set_account(sender, account.state);
                    }
                }
                // Also load recipient accounts for transfers
                if let pichain_types::TransactionKind::Transfer { recipient, .. } = &tx.data.kind {
                    if self.executor.get_account(recipient).is_none() {
                        if let Ok(Some(account)) = store.get_account(recipient) {
                            self.executor.set_account(*recipient, account.state);
                        }
                    }
                }
            }
        }

        // Set mining processor state for this block
        {
            let mut processor = self.executor.mining_processor().lock();
            processor.set_height(height);
            processor.set_block_timestamp(block.header.timestamp_ms);
        }

        // R36-FIX: Set executor timestamp, height, and DEX snapshot BEFORE execution.
        // Without this, sub-executors (token, DEX, NFT, launchpad) use stale timestamps
        // from the previous block, causing state divergence vs the proposer.
        self.executor.set_block_timestamp(block.header.timestamp_ms);
        self.executor.set_block_height(height);
        self.executor.snapshot_dex_reserves();

        // Re-execute the block's transactions against local state
        let execution_results = self
            .executor
            .execute_block(&block.transactions, block.header.base_fee);

        // Verify execution produces consistent results with the block header
        let computed_gas: u64 = execution_results
            .iter()
            .map(|r| r.effect.gas_used)
            .fold(0u64, |acc, v| acc.saturating_add(v));

        // Allow up to 10% gas discrepancy (proposer may have trimmed differently)
        // but reject blocks where gas_used diverges beyond tolerance (bidirectional)
        let tolerance = block.header.gas_used / 10; // 10%
        if computed_gas
            > block
                .header
                .gas_used
                .saturating_add(tolerance)
                .saturating_add(21_000)
        {
            return Err(anyhow::anyhow!(
                "peer block gas mismatch (over): computed {computed_gas}, header says {}",
                block.header.gas_used
            ));
        }
        if block.header.gas_used
            > computed_gas
                .saturating_add(tolerance)
                .saturating_add(21_000)
        {
            return Err(anyhow::anyhow!(
                "peer block gas mismatch (under): computed {computed_gas}, header says {}",
                block.header.gas_used
            ));
        }

        // Build a ProducedBlock for persistence
        let total_burned: u64 = execution_results
            .iter()
            .map(|r| r.pi_burned)
            .fold(0u64, |acc, v| acc.saturating_add(v));
        let total_minted: u64 = execution_results
            .iter()
            .map(|r| r.pi_minted)
            .fold(0u64, |acc, v| acc.saturating_add(v));
        let proposer_reward: u64 = execution_results
            .iter()
            .map(|r| r.proposer_reward)
            .fold(0u64, |acc, v| acc.saturating_add(v));
        let total_miner_fee: u64 = execution_results
            .iter()
            .map(|r| r.miner_fee)
            .fold(0u64, |acc, v| acc.saturating_add(v));

        // R36-FIX: Credit the proposer with priority fee + staker share rewards,
        // matching the block producer path. Without this, follower nodes have a
        // lower balance for the proposer, causing state root divergence.
        if proposer_reward > 0 {
            self.executor
                .credit_account(block.header.proposer, proposer_reward);
        }

        // Feed miner fees back into the mining pool (matching block producer path)
        if total_miner_fee > 0 {
            self.executor
                .mining_processor()
                .lock()
                .add_fee_income(total_miner_fee);
        }

        // Drip staking rewards from unmined emission recycling to proposer
        // (matching block producer path)
        {
            let staking_drip = self.executor.mining_processor().lock().drain_staking_drip();
            if staking_drip > 0 {
                self.executor
                    .credit_account(block.header.proposer, staking_drip);
            }
        }

        let produced = ProducedBlock {
            block: block.clone(),
            execution_results,
            total_burned,
            total_minted,
            proposer_reward,
            total_miner_fee,
            production_time_ms: 0, // Not locally produced
        };

        // Capture the proposer's claimed state root BEFORE persist_block overwrites it
        let claimed_state_root = block.header.state_root;

        // Persist using the same atomic path as locally-produced blocks.
        // persist_block returns the locally-computed state root from re-execution.
        let local_state_root = self.persist_block(&produced).await?;

        // CRITICAL: Verify state root matches proposer's claim.
        // persist_block stores OUR computed state root (not the proposer's claim).
        // If they differ, our node is in a correct but divergent state — the proposer
        // committed an invalid transition. We must halt to avoid building on bad state.
        if claimed_state_root != local_state_root {
            error!(
                height,
                proposer = %block.header.proposer,
                claimed = %claimed_state_root,
                computed = %local_state_root,
                "STATE ROOT MISMATCH: peer block state root does not match local execution — HALTING"
            );
            // Return error to halt sync. The node operator must investigate.
            // Our locally-stored state root is correct (we re-executed), but the
            // proposer's claim was wrong. The node should not continue syncing.
            return Err(anyhow::anyhow!(
                "state root mismatch at height {}: proposer claims {}, we computed {}",
                height,
                claimed_state_root,
                local_state_root
            ));
        }

        // R36-FIX: Evict confirmed transactions from the mempool, matching the
        // block producer path. Without this, stale txs accumulate and get
        // re-proposed if this follower becomes the next block producer.
        {
            let tx_hashes: Vec<pichain_crypto::Hash> =
                block.transactions.iter().map(|tx| tx.hash()).collect();
            let mut sender_nonces = std::collections::HashMap::new();
            for result in &produced.execution_results {
                for (addr, state) in &result.state_changes {
                    sender_nonces.insert(*addr, state.nonce);
                }
            }
            self.mempool.remove_committed(&tx_hashes, &sender_nonces);
        }

        info!(
            height,
            txs = block.header.tx_count,
            gas = computed_gas,
            burned = total_burned,
            minted = total_minted,
            proposer = %block.header.proposer,
            state_root = %local_state_root,
            "peer block executed and persisted (state root verified)"
        );

        Ok(())
    }
}

/// Implement the RPC StateProvider trait so the RPC server can query real state.
impl StateProvider for NodeState {
    fn chain_id(&self) -> u64 {
        self.chain_id
    }

    fn get_block_sync(&self, height: u64) -> Option<Block> {
        let store = self.store.read();
        store.get_block(height).ok().flatten()
    }

    fn get_account_sync(&self, address: &Address) -> Option<Account> {
        let store = self.store.read();
        store.get_account(address).ok().flatten()
    }

    fn get_transaction_sync(&self, tx_hash: &Hash) -> Option<pichain_types::SignedTransaction> {
        let store = self.store.read();
        store.get_transaction(tx_hash).ok().flatten()
    }

    fn get_receipt_sync(&self, tx_hash: &Hash) -> Option<pichain_types::TransactionEffect> {
        let store = self.store.read();
        store.get_receipt(tx_hash).ok().flatten()
    }

    fn get_tx_block_height(&self, tx_hash: &Hash) -> Option<u64> {
        let store = self.store.read();
        store.get_tx_block_height(tx_hash).ok().flatten()
    }

    fn current_height(&self) -> u64 {
        self.height()
    }

    fn current_base_fee(&self) -> u64 {
        self.get_base_fee()
    }

    fn total_burned(&self) -> u64 {
        self.get_total_burned()
    }

    fn total_minted(&self) -> u64 {
        self.get_total_minted()
    }

    fn state_root_hex(&self) -> String {
        self.state_root().to_string()
    }

    fn get_account_proof(&self, address: &pichain_crypto::keys::Address) -> Option<Vec<u8>> {
        let store = self.store.read();
        store.get_account_proof(address)
    }

    fn mempool_size(&self) -> usize {
        self.mempool.len()
    }

    fn mempool_insert(&self, tx: pichain_types::SignedTransaction) -> Result<(), String> {
        let sender = tx.data.sender;

        // Lazy-load sender account from storage into executor cache and mempool.
        // On restart, the executor cache is empty and the mempool's SenderQueue
        // defaults to next_nonce=0. Without this, transactions from accounts
        // with on-chain nonce > 0 would never be considered "ready" by the
        // mempool, and the executor would reject them for insufficient balance.
        //
        // R35-FIX: Only populate executor cache if the account is NOT already
        // cached, to avoid overwriting fresher in-memory state during concurrent
        // execute_block processing with stale storage data.
        let on_chain_nonce = if let Ok(Some(account)) = self.store.read().get_account(&sender) {
            if self.executor.get_account(&sender).is_none() {
                self.executor.set_account(sender, account.state.clone());
            }
            self.mempool.set_sender_nonce(sender, account.state.nonce);
            account.state.nonce
        } else {
            0
        };

        // R32-FIX: Reject transactions with nonces unreasonably far in the future.
        // This prevents an attacker from filling the mempool with far-future nonces
        // that will never execute but consume memory.
        const MAX_NONCE_GAP: u64 = 1024;
        if tx.data.nonce > on_chain_nonce.saturating_add(MAX_NONCE_GAP) {
            return Err(format!(
                "nonce {} is too far ahead of on-chain nonce {} (max gap: {})",
                tx.data.nonce, on_chain_nonce, MAX_NONCE_GAP
            ));
        }

        // Append to WAL before inserting — ensures crash recovery
        self.wal_append(&tx);
        self.mempool
            .insert(tx)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn pool_submit_mining_proof(
        &self,
        miner_address: &pichain_crypto::keys::Address,
        start_position: u64,
        digit_count: u32,
        digits: Vec<u8>,
        pow_nonce: u64,
        anchor_block_hash: Vec<u8>,
    ) -> Result<String, String> {
        // Verify pool mining is configured on this node
        if self.pool_signer.read().is_none() {
            return Err("pool mining not configured on this node".to_string());
        }

        // Process the proof directly through the mining processor
        let executor_guard = self.executor.mining_processor();
        let mut mining = executor_guard.lock();
        mining.set_height(self.height());
        mining.set_block_timestamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        );

        // Build the mining proof
        let proof = pichain_mining::MiningProof::new(
            start_position,
            digits,
            *miner_address,
            pow_nonce,
        );

        // Convert anchor hash to fixed-size array
        let mut anchor = [0u8; 32];
        if anchor_block_hash.len() >= 32 {
            anchor.copy_from_slice(&anchor_block_hash[..32]);
        } else {
            anchor[..anchor_block_hash.len()].copy_from_slice(&anchor_block_hash);
        }

        let result = mining.process_proof(&proof, &anchor);
        if !result.valid {
            return Err(result.error.unwrap_or_else(|| "proof rejected".to_string()));
        }

        if result.reward_amount > 0 {
            // Credit the miner's account
            let mut acct = self
                .executor
                .get_account(miner_address)
                .unwrap_or_default();
            acct.balance = acct.balance.saturating_add(result.reward_amount);
            self.executor.set_account(*miner_address, acct);
            self.total_minted.fetch_add(
                result.reward_amount,
                std::sync::atomic::Ordering::Relaxed,
            );
        }

        // Return a pseudo tx hash (proof was processed directly, not via mempool)
        let hash_input = format!("pool:{}:{}:{}", hex::encode(miner_address.0), start_position, digit_count);
        Ok(hex::encode(pichain_crypto::hash(hash_input.as_bytes()).0))
    }

    fn get_token_mint(&self, mint_id: &pichain_types::MintId) -> Option<pichain_types::TokenMint> {
        // First try the in-memory executor cache
        if let Some(mint) = self.executor.token_executor().get_mint(mint_id) {
            return Some(mint);
        }
        // Fall back to storage
        let store = self.store.read();
        pichain_storage::TokenStore::new(store.db())
            .get_mint(mint_id)
            .ok()
            .flatten()
    }

    fn get_token_account(
        &self,
        owner: &Address,
        mint: &pichain_types::MintId,
    ) -> Option<pichain_types::TokenAccount> {
        // First try the in-memory executor cache
        if let Some(account) = self
            .executor
            .token_executor()
            .get_token_account(owner, mint)
        {
            return Some(account);
        }
        // Fall back to storage
        let store = self.store.read();
        pichain_storage::TokenStore::new(store.db())
            .get_token_account(owner, mint)
            .ok()
            .flatten()
    }

    fn get_pool_by_mints(
        &self,
        mint_a: &pichain_types::MintId,
        mint_b: &pichain_types::MintId,
    ) -> Option<pichain_types::LiquidityPool> {
        self.executor
            .dex_executor()
            .get_pool_by_mints(mint_a, mint_b)
    }

    fn get_swap_quote(
        &self,
        mint_in: &pichain_types::MintId,
        mint_out: &pichain_types::MintId,
        amount_in: u64,
    ) -> Option<pichain_rpc::SwapQuote> {
        let pool = self
            .executor
            .dex_executor()
            .get_pool_by_mints(mint_in, mint_out)?;
        let is_a_to_b = pool.mint_a == *mint_in;
        let (amount_out, fee) = pool.calculate_swap_output(amount_in, is_a_to_b)?;
        let price_impact_bps = pool.price_impact_bps(amount_in, is_a_to_b);
        Some(pichain_rpc::SwapQuote {
            amount_out,
            fee,
            price_impact_bps,
        })
    }

    fn get_mining_stats(&self) -> Option<pichain_rpc::MiningStatusData> {
        let processor = self.executor.mining_processor().lock();
        let stats = processor.stats();
        let anchor = self.last_hash();
        Some(pichain_rpc::MiningStatusData {
            frontier_position: stats.frontier_position,
            total_digits_verified: stats.total_digits_verified,
            next_position: stats.next_position,
            max_batch_at_position: stats.max_batch_at_position,
            total_ranges: stats.total_ranges,
            unique_miners: stats.unique_miners,
            remaining_pool: stats.remaining_pool,
            total_mined: stats.total_mined,
            fee_income: stats.fee_income,
            reward_per_digit: stats.reward_per_digit,
            emission_year: stats.emission_year,
            difficulty_bits: stats.difficulty_bits,
            difficulty_target_hex: stats.difficulty_target_hex,
            anchor_block_hash: anchor.to_string(),
            min_batch_size: stats.min_batch_size,
            max_allowed_position: stats.max_allowed_position,
            frontier_bonus_at_next: stats.frontier_bonus_at_next,
            staking_reward_pool: stats.staking_reward_pool,
            epoch_actual_minted: stats.epoch_actual_minted,
        })
    }

    fn get_mining_slot(&self, address: &Address) -> Option<(u64, u32, usize)> {
        let mut processor = self.executor.mining_processor().lock();
        Some(processor.get_or_assign_slot(address))
    }

    fn get_mining_leaderboard(&self) -> Option<(Vec<(Address, u64)>, u64)> {
        let processor = self.executor.mining_processor().lock();
        let top = processor.registry().top_miners(50);
        let total = processor.registry().total_verified();
        Some((top, total))
    }

    fn get_miner_recent_proofs(&self, address: &Address, limit: usize) -> Vec<(u64, u32, u64)> {
        let processor = self.executor.mining_processor().lock();
        let mut miner_ranges: Vec<_> = processor
            .registry()
            .all_ranges()
            .iter()
            .filter(|r| &r.miner == address)
            .map(|r| (r.start, r.count, r.committed_at_height))
            .collect();
        miner_ranges.sort_by(|a, b| b.2.cmp(&a.2));
        miner_ranges.truncate(limit);
        miner_ranges
    }

    fn activation_count(&self) -> u64 {
        self.store.read().activation_count().unwrap_or(0)
    }

    fn activate_wallet(&self, address: &Address) -> Result<u64, String> {
        // 3.14 PI in base units (1 PI = 1_000_000_000)
        let locked_grant: u64 = 3_140_000_000;
        // Global cap: first 3.14 million wallets
        const MAX_ACTIVATIONS: u64 = 3_141_592;

        // Check if already activated (read lock first for fast path)
        {
            let store = self.store.read();
            if store.is_wallet_activated(address).unwrap_or(false) {
                return Err("wallet already activated".to_string());
            }
            if store.activation_count().unwrap_or(0) >= MAX_ACTIVATIONS {
                return Err("wallet activation cap reached (3,141,592 wallets)".to_string());
            }
        }

        // Mint locked PI directly to the target wallet under write lock
        let mut store = self.store.write();

        // Re-check under write lock (TOCTOU protection)
        if store.is_wallet_activated(address).unwrap_or(false) {
            return Err("wallet already activated".to_string());
        }
        if store.activation_count().unwrap_or(0) >= MAX_ACTIVATIONS {
            return Err("wallet activation cap reached (3,141,592 wallets)".to_string());
        }

        // Load or create target account
        let mut account = store
            .get_account(address)
            .map_err(|e| format!("storage error: {e}"))?
            .unwrap_or_else(|| Account::new(*address));

        // Mint locked balance (non-transferable, gas fees only)
        account.state.locked_balance = account
            .state
            .locked_balance
            .checked_add(locked_grant)
            .ok_or("locked balance overflow")?;

        // Persist account + mark activated
        store
            .put_account(&account)
            .map_err(|e| format!("storage error: {e}"))?;
        store
            .mark_wallet_activated(address)
            .map_err(|e| format!("storage error: {e}"))?;

        // Update executor cache
        self.executor.set_account(*address, account.state);

        info!(
            %address,
            locked_amount = locked_grant,
            "wallet activated with minted locked PI grant"
        );

        Ok(locked_grant)
    }

    fn scan_all_launches(&self) -> Vec<pichain_types::TokenLaunch> {
        let in_mem = self.executor.launchpad_executor().all_launches();
        if !in_mem.is_empty() {
            return in_mem.into_values().collect();
        }
        let store = self.store.read();
        pichain_storage::LaunchpadStore::new(store.db())
            .scan_all_launches()
            .unwrap_or_default()
    }

    fn scan_all_mints(&self) -> Vec<pichain_types::TokenMint> {
        let in_mem = self.executor.token_executor().all_mints();
        if !in_mem.is_empty() {
            return in_mem.into_values().collect();
        }
        let store = self.store.read();
        pichain_storage::TokenStore::new(store.db())
            .scan_all_mints()
            .unwrap_or_default()
    }

    fn scan_all_pools(&self) -> Vec<pichain_types::LiquidityPool> {
        let in_mem = self.executor.dex_executor().all_pools();
        if !in_mem.is_empty() {
            return in_mem.into_values().collect();
        }
        let store = self.store.read();
        pichain_storage::DexStore::new(store.db())
            .scan_all_pools()
            .unwrap_or_default()
    }

    fn get_pool_trades(
        &self,
        pool_id: &pichain_types::PoolId,
        limit: usize,
        before_ms: Option<u64>,
    ) -> Vec<pichain_storage::TradeRecord> {
        let store = self.store.read();
        pichain_storage::DexStore::new(store.db())
            .get_pool_trades(pool_id, limit, before_ms)
            .unwrap_or_default()
    }

    fn get_recent_trades(&self, limit: usize) -> Vec<pichain_storage::TradeRecord> {
        let store = self.store.read();
        pichain_storage::DexStore::new(store.db())
            .get_recent_trades(limit)
            .unwrap_or_default()
    }

    fn get_pool_trades_in_range(
        &self,
        pool_id: &pichain_types::PoolId,
        from_ms: u64,
        to_ms: u64,
    ) -> Vec<pichain_storage::TradeRecord> {
        let store = self.store.read();
        pichain_storage::DexStore::new(store.db())
            .get_pool_trades_in_range(pool_id, from_ms, to_ms)
            .unwrap_or_default()
    }

    fn scan_all_token_accounts(&self) -> Vec<pichain_types::TokenAccount> {
        let store = self.store.read();
        pichain_storage::TokenStore::new(store.db())
            .scan_all_token_accounts()
            .unwrap_or_default()
    }

    fn scan_all_lp_balances(
        &self,
    ) -> Vec<(pichain_types::PoolId, pichain_crypto::keys::Address, u64)> {
        let store = self.store.read();
        pichain_storage::DexStore::new(store.db())
            .scan_all_lp_balances()
            .unwrap_or_default()
    }

    fn post_comment(&self, comment: pichain_storage::Comment) -> Result<(), String> {
        let store = self.store.read();
        pichain_storage::CommentStore::new(store.db())
            .put_comment(&comment)
            .map_err(|e| e.to_string())
    }

    fn get_comments(
        &self,
        mint_id: &pichain_types::MintId,
        limit: usize,
    ) -> Vec<pichain_storage::Comment> {
        let store = self.store.read();
        pichain_storage::CommentStore::new(store.db())
            .get_comments(mint_id, limit)
            .unwrap_or_default()
    }

    fn get_comment_count(&self, mint_id: &pichain_types::MintId) -> u64 {
        let store = self.store.read();
        pichain_storage::CommentStore::new(store.db())
            .get_comment_count(mint_id)
            .unwrap_or(0)
    }

    fn get_claimable_dividends(
        &self,
        mint_id: &pichain_types::MintId,
        holder: &pichain_crypto::keys::Address,
    ) -> u64 {
        let balance = self
            .get_token_account(holder, mint_id)
            .map(|a| a.balance)
            .unwrap_or(0);
        if balance == 0 {
            return 0;
        }
        let store = self.store.read();
        pichain_storage::DividendStore::new(store.db())
            .claimable(mint_id, holder, balance)
            .unwrap_or(0)
    }

    fn get_dividend_pool(
        &self,
        mint_id: &pichain_types::MintId,
    ) -> Option<pichain_storage::DividendPool> {
        let store = self.store.read();
        pichain_storage::DividendStore::new(store.db())
            .get_pool(mint_id)
            .ok()
            .flatten()
    }

    fn claim_dividends(
        &self,
        mint_id: &pichain_types::MintId,
        holder: &pichain_crypto::keys::Address,
    ) -> Result<u64, String> {
        let balance = self
            .get_token_account(holder, mint_id)
            .map(|a| a.balance)
            .unwrap_or(0);
        if balance == 0 {
            return Err("no token balance".to_string());
        }

        let store = self.store.read();
        let div_store = pichain_storage::DividendStore::new(store.db());
        let pool = div_store
            .get_pool(mint_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no dividend pool for this token".to_string())?;

        let debt = div_store
            .get_reward_debt(mint_id, holder)
            .map_err(|e| e.to_string())?;
        let entitled = (balance as u128)
            .checked_mul(pool.reward_per_share_x1e12)
            .unwrap_or(0)
            / 1_000_000_000_000;
        let claimable = entitled.saturating_sub(debt) as u64;

        if claimable == 0 {
            return Err("nothing to claim".to_string());
        }

        // Update reward debt
        div_store
            .set_reward_debt(mint_id, holder, entitled)
            .map_err(|e| e.to_string())?;

        // Update pool claimed total
        let mut updated_pool = pool;
        updated_pool.total_claimed = updated_pool.total_claimed.saturating_add(claimable);
        div_store
            .put_pool(&updated_pool)
            .map_err(|e| e.to_string())?;

        // Credit PI to the holder's account
        self.executor.credit_account(*holder, claimable);

        Ok(claimable)
    }

    fn get_launch_by_mint(
        &self,
        mint: &pichain_types::MintId,
    ) -> Option<pichain_types::TokenLaunch> {
        if let Some(launch) = self.executor.launchpad_executor().get_launch_by_mint(mint) {
            return Some(launch);
        }
        let store = self.store.read();
        let id = pichain_types::LaunchId::from_mint(mint);
        pichain_storage::LaunchpadStore::new(store.db())
            .get_launch(&id)
            .ok()
            .flatten()
    }

    fn get_mint_nonce(&self, address: &Address) -> u64 {
        self.executor.token_executor().get_mint_nonce(address)
    }

    fn get_token_balances_for_owner(
        &self,
        owner: &Address,
    ) -> Vec<(pichain_types::MintId, pichain_types::TokenAccount)> {
        let store = self.store.read();
        let accounts = pichain_storage::TokenStore::new(store.db())
            .scan_all_token_accounts()
            .unwrap_or_default();
        accounts
            .into_iter()
            .filter(|a| a.owner == *owner && a.balance > 0)
            .map(|a| (a.mint, a))
            .collect()
    }

    fn bridge_mint(
        &self,
        mint_symbol: &str,
        recipient: &Address,
        amount: u64,
    ) -> Result<(), String> {
        use pichain_types::token::{token_account_key, MintId, TokenAccount};

        let bridge_operator = GenesisConfig::devnet_bridge_operator_address();

        // Map symbol to mint ID
        let mint_id = match mint_symbol.to_uppercase().as_str() {
            "WETH" => MintId::derive(&bridge_operator, 0),
            "WSOL" => MintId::derive(&bridge_operator, 1),
            "WBTC" => MintId::derive(&bridge_operator, 2),
            "WUSDT" => MintId::derive(&bridge_operator, 3),
            _ => return Err(format!("unknown token symbol: {mint_symbol}")),
        };

        // Verify mint exists and bridge operator is authority
        let mut mint = self
            .executor
            .token_executor()
            .get_mint(&mint_id)
            .ok_or_else(|| {
                format!(
                    "mint {} not found — bridge tokens not bootstrapped",
                    mint_symbol
                )
            })?;
        if mint.mint_authority != Some(bridge_operator) {
            return Err("bridge operator is not mint authority".to_string());
        }
        mint.total_supply = mint
            .total_supply
            .checked_add(amount)
            .ok_or("total supply overflow")?;

        // Prepare token account update
        let key = token_account_key(recipient, &mint_id);
        let mut acct = self
            .executor
            .token_executor()
            .get_token_account(recipient, &mint_id)
            .unwrap_or(TokenAccount {
                key,
                owner: *recipient,
                mint: mint_id,
                balance: 0,
                delegate: None,
                delegate_amount: 0,
                frozen: false,
            });
        acct.balance = acct
            .balance
            .checked_add(amount)
            .ok_or("token balance overflow")?;

        // Write to storage
        let store = self.store.read();
        let db = store.db();
        let token_store = pichain_storage::TokenStore::new(db);
        token_store
            .put_mint(&mint)
            .map_err(|e| format!("storage error: {e}"))?;
        token_store
            .put_token_account(&acct)
            .map_err(|e| format!("storage error: {e}"))?;

        // Update executor caches
        self.executor.token_executor().load_mint(mint);
        self.executor.token_executor().load_token_account(acct);

        info!(
            symbol = %mint_symbol,
            recipient = %recipient,
            amount,
            "bridge minted wrapped tokens"
        );

        Ok(())
    }

    fn bridge_register_addresses(
        &self,
        eth: &str,
        sol: &str,
        btc: &str,
        usdt: &str,
    ) -> Result<(), String> {
        let mut addrs = self.bridge_addresses.write();
        addrs.eth = eth.to_string();
        addrs.sol = sol.to_string();
        addrs.btc = btc.to_string();
        addrs.usdt = usdt.to_string();
        Ok(())
    }

    fn bridge_get_addresses(&self) -> Option<pichain_rpc::BridgeAddressesInfo> {
        let addrs = self.bridge_addresses.read();
        if addrs.eth.is_empty() && addrs.sol.is_empty() && addrs.btc.is_empty() {
            return None;
        }
        Some(pichain_rpc::BridgeAddressesInfo {
            eth: addrs.eth.clone(),
            sol: addrs.sol.clone(),
            btc: addrs.btc.clone(),
            usdt: addrs.usdt.clone(),
        })
    }

    fn bridge_status(&self) -> pichain_rpc::BridgeStatusInfo {
        use pichain_types::token::MintId;

        let bridge_operator = GenesisConfig::devnet_bridge_operator_address();
        let symbols = [("WETH", 0u64), ("WSOL", 1), ("WBTC", 2), ("WUSDT", 3)];

        let mut tokens = Vec::new();
        for (sym, nonce) in &symbols {
            let mint_id = MintId::derive(&bridge_operator, *nonce);
            if let Some(mint) = self.executor.token_executor().get_mint(&mint_id) {
                tokens.push(pichain_rpc::BridgeTokenStatus {
                    symbol: sym.to_string(),
                    total_supply: mint.total_supply,
                    decimals: mint.decimals,
                });
            }
        }

        let total_transfers = self.bridge_transfers.read().len();

        pichain_rpc::BridgeStatusInfo {
            tokens,
            total_transfers,
        }
    }

    fn bridge_get_transfers(
        &self,
        chain: Option<&str>,
        limit: usize,
    ) -> Vec<pichain_rpc::BridgeTransferInfo> {
        let transfers = self.bridge_transfers.read();
        transfers
            .iter()
            .rev()
            .filter(|t| chain.is_none_or(|c| t.chain == c))
            .take(limit)
            .map(|t| pichain_rpc::BridgeTransferInfo {
                chain: t.chain.clone(),
                tx_hash: t.tx_hash.clone(),
                symbol: t.symbol.clone(),
                recipient: t.recipient.clone(),
                amount: t.amount,
                timestamp: t.timestamp,
            })
            .collect()
    }

    fn bridge_register_intent(
        &self,
        chain: &str,
        external_address: &str,
        pichain_address: &str,
    ) -> Result<(), String> {
        let key = format!("{}:{}", chain, external_address.to_lowercase());
        let mut intents = self.deposit_intents.write();
        intents.insert(key, pichain_address.to_lowercase());
        Ok(())
    }

    fn bridge_get_intent(&self, chain: &str, external_address: &str) -> Option<String> {
        let key = format!("{}:{}", chain, external_address.to_lowercase());
        let intents = self.deposit_intents.read();
        intents.get(&key).cloned()
    }

    fn bridge_record_transfer(
        &self,
        chain: &str,
        tx_hash: &str,
        symbol: &str,
        recipient: &str,
        amount: u64,
    ) {
        let record = BridgeTransferRecord {
            chain: chain.to_string(),
            tx_hash: tx_hash.to_string(),
            symbol: symbol.to_string(),
            recipient: recipient.to_string(),
            amount,
            timestamp: chrono::Utc::now().timestamp(),
        };
        let mut transfers = self.bridge_transfers.write();
        transfers.push(record);
        // Keep at most 10000 records in memory
        let len = transfers.len();
        if len > 10_000 {
            transfers.drain(..len - 10_000);
        }
    }

    // --- Staking Queries ---

    fn get_validators(&self) -> Vec<pichain_rpc::ValidatorInfo> {
        let guard = match self.staking.try_read() {
            Ok(g) => g,
            Err(_) => return vec![],
        };
        guard
            .all_validators()
            .into_iter()
            .map(|v| {
                let total_slots = v.blocks_proposed.saturating_add(v.blocks_missed);
                let uptime_bps = if total_slots > 0 {
                    ((v.blocks_proposed as u128 * 10_000) / total_slots as u128) as u16
                } else {
                    10_000 // 100% uptime if no slots yet
                };
                pichain_rpc::ValidatorInfo {
                    address: v.validator.to_string(),
                    stake: v.self_stake,
                    delegated: v.delegated_stake,
                    commission_bps: v.commission_bps,
                    active: v.active && !v.jailed,
                    uptime_bps,
                    blocks_proposed: v.blocks_proposed,
                }
            })
            .collect()
    }

    fn get_delegations(&self, address: &Address) -> Vec<pichain_rpc::DelegationInfo> {
        let guard = match self.staking.try_read() {
            Ok(g) => g,
            Err(_) => return vec![],
        };
        guard
            .delegations_for(address)
            .into_iter()
            .map(|d| pichain_rpc::DelegationInfo {
                validator: d.validator.to_string(),
                amount: d.amount,
                rewards_earned: d.pending_rewards,
            })
            .collect()
    }

    fn get_staking_rewards(&self, address: &Address) -> u64 {
        let guard = match self.staking.try_read() {
            Ok(g) => g,
            Err(_) => return 0,
        };
        // Sum rewards from delegations where this address is the delegator
        let delegation_rewards: u64 = guard
            .delegations_for(address)
            .iter()
            .map(|d| d.pending_rewards)
            .sum();
        // Add any pending rewards from the validator entry itself (if this address is a validator)
        let validator_rewards: u64 = guard
            .all_validators()
            .iter()
            .filter(|v| v.validator == *address)
            .map(|v| v.pending_rewards)
            .sum();
        delegation_rewards.saturating_add(validator_rewards)
    }

    // --- Transaction History ---

    fn get_address_transactions(
        &self,
        address: &Address,
        before_height: Option<u64>,
        limit: usize,
    ) -> Vec<TxHistoryEntry> {
        let store = self.store.read();
        match store
            .db()
            .get_address_transactions(&address.0, before_height, limit)
        {
            Ok(entries) => entries
                .into_iter()
                .map(|(tx_hash, height, tx_index)| TxHistoryEntry {
                    tx_hash: hex::encode(tx_hash),
                    height,
                    tx_index,
                })
                .collect(),
            Err(e) => {
                warn!("Failed to query address transactions: {}", e);
                vec![]
            }
        }
    }

    // --- Event Queries ---

    fn query_events_by_topic(&self, topic: &[u8; 32], limit: usize) -> Vec<TxHistoryEntry> {
        let store = self.store.read();
        match store.db().query_events_by_topic(topic, limit) {
            Ok(entries) => entries
                .into_iter()
                .map(|(tx_hash, height, tx_index)| TxHistoryEntry {
                    tx_hash: hex::encode(tx_hash),
                    height,
                    tx_index,
                })
                .collect(),
            Err(e) => {
                warn!("Failed to query events by topic: {}", e);
                vec![]
            }
        }
    }

    fn query_events_by_address(&self, address: &Address, limit: usize) -> Vec<TxHistoryEntry> {
        let store = self.store.read();
        match store.db().query_events_by_address(&address.0, limit) {
            Ok(entries) => entries
                .into_iter()
                .map(|(tx_hash, height, tx_index)| TxHistoryEntry {
                    tx_hash: hex::encode(tx_hash),
                    height,
                    tx_index,
                })
                .collect(),
            Err(e) => {
                warn!("Failed to query events by address: {}", e);
                vec![]
            }
        }
    }

    // --- NFT Queries ---

    fn scan_all_collections(&self) -> Vec<pichain_types::NftCollection> {
        let in_mem = self.executor.nft_executor().all_collections();
        if !in_mem.is_empty() {
            return in_mem.into_values().collect();
        }
        let store = self.store.read();
        pichain_storage::NftStore::new(store.db())
            .scan_all_collections()
            .unwrap_or_default()
    }

    fn get_collection_items(
        &self,
        collection_id: &pichain_types::CollectionId,
    ) -> Vec<pichain_types::Nft> {
        // Try in-memory first
        let in_mem = self.executor.nft_executor().all_nfts();
        if !in_mem.is_empty() {
            return in_mem
                .into_values()
                .filter(|n| n.collection == *collection_id)
                .collect();
        }
        // Fall back to storage
        let store = self.store.read();
        pichain_storage::NftStore::new(store.db())
            .scan_all_nfts()
            .unwrap_or_default()
            .into_iter()
            .filter(|n| n.collection == *collection_id)
            .collect()
    }

    fn get_nfts_by_owner(&self, owner: &Address) -> Vec<pichain_types::Nft> {
        // Try in-memory first
        let in_mem = self.executor.nft_executor().all_nfts();
        if !in_mem.is_empty() {
            return in_mem.into_values().filter(|n| n.owner == *owner).collect();
        }
        // Fall back to storage
        let store = self.store.read();
        pichain_storage::NftStore::new(store.db())
            .scan_all_nfts()
            .unwrap_or_default()
            .into_iter()
            .filter(|n| n.owner == *owner)
            .collect()
    }

    // --- Richlist ---

    fn get_richlist(&self, limit: usize) -> Vec<(Address, u64)> {
        let store = self.store.read();
        let entries = match store.db().scan_state_prefix(b"a") {
            Ok(e) => e,
            Err(_) => return vec![],
        };
        let mut accounts: Vec<(Address, u64)> = entries
            .into_iter()
            .filter_map(|(key_suffix, value)| {
                if key_suffix.len() < 20 {
                    return None;
                }
                let mut addr_bytes = [0u8; 20];
                addr_bytes.copy_from_slice(&key_suffix[..20]);
                let address = Address(addr_bytes);
                let state: pichain_types::account::AccountState =
                    serde_json::from_slice(&value).ok()?;
                Some((address, state.balance))
            })
            .filter(|(_, balance)| *balance > 0)
            .collect();
        accounts.sort_by(|a, b| b.1.cmp(&a.1));
        accounts.truncate(limit);
        accounts
    }

    // --- Betting queries ---

    fn get_betting_match(
        &self,
        match_id: &pichain_types::betting::MatchId,
    ) -> Option<pichain_types::betting::BettingMatch> {
        self.executor.betting_executor().get_match(match_id)
    }

    fn get_active_matches(&self) -> Vec<pichain_types::betting::BettingMatch> {
        self.executor.betting_executor().active_matches()
    }

    fn get_matches_by_category(&self, category: u8) -> Vec<pichain_types::betting::BettingMatch> {
        let cat = match pichain_types::betting::GameCategory::from_tag(category) {
            Some(c) => c,
            None => return vec![],
        };
        self.executor.betting_executor().matches_by_category(&cat)
    }

    fn get_matches_by_player(
        &self,
        address: &Address,
    ) -> Vec<pichain_types::betting::BettingMatch> {
        self.executor.betting_executor().matches_by_player(address)
    }

    fn get_betting_stats(&self) -> pichain_rpc::BettingStatsData {
        let executor = self.executor.betting_executor();
        let all = executor.all_matches();
        let mut total_players = std::collections::HashSet::new();
        for m in all.values() {
            for p in &m.participants {
                total_players.insert(*p);
            }
        }
        pichain_rpc::BettingStatsData {
            total_wagered: executor.total_wagered(),
            active_matches: executor.active_count(),
            total_matches: all.len() as u64,
            total_players: total_players.len() as u64,
            house_burns: executor.total_wagered() * 314 / 10_000, // estimated from total wagered × house fee rate
        }
    }

    fn get_match_nonce(&self, address: &Address) -> u64 {
        self.executor
            .betting_executor()
            .all_nonces()
            .get(address)
            .copied()
            .unwrap_or(0)
    }
}
