//! Shared node state — the central coordinator for all PIChain subsystems.
//!
//! `NodeState` holds Arc references to all components so they can be shared
//! across async tasks (block producer, RPC server, networking, etc).
//!
//! Storage and block hash use `parking_lot::RwLock` (non-poisoning) for
//! synchronous access from the RPC layer. Staking and mining use
//! `tokio::sync::RwLock` since they are only accessed from async block
//! processing.

use pichain_consensus::StakingManager;
use pichain_crypto::ed25519::Address;
use pichain_crypto::Hash;
use pichain_execution::{ProducedBlock, TransactionExecutor, TransactionPool};
use pichain_rpc::StateProvider;
use pichain_storage::StateStore;
use pichain_types::account::Account;
use pichain_types::genesis::GenesisConfig;
use pichain_types::{Block, PiAmount};
use parking_lot::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{debug, error, info};

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
                if alloc.virtual_pool { continue; } // skip virtual pools
                if let Some(account) = store.get_account(&alloc.address)? {
                    self.executor.set_account(alloc.address, account.state);
                } else {
                    // Account not in storage yet — apply it
                    let account = Account::with_balance(alloc.address, alloc.amount);
                    self.executor.set_account(alloc.address, account.state.clone());
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
                if alloc.virtual_pool { continue; } // skip virtual pools
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
            self.executor.set_account(alloc.address, account.state.clone());

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
        let height;
        {
            let mut store = self.store.write();

            height = store.latest_height()?;
            if height == 0 {
                if store.get_block(0)?.is_some() {
                    // Even at genesis, rebuild JMT so state root is correct
                    let jmt_count = store.rebuild_jmt()?;
                    if jmt_count > 0 {
                        info!(accounts = jmt_count, "JMT rebuilt from genesis state");
                    }
                    info!("Chain at genesis, nothing to resume");
                }
                return Ok(());
            }

            // Rebuild the in-memory JMT from persisted account state FIRST
            // so that state_root() returns the correct value.
            let jmt_count = store.rebuild_jmt()?;

            // Load the latest block to get parent hash and base fee
            let last_block = store.get_block(height)?.ok_or_else(|| {
                anyhow::anyhow!("block at height {height} not found but latest_height says {height}")
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
            self.last_block_timestamp_ms.store(last_ts, Ordering::SeqCst);

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
                info!(staked_accounts = count, "pre-loaded staked accounts for anti-concentration tracking");
            }
        }
        // Rebuild staking concentration totals from loaded account state.
        self.executor.rebuild_staking_totals();

        Ok(())
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
            self.executor.dex_executor().load_lp_balance(pool_id, owner, balance);
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
                let addr = pichain_crypto::ed25519::Address(addr_bytes);
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
                let addr = pichain_crypto::ed25519::Address(addr_bytes);
                let nonce = u64::from_le_bytes(value[..8].try_into().unwrap());
                self.executor.nft_executor().load_collection_nonce(addr, nonce);
            }
        }

        if mint_count > 0 || account_count > 0 || pool_count > 0 || collection_count > 0
            || nft_count > 0 || launch_count > 0 || contract_count > 0
        {
            info!(
                mints = mint_count,
                token_accounts = account_count,
                pools = pool_count,
                lp_balances = lp_count,
                collections = collection_count,
                nfts = nft_count,
                launches = launch_count,
                contract_entries = contract_count,
                mint_nonces = mint_nonce_entries.len(),
                collection_nonces = coll_nonce_entries.len(),
                "sub-executor state reloaded from storage"
            );
        }

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
                debug!(genesis_timestamp_ms = genesis_ts, "set genesis timestamp for mining replay");
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
    pub async fn persist_block(&self, produced: &ProducedBlock) -> anyhow::Result<pichain_crypto::poseidon::PoseidonHash> {
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
                &txs_and_receipts.iter()
                    .map(|(h, tx, r)| (*h, *tx, r.as_ref().map(|x| *x)))
                    .collect::<Vec<_>>(),
                &state_changes.iter()
                    .map(|(a, s)| (*a, *s))
                    .collect::<Vec<_>>(),
                &[], // H6: object changes (currently added in Phase 2 via sub-executor batch_put_*)
            )?;

            // Phase 2: Add sub-executor state to the same batch (atomic with block)
            let sub_state = self.executor.snapshot_sub_state();
            let db = store.db();

            // Token mints + accounts
            let token_store = pichain_storage::TokenStore::new(db);
            for (_, mint) in &sub_state.mints {
                token_store.batch_put_mint(&mut batch, mint)?;
            }
            for (_, account) in &sub_state.token_accounts {
                token_store.batch_put_token_account(&mut batch, account)?;
            }

            // DEX pools + LP balances
            let dex_store = pichain_storage::DexStore::new(db);
            for (_, pool) in &sub_state.pools {
                dex_store.batch_put_pool(&mut batch, pool)?;
            }
            for ((pool_id, address), balance) in &sub_state.lp_balances {
                dex_store.batch_put_lp_balance(&mut batch, pool_id, address, *balance)?;
            }

            // NFT collections + NFTs
            let nft_store = pichain_storage::NftStore::new(db);
            for (_, collection) in &sub_state.collections {
                nft_store.batch_put_collection(&mut batch, collection)?;
            }
            for (_, nft) in &sub_state.nfts {
                nft_store.batch_put_nft(&mut batch, nft)?;
            }

            // Launchpad launches
            let launchpad_store = pichain_storage::LaunchpadStore::new(db);
            for (_, launch) in &sub_state.launches {
                launchpad_store.batch_put_launch(&mut batch, launch)?;
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
            let new_total_burned = self.total_burned.load(Ordering::SeqCst)
                .saturating_add(produced.total_burned);
            let new_total_minted = self.total_minted.load(Ordering::SeqCst)
                .saturating_add(produced.total_minted);

            // R26-FIX: Supply invariant check BEFORE commit. If total minted would
            // exceed the mining pool, reject the block rather than persisting an
            // invalid state. Previously this check ran after commit, making it a
            // detection-only log that couldn't prevent the violation.
            let mining_pool_128 = pichain_types::TOTAL_SUPPLY * 40 / 100;
            let mining_pool_base = u64::try_from(mining_pool_128).unwrap_or(u64::MAX);
            if new_total_minted > mining_pool_base {
                error!(
                    total_minted = new_total_minted,
                    mining_pool = mining_pool_base,
                    "SUPPLY INVARIANT VIOLATION: block would exceed mining pool — rejecting"
                );
                return Err(anyhow::anyhow!(
                    "supply invariant violation: total_minted {} > mining_pool {}", new_total_minted, mining_pool_base
                ));
            }

            db.batch_put_metadata(&mut batch, b"total_burned", &new_total_burned.to_le_bytes());
            db.batch_put_metadata(&mut batch, b"total_minted", &new_total_minted.to_le_bytes());

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
            computed_state_root
        }; // Release store lock before acquiring staking lock

        // Distribute staking rewards (65% of fees go to stakers)
        let fee_calculator = pichain_execution::FeeCalculator::new();
        let total_fees: u64 = produced.execution_results.iter().map(|r| {
            fee_calculator.calculate_fee(r.effect.gas_used, r.effect.base_fee, 0)
        }).fold(0u64, |acc, v| acc.saturating_add(v));
        let staker_reward_u128 = total_fees as u128 * pichain_types::FEE_STAKER_RATE_BPS as u128 / 10_000;
        let staker_reward = u64::try_from(staker_reward_u128).unwrap_or_else(|_| {
            error!(staker_reward_u128, "staker reward exceeds u64 — clamping to u64::MAX");
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
        self.mempool.insert(tx).map(|_| ()).map_err(|e| e.to_string())
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
        let execution_results = self.executor.execute_block(
            &block.transactions,
            block.header.base_fee,
        );

        // Verify execution produces consistent results with the block header
        let computed_gas: u64 = execution_results.iter()
            .map(|r| r.effect.gas_used)
            .fold(0u64, |acc, v| acc.saturating_add(v));

        // Allow up to 10% gas discrepancy (proposer may have trimmed differently)
        // but reject blocks where gas_used diverges beyond tolerance (bidirectional)
        let tolerance = block.header.gas_used / 10; // 10%
        if computed_gas > block.header.gas_used.saturating_add(tolerance).saturating_add(21_000) {
            return Err(anyhow::anyhow!(
                "peer block gas mismatch (over): computed {computed_gas}, header says {}",
                block.header.gas_used
            ));
        }
        if block.header.gas_used > computed_gas.saturating_add(tolerance).saturating_add(21_000) {
            return Err(anyhow::anyhow!(
                "peer block gas mismatch (under): computed {computed_gas}, header says {}",
                block.header.gas_used
            ));
        }

        // Build a ProducedBlock for persistence
        let total_burned: u64 = execution_results.iter()
            .map(|r| r.pi_burned)
            .fold(0u64, |acc, v| acc.saturating_add(v));
        let total_minted: u64 = execution_results.iter()
            .map(|r| r.pi_minted)
            .fold(0u64, |acc, v| acc.saturating_add(v));
        let proposer_reward: u64 = execution_results.iter()
            .map(|r| r.proposer_reward)
            .fold(0u64, |acc, v| acc.saturating_add(v));
        let total_miner_fee: u64 = execution_results.iter()
            .map(|r| r.miner_fee)
            .fold(0u64, |acc, v| acc.saturating_add(v));

        // R36-FIX: Credit the proposer with priority fee + staker share rewards,
        // matching the block producer path. Without this, follower nodes have a
        // lower balance for the proposer, causing state root divergence.
        if proposer_reward > 0 {
            self.executor.credit_account(block.header.proposer, proposer_reward);
        }

        // Feed miner fees back into the mining pool (matching block producer path)
        if total_miner_fee > 0 {
            self.executor.mining_processor().lock().add_fee_income(total_miner_fee);
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
                height, claimed_state_root, local_state_root
            ));
        }

        // R36-FIX: Evict confirmed transactions from the mempool, matching the
        // block producer path. Without this, stale txs accumulate and get
        // re-proposed if this follower becomes the next block producer.
        {
            let tx_hashes: Vec<pichain_crypto::Hash> = block.transactions.iter().map(|tx| tx.hash()).collect();
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

        self.mempool.insert(tx).map(|_| ()).map_err(|e| e.to_string())
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
        if let Some(account) = self.executor.token_executor().get_token_account(owner, mint) {
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
        self.executor.dex_executor().get_pool_by_mints(mint_a, mint_b)
    }

    fn get_swap_quote(
        &self,
        mint_in: &pichain_types::MintId,
        mint_out: &pichain_types::MintId,
        amount_in: u64,
    ) -> Option<pichain_rpc::SwapQuote> {
        let pool = self.executor.dex_executor().get_pool_by_mints(mint_in, mint_out)?;
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
        })
    }

    // WARNING: Faucet claims bypass the block execution pipeline and directly modify storage.
    // This means: (1) faucet state changes are not included in any block, (2) state root
    // will diverge from peers in multi-node mode. The faucet should ONLY be used on devnet/testnet.
    // For mainnet, the faucet must be replaced with a system transaction injected into the mempool.
    fn faucet_claim(&self, address: &Address) -> Result<u64, String> {
        // Reject faucet claims on mainnet (chain_id 314159) unconditionally
        if self.chain_id == 314159 {
            return Err("faucet is disabled on mainnet".to_string());
        }

        // Only available on devnet
        if self.chain_id != 31415 {
            return Err("faucet only available on devnet".to_string());
        }

        // Amount: 10 PI
        let faucet_amount: u64 = 10 * pichain_types::BASE_UNITS_PER_PI;

        // Check if address already has a balance (prevent double-claim)
        {
            let store = self.store.read();
            if let Ok(Some(account)) = store.get_account(address) {
                if account.state.balance > 0 {
                    return Err("address already has a balance".to_string());
                }
            }
        }

        // Debit faucet, credit target under write lock
        let faucet_addr = GenesisConfig::devnet_faucet_address();
        let mut store = self.store.write();

        // Re-read faucet under write lock to avoid TOCTOU
        let mut faucet_acct = store
            .get_account(&faucet_addr)
            .map_err(|e| format!("storage error: {e}"))?
            .ok_or("faucet account not found in storage")?;

        if faucet_acct.state.balance < faucet_amount {
            return Err("faucet depleted".to_string());
        }

        // Re-read target under write lock
        if let Ok(Some(existing)) = store.get_account(address) {
            if existing.state.balance > 0 {
                return Err("address already has a balance".to_string());
            }
        }

        faucet_acct.state.balance = faucet_acct.state.balance.checked_sub(faucet_amount)
            .ok_or("faucet balance underflow — should be impossible after check")?;
        let target_account = Account::with_balance(*address, faucet_amount);

        // Atomic write: debit faucet + credit target in a single synced batch
        // to prevent PI loss if the node crashes between the two writes.
        store.put_accounts_atomic(&[&faucet_acct, &target_account])
            .map_err(|e| format!("storage error: {e}"))?;

        // Update executor cache so next block execution sees the new balances
        self.executor.set_account(*address, target_account.state);
        self.executor.set_account(faucet_addr, faucet_acct.state);

        info!(
            %address,
            amount = faucet_amount,
            "devnet faucet claim processed"
        );

        Ok(faucet_amount)
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

    fn get_launch_by_mint(&self, mint: &pichain_types::MintId) -> Option<pichain_types::TokenLaunch> {
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
}
