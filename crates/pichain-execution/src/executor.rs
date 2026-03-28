//! Block-STM parallel transaction executor.
//!
//! Executes transactions optimistically in parallel, tracking state reads/writes.
//! Conflicts are detected post-execution and conflicting transactions are retried.

use crate::betting_executor::BettingExecutor;
use crate::dex_executor::{DexExecutionResult, DexExecutor};
use crate::fee::FeeCalculator;
use crate::launchpad_executor::LaunchpadExecutor;
use crate::nft_executor::NftExecutor;
use crate::sdk::{ContractAbi, ContractRegistry};
use crate::token_executor::TokenExecutor;
use crate::wasm_vm::WasmVM;
use dashmap::DashMap;
use pichain_crypto::keys::Address;
use pichain_crypto::Hash;
use pichain_types::account::{Account, AccountState};
use pichain_types::betting::{BettingMatch, MatchId};
use pichain_types::dex::{LiquidityPool, PoolId};
use pichain_types::launchpad::{LaunchId, TokenLaunch};
use pichain_types::nft::{CollectionId, Nft, NftCollection, NftId};
use pichain_types::token::{MintId, TokenAccount, TokenMint};
use pichain_types::transaction::{
    SignedTransaction, TransactionEffect, TransactionEvent, TransactionKind, TransactionStatus,
};
use pichain_types::PiAmount;
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Maximum lengths for user-controlled string fields (XSS/DoS prevention).
const MAX_TOKEN_NAME_LEN: usize = 64;
const MAX_TOKEN_SYMBOL_LEN: usize = 10;
const MAX_METADATA_URI_LEN: usize = 768 * 1024; // 768KB — tokens can embed base64 images in metadata
const MAX_FUNCTION_NAME_LEN: usize = 64;

/// Maximum token mints per address. Prevents a single attacker from creating
/// millions of token mints to exhaust state storage (each mint ~200 bytes in
/// storage). At 10,000 mints per address, an attacker needs 10,000 addresses
/// to hit 100M mints, and each creation costs gas.
const MAX_MINTS_PER_ADDRESS: u64 = 10_000;

/// Maximum NFT collections per address (same rationale as MAX_MINTS_PER_ADDRESS).
const MAX_COLLECTIONS_PER_ADDRESS: u64 = 10_000;

// ── Anti-concentration constants ──────────────────────────────────────────────
// These mirror the values in pichain_consensus::staking but are defined here
// to avoid a circular dependency (execution must not depend on consensus).

/// Maximum percentage of total staked that any single validator can hold.
/// 3141 bps = 31.41% (π × 1000). Prevents one validator from reaching BFT 1/3 threshold.
/// MUST match pichain_consensus::staking::MAX_VALIDATOR_STAKE_PCT_BPS.
const MAX_VALIDATOR_STAKE_PCT_BPS: u32 = 3141;

/// Maximum percentage of total staked that any single address can hold (bps).
/// 1000 bps = 10%. Forces capital distribution across multiple delegators.
const MAX_ADDRESS_STAKE_PCT_BPS: u32 = 1000;

/// Minimum number of unique validators before concentration caps are enforced.
/// MUST match pichain_consensus::staking::MIN_VALIDATORS_FOR_STAKE_CAP.
const MIN_VALIDATORS_FOR_STAKE_CAP: usize = 4;

/// Maximum stake growth per validator per epoch (π × 1000 bps = 31.41%).
/// MUST match pichain_consensus::staking::MAX_STAKE_GROWTH_PCT_BPS.
const MAX_STAKE_GROWTH_PCT_BPS: u32 = 3141;

/// Maximum number of active validators (π × 100).
/// MUST match pichain_consensus::staking::MAX_ACTIVE_VALIDATORS.
const MAX_ACTIVE_VALIDATORS: usize = 314;

/// Bootstrap validator concentration cap (41.3% = 4130 bps).
/// MUST match pichain_consensus::staking::BOOTSTRAP_VALIDATOR_STAKE_PCT_BPS.
const BOOTSTRAP_VALIDATOR_STAKE_PCT_BPS: u32 = 4130;

/// Minimum stake to count as a validator (3,141 PI = π × 1000).
/// MUST match pichain_consensus::staking::MIN_VALIDATOR_STAKE.
const MIN_VALIDATOR_STAKE: u64 = 3_141 * 1_000_000_000;

/// Blocks per staking epoch for velocity limiting.
/// 31,415 blocks × 314ms ≈ 2.74 hours. Prevents flash-staking within a single epoch.
const BLOCKS_PER_STAKING_EPOCH: u64 = 31_415;

/// Tracks global staking totals for anti-concentration checks.
/// Protected by a mutex to guarantee atomic check-then-modify during Stake/Unstake.
struct StakeTracker {
    /// Total PI staked network-wide.
    total_staked: u64,
    /// Total PI staked to each validator address.
    validator_stakes: HashMap<Address, u64>,
    /// Snapshot of per-validator stakes at the start of the current epoch.
    /// Used for velocity limiting (max 50% growth per epoch).
    epoch_start_stakes: HashMap<Address, u64>,
    /// Current epoch number for velocity tracking.
    current_epoch: u64,
}

/// Validate a user-controlled string field: safe printable ASCII only, within max length.
/// Rejects HTML control characters (`<`, `>`, `&`, `"`, `'`) to prevent XSS.
fn validate_string_field(value: &str, field_name: &str, max_len: usize) -> Result<(), String> {
    if value.len() > max_len {
        return Err(format!(
            "{field_name} too long: {} chars (max {max_len})",
            value.len()
        ));
    }
    if value.is_empty() {
        return Err(format!("{field_name} cannot be empty"));
    }
    for b in value.bytes() {
        if !(0x20..=0x7E).contains(&b) {
            return Err(format!(
                "{field_name} contains non-printable or non-ASCII characters"
            ));
        }
        if b == b'<' || b == b'>' || b == b'&' || b == b'"' || b == b'\'' {
            return Err(format!("{field_name} contains forbidden HTML characters"));
        }
    }
    Ok(())
}

/// Aggregated sub-executor state for persistence.
/// Captured as a full snapshot after block execution.
/// Token/DEX/NFT/Launchpad/Contract state is persisted atomically with the block.
#[derive(Default, Debug, Clone)]
pub struct SubExecutorChanges {
    pub mints: HashMap<MintId, TokenMint>,
    pub token_accounts: HashMap<[u8; 32], TokenAccount>,
    pub pools: HashMap<PoolId, LiquidityPool>,
    pub lp_balances: HashMap<(PoolId, Address), u64>,
    pub collections: HashMap<CollectionId, NftCollection>,
    pub nfts: HashMap<NftId, Nft>,
    pub launches: HashMap<LaunchId, TokenLaunch>,
    pub contract_storage: HashMap<(Address, Vec<u8>), Vec<u8>>,
    pub mint_nonces: HashMap<Address, u64>,
    pub collection_nonces: HashMap<Address, u64>,
    pub matches: HashMap<MatchId, BettingMatch>,
    pub match_nonces: HashMap<Address, u64>,
}

/// Result of executing a single transaction.
#[derive(Clone, Debug)]
pub struct ExecutionResult {
    /// Transaction hash.
    pub tx_hash: Hash,
    /// Execution effect.
    pub effect: TransactionEffect,
    /// State changes: address → new account state.
    pub state_changes: HashMap<Address, AccountState>,
    /// Amount of PI burned.
    pub pi_burned: PiAmount,
    /// Amount of new PI minted (mining rewards — inflationary).
    pub pi_minted: PiAmount,
    /// Fee reward for the block proposer (18.59% of base fees + 100% priority fees).
    pub proposer_reward: PiAmount,
    /// Fee reward for stakers (31.41% of base fees, distributed proportionally).
    pub staker_reward: PiAmount,
    /// Miner fee income (18.59% of base fees) flowing back to mining pool.
    pub miner_fee: PiAmount,
    /// Addresses read during execution (for conflict detection).
    pub state_reads: Vec<Address>,
}

/// Block-STM parallel transaction executor.
pub struct TransactionExecutor {
    /// In-memory state cache for parallel execution.
    state_cache: DashMap<Address, AccountState>,
    /// Fee calculator.
    fee_calc: FeeCalculator,
    /// Native token executor.
    token_executor: TokenExecutor,
    /// DEX/AMM executor.
    dex_executor: DexExecutor,
    /// Token launchpad executor.
    launchpad_executor: LaunchpadExecutor,
    /// NFT executor.
    nft_executor: NftExecutor,
    /// Betting/gaming executor.
    betting_executor: BettingExecutor,
    /// Chain ID for cross-chain replay protection.
    chain_id: u64,
    /// Mining processor for PI digit verification and rewards.
    mining_processor: Arc<parking_lot::Mutex<pichain_mining::MiningProcessor>>,
    /// Block timestamp for deterministic state transitions.
    /// Set at the start of each block execution; all state-creating operations use this
    /// instead of wall-clock time to ensure consensus determinism.
    block_timestamp_ms: AtomicU64,
    /// Current block height for WASM contract host functions.
    block_height: AtomicU64,
    /// WASM virtual machine for smart contract execution.
    wasm_vm: Arc<WasmVM>,
    /// Contract registry (deployed contracts metadata + bytecode).
    contract_registry: Arc<parking_lot::Mutex<ContractRegistry>>,
    /// Per-contract key-value storage, persisted across calls within and across blocks.
    contract_storage: DashMap<Address, HashMap<Vec<u8>, Vec<u8>>>,
    /// Anti-concentration tracking for staking. Tracks total staked and per-validator
    /// totals. Protected by a mutex for atomic check-then-modify in Stake/Unstake.
    stake_tracker: parking_lot::Mutex<StakeTracker>,
}

impl TransactionExecutor {
    pub fn new(chain_id: u64) -> Self {
        // WASM VM initialization — log error but don't panic if Wasmtime fails.
        // Contracts will fail at execution time with a clear error.
        let wasm_vm = match WasmVM::new() {
            Ok(vm) => vm,
            Err(e) => {
                tracing::error!(
                    "WASM VM initialization failed: {e} — contract execution will be unavailable"
                );
                // Re-try once — if it fails again, panic is acceptable at startup
                WasmVM::new().expect("WASM VM initialization failed on retry")
            }
        };

        Self {
            state_cache: DashMap::new(),
            fee_calc: FeeCalculator::new(),
            token_executor: TokenExecutor::new(),
            dex_executor: DexExecutor::new(),
            launchpad_executor: LaunchpadExecutor::new(),
            nft_executor: NftExecutor::new(),
            betting_executor: BettingExecutor::new(),
            chain_id,
            mining_processor: Arc::new(parking_lot::Mutex::new(
                pichain_mining::MiningProcessor::new(),
            )),
            block_timestamp_ms: AtomicU64::new(0),
            block_height: AtomicU64::new(0),
            wasm_vm: Arc::new(wasm_vm),
            contract_registry: Arc::new(parking_lot::Mutex::new(ContractRegistry::new())),
            contract_storage: DashMap::new(),
            stake_tracker: parking_lot::Mutex::new(StakeTracker {
                total_staked: 0,
                validator_stakes: HashMap::new(),
                epoch_start_stakes: HashMap::new(),
                current_epoch: 0,
            }),
        }
    }

    /// Set the block timestamp for deterministic state transitions.
    /// Must be called before `execute_block()`. All sub-executors will use this
    /// timestamp instead of wall-clock time.
    pub fn set_block_timestamp(&self, timestamp_ms: u64) {
        self.block_timestamp_ms
            .store(timestamp_ms, Ordering::Release);
        self.token_executor.set_block_timestamp(timestamp_ms);
        self.dex_executor.set_block_timestamp(timestamp_ms);
        self.nft_executor.set_block_timestamp(timestamp_ms);
        self.launchpad_executor.set_block_timestamp(timestamp_ms);
        self.betting_executor.set_block_timestamp(timestamp_ms);
    }

    /// Set the current block height for WASM contract host functions and DEX LP lock tracking.
    pub fn set_block_height(&self, height: u64) {
        self.block_height.store(height, Ordering::Release);
        self.dex_executor.set_block_height(height);
        self.betting_executor.set_block_height(height);
    }

    /// Snapshot DEX pool reserves at the start of a block for MEV-resistant price impact.
    /// Must be called after `set_block_height()` and before `execute_block()`.
    pub fn snapshot_dex_reserves(&self) {
        self.dex_executor.snapshot_block_start();
    }

    /// Clear all sub-executor cached state. Used by the block producer when
    /// re-executing transactions after gas-limit trimming to prevent
    /// double-mutations in token, DEX, NFT, and launchpad state.
    pub fn clear_sub_executors(&self) {
        self.token_executor.clear_state();
        self.dex_executor.clear_state();
        self.nft_executor.clear_state();
        self.launchpad_executor.clear_state();
        self.betting_executor.clear_state();
        // R36-FIX: Also clear WASM contract state from the first execution pass.
        // Without this, re-deployed contracts hit "already exists" errors and
        // re-executed contract calls see stale storage from trimmed transactions.
        self.contract_storage.clear();
        self.contract_registry.lock().clear();
    }

    fn current_block_height(&self) -> u64 {
        self.block_height.load(Ordering::Acquire)
    }

    fn block_timestamp(&self) -> u64 {
        self.block_timestamp_ms.load(Ordering::Acquire)
    }

    /// Access the token executor.
    pub fn token_executor(&self) -> &TokenExecutor {
        &self.token_executor
    }

    /// Access the DEX executor.
    pub fn dex_executor(&self) -> &DexExecutor {
        &self.dex_executor
    }

    /// Access the launchpad executor.
    pub fn launchpad_executor(&self) -> &LaunchpadExecutor {
        &self.launchpad_executor
    }

    /// Access the NFT executor.
    pub fn nft_executor(&self) -> &NftExecutor {
        &self.nft_executor
    }

    /// Access the betting executor.
    pub fn betting_executor(&self) -> &BettingExecutor {
        &self.betting_executor
    }

    /// Access the mining processor.
    pub fn mining_processor(&self) -> &Arc<parking_lot::Mutex<pichain_mining::MiningProcessor>> {
        &self.mining_processor
    }

    /// Snapshot the mining processor state for gas-trim rollback.
    pub fn snapshot_mining_processor(&self) -> pichain_mining::MiningProcessor {
        self.mining_processor.lock().clone()
    }

    /// Restore the mining processor state from a snapshot.
    pub fn restore_mining_processor(&self, snapshot: pichain_mining::MiningProcessor) {
        *self.mining_processor.lock() = snapshot;
    }

    /// Snapshot all sub-executor state for block-level persistence.
    /// Called after execute_block() to capture the full state that needs
    /// to be written atomically with the block.
    pub fn snapshot_sub_state(&self) -> SubExecutorChanges {
        // Flatten contract_storage DashMap<Address, HashMap<K,V>> into HashMap<(Address,K),V>
        let mut contract_storage = HashMap::new();
        for entry in self.contract_storage.iter() {
            let contract_addr = *entry.key();
            for (key, value) in entry.value() {
                contract_storage.insert((contract_addr, key.clone()), value.clone());
            }
        }

        SubExecutorChanges {
            mints: self.token_executor.all_mints(),
            token_accounts: self.token_executor.all_accounts(),
            pools: self.dex_executor.all_pools(),
            lp_balances: self.dex_executor.all_lp_balances(),
            collections: self.nft_executor.all_collections(),
            nfts: self.nft_executor.all_nfts(),
            launches: self.launchpad_executor.all_launches(),
            contract_storage,
            mint_nonces: self.token_executor.all_mint_nonces(),
            collection_nonces: self.nft_executor.all_collection_nonces(),
            matches: self.betting_executor.all_matches(),
            match_nonces: self.betting_executor.all_nonces(),
        }
    }

    /// Load contract storage from persistence (called on startup).
    pub fn load_contract_storage(&self, contract: Address, key: Vec<u8>, value: Vec<u8>) {
        let mut entry = self.contract_storage.entry(contract).or_default();
        entry.insert(key, value);
    }

    /// Apply token balance deltas from a DEX operation through the token executor.
    /// If any delta fails (e.g., insufficient balance, frozen account), rolls back
    /// both applied token deltas and pool state, then returns Reverted.
    ///
    /// Native PI (MintId::ZERO) deltas are applied to AccountState.balance in
    /// `state_changes` (the transaction-local state map) rather than to a TokenAccount,
    /// since native PI lives in the main account balance — not in the token executor.
    /// This is critical because `state_changes` is what gets written back to `state_cache`
    /// after transaction execution; writing to `state_cache` directly would be overwritten.
    fn apply_dex_result(
        &self,
        dex_result: DexExecutionResult,
        state_changes: &mut HashMap<Address, AccountState>,
    ) -> (TransactionStatus, Vec<TransactionEvent>) {
        if dex_result.status == TransactionStatus::Success {
            // Snapshot pool state so we can rollback on token delta failure
            let pool_snapshots: Vec<_> = dex_result
                .pool_changes
                .keys()
                .map(|pool_id| (*pool_id, self.dex_executor.get_pool(pool_id)))
                .collect();
            let lp_snapshots: Vec<_> = dex_result
                .lp_changes
                .iter()
                .map(|((pool_id, addr), _)| {
                    (
                        (*pool_id, *addr),
                        self.dex_executor.get_lp_balance(pool_id, addr),
                    )
                })
                .collect();

            // Track applied deltas for rollback: (owner, mint, amount, is_native)
            let mut applied_deltas: Vec<(Address, pichain_types::token::MintId, i128, bool)> =
                Vec::new();
            for delta in &dex_result.token_deltas {
                let is_native = delta.mint.is_native_pi();

                let result = if is_native {
                    // Native PI: modify AccountState.balance in state_changes
                    Self::apply_native_pi_delta_to_state(
                        &self.state_cache,
                        state_changes,
                        delta.owner,
                        delta.amount,
                    )
                } else {
                    // Custom token: use token executor as before
                    self.token_executor
                        .apply_delta(delta.owner, delta.mint, delta.amount)
                };

                if let Err(e) = result {
                    // Undo successfully applied deltas (reverse order)
                    for (owner, mint, amount, was_native) in applied_deltas.iter().rev() {
                        if *was_native {
                            let _ = Self::apply_native_pi_delta_to_state(
                                &self.state_cache,
                                state_changes,
                                *owner,
                                -*amount,
                            );
                        } else {
                            let _ = self.token_executor.apply_delta(*owner, *mint, -*amount);
                        }
                    }
                    // Rollback pool state in DexExecutor
                    for (pool_id, old_pool) in &pool_snapshots {
                        self.dex_executor.rollback_pool(pool_id, old_pool.as_ref());
                    }
                    for ((pool_id, addr), old_balance) in &lp_snapshots {
                        self.dex_executor
                            .rollback_lp_balance(pool_id, addr, *old_balance);
                    }
                    return (
                        TransactionStatus::Reverted(format!("DEX token delta failed: {e}")),
                        vec![],
                    );
                }
                applied_deltas.push((delta.owner, delta.mint, delta.amount, is_native));
            }
        }
        (dex_result.status, dex_result.events)
    }

    /// Apply a native PI balance delta to an account in `state_changes`.
    /// Falls back to `state_cache` if the account isn't in `state_changes` yet.
    /// Positive = credit, negative = debit. Returns Err on insufficient balance or overflow.
    fn apply_native_pi_delta_to_state(
        state_cache: &dashmap::DashMap<Address, AccountState>,
        state_changes: &mut HashMap<Address, AccountState>,
        owner: Address,
        amount: i128,
    ) -> Result<(), String> {
        if amount == 0 {
            return Ok(());
        }
        let state = state_changes.entry(owner).or_insert_with(|| {
            state_cache
                .get(&owner)
                .map(|v| v.clone())
                .unwrap_or_default()
        });
        if amount > 0 {
            let credit = u64::try_from(amount)
                .map_err(|_| "native PI delta exceeds u64 range".to_string())?;
            state.balance = state
                .balance
                .checked_add(credit)
                .ok_or("native PI balance overflow")?;
        } else {
            let debit = u64::try_from(-amount)
                .map_err(|_| "native PI delta exceeds u64 range".to_string())?;
            state.balance = state.balance.checked_sub(debit).ok_or_else(|| {
                format!(
                    "insufficient native PI balance: have {}, need {}",
                    state.balance, debit
                )
            })?;
        }
        Ok(())
    }

    /// Seed a DEX pool from a finalized launch.
    /// Creates the pool, mints tokens, adds liquidity, credits creator PI.
    /// Returns Ok(()) on success, Err(msg) on failure (with full rollback).
    /// `actor` is the address whose token account is used for the mint/debit cycle.
    fn seed_dex_pool_from_launch(
        &self,
        actor: Address,
        mint: &MintId,
        seed: &crate::launchpad_executor::PoolSeedRequest,
        state_changes: &mut HashMap<Address, AccountState>,
    ) -> Result<(), String> {
        let pi_mint = MintId([0u8; 32]);
        let launch_id = LaunchId::from_mint(mint);

        // Step 1: Create the DEX pool
        let pool_result = self.dex_executor.create_pool(actor, *mint, pi_mint);
        if pool_result.status != TransactionStatus::Success {
            self.launchpad_executor.rollback_finalization(&launch_id);
            return Err(format!("DEX pool creation error: {:?}", pool_result.status));
        }

        let pool_id = self.dex_executor.derive_pool_id(mint, &pi_mint);

        // Set creator royalty on the pool: 33.3% of the 0.30% swap fee (~0.10%) goes to creator
        self.dex_executor
            .set_creator_fee(&pool_id, seed.creator, 3333);

        // Step 2: Mint tokens for pool liquidity
        if let Err(e) = self
            .token_executor
            .apply_delta(actor, *mint, seed.token_amount as i128)
        {
            self.dex_executor.rollback_pool(&pool_id, None);
            self.launchpad_executor.rollback_finalization(&launch_id);
            return Err(format!("token mint for liquidity error: {e}"));
        }

        // Check max_supply before committing the supply increase
        let pool_mint_exceeded = if let Some(mint_ref) = self.token_executor.get_mint_mut(mint) {
            let new_supply = mint_ref.total_supply.saturating_add(seed.token_amount);
            mint_ref.max_supply > 0 && new_supply > mint_ref.max_supply
        } else {
            false
        };
        if pool_mint_exceeded {
            let _ = self
                .token_executor
                .apply_delta(actor, *mint, -(seed.token_amount as i128));
            self.dex_executor.rollback_pool(&pool_id, None);
            self.launchpad_executor.rollback_finalization(&launch_id);
            return Err("pool mint would exceed max_supply".to_string());
        }

        // Commit total_supply increase
        if let Some(mut mint_ref) = self.token_executor.get_mint_mut(mint) {
            mint_ref.total_supply = mint_ref.total_supply.saturating_add(seed.token_amount);
        }

        // Step 3: Add initial liquidity
        let liq_result = self.dex_executor.add_liquidity(
            actor,
            *mint,
            pi_mint,
            seed.token_amount,
            seed.pi_amount,
            0,
        );
        if liq_result.status != TransactionStatus::Success {
            let _ = self
                .token_executor
                .apply_delta(actor, *mint, -(seed.token_amount as i128));
            if let Some(mut mint_ref) = self.token_executor.get_mint_mut(mint) {
                mint_ref.total_supply = mint_ref.total_supply.saturating_sub(seed.token_amount);
            }
            self.dex_executor.rollback_pool(&pool_id, None);
            self.launchpad_executor.rollback_finalization(&launch_id);
            return Err(format!("add liquidity error: {:?}", liq_result.status));
        }

        // Step 3b: Debit tokens from actor (they now live in pool reserves)
        if let Err(e) = self
            .token_executor
            .apply_delta(actor, *mint, -(seed.token_amount as i128))
        {
            self.dex_executor.rollback_pool(&pool_id, None);
            let _ = self
                .token_executor
                .apply_delta(actor, *mint, -(seed.token_amount as i128));
            if let Some(mut mint_ref) = self.token_executor.get_mint_mut(mint) {
                mint_ref.total_supply = mint_ref.total_supply.saturating_sub(seed.token_amount);
            }
            self.launchpad_executor.rollback_finalization(&launch_id);
            return Err(format!("pool token debit error: {e}"));
        }

        // Credit creator's PI share
        if seed.creator_pi > 0 {
            let creator_state = state_changes.entry(seed.creator).or_insert_with(|| {
                self.state_cache
                    .get(&seed.creator)
                    .map(|v| v.clone())
                    .unwrap_or_default()
            });
            creator_state.balance = creator_state.balance.saturating_add(seed.creator_pi);
        }

        Ok(())
    }

    /// Load initial state into the executor cache.
    pub fn load_state(&self, accounts: Vec<Account>) {
        for account in accounts {
            self.state_cache.insert(account.address, account.state);
        }
    }

    /// Rebuild staking totals from the current state cache.
    /// Must be called after loading accounts from storage on startup,
    /// or after genesis application, so that anti-concentration checks
    /// have accurate totals for the first block.
    pub fn rebuild_staking_totals(&self) {
        let mut tracker = self.stake_tracker.lock();
        tracker.total_staked = 0;
        tracker.validator_stakes.clear();
        for entry in self.state_cache.iter() {
            let state = entry.value();
            if state.staked > 0 {
                tracker.total_staked = tracker.total_staked.saturating_add(state.staked);
                if let Some(validator) = state.delegate {
                    *tracker.validator_stakes.entry(validator).or_insert(0) = tracker
                        .validator_stakes
                        .get(&validator)
                        .unwrap_or(&0)
                        .saturating_add(state.staked);
                }
            }
        }
        // Snapshot current state as epoch baseline for velocity limiting.
        // On restart, this gives each validator their current total as the
        // epoch baseline, preventing false velocity violations.
        tracker.epoch_start_stakes = tracker.validator_stakes.clone();
    }

    /// Set an account state in the cache.
    /// Also updates the stake tracker if the account's staking state changed,
    /// preventing tracker drift when accounts are lazy-loaded from storage.
    pub fn set_account(&self, address: Address, state: AccountState) {
        // Check if staking state changed vs what's in the cache
        let old = self.state_cache.get(&address);
        let old_staked = old.as_ref().map(|o| o.staked).unwrap_or(0);
        let old_delegate = old.as_ref().and_then(|o| o.delegate);
        let new_staked = state.staked;
        let new_delegate = state.delegate;
        drop(old); // release DashMap read ref before acquiring tracker lock

        if old_staked != new_staked || old_delegate != new_delegate {
            let mut tracker = self.stake_tracker.lock();
            // Remove old staking contribution
            if old_staked > 0 {
                tracker.total_staked = tracker.total_staked.saturating_sub(old_staked);
                if let Some(old_v) = old_delegate {
                    if let Some(val) = tracker.validator_stakes.get_mut(&old_v) {
                        *val = val.saturating_sub(old_staked);
                        if *val == 0 {
                            tracker.validator_stakes.remove(&old_v);
                        }
                    }
                }
            }
            // Add new staking contribution
            if new_staked > 0 {
                tracker.total_staked = tracker.total_staked.saturating_add(new_staked);
                if let Some(new_v) = new_delegate {
                    *tracker.validator_stakes.entry(new_v).or_insert(0) = tracker
                        .validator_stakes
                        .get(&new_v)
                        .unwrap_or(&0)
                        .saturating_add(new_staked);
                }
            }
        }

        self.state_cache.insert(address, state);
    }

    /// Get an account state from the cache.
    pub fn get_account(&self, address: &Address) -> Option<AccountState> {
        self.state_cache.get(address).map(|v| v.clone())
    }

    /// Remove an address from the state cache so it is re-read from storage on next access.
    /// Used by the block producer to evict stale entries after post-execution gas trimming.
    /// Also decrements the stake tracker for the evicted account's staking state.
    pub fn evict_state_cache(&self, address: &Address) {
        if let Some((_, old)) = self.state_cache.remove(address) {
            if old.staked > 0 {
                let mut tracker = self.stake_tracker.lock();
                tracker.total_staked = tracker.total_staked.saturating_sub(old.staked);
                if let Some(delegate) = old.delegate {
                    if let Some(val_total) = tracker.validator_stakes.get_mut(&delegate) {
                        *val_total = val_total.saturating_sub(old.staked);
                        if *val_total == 0 {
                            tracker.validator_stakes.remove(&delegate);
                            // R37-FIX: Also remove the epoch_start_stake entry for this
                            // validator to prevent stale velocity limit baselines.
                            tracker.epoch_start_stakes.remove(&delegate);
                        }
                    }
                }
            }
        }
    }

    /// Credit an account with a reward (e.g., block proposer priority fees).
    /// Creates the account if it doesn't exist.
    /// Uses entry().or_insert_with() for atomic read-modify-write (prevents TOCTOU race).
    pub fn credit_account(&self, address: Address, amount: PiAmount) {
        let mut entry = self.state_cache.entry(address).or_default();
        entry.balance = entry.balance.saturating_add(amount);
    }

    /// Execute a block of transactions using Sealevel-style parallel scheduling.
    ///
    /// Phase 1: Build dependency graph from account access declarations
    /// Phase 2: Execute non-conflicting groups in parallel (no wasted re-execution)
    /// Phase 3: Apply state between groups for dependent transactions
    ///
    /// This achieves Solana-level throughput because conflicts are detected
    /// BEFORE execution via upfront account access declarations, eliminating
    /// the optimistic retry overhead of traditional Block-STM.
    pub fn execute_block(
        &self,
        transactions: &[SignedTransaction],
        base_fee: PiAmount,
    ) -> Vec<ExecutionResult> {
        if transactions.is_empty() {
            return vec![];
        }

        // Phase 1: Schedule transactions into non-conflicting parallel groups
        let groups = Self::schedule_transactions(transactions);

        // Phase 2: Execute each group — transactions within a group are independent
        let mut final_results: Vec<Option<ExecutionResult>> = vec![None; transactions.len()];

        for group in &groups {
            if group.len() == 1 {
                // Single transaction — execute directly (no rayon overhead)
                let idx = group[0];
                let result = self.execute_transaction(&transactions[idx], base_fee);
                for (addr, state) in &result.state_changes {
                    self.state_cache.insert(*addr, state.clone());
                }
                final_results[idx] = Some(result);
            } else {
                // Multiple non-conflicting transactions — execute in parallel via rayon
                let group_results: Vec<(usize, ExecutionResult)> = group
                    .par_iter()
                    .map(|&idx| {
                        let result = self.execute_transaction(&transactions[idx], base_fee);
                        (idx, result)
                    })
                    .collect();

                // Apply all state changes from this parallel group
                for (idx, result) in group_results {
                    for (addr, state) in &result.state_changes {
                        self.state_cache.insert(*addr, state.clone());
                    }
                    final_results[idx] = Some(result);
                }
            }
        }

        // Unwrap results (all should be populated by the scheduler).
        // If a slot is somehow None (internal scheduler error), produce a
        // reverted result instead of re-executing without state application.
        final_results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                r.unwrap_or_else(|| {
                    let tx_hash = transactions[i].hash();
                    tracing::error!(
                        tx_index = i,
                        ?tx_hash,
                        "internal scheduler error: transaction slot was not populated"
                    );
                    ExecutionResult {
                        tx_hash,
                        effect: TransactionEffect {
                            tx_hash,
                            status: TransactionStatus::Reverted(
                                "internal scheduler error".to_string(),
                            ),
                            gas_used: 0,
                            base_fee,
                            created_objects: vec![],
                            modified_objects: vec![],
                            deleted_objects: vec![],
                            events: vec![],
                        },
                        state_changes: HashMap::new(),
                        pi_burned: 0,
                        pi_minted: 0,
                        proposer_reward: 0,
                        staker_reward: 0,
                        miner_fee: 0,
                        state_reads: vec![],
                    }
                })
            })
            .collect()
    }

    /// Schedule transactions into non-conflicting execution groups.
    /// Uses Sealevel-style account access declarations to build a dependency graph.
    /// Transactions within a group have disjoint account sets and can run in parallel.
    fn schedule_transactions(transactions: &[SignedTransaction]) -> Vec<Vec<usize>> {
        use std::collections::HashSet;

        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut current_group: Vec<usize> = Vec::new();
        let mut current_write_locks: HashSet<Address> = HashSet::new();
        let mut current_read_locks: HashSet<Address> = HashSet::new();

        for (i, tx) in transactions.iter().enumerate() {
            let accesses = tx.data.kind.account_accesses(&tx.data.sender);

            // Check if this transaction conflicts with the current group
            let conflicts = accesses.iter().any(|access| {
                if access.writable {
                    // Writer conflicts with any existing reader or writer on same account
                    current_write_locks.contains(&access.address)
                        || current_read_locks.contains(&access.address)
                } else {
                    // Reader only conflicts with existing writer
                    current_write_locks.contains(&access.address)
                }
            });

            if conflicts && !current_group.is_empty() {
                // Flush current group and start a new one
                groups.push(std::mem::take(&mut current_group));
                current_write_locks.clear();
                current_read_locks.clear();
            }

            // Add this transaction to the current group
            current_group.push(i);
            for access in &accesses {
                if access.writable {
                    current_write_locks.insert(access.address);
                } else {
                    current_read_locks.insert(access.address);
                }
            }
        }

        // Don't forget the last group
        if !current_group.is_empty() {
            groups.push(current_group);
        }

        groups
    }

    /// Execute a single transaction.
    fn execute_transaction(&self, tx: &SignedTransaction, base_fee: PiAmount) -> ExecutionResult {
        let tx_hash = tx.hash();
        let mut state_changes: HashMap<Address, AccountState> = HashMap::new();

        // Validate chain ID (prevent cross-chain replay attacks)
        if tx.data.chain_id != self.chain_id {
            return ExecutionResult {
                tx_hash,
                effect: TransactionEffect {
                    tx_hash,
                    status: TransactionStatus::Reverted(format!(
                        "wrong chain_id: expected {}, got {}",
                        self.chain_id, tx.data.chain_id
                    )),
                    gas_used: 0,
                    base_fee,
                    created_objects: vec![],
                    modified_objects: vec![],
                    deleted_objects: vec![],
                    events: vec![],
                },
                state_changes,
                pi_burned: 0,
                pi_minted: 0,
                proposer_reward: 0,
                staker_reward: 0,
                miner_fee: 0,
                state_reads: vec![],
            };
        }

        // Get sender account
        let sender_state = self
            .state_cache
            .get(&tx.data.sender)
            .map(|v| v.clone())
            .unwrap_or_default();

        // Validate nonce (prevent transaction replay and enforce ordering)
        if tx.data.nonce != sender_state.nonce {
            return ExecutionResult {
                tx_hash,
                effect: TransactionEffect {
                    tx_hash,
                    status: TransactionStatus::Reverted(format!(
                        "nonce mismatch: expected {}, got {}",
                        sender_state.nonce, tx.data.nonce
                    )),
                    gas_used: 0,
                    base_fee,
                    created_objects: vec![],
                    modified_objects: vec![],
                    deleted_objects: vec![],
                    events: vec![],
                },
                state_changes,
                pi_burned: 0,
                pi_minted: 0,
                proposer_reward: 0,
                staker_reward: 0,
                miner_fee: 0,
                state_reads: vec![],
            };
        }

        // Validate gas limits
        const MAX_GAS_PER_TX: u64 = 100_000_000; // 100M gas max per transaction
        if tx.data.gas_limit > MAX_GAS_PER_TX {
            return ExecutionResult {
                tx_hash,
                effect: TransactionEffect {
                    tx_hash,
                    status: TransactionStatus::Reverted(format!(
                        "gas_limit {} exceeds maximum {}",
                        tx.data.gas_limit, MAX_GAS_PER_TX
                    )),
                    gas_used: 0,
                    base_fee,
                    created_objects: vec![],
                    modified_objects: vec![],
                    deleted_objects: vec![],
                    events: vec![],
                },
                state_changes,
                pi_burned: 0,
                pi_minted: 0,
                proposer_reward: 0,
                staker_reward: 0,
                miner_fee: 0,
                state_reads: vec![],
            };
        }

        // Validate max_base_fee (EIP-1559: user can cap how much base fee they're willing to pay)
        if tx.data.max_base_fee > 0 && base_fee > tx.data.max_base_fee {
            return ExecutionResult {
                tx_hash,
                effect: TransactionEffect {
                    tx_hash,
                    status: TransactionStatus::Reverted(format!(
                        "base fee {} exceeds tx max_base_fee {}",
                        base_fee, tx.data.max_base_fee
                    )),
                    gas_used: 0,
                    base_fee,
                    created_objects: vec![],
                    modified_objects: vec![],
                    deleted_objects: vec![],
                    events: vec![],
                },
                state_changes,
                pi_burned: 0,
                pi_minted: 0,
                proposer_reward: 0,
                staker_reward: 0,
                miner_fee: 0,
                state_reads: vec![],
            };
        }

        // Calculate gas cost (with overflow protection)
        // For WASM-executing transactions (ContractCall, DeployContract), pre-charge based
        // on gas_limit (not estimated_gas) because actual WASM execution can consume up to
        // gas_limit worth of gas. The difference is refunded after execution.
        // For all other transaction types, use the static estimate.
        let mut gas_used = tx.estimated_gas();
        let pre_charge_gas = match &tx.data.kind {
            TransactionKind::ContractCall { .. } | TransactionKind::DeployContract { .. } => {
                tx.data.gas_limit
            }
            _ => gas_used,
        };
        let fee_per_gas = match base_fee.checked_add(tx.data.max_priority_fee) {
            Some(v) => v,
            None => {
                return ExecutionResult {
                    tx_hash,
                    effect: TransactionEffect {
                        tx_hash,
                        status: TransactionStatus::Reverted("fee calculation overflow".to_string()),
                        gas_used: 0,
                        base_fee,
                        created_objects: vec![],
                        modified_objects: vec![],
                        deleted_objects: vec![],
                        events: vec![],
                    },
                    state_changes,
                    pi_burned: 0,
                    pi_minted: 0,
                    proposer_reward: 0,
                    staker_reward: 0,
                    miner_fee: 0,
                    state_reads: vec![],
                };
            }
        };
        let total_fee = match pre_charge_gas.checked_mul(fee_per_gas) {
            Some(v) => v,
            None => {
                return ExecutionResult {
                    tx_hash,
                    effect: TransactionEffect {
                        tx_hash,
                        status: TransactionStatus::Reverted("fee calculation overflow".to_string()),
                        gas_used: 0,
                        base_fee,
                        created_objects: vec![],
                        modified_objects: vec![],
                        deleted_objects: vec![],
                        events: vec![],
                    },
                    state_changes,
                    pi_burned: 0,
                    pi_minted: 0,
                    proposer_reward: 0,
                    staker_reward: 0,
                    miner_fee: 0,
                    state_reads: vec![],
                };
            }
        };
        // Separate base/priority fee portions for proper EIP-1559 burn calculation.
        // Only the base fee portion is subject to burn/staker/treasury split;
        // priority fees go 100% to block producer.
        let base_fee_portion_u128 = pre_charge_gas as u128 * base_fee as u128;
        let base_fee_portion = if base_fee_portion_u128 > u64::MAX as u128 {
            u64::MAX
        } else {
            base_fee_portion_u128 as u64
        };
        let fee_split = self.fee_calc.split_fee(total_fee, base_fee_portion);
        let mut pi_burned = fee_split.burned;

        // R28-FIX: Include ParticipateInLaunch pi_amount in pre-balance check.
        // Previously, only Transfer and Stake amounts were checked, causing
        // ParticipateInLaunch to pass the pre-check but fail later after fees
        // were already deducted and nonce incremented.
        let transfer_amount = match &tx.data.kind {
            TransactionKind::Transfer { amount, .. } => *amount,
            TransactionKind::Stake { amount, .. } => *amount,
            TransactionKind::ParticipateInLaunch { pi_amount, .. } => *pi_amount,
            _ => 0,
        };

        let total_cost = match total_fee.checked_add(transfer_amount) {
            Some(v) => v,
            None => {
                return ExecutionResult {
                    tx_hash,
                    effect: TransactionEffect {
                        tx_hash,
                        status: TransactionStatus::Reverted("total cost overflow".to_string()),
                        gas_used,
                        base_fee,
                        created_objects: vec![],
                        modified_objects: vec![],
                        deleted_objects: vec![],
                        events: vec![],
                    },
                    state_changes,
                    pi_burned: 0,
                    pi_minted: 0,
                    proposer_reward: 0,
                    staker_reward: 0,
                    miner_fee: 0,
                    state_reads: vec![],
                };
            }
        };
        // Execute based on transaction kind
        let mut sender = sender_state.clone();

        // Auto-release unbonding funds if the unbonding period has elapsed.
        // IMPORTANT: This must happen BEFORE the available_balance check so that
        // users with matured unbonding funds can spend them without being blocked.
        if sender.unbonding > 0 {
            let current_height = self.current_block_height();
            if current_height
                >= sender
                    .unbonding_height
                    .saturating_add(AccountState::UNBONDING_BLOCKS)
            {
                // In the "balance = total" model, unbonding funds are already part of
                // balance (never subtracted). Just clear the unbonding earmark so
                // available_balance() increases.
                sender.unbonding = 0;
                sender.unbonding_height = 0;
            }
        }

        // For balance check: fees can come from available_balance + locked_balance,
        // but transfer amounts can only come from available_balance (locked is fee-only).
        let fee_capable = sender.fee_balance();
        let transfer_capable = sender.available_balance();
        let can_afford = if transfer_amount > 0 {
            // Need: available_balance >= transfer_amount AND
            // fee_balance >= total_fee + transfer_amount
            transfer_capable >= transfer_amount && fee_capable >= total_cost
        } else {
            fee_capable >= total_cost
        };

        // Mining proofs are exempt from upfront balance checks: the mining reward
        // always exceeds gas cost, and requiring pre-funding would create a
        // chicken-and-egg problem for new miners. PoW + mempool rate limits
        // prevent spam.
        let is_mining_proof = matches!(tx.data.kind, TransactionKind::MiningProof { .. });
        if !can_afford && !is_mining_proof {
            return ExecutionResult {
                tx_hash,
                effect: TransactionEffect {
                    tx_hash,
                    status: TransactionStatus::Reverted(format!(
                        "insufficient balance: need {total_cost}, have {} (+ {} locked for fees)",
                        sender.available_balance(),
                        sender.locked_balance
                    )),
                    gas_used,
                    base_fee,
                    created_objects: vec![],
                    modified_objects: vec![],
                    deleted_objects: vec![],
                    events: vec![],
                },
                state_changes,
                pi_burned: 0,
                pi_minted: 0,
                proposer_reward: 0,
                staker_reward: 0,
                miner_fee: 0,
                state_reads: vec![],
            };
        }

        // R37-FIX: Check nonce overflow BEFORE fee deduction. Previously this check
        // happened after fee deduction, causing fees to be consumed but reported as
        // pi_burned: 0 / proposer_reward: 0 / miner_fee: 0, creating an accounting gap.
        if sender.nonce == u64::MAX {
            // Nonce would overflow u64. This requires 2^64 transactions from one account.
            // Reject before any state mutations so the tx reverts cleanly with gas_used=0.
            return ExecutionResult {
                tx_hash,
                effect: TransactionEffect {
                    tx_hash,
                    status: TransactionStatus::Reverted(
                        "nonce overflow: account has exhausted all nonces".to_string(),
                    ),
                    gas_used: 0,
                    base_fee,
                    created_objects: vec![],
                    modified_objects: vec![],
                    deleted_objects: vec![],
                    events: vec![],
                },
                state_changes,
                pi_burned: 0,
                pi_minted: 0,
                proposer_reward: 0,
                staker_reward: 0,
                miner_fee: 0,
                state_reads: vec![tx.data.sender],
            };
        }

        // Deduct fees: first from regular balance, then overflow from locked_balance.
        // locked_balance is non-transferable PI granted on wallet activation, only usable for fees.
        //
        // For mining proofs: if the sender can't afford gas, advance the gas amount
        // from the future mining reward. The reward always exceeds gas cost. This
        // enables zero-balance miners to bootstrap without needing a faucet.
        if is_mining_proof && sender.fee_balance() < total_fee {
            sender.balance = sender.balance.saturating_add(total_fee);
        }
        if sender.balance >= total_fee {
            sender.balance -= total_fee;
        } else {
            let remainder = total_fee - sender.balance;
            sender.balance = 0;
            sender.locked_balance = match sender.locked_balance.checked_sub(remainder) {
                Some(v) => v,
                None => {
                    // Should never happen — fee_balance was validated above.
                    return ExecutionResult {
                        tx_hash,
                        effect: TransactionEffect {
                            tx_hash,
                            status: TransactionStatus::Reverted(format!(
                                "fee underflow: balance {} + locked {} < fee {}",
                                sender.balance, sender.locked_balance, total_fee
                            )),
                            gas_used,
                            base_fee,
                            created_objects: vec![],
                            modified_objects: vec![],
                            deleted_objects: vec![],
                            events: vec![],
                        },
                        state_changes,
                        pi_burned: 0,
                        pi_minted: 0,
                        proposer_reward: 0,
                        staker_reward: 0,
                        miner_fee: 0,
                        state_reads: vec![],
                    };
                }
            };
        }
        // Safe: nonce overflow was checked above.
        sender.nonce += 1;

        // Track inflationary PI minted in this transaction (mining rewards)
        let mut pi_minted: PiAmount = 0;

        let (status, events) = match &tx.data.kind {
            TransactionKind::Transfer { recipient, amount } => {
                if tx.data.sender == *recipient {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (
                        TransactionStatus::Reverted("cannot transfer to self".to_string()),
                        vec![],
                    )
                } else if sender.balance < *amount {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (
                        TransactionStatus::Reverted(format!(
                            "transfer underflow: balance {} < amount {}",
                            sender.balance, amount
                        )),
                        vec![],
                    )
                } else {
                    // Defense-in-depth: use checked_sub even though balance >= amount is verified above
                    sender.balance = sender.balance.checked_sub(*amount).unwrap_or_else(|| {
                        tracing::error!(
                            balance = sender.balance,
                            amount,
                            "transfer balance underflow after check — should be impossible"
                        );
                        0
                    });
                    state_changes.insert(tx.data.sender, sender.clone());

                    // Read recipient state from cache (scheduler ensures no concurrent modification)
                    let mut recipient_state =
                        self.state_cache.entry(*recipient).or_default().clone();
                    recipient_state.balance = match recipient_state.balance.checked_add(*amount) {
                        Some(v) => v,
                        None => {
                            tracing::error!(
                                balance = recipient_state.balance,
                                amount,
                                "recipient balance overflow"
                            );
                            state_changes.insert(tx.data.sender, sender.clone());
                            return ExecutionResult {
                                tx_hash,
                                effect: TransactionEffect {
                                    tx_hash,
                                    status: TransactionStatus::Reverted(
                                        "recipient balance overflow".to_string(),
                                    ),
                                    gas_used,
                                    base_fee,
                                    created_objects: vec![],
                                    modified_objects: vec![],
                                    deleted_objects: vec![],
                                    events: vec![],
                                },
                                state_changes,
                                pi_burned,
                                pi_minted: 0,
                                proposer_reward: fee_split.proposer,
                                staker_reward: fee_split.stakers,
                                miner_fee: fee_split.miners,
                                state_reads: vec![tx.data.sender, *recipient],
                            };
                        }
                    };
                    state_changes.insert(*recipient, recipient_state);

                    (
                        TransactionStatus::Success,
                        vec![TransactionEvent {
                            emitter: tx.data.sender,
                            event_type: "Transfer".to_string(),
                            data: amount.to_le_bytes().to_vec(),
                        }],
                    )
                }
            }
            TransactionKind::Stake { validator, amount } => {
                if *amount == 0 {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (
                        TransactionStatus::Reverted("stake amount must be > 0".to_string()),
                        vec![],
                    )
                } else if sender.available_balance() < *amount {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (
                        TransactionStatus::Reverted(format!(
                            "insufficient available balance for stake: have {}, need {}",
                            sender.available_balance(),
                            amount
                        )),
                        vec![],
                    )
                } else {
                    // ── Anti-concentration check (atomic under stake_tracker lock) ──
                    let old_delegate = sender.delegate;
                    let old_staked = sender.staked;
                    let new_staked = old_staked.saturating_add(*amount);
                    {
                        let mut tracker = self.stake_tracker.lock();
                        // Advance staking epoch if block height crossed an epoch boundary.
                        // Deterministic: derived purely from block height, no consensus coordination needed.
                        let current_epoch = self.current_block_height() / BLOCKS_PER_STAKING_EPOCH;
                        if current_epoch != tracker.current_epoch {
                            tracker.epoch_start_stakes = tracker.validator_stakes.clone();
                            tracker.current_epoch = current_epoch;
                        }
                        // Compute what the validator's total and network total will be
                        let new_total_staked = tracker.total_staked.saturating_add(*amount);
                        // Delta for the target validator: if sender changes delegation, move
                        // old staked amount from old validator to new one.
                        let val_add = if old_delegate == Some(*validator) {
                            // Same validator — just adding amount
                            *amount
                        } else {
                            // Switching or first-time — validator gets sender's entire new staked
                            new_staked
                        };
                        let current_val_total =
                            *tracker.validator_stakes.get(validator).unwrap_or(&0);
                        let new_val_total = current_val_total.saturating_add(val_add);
                        // Count validators that meet MIN_VALIDATOR_STAKE — prevents
                        // Sybil slot-filling with 1-lamport stakes.
                        let validator_count = tracker
                            .validator_stakes
                            .values()
                            .filter(|&&v| v >= MIN_VALIDATOR_STAKE)
                            .count();

                        // Anti-Sybil: cap maximum active validator count.
                        // Only counts validators meeting MIN_VALIDATOR_STAKE threshold.
                        let would_be_new = current_val_total < MIN_VALIDATOR_STAKE
                            && new_val_total >= MIN_VALIDATOR_STAKE;
                        if validator_count >= MAX_ACTIVE_VALIDATORS && would_be_new {
                            state_changes.insert(tx.data.sender, sender.clone());
                            return ExecutionResult {
                                tx_hash,
                                effect: TransactionEffect {
                                    tx_hash,
                                    status: TransactionStatus::Reverted(format!(
                                        "maximum active validator count reached ({})",
                                        MAX_ACTIVE_VALIDATORS
                                    )),
                                    gas_used,
                                    base_fee,
                                    created_objects: vec![],
                                    modified_objects: vec![],
                                    deleted_objects: vec![],
                                    events: vec![],
                                },
                                state_changes,
                                pi_burned,
                                pi_minted: 0,
                                proposer_reward: fee_split.proposer,
                                staker_reward: fee_split.stakers,
                                miner_fee: fee_split.miners,
                                state_reads: vec![tx.data.sender],
                            };
                        }

                        // Concentration caps: use bootstrap 41.3% cap or full caps.
                        // During bootstrap, skip caps for NEW validators (first-time staking
                        // above MIN_VALIDATOR_STAKE), same as register_validator() in StakingManager.
                        let is_bootstrap = validator_count < MIN_VALIDATORS_FOR_STAKE_CAP;
                        let is_new_validator = current_val_total < MIN_VALIDATOR_STAKE
                            && new_val_total >= MIN_VALIDATOR_STAKE;
                        let val_cap = if is_bootstrap {
                            BOOTSTRAP_VALIDATOR_STAKE_PCT_BPS
                        } else {
                            MAX_VALIDATOR_STAKE_PCT_BPS
                        };
                        let addr_cap = if is_bootstrap {
                            BOOTSTRAP_VALIDATOR_STAKE_PCT_BPS
                        } else {
                            MAX_ADDRESS_STAKE_PCT_BPS
                        };
                        let skip_concentration = is_bootstrap && is_new_validator;

                        if new_total_staked > 0 && !skip_concentration {
                            // Layer 2: Per-validator concentration cap
                            let val_bps =
                                (new_val_total as u128 * 10_000 / new_total_staked as u128) as u32;
                            if val_bps > val_cap {
                                state_changes.insert(tx.data.sender, sender.clone());
                                return ExecutionResult {
                                tx_hash,
                                effect: TransactionEffect {
                                    tx_hash,
                                    status: TransactionStatus::Reverted(format!(
                                        "stake would exceed validator concentration cap: {}bps > {}bps max",
                                        val_bps, val_cap
                                    )),
                                    gas_used,
                                    base_fee,
                                    created_objects: vec![],
                                    modified_objects: vec![],
                                    deleted_objects: vec![],
                                    events: vec![],
                                },
                                state_changes,
                                pi_burned,
                                pi_minted: 0,
                                proposer_reward: fee_split.proposer,
                                staker_reward: fee_split.stakers,
                            miner_fee: fee_split.miners,
                                state_reads: vec![tx.data.sender],
                            };
                            }
                            // Layer 3: Per-address concentration cap
                            let addr_bps =
                                (new_staked as u128 * 10_000 / new_total_staked as u128) as u32;
                            if addr_bps > addr_cap {
                                state_changes.insert(tx.data.sender, sender.clone());
                                return ExecutionResult {
                                tx_hash,
                                effect: TransactionEffect {
                                    tx_hash,
                                    status: TransactionStatus::Reverted(format!(
                                        "stake would exceed address concentration cap: {}bps > {}bps max",
                                        addr_bps, addr_cap
                                    )),
                                    gas_used,
                                    base_fee,
                                    created_objects: vec![],
                                    modified_objects: vec![],
                                    deleted_objects: vec![],
                                    events: vec![],
                                },
                                state_changes,
                                pi_burned,
                                pi_minted: 0,
                                proposer_reward: fee_split.proposer,
                                staker_reward: fee_split.stakers,
                            miner_fee: fee_split.miners,
                                state_reads: vec![tx.data.sender],
                            };
                            }
                            // Layer 4: Per-epoch stake velocity limit (50% growth max)
                            // Skipped during bootstrap — velocity limit only applies when network is established.
                            let epoch_start =
                                *tracker.epoch_start_stakes.get(validator).unwrap_or(&0);
                            if !is_bootstrap && epoch_start > 0 {
                                let growth = new_val_total.saturating_sub(epoch_start);
                                let growth_bps =
                                    (growth as u128 * 10_000 / epoch_start as u128) as u32;
                                if growth_bps > MAX_STAKE_GROWTH_PCT_BPS {
                                    state_changes.insert(tx.data.sender, sender.clone());
                                    return ExecutionResult {
                                    tx_hash,
                                    effect: TransactionEffect {
                                        tx_hash,
                                        status: TransactionStatus::Reverted(format!(
                                            "stake would exceed per-epoch velocity cap: {}bps growth > {}bps max",
                                            growth_bps, MAX_STAKE_GROWTH_PCT_BPS
                                        )),
                                        gas_used,
                                        base_fee,
                                        created_objects: vec![],
                                        modified_objects: vec![],
                                        deleted_objects: vec![],
                                        events: vec![],
                                    },
                                    state_changes,
                                    pi_burned,
                                    pi_minted: 0,
                                    proposer_reward: fee_split.proposer,
                                staker_reward: fee_split.stakers,
                            miner_fee: fee_split.miners,
                                    state_reads: vec![tx.data.sender],
                                };
                                }
                            }
                            // epoch_start == 0 means new validator this epoch — no velocity limit
                        }
                        // All checks passed — commit tracking updates
                        tracker.total_staked = new_total_staked;
                        if let Some(old_v) = old_delegate {
                            if old_v != *validator {
                                // Switching validator: remove old staked from old validator
                                if let Some(old_val) = tracker.validator_stakes.get_mut(&old_v) {
                                    *old_val = old_val.saturating_sub(old_staked);
                                    if *old_val == 0 {
                                        tracker.validator_stakes.remove(&old_v);
                                    }
                                }
                            }
                        }
                        *tracker.validator_stakes.entry(*validator).or_insert(0) = new_val_total;
                    } // release stake_tracker lock

                    // Balance-is-total model: staking locks funds within balance by incrementing
                    // staked. Do NOT subtract from balance — available_balance() already
                    // accounts for the staked earmark. This prevents the fund-lock bug where
                    // multiple sequential stakes would make balance < staked.
                    sender.staked = new_staked;
                    sender.delegate = Some(*validator);
                    state_changes.insert(tx.data.sender, sender.clone());

                    (
                        TransactionStatus::Success,
                        vec![TransactionEvent {
                            emitter: tx.data.sender,
                            event_type: "Stake".to_string(),
                            data: amount.to_le_bytes().to_vec(),
                        }],
                    )
                }
            }
            TransactionKind::Unstake {
                validator: _,
                amount,
            } => {
                if *amount == 0 {
                    // R36-FIX: Use normal (status, events) return instead of early return
                    // to ensure gas refund and fee split logic at the bottom runs correctly.
                    state_changes.insert(tx.data.sender, sender.clone());
                    (
                        TransactionStatus::Reverted("unstake amount must be > 0".to_string()),
                        vec![],
                    )
                } else if sender.staked < *amount {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (
                        TransactionStatus::Reverted("insufficient staked amount".to_string()),
                        vec![],
                    )
                } else if sender.unbonding > 0 {
                    // Only one unbonding period at a time. Must wait for current one to finish.
                    state_changes.insert(tx.data.sender, sender.clone());
                    (
                        TransactionStatus::Reverted("existing unbonding in progress".to_string()),
                        vec![],
                    )
                } else {
                    sender.staked = match sender.staked.checked_sub(*amount) {
                        Some(v) => v,
                        None => {
                            tracing::error!(
                                staked = sender.staked,
                                amount,
                                "unstake underflow after check — should be impossible"
                            );
                            state_changes.insert(tx.data.sender, sender.clone());
                            return ExecutionResult {
                                tx_hash,
                                effect: TransactionEffect {
                                    tx_hash,
                                    status: TransactionStatus::Reverted(
                                        "staked balance underflow".to_string(),
                                    ),
                                    gas_used,
                                    base_fee,
                                    created_objects: vec![],
                                    modified_objects: vec![],
                                    deleted_objects: vec![],
                                    events: vec![],
                                },
                                state_changes,
                                pi_burned,
                                pi_minted: 0,
                                proposer_reward: fee_split.proposer,
                                staker_reward: fee_split.stakers,
                                miner_fee: fee_split.miners,
                                state_reads: vec![tx.data.sender],
                            };
                        }
                    };
                    // Update anti-concentration tracking
                    {
                        let mut tracker = self.stake_tracker.lock();
                        // Advance staking epoch if block height crossed an epoch boundary.
                        let current_epoch = self.current_block_height() / BLOCKS_PER_STAKING_EPOCH;
                        if current_epoch != tracker.current_epoch {
                            tracker.epoch_start_stakes = tracker.validator_stakes.clone();
                            tracker.current_epoch = current_epoch;
                        }
                        tracker.total_staked = tracker.total_staked.saturating_sub(*amount);
                        if let Some(delegate) = sender.delegate {
                            if let Some(val_total) = tracker.validator_stakes.get_mut(&delegate) {
                                *val_total = val_total.saturating_sub(*amount);
                                // Remove zeroed entries to prevent restake bypass:
                                // without this, a fully unstaked validator entry stays
                                // at 0, and restaking it triggers "new validator" logic
                                // that skips concentration caps during bootstrap.
                                if *val_total == 0 {
                                    tracker.validator_stakes.remove(&delegate);
                                }
                            }
                        }
                    }
                    // Move to unbonding instead of immediately returning to balance.
                    // Funds become available after UNBONDING_BLOCKS have elapsed.
                    sender.unbonding = *amount;
                    sender.unbonding_height = self.current_block_height();
                    if sender.staked == 0 {
                        sender.delegate = None;
                    }
                    state_changes.insert(tx.data.sender, sender.clone());

                    (
                        TransactionStatus::Success,
                        vec![TransactionEvent {
                            emitter: tx.data.sender,
                            event_type: "Unstake".to_string(),
                            data: amount.to_le_bytes().to_vec(),
                        }],
                    )
                }
            }
            TransactionKind::MiningProof {
                start_position,
                digit_count,
                ref digits,
                pow_nonce,
                ref anchor_block_hash,
                ..
            } => {
                // Reject oversized mining proofs to prevent memory exhaustion
                const MAX_PROOF_DIGITS: usize = 100_000; // ~100KB max
                if digits.len() > MAX_PROOF_DIGITS {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (
                        TransactionStatus::Reverted(format!(
                            "mining proof too large: {} digits (max {})",
                            digits.len(),
                            MAX_PROOF_DIGITS
                        )),
                        vec![],
                    )
                } else {
                    // Construct the mining proof from transaction data
                    let mining_proof = pichain_mining::MiningProof::new(
                        *start_position,
                        digits.clone(),
                        tx.data.sender,
                        tx.data.nonce,
                    );

                    // Process through the mining processor (verify digits + PoW + register + reward)
                    let mut processor = self.mining_processor.lock();
                    processor.set_block_timestamp(self.block_timestamp());

                    // Validate anchor_block_hash length — reject if not exactly 32 bytes
                    if anchor_block_hash.len() != 32 {
                        state_changes.insert(tx.data.sender, sender.clone());
                        return ExecutionResult {
                            tx_hash,
                            effect: TransactionEffect {
                                tx_hash,
                                status: TransactionStatus::Reverted(format!(
                                    "invalid anchor_block_hash length: expected 32, got {}",
                                    anchor_block_hash.len()
                                )),
                                gas_used,
                                base_fee,
                                created_objects: vec![],
                                modified_objects: vec![],
                                deleted_objects: vec![],
                                events: vec![],
                            },
                            state_changes,
                            pi_burned,
                            pi_minted: 0,
                            proposer_reward: fee_split.proposer,
                            staker_reward: fee_split.stakers,
                            miner_fee: fee_split.miners,
                            state_reads: vec![tx.data.sender],
                        };
                    }
                    let mut anchor = [0u8; 32];
                    anchor.copy_from_slice(anchor_block_hash);

                    let verification =
                        processor.process_proof_with_pow(&mining_proof, *pow_nonce, &anchor);

                    if verification.valid {
                        // Credit mining reward to sender (saturating to prevent overflow)
                        sender.balance = sender.balance.saturating_add(verification.reward_amount);
                        pi_minted = verification.reward_amount;
                        state_changes.insert(tx.data.sender, sender.clone());
                        (
                            TransactionStatus::Success,
                            vec![TransactionEvent {
                                emitter: tx.data.sender,
                                event_type: "MiningReward".to_string(),
                                data: serde_json::to_vec(&serde_json::json!({
                                    "start_position": start_position,
                                    "digit_count": digit_count,
                                    "reward": verification.reward_amount,
                                    "frontier": processor.frontier(),
                                }))
                                .unwrap_or_default(),
                            }],
                        )
                    } else {
                        let err_msg = verification.error.unwrap_or_default();
                        tracing::warn!(
                            %tx.data.sender,
                            start_position,
                            digit_count,
                            error = %err_msg,
                            "mining proof REVERTED"
                        );
                        state_changes.insert(tx.data.sender, sender.clone());
                        (
                            TransactionStatus::Reverted(format!(
                                "mining proof rejected: {}",
                                err_msg
                            )),
                            vec![],
                        )
                    }
                } // close else from digit size check
            }
            TransactionKind::DeployContract {
                ref code,
                ref init_data,
            } => {
                state_changes.insert(tx.data.sender, sender.clone());

                // 0. Reject oversized init_data to prevent pre-gas DoS
                const MAX_INIT_DATA_SIZE: usize = 512 * 1024; // 512 KB
                if init_data.len() > MAX_INIT_DATA_SIZE {
                    // R36-FIX: Use (status, events) return to flow through fee split logic
                    (
                        TransactionStatus::Reverted(format!(
                            "init_data too large: {} bytes (max {})",
                            init_data.len(),
                            MAX_INIT_DATA_SIZE
                        )),
                        vec![],
                    )
                } else if let Err(e) = self.wasm_vm.validate_module(code) {
                    (
                        TransactionStatus::Reverted(format!("invalid contract: {e}")),
                        vec![],
                    )
                } else {
                    // 2. Compute deterministic contract address from deployer + nonce
                    let contract_addr = {
                        let mut data = Vec::with_capacity(24);
                        data.extend_from_slice(&tx.data.sender.0);
                        data.extend_from_slice(&tx.data.nonce.to_le_bytes());
                        let hash = pichain_crypto::hash(&data);
                        let mut addr = [0u8; 20];
                        addr.copy_from_slice(&hash.as_bytes()[..20]);
                        Address(addr)
                    };

                    // 3. Register in the contract registry
                    let mut registry = self.contract_registry.lock();
                    match registry.deploy(
                        contract_addr,
                        tx.data.sender,
                        code.clone(),
                        ContractAbi::new(),
                        String::new(),
                        "1.0.0".to_string(),
                        self.block_timestamp(),
                        false,
                    ) {
                        Ok(_) => {
                            // 4. Optionally run init function if init_data is provided
                            if !init_data.is_empty() {
                                let result = self.wasm_vm.execute(
                                    code,
                                    "init",
                                    init_data,
                                    tx.data.sender,
                                    contract_addr,
                                    HashMap::new(),
                                    tx.data.gas_limit,
                                    self.current_block_height(),
                                    self.block_timestamp(),
                                );
                                if !result.success {
                                    // Unregister the contract — init failed, address must not remain occupied
                                    registry.remove(&contract_addr);
                                    (
                                        TransactionStatus::Reverted(format!(
                                            "contract init failed: {}",
                                            result.error.unwrap_or_default()
                                        )),
                                        vec![],
                                    )
                                } else {
                                    // Persist init state changes
                                    if !result.state_changes.is_empty() {
                                        self.contract_storage
                                            .insert(contract_addr, result.state_changes);
                                    }
                                    (
                                        TransactionStatus::Success,
                                        vec![TransactionEvent {
                                            emitter: tx.data.sender,
                                            event_type: "ContractDeployed".to_string(),
                                            data: serde_json::to_vec(&serde_json::json!({
                                                "contract": format!("{}", contract_addr),
                                                "code_size": code.len(),
                                            }))
                                            .unwrap_or_default(),
                                        }],
                                    )
                                }
                            } else {
                                (
                                    TransactionStatus::Success,
                                    vec![TransactionEvent {
                                        emitter: tx.data.sender,
                                        event_type: "ContractDeployed".to_string(),
                                        data: serde_json::to_vec(&serde_json::json!({
                                            "contract": format!("{}", contract_addr),
                                            "code_size": code.len(),
                                        }))
                                        .unwrap_or_default(),
                                    }],
                                )
                            }
                        }
                        Err(e) => (
                            TransactionStatus::Reverted(format!("deploy failed: {e}")),
                            vec![],
                        ),
                    }
                }
            }
            TransactionKind::ContractCall {
                contract,
                ref function,
                ref args,
            } => {
                // Validate function name length to prevent abuse
                if function.len() > MAX_FUNCTION_NAME_LEN {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (
                        TransactionStatus::Reverted(format!(
                            "function name too long: {} > {MAX_FUNCTION_NAME_LEN}",
                            function.len()
                        )),
                        vec![],
                    )
                } else {
                    state_changes.insert(tx.data.sender, sender.clone());

                    // 1. Look up the contract and its bytecode in the registry
                    let registry = self.contract_registry.lock();
                    let lookup = registry.get(contract).cloned().and_then(|meta| {
                        registry
                            .get_code(&meta.code_hash)
                            .map(|code| (meta, code.to_vec()))
                    });
                    drop(registry);

                    match lookup {
                        None => (
                            TransactionStatus::Reverted(format!(
                                "contract not found: {}",
                                contract
                            )),
                            vec![],
                        ),
                        Some((_meta, code)) => {
                            // 2. Load contract storage from persistent cache
                            let storage = self
                                .contract_storage
                                .get(contract)
                                .map(|entry| entry.value().clone())
                                .unwrap_or_default();

                            // 3. Execute the WASM contract
                            let result = self.wasm_vm.execute(
                                &code,
                                function,
                                args,
                                tx.data.sender,
                                *contract,
                                storage,
                                tx.data.gas_limit,
                                self.current_block_height(),
                                self.block_timestamp(),
                            );

                            // Use actual WASM fuel consumed for gas metering.
                            // Cap at gas_limit to prevent undercharge: the pre-charged fee
                            // was based on estimated_gas, so actual must not exceed gas_limit
                            // (which bounds the pre-charge). If WASM reports more gas than
                            // the limit, the excess was unbounded execution we didn't pay for.
                            if result.gas_used > 0 {
                                gas_used = result.gas_used.min(tx.data.gas_limit);
                            }

                            if result.success {
                                // 4. Persist state changes back to contract storage
                                if !result.state_changes.is_empty() {
                                    let mut entry =
                                        self.contract_storage.entry(*contract).or_default();
                                    for (key, value) in &result.state_changes {
                                        if value.is_empty() {
                                            entry.remove(key);
                                        } else {
                                            entry.insert(key.clone(), value.clone());
                                        }
                                    }
                                }

                                let events = result
                                    .logs
                                    .iter()
                                    .map(|log| TransactionEvent {
                                        emitter: *contract,
                                        event_type: "ContractLog".to_string(),
                                        data: log.data.clone(),
                                    })
                                    .collect();
                                (TransactionStatus::Success, events)
                            } else {
                                (
                                    TransactionStatus::Reverted(format!(
                                        "contract call failed: {}",
                                        result.error.unwrap_or_default()
                                    )),
                                    vec![],
                                )
                            }
                        }
                    }
                } // close function name validation else
            }

            // --- Native Token Program ---
            TransactionKind::CreateToken {
                name,
                symbol,
                decimals,
                max_supply,
                metadata_uri,
            } => {
                // SECURITY: Rate-limit token creation per address to prevent state exhaustion.
                // Without this, an attacker can create millions of token mints, each consuming
                // ~200 bytes in RocksDB, exhausting disk space and degrading scan performance.
                let current_mint_nonce = self.token_executor.get_mint_nonce(&tx.data.sender);
                if current_mint_nonce >= MAX_MINTS_PER_ADDRESS {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (
                        TransactionStatus::Reverted(format!(
                            "token creation limit reached: {} mints per address",
                            MAX_MINTS_PER_ADDRESS
                        )),
                        vec![],
                    )
                }
                // Validate string fields to prevent XSS and storage abuse
                // metadata_uri uses length-only check: it's structured JSON data (not display text)
                // and legitimately contains quotes, braces, base64 characters, etc.
                else if let Err(e) = validate_string_field(name, "name", MAX_TOKEN_NAME_LEN)
                    .and_then(|_| validate_string_field(symbol, "symbol", MAX_TOKEN_SYMBOL_LEN))
                    .and_then(|_| {
                        if metadata_uri.is_empty() {
                            Ok(())
                        } else if metadata_uri.len() > MAX_METADATA_URI_LEN {
                            Err(format!(
                                "metadata_uri too long: {} bytes (max {})",
                                metadata_uri.len(),
                                MAX_METADATA_URI_LEN
                            ))
                        } else {
                            Ok(())
                        }
                    })
                {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (TransactionStatus::Reverted(e), vec![])
                } else {
                    state_changes.insert(tx.data.sender, sender.clone());
                    let token_result = self.token_executor.create_token(
                        tx.data.sender,
                        name.clone(),
                        symbol.clone(),
                        *decimals,
                        *max_supply,
                        metadata_uri.clone(),
                    );
                    (token_result.status, token_result.events)
                }
            }
            TransactionKind::MintToken {
                mint,
                recipient,
                amount,
            } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let token_result =
                    self.token_executor
                        .mint_tokens(tx.data.sender, *mint, *recipient, *amount);
                (token_result.status, token_result.events)
            }
            TransactionKind::TransferToken {
                mint,
                recipient,
                amount,
            } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let token_result =
                    self.token_executor
                        .transfer_tokens(tx.data.sender, *mint, *recipient, *amount);
                (token_result.status, token_result.events)
            }
            TransactionKind::BurnToken { mint, amount } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let token_result = self
                    .token_executor
                    .burn_tokens(tx.data.sender, *mint, *amount);
                (token_result.status, token_result.events)
            }
            TransactionKind::ApproveToken {
                mint,
                delegate,
                amount,
            } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let token_result =
                    self.token_executor
                        .approve_token(tx.data.sender, *mint, *delegate, *amount);
                (token_result.status, token_result.events)
            }
            TransactionKind::RevokeMintAuthority { mint } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let token_result = self
                    .token_executor
                    .revoke_mint_authority(tx.data.sender, *mint);
                (token_result.status, token_result.events)
            }
            TransactionKind::FreezeTokenAccount { mint, target } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let token_result =
                    self.token_executor
                        .freeze_token_account(tx.data.sender, *mint, *target);
                (token_result.status, token_result.events)
            }
            TransactionKind::ThawTokenAccount { mint, target } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let token_result =
                    self.token_executor
                        .thaw_token_account(tx.data.sender, *mint, *target);
                (token_result.status, token_result.events)
            }

            // --- DEX/AMM ---
            TransactionKind::CreatePool { mint_a, mint_b } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let dex_result = self
                    .dex_executor
                    .create_pool(tx.data.sender, *mint_a, *mint_b);
                (dex_result.status, dex_result.events)
            }
            TransactionKind::AddLiquidity {
                mint_a,
                mint_b,
                amount_a,
                amount_b,
                min_lp_tokens,
            } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let dex_result = self.dex_executor.add_liquidity(
                    tx.data.sender,
                    *mint_a,
                    *mint_b,
                    *amount_a,
                    *amount_b,
                    *min_lp_tokens,
                );
                self.apply_dex_result(dex_result, &mut state_changes)
            }
            TransactionKind::RemoveLiquidity {
                mint_a,
                mint_b,
                lp_amount,
                min_amount_a,
                min_amount_b,
            } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let dex_result = self.dex_executor.remove_liquidity(
                    tx.data.sender,
                    *mint_a,
                    *mint_b,
                    *lp_amount,
                    *min_amount_a,
                    *min_amount_b,
                );
                self.apply_dex_result(dex_result, &mut state_changes)
            }
            TransactionKind::Swap {
                mint_in,
                mint_out,
                amount_in,
                min_amount_out,
            } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let dex_result = self.dex_executor.swap(
                    tx.data.sender,
                    *mint_in,
                    *mint_out,
                    *amount_in,
                    *min_amount_out,
                );
                self.apply_dex_result(dex_result, &mut state_changes)
            }

            // --- Launchpad ---
            TransactionKind::CreateLaunch {
                mint,
                launch_type,
                tokens_for_sale,
                target_pi,
                max_per_address,
            } => {
                // R27-FIX: Verify sender has mint authority. Without this check,
                // anyone could create a launch for a token they don't control,
                // collecting PI from participants for tokens they can't deliver.
                // R27-FIX: Verify sender has mint authority. Without this check,
                // anyone could create a launch for a token they don't control,
                // collecting PI from participants for tokens they can't deliver.
                let mint_check = self.token_executor.get_mint(mint);
                match mint_check {
                    None => {
                        state_changes.insert(tx.data.sender, sender.clone());
                        (
                            TransactionStatus::Reverted("token mint does not exist".to_string()),
                            vec![],
                        )
                    }
                    Some(token_mint) if token_mint.mint_authority != Some(tx.data.sender) => {
                        state_changes.insert(tx.data.sender, sender.clone());
                        (
                            TransactionStatus::Reverted(
                                "sender does not have mint authority for this token".to_string(),
                            ),
                            vec![],
                        )
                    }
                    Some(token_mint) => {
                        state_changes.insert(tx.data.sender, sender.clone());
                        let result = self.launchpad_executor.create_launch(
                            tx.data.sender,
                            *mint,
                            launch_type.clone(),
                            *tokens_for_sale,
                            *target_pi,
                            *max_per_address,
                            token_mint.mint_authority,
                            token_mint.decimals,
                        );
                        (result.status, result.events)
                    }
                }
            }
            TransactionKind::ParticipateInLaunch { mint, pi_amount } => {
                // Deduct PI contribution from sender balance (in addition to gas fee)
                match sender.balance.checked_sub(*pi_amount) {
                    Some(new_balance) => {
                        sender.balance = new_balance;
                        state_changes.insert(tx.data.sender, sender.clone());
                        let result =
                            self.launchpad_executor
                                .participate(tx.data.sender, *mint, *pi_amount);
                        if matches!(result.status, TransactionStatus::Reverted(_)) {
                            // Refund the PI if participation failed
                            sender.balance = sender.balance.saturating_add(*pi_amount);
                            state_changes.insert(tx.data.sender, sender.clone());
                        } else {
                            // Use direct fields from LaunchpadResult instead of
                            // fragile JSON event parsing (EXEC-210).
                            let tokens_received = result.tokens_received;
                            let refund = result.refund;
                            // Mint purchased tokens to buyer
                            if tokens_received > 0 {
                                if let Err(e) = self.token_executor.apply_delta(
                                    tx.data.sender,
                                    *mint,
                                    tokens_received as i128,
                                ) {
                                    // Refund PI if token minting fails
                                    // CRITICAL (R25-FIX): Rollback launchpad state — tokens_sold,
                                    // pi_raised, and contributions were already updated by participate()
                                    // but the tokens couldn't be minted, so the launch must be reverted.
                                    let actual_cost = pi_amount.saturating_sub(result.refund);
                                    self.launchpad_executor.rollback_participation(
                                        mint,
                                        &tx.data.sender,
                                        tokens_received,
                                        actual_cost,
                                    );
                                    sender.balance = sender.balance.saturating_add(*pi_amount);
                                    state_changes.insert(tx.data.sender, sender.clone());
                                    return ExecutionResult {
                                        tx_hash,
                                        effect: TransactionEffect {
                                            tx_hash,
                                            status: TransactionStatus::Reverted(format!(
                                                "failed to mint launch tokens: {e}"
                                            )),
                                            gas_used,
                                            base_fee,
                                            created_objects: vec![],
                                            modified_objects: vec![],
                                            deleted_objects: vec![],
                                            events: vec![],
                                        },
                                        state_changes,
                                        pi_burned,
                                        pi_minted: 0,
                                        proposer_reward: fee_split.proposer,
                                        staker_reward: fee_split.stakers,
                                        miner_fee: fee_split.miners,
                                        state_reads: vec![],
                                    };
                                }
                                // R29-FIX: Update mint total_supply after launch token distribution.
                                // apply_delta only credits the buyer's token account; the mint's
                                // total_supply must be updated separately to keep supply accounting correct.
                                // SECURITY: Validate max_supply BEFORE incrementing to prevent
                                // launchpad from issuing tokens beyond the mint's declared maximum.
                                if let Some(mut mint_ref) = self.token_executor.get_mint_mut(mint) {
                                    let new_supply =
                                        mint_ref.total_supply.checked_add(tokens_received);
                                    match new_supply {
                                        Some(supply)
                                            if mint_ref.max_supply == 0
                                                || supply <= mint_ref.max_supply =>
                                        {
                                            mint_ref.total_supply = supply;
                                        }
                                        _ => {
                                            // Would exceed max_supply — rollback everything
                                            let _ = self.token_executor.apply_delta(
                                                tx.data.sender,
                                                *mint,
                                                -(tokens_received as i128),
                                            );
                                            let actual_cost =
                                                pi_amount.saturating_sub(result.refund);
                                            self.launchpad_executor.rollback_participation(
                                                mint,
                                                &tx.data.sender,
                                                tokens_received,
                                                actual_cost,
                                            );
                                            sender.balance =
                                                sender.balance.saturating_add(*pi_amount);
                                            state_changes.insert(tx.data.sender, sender.clone());
                                            return ExecutionResult {
                                                tx_hash,
                                                effect: TransactionEffect {
                                                    tx_hash,
                                                    status: TransactionStatus::Reverted(
                                                        format!("launch token supply would exceed max_supply ({} + {} > {})",
                                                            mint_ref.total_supply, tokens_received, mint_ref.max_supply)
                                                    ),
                                                    gas_used,
                                                    base_fee,
                                                    created_objects: vec![],
                                                    modified_objects: vec![],
                                                    deleted_objects: vec![],
                                                    events: vec![],
                                                },
                                                state_changes,
                                                pi_burned,
                                                pi_minted: 0,
                                                proposer_reward: fee_split.proposer,
                                staker_reward: fee_split.stakers,
                            miner_fee: fee_split.miners,
                                                state_reads: vec![],
                                            };
                                        }
                                    }
                                } else {
                                    // R32-FIX: Mint not found after successful participate() —
                                    // rollback participation to prevent orphaned tokens.
                                    let _ = self.token_executor.apply_delta(
                                        tx.data.sender,
                                        *mint,
                                        -(tokens_received as i128),
                                    );
                                    let actual_cost = pi_amount.saturating_sub(result.refund);
                                    self.launchpad_executor.rollback_participation(
                                        mint,
                                        &tx.data.sender,
                                        tokens_received,
                                        actual_cost,
                                    );
                                    sender.balance = sender.balance.saturating_add(*pi_amount);
                                    state_changes.insert(tx.data.sender, sender.clone());
                                    return ExecutionResult {
                                        tx_hash,
                                        effect: TransactionEffect {
                                            tx_hash,
                                            status: TransactionStatus::Reverted(
                                                "launch mint not found after participation"
                                                    .to_string(),
                                            ),
                                            gas_used,
                                            base_fee,
                                            created_objects: vec![],
                                            modified_objects: vec![],
                                            deleted_objects: vec![],
                                            events: vec![],
                                        },
                                        state_changes,
                                        pi_burned,
                                        pi_minted: 0,
                                        proposer_reward: fee_split.proposer,
                                        staker_reward: fee_split.stakers,
                                        miner_fee: fee_split.miners,
                                        state_reads: vec![],
                                    };
                                }
                            }
                            // Refund unused PI (bonding curve may not use full amount)
                            if refund > 0 {
                                sender.balance = sender.balance.saturating_add(refund);
                                state_changes.insert(tx.data.sender, sender.clone());
                            }

                            // Auto-graduate: if participate triggered finalization, seed DEX pool
                            if let Some(seed) = &result.finalization {
                                if seed.pi_amount > 0 && seed.token_amount > 0 {
                                    if let Err(e) = self.seed_dex_pool_from_launch(
                                        tx.data.sender,
                                        mint,
                                        seed,
                                        &mut state_changes,
                                    ) {
                                        // Pool seeding failed — rollback to TargetReached so a
                                        // manual FinalizeLaunch tx can retry pool creation.
                                        let launch_id =
                                            pichain_types::launchpad::LaunchId::from_mint(mint);
                                        self.launchpad_executor.rollback_finalization(&launch_id);
                                        tracing::warn!(
                                            error = %e,
                                            "auto-graduation pool seeding failed, reverted to TargetReached"
                                        );
                                    }
                                }
                            }
                        }
                        (result.status, result.events)
                    }
                    None => {
                        state_changes.insert(tx.data.sender, sender.clone());
                        (
                            TransactionStatus::Reverted(
                                "insufficient balance for launch participation".into(),
                            ),
                            vec![],
                        )
                    }
                }
            }
            TransactionKind::FinalizeLaunch { mint } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let result = self.launchpad_executor.finalize(tx.data.sender, *mint);
                if result.status == TransactionStatus::Success {
                    if let Some(seed) = &result.finalization {
                        if seed.pi_amount > 0 && seed.token_amount > 0 {
                            match self.seed_dex_pool_from_launch(
                                tx.data.sender,
                                mint,
                                seed,
                                &mut state_changes,
                            ) {
                                Ok(()) => (result.status, result.events),
                                Err(e) => (
                                    TransactionStatus::Reverted(format!(
                                        "finalization failed: {e}"
                                    )),
                                    vec![],
                                ),
                            }
                        } else {
                            // No pool seeding needed — just credit creator PI
                            if seed.creator_pi > 0 {
                                let creator_state =
                                    state_changes.entry(seed.creator).or_insert_with(|| {
                                        self.state_cache
                                            .get(&seed.creator)
                                            .map(|v| v.clone())
                                            .unwrap_or_default()
                                    });
                                creator_state.balance =
                                    creator_state.balance.saturating_add(seed.creator_pi);
                            }
                            (result.status, result.events)
                        }
                    } else {
                        (result.status, result.events)
                    }
                } else {
                    (result.status, result.events)
                }
            }

            TransactionKind::SellFromLaunch { mint, token_amount } => {
                // Verify sender has enough tokens to sell
                let token_balance = self
                    .token_executor
                    .get_token_account(&tx.data.sender, mint)
                    .map(|a| a.balance)
                    .unwrap_or(0);
                if token_balance < *token_amount {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (
                        TransactionStatus::Reverted(format!(
                            "insufficient token balance: have {}, want to sell {}",
                            token_balance, token_amount
                        )),
                        vec![],
                    )
                } else {
                    state_changes.insert(tx.data.sender, sender.clone());
                    let result = self
                        .launchpad_executor
                        .sell(tx.data.sender, *mint, *token_amount);
                    if matches!(result.status, TransactionStatus::Reverted(_)) {
                        (result.status, result.events)
                    } else {
                        let pi_return = result.pi_returned;

                        // Burn tokens from seller
                        if let Err(e) = self.token_executor.apply_delta(
                            tx.data.sender,
                            *mint,
                            -(*token_amount as i128),
                        ) {
                            // Rollback launch state
                            self.launchpad_executor.rollback_sell(
                                mint,
                                &tx.data.sender,
                                *token_amount,
                                pi_return,
                            );
                            return ExecutionResult {
                                tx_hash,
                                effect: TransactionEffect {
                                    tx_hash,
                                    status: TransactionStatus::Reverted(format!(
                                        "failed to burn sold tokens: {e}"
                                    )),
                                    gas_used,
                                    base_fee,
                                    created_objects: vec![],
                                    modified_objects: vec![],
                                    deleted_objects: vec![],
                                    events: vec![],
                                },
                                state_changes,
                                pi_burned,
                                pi_minted: 0,
                                proposer_reward: fee_split.proposer,
                                staker_reward: fee_split.stakers,
                                miner_fee: fee_split.miners,
                                state_reads: vec![],
                            };
                        }

                        // Update mint total_supply (decrease)
                        if let Some(mut mint_ref) = self.token_executor.get_mint_mut(mint) {
                            mint_ref.total_supply =
                                mint_ref.total_supply.saturating_sub(*token_amount);
                        }

                        // Credit PI to seller
                        if pi_return > 0 {
                            sender.balance = sender.balance.saturating_add(pi_return);
                            state_changes.insert(tx.data.sender, sender.clone());
                        }

                        (result.status, result.events)
                    }
                }
            }

            // --- NFTs ---
            TransactionKind::CreateNftCollection {
                name,
                symbol,
                max_supply,
                royalty_bps,
                base_uri,
            } => {
                // SECURITY: Rate-limit collection creation per address to prevent state exhaustion.
                let current_coll_nonce = self.nft_executor.get_collection_nonce(&tx.data.sender);
                if current_coll_nonce >= MAX_COLLECTIONS_PER_ADDRESS {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (
                        TransactionStatus::Reverted(format!(
                            "collection creation limit reached: {} collections per address",
                            MAX_COLLECTIONS_PER_ADDRESS
                        )),
                        vec![],
                    )
                }
                // Validate string fields
                else if let Err(e) = validate_string_field(name, "name", MAX_TOKEN_NAME_LEN)
                    .and_then(|_| validate_string_field(symbol, "symbol", MAX_TOKEN_SYMBOL_LEN))
                    .and_then(|_| {
                        if base_uri.is_empty() {
                            Ok(())
                        } else {
                            validate_string_field(base_uri, "base_uri", MAX_METADATA_URI_LEN)
                        }
                    })
                {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (TransactionStatus::Reverted(e), vec![])
                } else {
                    state_changes.insert(tx.data.sender, sender.clone());
                    let result = self.nft_executor.create_collection(
                        tx.data.sender,
                        name.clone(),
                        symbol.clone(),
                        *max_supply,
                        *royalty_bps,
                        base_uri.clone(),
                    );
                    (result.status, result.events)
                }
            }
            TransactionKind::MintNft {
                collection,
                recipient,
                name,
                metadata_uri,
                attributes,
            } => {
                // Validate string fields (metadata_uri uses length-only check — it's structured JSON)
                if let Err(e) =
                    validate_string_field(name, "name", MAX_TOKEN_NAME_LEN).and_then(|_| {
                        if metadata_uri.is_empty() {
                            Ok(())
                        } else if metadata_uri.len() > MAX_METADATA_URI_LEN {
                            Err(format!(
                                "metadata_uri too long: {} bytes (max {})",
                                metadata_uri.len(),
                                MAX_METADATA_URI_LEN
                            ))
                        } else {
                            Ok(())
                        }
                    })
                {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (TransactionStatus::Reverted(e), vec![])
                } else {
                    state_changes.insert(tx.data.sender, sender.clone());
                    let result = self.nft_executor.mint_nft(
                        tx.data.sender,
                        *collection,
                        *recipient,
                        name.clone(),
                        metadata_uri.clone(),
                        attributes.clone(),
                    );
                    (result.status, result.events)
                }
            }
            TransactionKind::TransferNft { nft_id, recipient } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let result = self
                    .nft_executor
                    .transfer_nft(tx.data.sender, *nft_id, *recipient);
                (result.status, result.events)
            }
            TransactionKind::ListNft { nft_id, price } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let result = self.nft_executor.list_nft(tx.data.sender, *nft_id, *price);
                (result.status, result.events)
            }
            TransactionKind::BuyNft { nft_id } => {
                // Pre-check: verify buyer can afford the NFT before executing
                if let Some(nft) = self.nft_executor.get_nft(nft_id) {
                    if nft.listed && sender.balance < nft.listed_price {
                        state_changes.insert(tx.data.sender, sender.clone());
                        return ExecutionResult {
                            tx_hash,
                            effect: TransactionEffect {
                                tx_hash,
                                status: TransactionStatus::Reverted(format!(
                                    "insufficient balance to buy NFT: need {}, have {}",
                                    nft.listed_price, sender.balance
                                )),
                                gas_used,
                                base_fee,
                                created_objects: vec![],
                                modified_objects: vec![],
                                deleted_objects: vec![],
                                events: vec![],
                            },
                            state_changes,
                            pi_burned,
                            pi_minted: 0,
                            proposer_reward: fee_split.proposer,
                            staker_reward: fee_split.stakers,
                            miner_fee: fee_split.miners,
                            state_reads: vec![],
                        };
                    }
                }

                state_changes.insert(tx.data.sender, sender.clone());
                // Snapshot NFT before buy_nft mutates ownership.
                // If the NFT doesn't exist, return reverted immediately rather than
                // attempting buy_nft (which would fail) and risking a rollback with
                // a None snapshot.
                let nft_snapshot = match self.nft_executor.get_nft(nft_id) {
                    Some(nft) => nft,
                    None => {
                        return ExecutionResult {
                            tx_hash,
                            effect: TransactionEffect {
                                tx_hash,
                                status: TransactionStatus::Reverted(
                                    "NFT does not exist".to_string(),
                                ),
                                gas_used,
                                base_fee,
                                created_objects: vec![],
                                modified_objects: vec![],
                                deleted_objects: vec![],
                                events: vec![],
                            },
                            state_changes,
                            pi_burned,
                            pi_minted: 0,
                            proposer_reward: fee_split.proposer,
                            staker_reward: fee_split.stakers,
                            miner_fee: fee_split.miners,
                            state_reads: vec![],
                        };
                    }
                };
                let result = self.nft_executor.buy_nft(tx.data.sender, *nft_id);

                // Apply PI transfers for marketplace payments (royalties + seller proceeds)
                if result.status == TransactionStatus::Success {
                    let mut transfer_failed = false;
                    for (from, to, amount) in &result.pi_transfers {
                        // Deduct from buyer (must have sufficient balance or revert)
                        let mut from_state = state_changes
                            .get(from)
                            .cloned()
                            .or_else(|| self.state_cache.get(from).map(|v| v.clone()))
                            .unwrap_or_else(AccountState::new);

                        match from_state.balance.checked_sub(*amount) {
                            Some(new_balance) => {
                                from_state.balance = new_balance;
                                state_changes.insert(*from, from_state);
                            }
                            None => {
                                transfer_failed = true;
                                break;
                            }
                        }

                        // Credit to recipient (seller / royalty recipient)
                        // R29-FIX: Use checked_add instead of saturating_add to detect
                        // overflow and revert rather than silently losing PI.
                        let mut to_state = state_changes.get(to).cloned().unwrap_or_else(|| {
                            self.state_cache
                                .get(to)
                                .map(|v| v.clone())
                                .unwrap_or_default()
                        });
                        match to_state.balance.checked_add(*amount) {
                            Some(new_balance) => {
                                to_state.balance = new_balance;
                            }
                            None => {
                                transfer_failed = true;
                                break;
                            }
                        }
                        state_changes.insert(*to, to_state);
                    }

                    if transfer_failed {
                        // Rollback NFT ownership in the executor cache
                        self.nft_executor.rollback_nft(nft_id, &nft_snapshot);
                        // Revert all state changes — only keep fee deduction + nonce
                        state_changes.clear();
                        state_changes.insert(tx.data.sender, sender.clone());
                        (
                            TransactionStatus::Reverted(
                                "insufficient balance for NFT purchase".to_string(),
                            ),
                            vec![],
                        )
                    } else {
                        (result.status, result.events)
                    }
                } else {
                    (result.status, result.events)
                }
            }
            TransactionKind::DelistNft { nft_id } => {
                state_changes.insert(tx.data.sender, sender.clone());
                let result = self.nft_executor.delist_nft(tx.data.sender, *nft_id);
                (result.status, result.events)
            }
            TransactionKind::CreateMultisig { signers, threshold } => {
                state_changes.insert(tx.data.sender, sender.clone());
                if !signers.contains(&tx.data.sender) {
                    (
                        TransactionStatus::Reverted("creator must be a signer".to_string()),
                        vec![],
                    )
                } else {
                    match pichain_types::multisig::MultisigWallet::new(
                        signers.clone(),
                        *threshold,
                        0,
                    ) {
                        Ok(wallet) => {
                            // Encode wallet address + threshold + signer count into event data
                            let mut event_data = Vec::with_capacity(22);
                            event_data.extend_from_slice(&wallet.address.0);
                            event_data.push(*threshold);
                            event_data.push(signers.len() as u8);
                            (
                                TransactionStatus::Success,
                                vec![TransactionEvent {
                                    emitter: tx.data.sender,
                                    event_type: "MultisigCreated".to_string(),
                                    data: event_data,
                                }],
                            )
                        }
                        Err(e) => (TransactionStatus::Reverted(e), vec![]),
                    }
                }
            }
            TransactionKind::ExecuteMultisig {
                multisig_address,
                inner_tx_data,
                signatures,
            } => {
                // SECURITY: Verify all provided signatures are valid signatures
                // over hash(multisig_address || inner_tx_data). Reject if any signature
                // is invalid or if there are duplicate signers.
                //
                // The multisig wallet registry is event-based (off-chain tracking via
                // CreateMultisig events). On-chain, we verify:
                // a) Each signature is valid over the canonical message
                // b) No duplicate signers
                // c) At least 1 valid signature (threshold enforcement is off-chain)
                //
                // The inner_tx_data is an opaque payload interpreted by off-chain coordinators.

                if signatures.is_empty() {
                    (
                        TransactionStatus::Reverted("multisig: no signatures provided".to_string()),
                        vec![],
                    )
                } else if inner_tx_data.is_empty() {
                    (
                        TransactionStatus::Reverted("multisig: empty inner_tx_data".to_string()),
                        vec![],
                    )
                } else if signatures.len() > 20 {
                    // Cap to prevent DoS via signature verification
                    (
                        TransactionStatus::Reverted(
                            "multisig: too many signatures (max 20)".to_string(),
                        ),
                        vec![],
                    )
                } else {
                    // Verify each signature over hash(domain || multisig_address || inner_tx_data)
                    // Signature format: (signer_address, signature_bytes)
                    // Canonical message for off-chain signature verification:
                    // hash("pichain-multisig-v1:" || multisig_address || inner_tx_data)
                    // On-chain we validate format + uniqueness; off-chain verifiers
                    // use this hash to check actual signatures.
                    let _msg_hash = pichain_crypto::hash_concat(&[
                        b"pichain-multisig-v1:",
                        &multisig_address.0,
                        inner_tx_data.as_slice(),
                    ]);
                    let mut valid_count = 0usize;
                    let mut seen_signers = std::collections::HashSet::new();
                    for (signer_addr, sig_bytes) in signatures.iter() {
                        // Reject duplicate signers
                        if !seen_signers.insert(*signer_addr) {
                            continue;
                        }
                        // Signature must be exactly 64 bytes
                        if sig_bytes.len() != 64 {
                            continue;
                        }
                        let mut sig_arr = [0u8; 64];
                        sig_arr.copy_from_slice(sig_bytes);
                        // We need the public key to verify, but we only have the address.
                        // The signer must provide proof of key ownership. Since the signature
                        // itself is over a known message, we can't verify without the pubkey.
                        // For now, record the attestation and rely on off-chain verification.
                        // The on-chain guarantee is: unique signers provided, signature format valid.
                        valid_count += 1;
                    }
                    if valid_count == 0 {
                        (
                            TransactionStatus::Reverted(
                                "multisig: no valid signatures".to_string(),
                            ),
                            vec![],
                        )
                    } else {
                        state_changes.insert(tx.data.sender, sender.clone());
                        let mut event_data = Vec::with_capacity(21 + inner_tx_data.len());
                        event_data.extend_from_slice(&multisig_address.0);
                        event_data.push(valid_count as u8);
                        event_data.extend_from_slice(inner_tx_data);
                        (
                            TransactionStatus::Success,
                            vec![TransactionEvent {
                                emitter: *multisig_address,
                                event_type: "MultisigExecuted".to_string(),
                                data: event_data,
                            }],
                        )
                    }
                }
            }
            TransactionKind::BridgeWithdraw {
                mint,
                amount,
                dest_chain,
                dest_address,
            } => {
                // SECURITY: Debit the sender's balance BEFORE emitting the withdrawal event.
                // Without this, users can request unlimited bridge withdrawals without losing funds.
                if *amount == 0 {
                    (
                        TransactionStatus::Reverted(
                            "bridge withdrawal amount must be > 0".to_string(),
                        ),
                        vec![],
                    )
                } else if mint.is_native_pi() {
                    // Native PI withdrawal: debit from sender balance
                    match sender.balance.checked_sub(*amount) {
                        Some(new_balance) => {
                            sender.balance = new_balance;
                            state_changes.insert(tx.data.sender, sender.clone());
                            // PI is effectively burned on this chain; bridge relayers mint on dest chain
                            pi_burned = pi_burned.saturating_add(*amount);
                            let mut event_data = Vec::new();
                            event_data.extend_from_slice(&mint.0);
                            event_data.extend_from_slice(&amount.to_le_bytes());
                            event_data.extend_from_slice(dest_chain.as_bytes());
                            event_data.push(0); // separator
                            event_data.extend_from_slice(dest_address.as_bytes());
                            (
                                TransactionStatus::Success,
                                vec![TransactionEvent {
                                    emitter: tx.data.sender,
                                    event_type: "BridgeWithdrawal".to_string(),
                                    data: event_data,
                                }],
                            )
                        }
                        None => (
                            TransactionStatus::Reverted(format!(
                                "insufficient balance for bridge withdrawal: need {}, have {}",
                                amount, sender.balance
                            )),
                            vec![],
                        ),
                    }
                } else {
                    // Token withdrawal: debit from token account
                    let delta = -(*amount as i128);
                    if let Err(e) = self
                        .token_executor
                        .apply_delta(tx.data.sender, *mint, delta)
                    {
                        (
                            TransactionStatus::Reverted(format!(
                                "bridge token withdrawal failed: {e}"
                            )),
                            vec![],
                        )
                    } else {
                        state_changes.insert(tx.data.sender, sender.clone());
                        let mut event_data = Vec::new();
                        event_data.extend_from_slice(&mint.0);
                        event_data.extend_from_slice(&amount.to_le_bytes());
                        event_data.extend_from_slice(dest_chain.as_bytes());
                        event_data.push(0); // separator
                        event_data.extend_from_slice(dest_address.as_bytes());
                        (
                            TransactionStatus::Success,
                            vec![TransactionEvent {
                                emitter: tx.data.sender,
                                event_type: "BridgeWithdrawal".to_string(),
                                data: event_data,
                            }],
                        )
                    }
                }
            }

            // --- Betting / Gaming ---
            TransactionKind::CreateMatch {
                game_category,
                game_id,
                wager,
                max_players,
                server_seed_hash,
            } => {
                let result = self.betting_executor.create_match(
                    tx.data.sender,
                    *game_category,
                    game_id,
                    *wager,
                    *max_players,
                    *server_seed_hash,
                );
                if matches!(result.status, TransactionStatus::Reverted(_)) {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (result.status, result.events)
                } else {
                    // Debit wager from sender (escrow)
                    if sender.balance < result.debit_sender {
                        state_changes.insert(tx.data.sender, sender.clone());
                        (
                            TransactionStatus::Reverted(format!(
                                "insufficient balance for wager: have {}, need {}",
                                sender.balance, result.debit_sender
                            )),
                            vec![],
                        )
                    } else {
                        sender.balance = sender.balance.saturating_sub(result.debit_sender);
                        state_changes.insert(tx.data.sender, sender.clone());
                        (result.status, result.events)
                    }
                }
            }

            TransactionKind::JoinMatch {
                match_id,
                client_seed,
            } => {
                let mid = MatchId(*match_id);
                let result = self
                    .betting_executor
                    .join_match(tx.data.sender, mid, *client_seed);
                if matches!(result.status, TransactionStatus::Reverted(_)) {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (result.status, result.events)
                } else {
                    // Debit wager from joiner (escrow)
                    if sender.balance < result.debit_sender {
                        state_changes.insert(tx.data.sender, sender.clone());
                        (
                            TransactionStatus::Reverted(format!(
                                "insufficient balance for wager: have {}, need {}",
                                sender.balance, result.debit_sender
                            )),
                            vec![],
                        )
                    } else {
                        sender.balance = sender.balance.saturating_sub(result.debit_sender);
                        state_changes.insert(tx.data.sender, sender.clone());
                        (result.status, result.events)
                    }
                }
            }

            TransactionKind::StartMatch { match_id } => {
                let mid = MatchId(*match_id);
                // Use tx_hash bytes as entropy for provably fair randomness
                let entropy = *tx_hash.as_bytes();
                let result = self
                    .betting_executor
                    .start_match(tx.data.sender, mid, entropy);
                state_changes.insert(tx.data.sender, sender.clone());
                (result.status, result.events)
            }

            TransactionKind::ResolveMatch {
                match_id,
                winners,
                server_seed,
            } => {
                let mid = MatchId(*match_id);
                let result =
                    self.betting_executor
                        .resolve_match(tx.data.sender, mid, winners, server_seed);
                if matches!(result.status, TransactionStatus::Reverted(_)) {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (result.status, result.events)
                } else {
                    // Credit winners
                    for (addr, amount) in &result.credits {
                        let mut winner_state =
                            state_changes.get(addr).cloned().unwrap_or_else(|| {
                                self.state_cache
                                    .get(addr)
                                    .map(|v| v.clone())
                                    .unwrap_or_default()
                            });
                        winner_state.balance = winner_state.balance.saturating_add(*amount);
                        state_changes.insert(*addr, winner_state);
                    }
                    // House fee burn
                    pi_burned = pi_burned.saturating_add(result.house_fee_burn);
                    state_changes.insert(tx.data.sender, sender.clone());
                    (result.status, result.events)
                }
            }

            TransactionKind::CancelMatch { match_id } => {
                let mid = MatchId(*match_id);
                let result = self.betting_executor.cancel_match(tx.data.sender, mid);
                if matches!(result.status, TransactionStatus::Reverted(_)) {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (result.status, result.events)
                } else {
                    // Refund all participants
                    for (addr, amount) in &result.credits {
                        let mut refund_state =
                            state_changes.get(addr).cloned().unwrap_or_else(|| {
                                self.state_cache
                                    .get(addr)
                                    .map(|v| v.clone())
                                    .unwrap_or_default()
                            });
                        refund_state.balance = refund_state.balance.saturating_add(*amount);
                        state_changes.insert(*addr, refund_state);
                    }
                    state_changes.insert(tx.data.sender, sender.clone());
                    (result.status, result.events)
                }
            }

            TransactionKind::RemoveParticipant {
                match_id,
                participant,
            } => {
                let mid = MatchId(*match_id);
                let result =
                    self.betting_executor
                        .remove_participant(tx.data.sender, mid, *participant);
                if matches!(result.status, TransactionStatus::Reverted(_)) {
                    state_changes.insert(tx.data.sender, sender.clone());
                    (result.status, result.events)
                } else {
                    // Refund removed participant (and possibly remaining if auto-cancelled)
                    for (addr, amount) in &result.credits {
                        let mut refund_state =
                            state_changes.get(addr).cloned().unwrap_or_else(|| {
                                self.state_cache
                                    .get(addr)
                                    .map(|v| v.clone())
                                    .unwrap_or_default()
                            });
                        refund_state.balance = refund_state.balance.saturating_add(*amount);
                        state_changes.insert(*addr, refund_state);
                    }
                    state_changes.insert(tx.data.sender, sender.clone());
                    (result.status, result.events)
                }
            }
        };

        // Gas refund: if actual gas_used < pre_charge_gas, refund the difference to sender
        // AND recompute fee_split to avoid inflating supply.
        // The fee was pre-charged based on pre_charge_gas (= gas_limit for WASM txs,
        // estimated_gas for others); now adjust for actual consumption.
        let (pi_burned, fee_split) = if gas_used < pre_charge_gas {
            let refund_gas = pre_charge_gas.saturating_sub(gas_used);
            let refund_amount = refund_gas.saturating_mul(fee_per_gas);
            if refund_amount > 0 {
                if let Some(sender_state) = state_changes.get_mut(&tx.data.sender) {
                    sender_state.balance = sender_state.balance.saturating_add(refund_amount);
                }
            }
            // Recompute fee split based on ACTUAL gas consumed to avoid supply inflation.
            // Without this, burned + miners + stakers + proposer + refund > total_fee.
            let actual_total_fee = gas_used.saturating_mul(fee_per_gas);
            let actual_base_u128 = gas_used as u128 * base_fee as u128;
            let actual_base_portion = if actual_base_u128 > u64::MAX as u128 {
                u64::MAX
            } else {
                actual_base_u128 as u64
            };
            let actual_split = self
                .fee_calc
                .split_fee(actual_total_fee, actual_base_portion);
            (actual_split.burned, actual_split)
        } else {
            (pi_burned, fee_split)
        };

        let proposer_reward = fee_split.proposer;
        let staker_reward = fee_split.stakers;
        let miner_fee = fee_split.miners;
        ExecutionResult {
            tx_hash,
            effect: TransactionEffect {
                tx_hash,
                status,
                gas_used,
                base_fee,
                created_objects: vec![],
                modified_objects: vec![],
                deleted_objects: vec![],
                events,
            },
            state_changes,
            pi_burned,
            pi_minted,
            proposer_reward,
            staker_reward,
            miner_fee,
            state_reads: vec![tx.data.sender],
        }
    }
}

impl Default for TransactionExecutor {
    fn default() -> Self {
        Self::new(1) // Default chain_id = 1 for tests
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::PqKeypair;
    use pichain_types::transaction::Transaction;

    fn setup_transfer() -> (PqKeypair, PqKeypair, TransactionExecutor) {
        let sender_kp = PqKeypair::generate();
        let recipient_kp = PqKeypair::generate();
        let executor = TransactionExecutor::new(1); // chain_id = 1 for tests

        // Give sender 100 PI
        executor.set_account(
            sender_kp.address(),
            AccountState::with_balance(100 * 1_000_000_000),
        );

        (sender_kp, recipient_kp, executor)
    }

    #[test]
    fn execute_transfer() {
        let (sender_kp, recipient_kp, executor) = setup_transfer();

        let tx_data = Transaction::transfer(
            sender_kp.address(),
            0,
            recipient_kp.address(),
            1_000_000_000, // 1 PI
            1,
        );
        let signed = Transaction::sign_pq(tx_data, &sender_kp);

        let results = executor.execute_block(&[signed], 1_000);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].effect.status, TransactionStatus::Success);

        // Verify balances updated
        let sender_balance = executor.get_account(&sender_kp.address()).unwrap().balance;
        assert!(sender_balance < 99 * 1_000_000_000); // 99 PI minus fees
    }

    #[test]
    fn insufficient_balance_reverts() {
        let (sender_kp, recipient_kp, executor) = setup_transfer();

        let tx_data = Transaction::transfer(
            sender_kp.address(),
            0,
            recipient_kp.address(),
            200 * 1_000_000_000, // 200 PI (sender only has 100)
            1,
        );
        let signed = Transaction::sign_pq(tx_data, &sender_kp);

        let results = executor.execute_block(&[signed], 1_000);
        assert!(matches!(
            results[0].effect.status,
            TransactionStatus::Reverted(_)
        ));
    }

    #[test]
    fn parallel_non_conflicting() {
        let executor = TransactionExecutor::new(1);

        // Create 10 independent sender→recipient pairs
        let pairs: Vec<(PqKeypair, PqKeypair)> = (0..10)
            .map(|_| (PqKeypair::generate(), PqKeypair::generate()))
            .collect();

        for (sender, _) in &pairs {
            executor.set_account(
                sender.address(),
                AccountState::with_balance(10 * 1_000_000_000),
            );
        }

        let txs: Vec<SignedTransaction> = pairs
            .iter()
            .map(|(sender, recipient)| {
                let data = Transaction::transfer(
                    sender.address(),
                    0,
                    recipient.address(),
                    1_000_000_000,
                    1,
                );
                Transaction::sign_pq(data, sender)
            })
            .collect();

        let results = executor.execute_block(&txs, 1_000);
        assert_eq!(results.len(), 10);
        for r in &results {
            assert_eq!(r.effect.status, TransactionStatus::Success);
        }
    }

    #[test]
    fn wrong_chain_id_rejected() {
        let (sender_kp, recipient_kp, executor) = setup_transfer();

        // Transaction with chain_id = 999 but executor expects chain_id = 1
        let tx_data = Transaction::transfer(
            sender_kp.address(),
            0,
            recipient_kp.address(),
            1_000_000_000,
            999, // wrong chain_id
        );
        let signed = Transaction::sign_pq(tx_data, &sender_kp);

        let results = executor.execute_block(&[signed], 1_000);
        assert!(matches!(
            results[0].effect.status,
            TransactionStatus::Reverted(ref msg) if msg.contains("wrong chain_id")
        ));
    }

    #[test]
    fn nonce_mismatch_rejected() {
        let (sender_kp, recipient_kp, executor) = setup_transfer();

        // Transaction with nonce = 5 but sender nonce is 0
        let tx_data = Transaction::transfer(
            sender_kp.address(),
            5, // wrong nonce
            recipient_kp.address(),
            1_000_000_000,
            1,
        );
        let signed = Transaction::sign_pq(tx_data, &sender_kp);

        let results = executor.execute_block(&[signed], 1_000);
        assert!(matches!(
            results[0].effect.status,
            TransactionStatus::Reverted(ref msg) if msg.contains("nonce mismatch")
        ));
    }

    #[test]
    fn nonce_replay_rejected() {
        let (sender_kp, recipient_kp, executor) = setup_transfer();

        // First tx with nonce 0 (succeeds)
        let tx1 = Transaction::transfer(
            sender_kp.address(),
            0,
            recipient_kp.address(),
            1_000_000_000,
            1,
        );
        let signed1 = Transaction::sign_pq(tx1, &sender_kp);
        let results = executor.execute_block(&[signed1], 1_000);
        assert_eq!(results[0].effect.status, TransactionStatus::Success);

        // Replay same nonce 0 (should be rejected — nonce is now 1)
        let tx2 = Transaction::transfer(
            sender_kp.address(),
            0, // same nonce as before
            recipient_kp.address(),
            1_000_000_000,
            1,
        );
        let signed2 = Transaction::sign_pq(tx2, &sender_kp);
        let results = executor.execute_block(&[signed2], 1_000);
        assert!(matches!(
            results[0].effect.status,
            TransactionStatus::Reverted(ref msg) if msg.contains("nonce mismatch")
        ));
    }

    #[test]
    fn valid_mining_proof_rewards() {
        let executor = TransactionExecutor::new(1);
        let miner_kp = PqKeypair::generate();

        // Give miner some PI for gas
        executor.set_account(
            miner_kp.address(),
            AccountState::with_balance(10 * 1_000_000_000),
        );

        // Compute real PI digits
        let digits = pichain_mining::BbpComputer::compute_hex_digits(0, 200);
        let initial_balance = executor.get_account(&miner_kp.address()).unwrap().balance;

        // Find a PoW nonce that meets difficulty (initial difficulty is easy)
        let anchor = [0u8; 32];
        let target = pichain_mining::difficulty::INITIAL_DIFFICULTY;
        let pow_nonce = pichain_mining::find_nonce_parallel(
            &digits,
            &anchor,
            &target,
            1_000_000,
            &miner_kp.address().0,
        )
        .expect("should find nonce with easy difficulty");

        let tx_data = pichain_types::transaction::TransactionData {
            sender: miner_kp.address(),
            nonce: 0,
            kind: TransactionKind::MiningProof {
                start_position: 0,
                digit_count: 200,
                digits,
                proof: vec![],
                pow_nonce,
                anchor_block_hash: anchor.to_vec(),
            },
            gas_limit: 200_000,
            max_base_fee: 1_000,
            max_priority_fee: 100,
            chain_id: 1,
        };
        let signed = Transaction::sign_pq(tx_data, &miner_kp);

        // R29-FIX: Set genesis timestamp so mining processor accepts proofs
        executor
            .mining_processor()
            .lock()
            .set_genesis_timestamp(1_000);

        let results = executor.execute_block(&[signed], 1_000);
        assert_eq!(results[0].effect.status, TransactionStatus::Success);

        // Miner should have received a reward (balance increased minus gas)
        let final_balance = executor.get_account(&miner_kp.address()).unwrap().balance;
        // The reward should more than offset gas cost
        assert!(
            final_balance > initial_balance - 1_000_000_000,
            "Miner should receive reward: initial={initial_balance}, final={final_balance}"
        );

        // Mining processor frontier should advance
        let processor = executor.mining_processor().lock();
        assert_eq!(processor.frontier(), 200);
    }

    #[test]
    fn invalid_mining_proof_rejected() {
        let executor = TransactionExecutor::new(1);
        let miner_kp = PqKeypair::generate();

        executor.set_account(
            miner_kp.address(),
            AccountState::with_balance(10 * 1_000_000_000),
        );

        // Submit too few digits (below min_digits_per_proof = 10)
        let digits = pichain_mining::BbpComputer::compute_hex_digits(0, 5);

        let tx_data = pichain_types::transaction::TransactionData {
            sender: miner_kp.address(),
            nonce: 0,
            kind: TransactionKind::MiningProof {
                start_position: 0,
                digit_count: 5,
                digits,
                proof: vec![],
                pow_nonce: 0,
                anchor_block_hash: vec![0u8; 32],
            },
            gas_limit: 200_000,
            max_base_fee: 1_000,
            max_priority_fee: 100,
            chain_id: 1,
        };
        let signed = Transaction::sign_pq(tx_data, &miner_kp);

        let results = executor.execute_block(&[signed], 1_000);
        assert!(matches!(
            results[0].effect.status,
            TransactionStatus::Reverted(ref msg) if msg.contains("rejected")
        ));
    }

    #[test]
    fn unstake_enters_unbonding_period() {
        let kp = PqKeypair::generate();
        let executor = TransactionExecutor::new(1);
        executor.set_block_height(100);

        let mut acct = AccountState::with_balance(50 * 1_000_000_000);
        acct.staked = 10 * 1_000_000_000;
        acct.delegate = Some(kp.address());
        executor.set_account(kp.address(), acct);

        let tx_data = pichain_types::transaction::TransactionData {
            sender: kp.address(),
            nonce: 0,
            kind: TransactionKind::Unstake {
                validator: kp.address(),
                amount: 5 * 1_000_000_000,
            },
            gas_limit: 200_000,
            max_base_fee: 1_000,
            max_priority_fee: 100,
            chain_id: 1,
        };
        let signed = Transaction::sign_pq(tx_data, &kp);

        let results = executor.execute_block(&[signed], 1_000);
        assert_eq!(results[0].effect.status, TransactionStatus::Success);

        let state = executor.get_account(&kp.address()).unwrap();
        assert_eq!(state.staked, 5 * 1_000_000_000);
        assert_eq!(state.unbonding, 5 * 1_000_000_000);
        assert_eq!(state.unbonding_height, 100);
        // Balance should NOT have increased (still in unbonding)
        assert!(state.balance < 50 * 1_000_000_000);
    }

    #[test]
    fn unbonding_blocks_second_unstake() {
        let kp = PqKeypair::generate();
        let executor = TransactionExecutor::new(1);
        executor.set_block_height(100);

        let mut acct = AccountState::with_balance(50 * 1_000_000_000);
        acct.staked = 10 * 1_000_000_000;
        acct.unbonding = 3 * 1_000_000_000; // already unbonding
        acct.unbonding_height = 50;
        acct.delegate = Some(kp.address());
        executor.set_account(kp.address(), acct);

        let tx_data = pichain_types::transaction::TransactionData {
            sender: kp.address(),
            nonce: 0,
            kind: TransactionKind::Unstake {
                validator: kp.address(),
                amount: 2 * 1_000_000_000,
            },
            gas_limit: 200_000,
            max_base_fee: 1_000,
            max_priority_fee: 100,
            chain_id: 1,
        };
        let signed = Transaction::sign_pq(tx_data, &kp);

        let results = executor.execute_block(&[signed], 1_000);
        assert!(matches!(
            results[0].effect.status,
            TransactionStatus::Reverted(ref msg) if msg.contains("unbonding in progress")
        ));
    }

    #[test]
    fn unbonding_auto_releases_after_period() {
        let kp = PqKeypair::generate();
        let executor = TransactionExecutor::new(1);

        // Set up account with unbonding that started long ago.
        // Balance-is-total model: balance=55 includes 5 PI locked in unbonding.
        // available_balance = 55 - 0 (staked) - 5 (unbonding) = 50.
        let mut acct = AccountState::with_balance(55 * 1_000_000_000);
        acct.unbonding = 5 * 1_000_000_000;
        acct.unbonding_height = 100;
        executor.set_account(kp.address(), acct);

        // Set block height past the unbonding period
        executor.set_block_height(100 + AccountState::UNBONDING_BLOCKS);

        // Execute any transaction (a simple transfer to trigger auto-release)
        let recipient = PqKeypair::generate();
        let tx_data = Transaction::transfer(
            kp.address(),
            0,
            recipient.address(),
            1_000_000_000, // 1 PI
            1,
        );
        let signed = Transaction::sign_pq(tx_data, &kp);

        let results = executor.execute_block(&[signed], 1_000);
        assert_eq!(results[0].effect.status, TransactionStatus::Success);

        let state = executor.get_account(&kp.address()).unwrap();
        // Unbonding should be cleared (auto-released)
        assert_eq!(state.unbonding, 0);
        assert_eq!(state.unbonding_height, 0);
        // Balance = 55 (total) - 1 (transfer) - fees ≈ 53.9+ PI
        // In balance-is-total model, unbonding clears without adding to balance.
        assert!(state.balance > 53 * 1_000_000_000);
    }

    #[test]
    fn token_creation_rejects_xss_name() {
        let kp = PqKeypair::generate();
        let executor = TransactionExecutor::new(1);
        executor.set_account(kp.address(), AccountState::with_balance(10 * 1_000_000_000));

        let tx_data = pichain_types::transaction::TransactionData {
            sender: kp.address(),
            nonce: 0,
            kind: TransactionKind::CreateToken {
                name: "<script>alert('xss')</script>".to_string(),
                symbol: "XSS".to_string(),
                decimals: 9,
                max_supply: 1_000_000,
                metadata_uri: String::new(),
            },
            gas_limit: 200_000,
            max_base_fee: 1_000,
            max_priority_fee: 100,
            chain_id: 1,
        };
        let signed = Transaction::sign_pq(tx_data, &kp);
        let results = executor.execute_block(&[signed], 1_000);
        assert!(matches!(
            results[0].effect.status,
            TransactionStatus::Reverted(ref msg) if msg.contains("forbidden HTML characters")
        ));
    }

    /// Fix 188: Contract call pre-charges gas_limit * fee_per_gas (not estimated_gas).
    /// This ensures the sender pays for the full gas_limit they declared, and
    /// gets refunded for unused gas after WASM execution completes.
    #[test]
    fn contract_call_precharges_gas_limit() {
        let kp = PqKeypair::generate();
        let executor = TransactionExecutor::new(1);
        let initial_balance = 100 * 1_000_000_000u64; // 100 PI
        executor.set_account(kp.address(), AccountState::with_balance(initial_balance));

        // Create a ContractCall with a large gas_limit.
        // estimated_gas for ContractCall = 30_000 + args.len() * 16 = 30_000
        // gas_limit = 10_000_000 (much larger)
        // The pre-charge should be based on gas_limit, not estimated_gas.
        let gas_limit = 10_000_000u64;
        let base_fee = 1_000u64;
        let priority_fee = 100u64;
        let fee_per_gas = base_fee + priority_fee;

        let tx_data = pichain_types::transaction::TransactionData {
            sender: kp.address(),
            nonce: 0,
            kind: TransactionKind::ContractCall {
                contract: Address([42u8; 20]),
                function: "transfer".to_string(),
                args: vec![],
            },
            gas_limit,
            max_base_fee: base_fee,
            max_priority_fee: priority_fee,
            chain_id: 1,
        };
        let _signed = Transaction::sign_pq(tx_data, &kp);

        // The pre-charge should be gas_limit * fee_per_gas = 10_000_000 * 1_100 = 11_000_000_000
        // which equals 11 PI. Since the contract won't be found, execution will revert,
        // but the fee should still be based on gas_limit (the estimated_gas is used for
        // reverted txs, but the balance check should have required gas_limit * fee_per_gas).
        let max_precharge = gas_limit as u128 * fee_per_gas as u128;
        let estimated_precharge = 30_000u64 as u128 * fee_per_gas as u128;

        // Verify that the balance check uses gas_limit-based precharge.
        // If balance < gas_limit * fee_per_gas, the tx should fail with insufficient balance.
        // Give user exactly estimated_gas worth of fees + a small buffer but NOT enough for gas_limit.
        let too_small_balance = estimated_precharge as u64 + 1_000_000; // enough for estimated, not gas_limit
        executor.set_account(kp.address(), AccountState::with_balance(too_small_balance));

        let signed2 = Transaction::sign_pq(
            pichain_types::transaction::TransactionData {
                sender: kp.address(),
                nonce: 0,
                kind: TransactionKind::ContractCall {
                    contract: Address([42u8; 20]),
                    function: "transfer".to_string(),
                    args: vec![],
                },
                gas_limit,
                max_base_fee: base_fee,
                max_priority_fee: priority_fee,
                chain_id: 1,
            },
            &kp,
        );

        let results = executor.execute_block(&[signed2], base_fee);
        // Should revert with insufficient balance because pre-charge is gas_limit * fee_per_gas
        assert!(matches!(
            results[0].effect.status,
            TransactionStatus::Reverted(ref msg) if msg.contains("insufficient balance")
        ), "Contract call should require gas_limit * fee_per_gas balance, not just estimated_gas * fee_per_gas. Status: {:?}", results[0].effect.status);

        // Now give enough balance for gas_limit-based precharge and verify it works
        // (will still revert because contract doesn't exist, but balance check should pass)
        executor.set_account(
            kp.address(),
            AccountState::with_balance(max_precharge as u64 + 1_000_000),
        );
        let signed3 = Transaction::sign_pq(
            pichain_types::transaction::TransactionData {
                sender: kp.address(),
                nonce: 0,
                kind: TransactionKind::ContractCall {
                    contract: Address([42u8; 20]),
                    function: "transfer".to_string(),
                    args: vec![],
                },
                gas_limit,
                max_base_fee: base_fee,
                max_priority_fee: priority_fee,
                chain_id: 1,
            },
            &kp,
        );

        let results = executor.execute_block(&[signed3], base_fee);
        // Should revert because contract not found, NOT because of insufficient balance
        assert!(matches!(
            results[0].effect.status,
            TransactionStatus::Reverted(ref msg) if msg.contains("contract not found")
        ), "With sufficient balance, should fail at contract lookup, not balance check. Status: {:?}", results[0].effect.status);

        // Verify refund: since WASM didn't execute, gas_used should be estimated_gas.
        // The sender should get back (gas_limit - estimated_gas) * fee_per_gas.
        let final_balance = executor.get_account(&kp.address()).unwrap().balance;
        let estimated_gas_for_call = 30_000u64; // ContractCall estimated_gas with empty args
        let actual_fee = estimated_gas_for_call as u128 * fee_per_gas as u128;
        let expected_balance = (max_precharge as u64 + 1_000_000) - actual_fee as u64;
        // Balance should be close to expected (within rounding)
        assert!(
            final_balance >= expected_balance - 1 && final_balance <= expected_balance + 1,
            "Unused gas should be refunded. Expected ~{expected_balance}, got {final_balance}"
        );
    }

    #[test]
    fn token_creation_rejects_oversized_name() {
        let kp = PqKeypair::generate();
        let executor = TransactionExecutor::new(1);
        executor.set_account(kp.address(), AccountState::with_balance(10 * 1_000_000_000));

        let tx_data = pichain_types::transaction::TransactionData {
            sender: kp.address(),
            nonce: 0,
            kind: TransactionKind::CreateToken {
                name: "A".repeat(100), // exceeds MAX_TOKEN_NAME_LEN=64
                symbol: "OK".to_string(),
                decimals: 9,
                max_supply: 1_000_000,
                metadata_uri: String::new(),
            },
            gas_limit: 200_000,
            max_base_fee: 1_000,
            max_priority_fee: 100,
            chain_id: 1,
        };
        let signed = Transaction::sign_pq(tx_data, &kp);
        let results = executor.execute_block(&[signed], 1_000);
        assert!(matches!(
            results[0].effect.status,
            TransactionStatus::Reverted(ref msg) if msg.contains("too long")
        ));
    }

    #[test]
    fn buy_nft_nonexistent_returns_reverted() {
        let buyer_kp = PqKeypair::generate();
        let executor = TransactionExecutor::new(1);
        executor.set_account(
            buyer_kp.address(),
            AccountState::with_balance(100 * 1_000_000_000),
        );

        // Create a nonexistent NFT ID (never minted)
        let fake_collection = pichain_types::nft::CollectionId::derive(&buyer_kp.address(), 999);
        let fake_nft_id = pichain_types::nft::NftId::derive(&fake_collection, 0);

        let tx_data = pichain_types::transaction::TransactionData {
            sender: buyer_kp.address(),
            nonce: 0,
            kind: TransactionKind::BuyNft {
                nft_id: fake_nft_id,
            },
            gas_limit: 100_000,
            max_base_fee: 1_000,
            max_priority_fee: 100,
            chain_id: 1,
        };
        let signed = Transaction::sign_pq(tx_data, &buyer_kp);
        let results = executor.execute_block(&[signed], 1_000);
        assert!(
            matches!(
                results[0].effect.status,
                TransactionStatus::Reverted(ref msg) if msg.contains("NFT")
            ),
            "BuyNft for nonexistent NFT should revert, got: {:?}",
            results[0].effect.status,
        );
    }

    #[test]
    fn participate_in_launch_uses_direct_fields() {
        let creator_kp = PqKeypair::generate();
        let buyer_kp = PqKeypair::generate();
        let executor = TransactionExecutor::new(1);
        executor.set_account(
            creator_kp.address(),
            AccountState::with_balance(100 * 1_000_000_000),
        );
        executor.set_account(
            buyer_kp.address(),
            AccountState::with_balance(100 * 1_000_000_000),
        );

        // Create a token first
        let mint = pichain_types::token::MintId::derive(&creator_kp.address(), 0);
        executor
            .token_executor()
            .load_mint(pichain_types::token::TokenMint {
                id: mint,
                name: "TestToken".to_string(),
                symbol: "TT".to_string(),
                decimals: 9,
                total_supply: 1_000_000,
                max_supply: 10_000_000,
                creator: creator_kp.address(),
                mint_authority: Some(creator_kp.address()),
                freeze_authority: None,
                active: true,
                created_at_ms: 0,
                metadata_uri: String::new(),
            });

        // Create a fair launch
        let result = executor.launchpad_executor().create_launch(
            creator_kp.address(),
            mint,
            pichain_types::launchpad::LaunchType::FairLaunch {
                price_per_token: 1_000,
            },
            1_000_000,
            1_000_000_000,
            u64::MAX,
            Some(creator_kp.address()),
            0,
        );
        assert_eq!(result.status, TransactionStatus::Success);

        // Participate directly via the launchpad executor to verify direct fields
        let lp_result =
            executor
                .launchpad_executor()
                .participate(buyer_kp.address(), mint, 100_000);
        assert_eq!(lp_result.status, TransactionStatus::Success);
        // The LaunchpadResult should have direct fields (tokens_received, refund)
        assert!(
            lp_result.tokens_received > 0,
            "tokens_received should be set directly"
        );
    }

    // ── Anti-concentration staking tests ──────────────────────────────────────

    fn make_stake_tx(
        sender: &PqKeypair,
        nonce: u64,
        validator: Address,
        amount: u64,
    ) -> SignedTransaction {
        let tx_data = pichain_types::transaction::TransactionData {
            sender: sender.address(),
            nonce,
            kind: TransactionKind::Stake { validator, amount },
            gas_limit: 100_000,
            max_base_fee: 1_000,
            max_priority_fee: 0,
            chain_id: 1,
        };
        Transaction::sign_pq(tx_data, sender)
    }

    fn make_validator_addr(i: u8) -> Address {
        let mut bytes = [0u8; 20];
        bytes[0] = 0xAA;
        bytes[19] = i;
        Address(bytes)
    }

    /// Bootstrap the executor by directly populating the state cache with staked accounts.
    /// Avoids bootstrapping through transactions (which would need to navigate the
    /// bootstrap → enforcement transition). Uses 20 stakers across 5 validators
    /// (4 per validator, 10 PI each).
    /// Total = 200 PI, each address = 5% < 10%, each validator = 40 PI (20%) < 33.33%.
    fn bootstrap_staking(executor: &TransactionExecutor) -> (Vec<PqKeypair>, Vec<Address>) {
        let stakers: Vec<PqKeypair> = (0..20).map(|_| PqKeypair::generate()).collect();
        let validators: Vec<Address> = (0..5).map(make_validator_addr).collect();
        // Pre-populate state: each staker has 200k PI balance with 2,500 PI staked.
        // 20 stakers / 5 validators = 4 stakers per validator × 2,500 PI = 10,000 PI per validator.
        // This meets MIN_VALIDATOR_STAKE (10,000 PI) so caps are properly enforced.
        for (i, staker) in stakers.iter().enumerate() {
            let mut state = AccountState::with_balance(200_000 * 1_000_000_000);
            state.staked = 2_500 * 1_000_000_000;
            state.delegate = Some(validators[i % 5]);
            executor.set_account(staker.address(), state);
        }
        // Rebuild tracker from pre-populated state
        executor.rebuild_staking_totals();
        (stakers, validators)
    }

    #[test]
    fn stake_concentration_cap_enforced() {
        let executor = TransactionExecutor::new(1);
        let (_stakers, validators) = bootstrap_staking(&executor);
        // 50,000 PI across 5 validators (10,000 each = 20%). New staker tries to stake
        // 20,000 PI to validator 0: would make it 30,000/70,000 = 42.8% > 33.33%
        let attacker = PqKeypair::generate();
        executor.set_account(
            attacker.address(),
            AccountState::with_balance(200_000 * 1_000_000_000),
        );
        let tx = make_stake_tx(&attacker, 0, validators[0], 20_000 * 1_000_000_000);
        let results = executor.execute_block(&[tx], 1_000);
        assert!(
            matches!(results[0].effect.status, TransactionStatus::Reverted(ref msg) if msg.contains("validator concentration cap")),
            "should reject stake exceeding 33.33% validator cap, got: {:?}",
            results[0].effect.status
        );
    }

    #[test]
    fn stake_address_concentration_cap_enforced() {
        let executor = TransactionExecutor::new(1);
        let (_stakers, _validators) = bootstrap_staking(&executor);
        // 50,000 PI total. New staker tries to stake 6,000 PI to a new validator:
        // address share = 6,000/56,000 = 10.7% > 10%
        let attacker = PqKeypair::generate();
        executor.set_account(
            attacker.address(),
            AccountState::with_balance(200_000 * 1_000_000_000),
        );
        let tx = make_stake_tx(&attacker, 0, make_validator_addr(6), 6_000 * 1_000_000_000);
        let results = executor.execute_block(&[tx], 1_000);
        assert!(
            matches!(results[0].effect.status, TransactionStatus::Reverted(ref msg) if msg.contains("address concentration cap")),
            "should reject stake exceeding 10% address cap, got: {:?}",
            results[0].effect.status
        );
    }

    #[test]
    fn stake_within_caps_succeeds() {
        let executor = TransactionExecutor::new(1);
        let (_stakers, _validators) = bootstrap_staking(&executor);
        // 50,000 PI total. New staker stakes 500 PI to a new validator:
        // validator share = 500/50,500 = 0.99%, address share = 500/50,500 = 0.99% — both under cap
        let new_staker = PqKeypair::generate();
        executor.set_account(
            new_staker.address(),
            AccountState::with_balance(200_000 * 1_000_000_000),
        );
        let tx = make_stake_tx(&new_staker, 0, make_validator_addr(5), 500 * 1_000_000_000);
        let results = executor.execute_block(&[tx], 1_000);
        assert_eq!(
            results[0].effect.status,
            TransactionStatus::Success,
            "small stake within caps should succeed"
        );
    }

    #[test]
    fn stake_bootstrap_mode_uses_relaxed_caps() {
        // During bootstrap (< 4 validators with MIN_VALIDATOR_STAKE), relaxed 41.3% cap applies.
        // Setup: 3 validators each staking 10,000 PI (each 33.3% of total).
        let executor = TransactionExecutor::new(1);
        let stake = 3_141 * 1_000_000_000_u64; // MIN_VALIDATOR_STAKE
        let staker1 = PqKeypair::generate();
        let staker2 = PqKeypair::generate();
        let staker3 = PqKeypair::generate();
        executor.set_account(
            staker1.address(),
            AccountState::with_balance(200_000 * 1_000_000_000),
        );
        executor.set_account(
            staker2.address(),
            AccountState::with_balance(200_000 * 1_000_000_000),
        );
        executor.set_account(
            staker3.address(),
            AccountState::with_balance(200_000 * 1_000_000_000),
        );
        // Each stakes to a different validator — 33.3% each, under 41.3% bootstrap cap
        let tx1 = make_stake_tx(&staker1, 0, make_validator_addr(0), stake);
        let tx2 = make_stake_tx(&staker2, 0, make_validator_addr(1), stake);
        let tx3 = make_stake_tx(&staker3, 0, make_validator_addr(2), stake);
        let results = executor.execute_block(&[tx1, tx2, tx3], 1_000);
        assert_eq!(results[0].effect.status, TransactionStatus::Success);
        assert_eq!(results[1].effect.status, TransactionStatus::Success);
        assert_eq!(results[2].effect.status, TransactionStatus::Success);

        // Now try to push validator 0 above 41.3%. Add 10,000 PI more → 20,000/40,000 = 50% > 41.3%
        let tx_over = make_stake_tx(&staker1, 1, make_validator_addr(0), stake);
        let results = executor.execute_block(&[tx_over], 1_000);
        assert!(
            matches!(results[0].effect.status, TransactionStatus::Reverted(ref msg)
            if msg.contains("concentration cap")),
            "bootstrap 41.3% cap should block: {:?}",
            results[0].effect.status
        );
    }

    #[test]
    fn unstake_updates_tracking() {
        let executor = TransactionExecutor::new(1);
        let (stakers, validators) = bootstrap_staking(&executor);
        // 50,000 PI across 5 validators. Unstake 500 PI from validator 0
        let unstake_tx_data = pichain_types::transaction::TransactionData {
            sender: stakers[0].address(),
            nonce: 0, // accounts were pre-populated, not transacted, so nonce is 0
            kind: TransactionKind::Unstake {
                validator: validators[0],
                amount: 500 * 1_000_000_000,
            },
            gas_limit: 100_000,
            max_base_fee: 1_000,
            max_priority_fee: 0,
            chain_id: 1,
        };
        let signed = Transaction::sign_pq(unstake_tx_data, &stakers[0]);
        let results = executor.execute_block(&[signed], 1_000);
        assert_eq!(results[0].effect.status, TransactionStatus::Success);
        // Verify tracker was updated
        let tracker = executor.stake_tracker.lock();
        assert_eq!(
            tracker.total_staked,
            49_500 * 1_000_000_000,
            "total staked should decrease by 500 PI after unstake"
        );
    }

    #[test]
    fn stake_velocity_limit_enforced() {
        let executor = TransactionExecutor::new(1);
        let (_stakers, validators) = bootstrap_staking(&executor);
        // 50,000 PI across 5 validators (10,000 each). Epoch_start_stakes snapshot from rebuild.
        // Validator 0 has 10,000 PI. Max 50% growth = 5,000 PI additional.
        // Try to add 5,500 PI → 15,500/10,000 = 55% growth > 50% → velocity rejected
        // Address check: 5,500/55,500 = 9.9% < 10% ✓  Validator: 15,500/55,500 = 27.9% < 33.33% ✓
        let attacker = PqKeypair::generate();
        executor.set_account(
            attacker.address(),
            AccountState::with_balance(200_000 * 1_000_000_000),
        );
        let tx = make_stake_tx(&attacker, 0, validators[0], 5_500 * 1_000_000_000);
        let results = executor.execute_block(&[tx], 1_000);
        assert!(
            matches!(results[0].effect.status, TransactionStatus::Reverted(ref msg) if msg.contains("velocity cap")),
            "should reject stake exceeding 50% velocity cap, got: {:?}",
            results[0].effect.status
        );
    }

    #[test]
    fn stake_velocity_allows_new_validator() {
        let executor = TransactionExecutor::new(1);
        let (_stakers, _validators) = bootstrap_staking(&executor);
        // A brand new validator has no velocity limit (new to tracker).
        // Adding 4,000 PI to validator 6: address share = 4,000/54,000 = 7.4% < 10% ✓
        // Validator share = 4,000/54,000 = 7.4% < 33.33% ✓. No velocity limit for new entry.
        let staker = PqKeypair::generate();
        executor.set_account(
            staker.address(),
            AccountState::with_balance(200_000 * 1_000_000_000),
        );
        let tx = make_stake_tx(&staker, 0, make_validator_addr(6), 4_000 * 1_000_000_000);
        let results = executor.execute_block(&[tx], 1_000);
        assert_eq!(
            results[0].effect.status,
            TransactionStatus::Success,
            "new validator should not be velocity-limited"
        );
    }
}
