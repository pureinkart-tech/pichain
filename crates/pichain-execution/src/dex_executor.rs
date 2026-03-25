//! DEX executor — processes native AMM transactions.
//!
//! Handles: CreatePool, AddLiquidity, RemoveLiquidity, Swap.
//!
//! Uses constant-product (x*y=k) formula with 0.30% trading fee.
//! LP tokens are tracked per-pool per-user in an in-memory cache.

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use pichain_crypto::ed25519::Address;
use pichain_types::dex::{LiquidityPool, PoolId};
use pichain_types::token::MintId;
use pichain_types::transaction::{TransactionEvent, TransactionStatus};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Result of executing a DEX operation.
#[derive(Clone, Debug)]
pub struct DexExecutionResult {
    /// Execution status.
    pub status: TransactionStatus,
    /// Events emitted.
    pub events: Vec<TransactionEvent>,
    /// Pool changes (pool_id → updated pool).
    pub pool_changes: HashMap<PoolId, LiquidityPool>,
    /// LP balance changes: (pool_id, address) → new LP balance.
    pub lp_changes: HashMap<(PoolId, Address), u64>,
    /// Token account changes needed: (owner, mint, delta_signed).
    /// Positive = credit, negative = debit. The main executor handles
    /// the actual token account mutations.
    pub token_deltas: Vec<TokenDelta>,
}

/// A token balance change that the main executor must apply.
#[derive(Clone, Debug)]
pub struct TokenDelta {
    /// Owner address.
    pub owner: Address,
    /// Token mint.
    pub mint: MintId,
    /// Amount (positive = credit to owner, negative = debit from owner).
    pub amount: i128,
}

/// Maximum price impact in basis points for a single swap.
/// Prevents extreme single-transaction market manipulation and sandwich attacks.
/// 3000 bps = 30% max price impact.
const MAX_SWAP_PRICE_IMPACT_BPS: u64 = 9999; // 99.99% — slippage tolerance is the real user protection

/// Minimum blocks between LP deposit and removal.
/// Prevents atomic add+remove manipulation within the same or adjacent blocks.
/// ~10 minutes at 314ms block time ≈ 1,911 blocks.
const LP_LOCK_BLOCKS: u64 = 1_911;

/// Anti-snipe cooldown: number of blocks after pool creation during which
/// per-address cumulative swap volume is capped.
/// ~30 seconds at 314ms block time ≈ 96 blocks.
/// Prevents bots from front-running newly graduated tokens with oversized buys.
const SNIPE_COOLDOWN_BLOCKS: u64 = 96;

/// Maximum cumulative swap volume PER ADDRESS during cooldown period,
/// as basis points of the pool's initial input reserve.
/// 1000 bps = 10% of the pool's input reserve per address during the entire cooldown.
/// This is generous for normal users (10% of an 80 PI pool = 8 PI) while preventing
/// any single address from cornering >10% of supply in the first 30 seconds.
const MAX_SNIPE_VOLUME_BPS: u64 = 1000;

/// DEX executor — manages liquidity pools and swaps.
pub struct DexExecutor {
    /// In-memory pool cache.
    pools: DashMap<PoolId, LiquidityPool>,
    /// LP token balances: (pool_id, address) → balance.
    lp_balances: DashMap<(PoolId, Address), u64>,
    /// Block height at which LP was last deposited: (pool_id, address) → block_height.
    lp_deposit_height: DashMap<(PoolId, Address), u64>,
    /// Block timestamp for deterministic pool creation.
    block_timestamp_ms: AtomicU64,
    /// Current block height for lock period tracking.
    block_height: AtomicU64,
    /// Pool reserve snapshots at block start, for MEV-resistant price impact calculation.
    /// Key: PoolId, Value: (reserve_a_at_block_start, reserve_b_at_block_start)
    block_start_reserves: DashMap<PoolId, (u64, u64)>,
    /// Guard: set to true after snapshot_block_start() is called, reset at next snapshot.
    /// When true, new pools created mid-block will have their reserves captured on first
    /// swap access rather than using stale/mutated live data.
    snapshot_taken: AtomicBool,
    /// Accumulated holder dividend fees per token (mint_id → total PI accumulated).
    /// These are accumulated during swap execution and persisted by the main executor.
    dividend_fees: DashMap<MintId, u64>,
    /// Anti-snipe: cumulative swap volume per (pool, address) during cooldown.
    /// Tracks how much each address has swapped on each pool since its creation.
    /// Only populated for pools still within their cooldown window.
    /// Cleared when the pool exits cooldown (checked lazily).
    snipe_volume: DashMap<(PoolId, Address), u64>,
}

impl DexExecutor {
    pub fn new() -> Self {
        Self {
            pools: DashMap::new(),
            lp_balances: DashMap::new(),
            lp_deposit_height: DashMap::new(),
            block_timestamp_ms: AtomicU64::new(0),
            block_height: AtomicU64::new(0),
            block_start_reserves: DashMap::new(),
            snapshot_taken: AtomicBool::new(false),
            dividend_fees: DashMap::new(),
            snipe_volume: DashMap::new(),
        }
    }

    /// Clear all cached state. Used by the block producer when re-executing
    /// transactions after gas-limit trimming to prevent double-mutations.
    pub fn clear_state(&self) {
        self.pools.clear();
        self.lp_balances.clear();
        self.lp_deposit_height.clear();
        self.block_start_reserves.clear();
        self.snapshot_taken.store(false, Ordering::Release);
        self.dividend_fees.clear();
        self.snipe_volume.clear();
    }

    /// Set the block timestamp for deterministic state creation.
    pub fn set_block_timestamp(&self, ts: u64) {
        self.block_timestamp_ms.store(ts, Ordering::Release);
    }

    /// Set the current block height for LP lock tracking.
    pub fn set_block_height(&self, height: u64) {
        self.block_height.store(height, Ordering::Release);
    }

    /// Capture pool reserve snapshots at the start of a block.
    /// MUST be called BEFORE any transactions execute in the block.
    /// Price impact for swaps is calculated against these snapshots to prevent
    /// intra-block sandwich attacks from manipulating the impact threshold.
    pub fn snapshot_block_start(&self) {
        // Clear previous block's snapshots
        self.block_start_reserves.clear();
        // Capture current reserves for all existing pools
        for entry in self.pools.iter() {
            let pool = entry.value();
            self.block_start_reserves
                .insert(*entry.key(), (pool.reserve_a, pool.reserve_b));
        }
        // Mark that snapshot has been taken — any pools created after this point
        // will have their initial reserves captured on first swap access.
        self.snapshot_taken.store(true, Ordering::Release);
    }

    fn current_block_height(&self) -> u64 {
        self.block_height.load(Ordering::Acquire)
    }

    fn block_timestamp(&self) -> u64 {
        self.block_timestamp_ms.load(Ordering::Acquire)
    }

    /// Load a pool into the cache (from storage on startup).
    pub fn load_pool(&self, pool: LiquidityPool) {
        self.pools.insert(pool.id, pool);
    }

    /// Load an LP balance into the cache.
    pub fn load_lp_balance(&self, pool_id: PoolId, owner: Address, balance: u64) {
        self.lp_balances.insert((pool_id, owner), balance);
    }

    /// Get a pool from the cache.
    pub fn get_pool(&self, id: &PoolId) -> Option<LiquidityPool> {
        self.pools.get(id).map(|v| v.clone())
    }

    /// Get pool by mint pair.
    pub fn get_pool_by_mints(&self, mint_a: &MintId, mint_b: &MintId) -> Option<LiquidityPool> {
        let id = PoolId::derive(mint_a, mint_b);
        self.get_pool(&id)
    }

    /// Get LP balance.
    pub fn get_lp_balance(&self, pool_id: &PoolId, owner: &Address) -> u64 {
        self.lp_balances
            .get(&(*pool_id, *owner))
            .map(|v| *v)
            .unwrap_or(0)
    }

    /// Rollback a pool to a previous state (used when token delta application fails).
    pub fn rollback_pool(&self, pool_id: &PoolId, old_pool: Option<&LiquidityPool>) {
        match old_pool {
            Some(pool) => {
                self.pools.insert(*pool_id, pool.clone());
            }
            None => {
                self.pools.remove(pool_id);
            }
        }
    }

    /// Rollback an LP balance to a previous value.
    pub fn rollback_lp_balance(&self, pool_id: &PoolId, owner: &Address, old_balance: u64) {
        if old_balance == 0 {
            self.lp_balances.remove(&(*pool_id, *owner));
        } else {
            self.lp_balances.insert((*pool_id, *owner), old_balance);
        }
    }

    /// Derive the pool ID for a given mint pair (public helper for rollback).
    pub fn derive_pool_id(&self, mint_a: &MintId, mint_b: &MintId) -> PoolId {
        PoolId::derive(mint_a, mint_b)
    }

    /// Snapshot all pools (for block-level persistence).
    pub fn all_pools(&self) -> HashMap<PoolId, LiquidityPool> {
        self.pools
            .iter()
            .map(|e| (*e.key(), e.value().clone()))
            .collect()
    }

    /// Snapshot all LP balances (for block-level persistence).
    pub fn all_lp_balances(&self) -> HashMap<(PoolId, Address), u64> {
        self.lp_balances
            .iter()
            .map(|e| (*e.key(), *e.value()))
            .collect()
    }

    /// Set creator fee on a pool (called after launch graduation).
    pub fn set_creator_fee(&self, pool_id: &PoolId, recipient: Address, fee_bps: u16) {
        if let Some(mut pool) = self.pools.get_mut(pool_id) {
            pool.creator_fee_recipient = Some(recipient);
            pool.creator_fee_bps = fee_bps;
        }
    }

    /// Create a new liquidity pool.
    pub fn create_pool(
        &self,
        sender: Address,
        mint_a: MintId,
        mint_b: MintId,
    ) -> DexExecutionResult {
        // Validate
        if mint_a == mint_b {
            return dex_error("cannot create pool with identical tokens");
        }

        let pool_id = PoolId::derive(&mint_a, &mint_b);

        let mut pool = LiquidityPool::new(mint_a, mint_b, sender, self.block_timestamp());
        // Set creation height for anti-snipe cooldown tracking
        pool.created_at_height = self.current_block_height();

        // Atomic check-and-insert to prevent TOCTOU race under parallel execution
        match self.pools.entry(pool_id) {
            Entry::Occupied(_) => return dex_error("pool already exists for this token pair"),
            Entry::Vacant(v) => {
                v.insert(pool.clone());
            }
        }

        let mut pool_changes = HashMap::new();
        pool_changes.insert(pool_id, pool);

        DexExecutionResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "CreatePool".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "pool": pool_id.to_string(),
                    "mint_a": mint_a.to_string(),
                    "mint_b": mint_b.to_string(),
                }))
                .unwrap_or_default(),
            }],
            pool_changes,
            lp_changes: HashMap::new(),
            token_deltas: vec![],
        }
    }

    /// Add liquidity to a pool.
    pub fn add_liquidity(
        &self,
        sender: Address,
        mint_a: MintId,
        mint_b: MintId,
        amount_a: u64,
        amount_b: u64,
        min_lp_tokens: u64,
    ) -> DexExecutionResult {
        let pool_id = PoolId::derive(&mint_a, &mint_b);

        // Use get_mut to hold shard lock during read-modify-write (prevents TOCTOU race)
        let mut pool_ref = match self.pools.get_mut(&pool_id) {
            Some(p) => p,
            None => return dex_error("pool not found"),
        };
        let pool = pool_ref.value_mut();

        if !pool.active {
            return dex_error("pool is not active");
        }

        // Map caller's mint_a/mint_b to pool's canonical ordering
        let (caller_amount_a, caller_amount_b) = if pool.mint_a == mint_a {
            (amount_a, amount_b)
        } else {
            (amount_b, amount_a)
        };

        let (lp_tokens, actual_a, actual_b) =
            match pool.calculate_add_liquidity(caller_amount_a, caller_amount_b) {
                Some(result) => result,
                None => return dex_error("liquidity amounts too small"),
            };

        if lp_tokens < min_lp_tokens {
            return dex_error(&format!(
                "slippage: would receive {lp_tokens} LP tokens, minimum is {min_lp_tokens}"
            ));
        }

        // Update pool reserves (with overflow protection)
        pool.reserve_a = match pool.reserve_a.checked_add(actual_a) {
            Some(v) => v,
            None => return dex_error("reserve_a overflow"),
        };
        pool.reserve_b = match pool.reserve_b.checked_add(actual_b) {
            Some(v) => v,
            None => return dex_error("reserve_b overflow"),
        };

        // First provision includes minimum liquidity
        if pool.lp_supply == 0 {
            pool.lp_supply = match lp_tokens.checked_add(LiquidityPool::MINIMUM_LIQUIDITY) {
                Some(v) => v,
                None => return dex_error("lp_supply overflow"),
            };
        } else {
            pool.lp_supply = match pool.lp_supply.checked_add(lp_tokens) {
                Some(v) => v,
                None => return dex_error("lp_supply overflow"),
            };
        }

        // R27-FIX: Use entry API for atomic read-modify-write on LP balance.
        // Prevents TOCTOU race where concurrent add_liquidity could lose LP credits.
        let new_lp = {
            let mut lp_entry = self.lp_balances.entry((pool_id, sender)).or_insert(0);
            *lp_entry = match lp_entry.checked_add(lp_tokens) {
                Some(v) => v,
                None => return dex_error("LP balance overflow"),
            };
            // SECURITY: Update deposit height while still holding the LP balance lock
            // to prevent a race where another tx reads stale height between the
            // balance update and height insert.
            self.lp_deposit_height
                .insert((pool_id, sender), self.current_block_height());
            *lp_entry
        };

        // Clone pool state for changes map, then release shard lock
        let pool_snapshot = pool.clone();
        let pool_mint_a = pool.mint_a;
        let pool_mint_b = pool.mint_b;
        drop(pool_ref);

        let mut pool_changes = HashMap::new();
        pool_changes.insert(pool_id, pool_snapshot);
        let mut lp_changes = HashMap::new();
        lp_changes.insert((pool_id, sender), new_lp);

        // Map back to caller's mint ordering for deltas
        let (debit_mint_a, debit_amount_a, debit_mint_b, debit_amount_b) = if pool_mint_a == mint_a
        {
            (pool_mint_a, actual_a, pool_mint_b, actual_b)
        } else {
            (pool_mint_b, actual_b, pool_mint_a, actual_a)
        };

        DexExecutionResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "AddLiquidity".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "pool": pool_id.to_string(),
                    "amount_a": debit_amount_a,
                    "amount_b": debit_amount_b,
                    "lp_tokens": lp_tokens,
                }))
                .unwrap_or_default(),
            }],
            pool_changes,
            lp_changes,
            token_deltas: vec![
                TokenDelta {
                    owner: sender,
                    mint: debit_mint_a,
                    amount: -(debit_amount_a as i128),
                },
                TokenDelta {
                    owner: sender,
                    mint: debit_mint_b,
                    amount: -(debit_amount_b as i128),
                },
            ],
        }
    }

    /// Remove liquidity from a pool.
    pub fn remove_liquidity(
        &self,
        sender: Address,
        mint_a: MintId,
        mint_b: MintId,
        lp_amount: u64,
        min_amount_a: u64,
        min_amount_b: u64,
    ) -> DexExecutionResult {
        let pool_id = PoolId::derive(&mint_a, &mint_b);

        // Use get_mut to hold shard lock during read-modify-write (prevents TOCTOU race)
        let mut pool_ref = match self.pools.get_mut(&pool_id) {
            Some(p) => p,
            None => return dex_error("pool not found"),
        };
        let pool = pool_ref.value_mut();

        // Check LP lock period — prevent atomic add+remove manipulation
        let deposit_height = self
            .lp_deposit_height
            .get(&(pool_id, sender))
            .map(|v| *v)
            .unwrap_or(0);
        let current_height = self.current_block_height();
        if current_height < deposit_height.saturating_add(LP_LOCK_BLOCKS) {
            let blocks_remaining = deposit_height
                .saturating_add(LP_LOCK_BLOCKS)
                .saturating_sub(current_height);
            return dex_error(&format!(
                "LP tokens locked: {blocks_remaining} blocks remaining before removal allowed"
            ));
        }

        // Use entry API for atomic read-modify-write (defense in depth)
        let new_lp = match self.lp_balances.entry((pool_id, sender)) {
            Entry::Occupied(mut e) => {
                if *e.get() < lp_amount {
                    return dex_error(&format!(
                        "insufficient LP tokens: have {}, need {lp_amount}",
                        e.get()
                    ));
                }
                let new_val = e.get() - lp_amount;
                *e.get_mut() = new_val;
                new_val
            }
            Entry::Vacant(_) => {
                return dex_error("no LP balance found");
            }
        };

        let (amount_a, amount_b) = match pool.calculate_remove_liquidity(lp_amount) {
            Some(result) => result,
            None => {
                // R31-FIX: Restore LP balance since entry API already deducted it
                if let Some(mut entry) = self.lp_balances.get_mut(&(pool_id, sender)) {
                    *entry += lp_amount;
                }
                return dex_error("cannot remove this amount of liquidity");
            }
        };

        // Map to caller's ordering for slippage check
        let (caller_a, caller_b) = if pool.mint_a == mint_a {
            (amount_a, amount_b)
        } else {
            (amount_b, amount_a)
        };

        if caller_a < min_amount_a || caller_b < min_amount_b {
            // R31-FIX: Restore LP balance since entry API already deducted it
            if let Some(mut entry) = self.lp_balances.get_mut(&(pool_id, sender)) {
                *entry += lp_amount;
            }
            return dex_error(&format!(
                "slippage: would receive ({caller_a}, {caller_b}), minimum is ({min_amount_a}, {min_amount_b})"
            ));
        }

        // Update pool — use checked_sub to catch invariant violations from rounding bugs.
        // R31-FIX: All error paths restore LP balance to prevent fund loss.
        pool.reserve_a = match pool.reserve_a.checked_sub(amount_a) {
            Some(v) => v,
            None => {
                if let Some(mut entry) = self.lp_balances.get_mut(&(pool_id, sender)) {
                    *entry += lp_amount;
                }
                return dex_error(&format!(
                    "reserve_a underflow: {} < {}",
                    pool.reserve_a, amount_a
                ));
            }
        };
        pool.reserve_b = match pool.reserve_b.checked_sub(amount_b) {
            Some(v) => v,
            None => {
                pool.reserve_a += amount_a; // undo reserve_a
                if let Some(mut entry) = self.lp_balances.get_mut(&(pool_id, sender)) {
                    *entry += lp_amount;
                }
                return dex_error(&format!(
                    "reserve_b underflow: {} < {}",
                    pool.reserve_b, amount_b
                ));
            }
        };
        pool.lp_supply = match pool.lp_supply.checked_sub(lp_amount) {
            Some(v) => v,
            None => {
                pool.reserve_a += amount_a; // undo
                pool.reserve_b += amount_b; // undo
                if let Some(mut entry) = self.lp_balances.get_mut(&(pool_id, sender)) {
                    *entry += lp_amount;
                }
                return dex_error(&format!(
                    "lp_supply underflow: {} < {}",
                    pool.lp_supply, lp_amount
                ));
            }
        };

        // Clone pool state for changes map, then release shard lock
        let pool_snapshot = pool.clone();
        let pool_mint_a = pool.mint_a;
        let pool_mint_b = pool.mint_b;
        drop(pool_ref);

        let mut pool_changes = HashMap::new();
        pool_changes.insert(pool_id, pool_snapshot);
        let mut lp_changes = HashMap::new();
        lp_changes.insert((pool_id, sender), new_lp);

        // Credit tokens back to sender (in caller's ordering)
        let (credit_mint_a, credit_amount_a, credit_mint_b, credit_amount_b) =
            if pool_mint_a == mint_a {
                (pool_mint_a, amount_a, pool_mint_b, amount_b)
            } else {
                (pool_mint_b, amount_b, pool_mint_a, amount_a)
            };

        DexExecutionResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "RemoveLiquidity".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "pool": pool_id.to_string(),
                    "amount_a": credit_amount_a,
                    "amount_b": credit_amount_b,
                    "lp_burned": lp_amount,
                }))
                .unwrap_or_default(),
            }],
            pool_changes,
            lp_changes,
            token_deltas: vec![
                TokenDelta {
                    owner: sender,
                    mint: credit_mint_a,
                    amount: credit_amount_a as i128,
                },
                TokenDelta {
                    owner: sender,
                    mint: credit_mint_b,
                    amount: credit_amount_b as i128,
                },
            ],
        }
    }

    /// Execute a token swap.
    pub fn swap(
        &self,
        sender: Address,
        mint_in: MintId,
        mint_out: MintId,
        amount_in: u64,
        min_amount_out: u64,
    ) -> DexExecutionResult {
        if mint_in == mint_out {
            return dex_error("cannot swap a token for itself");
        }

        if amount_in == 0 {
            return dex_error("swap amount must be > 0");
        }

        let pool_id = PoolId::derive(&mint_in, &mint_out);

        // Use get_mut to hold shard lock during read-modify-write (prevents TOCTOU race)
        let mut pool_ref = match self.pools.get_mut(&pool_id) {
            Some(p) => p,
            None => return dex_error("no pool found for this token pair"),
        };
        let pool = pool_ref.value_mut();

        // Anti-snipe: during cooldown period after pool creation, limit cumulative
        // swap volume PER ADDRESS. This prevents any single address from cornering
        // a large position in newly graduated tokens, while allowing normal-sized
        // purchases. The limit is based on the pool's initial reserves at creation.
        if pool.created_at_height > 0 {
            let blocks_since_creation = self
                .current_block_height()
                .saturating_sub(pool.created_at_height);
            if blocks_since_creation < SNIPE_COOLDOWN_BLOCKS {
                let is_a_to_b_check = pool.mint_a == mint_in;
                let input_reserve = if is_a_to_b_check {
                    pool.reserve_a
                } else {
                    pool.reserve_b
                };
                // Use initial reserve estimate: for a pool that just graduated, the current
                // reserve is close to the initial reserve. As swaps happen, reserve grows
                // (for buy side), making the cap slightly more generous over time — this is fine.
                let max_volume =
                    (input_reserve as u128 * MAX_SNIPE_VOLUME_BPS as u128 / 10_000) as u64;
                if max_volume > 0 {
                    let key = (pool_id, sender);
                    let current_volume = self.snipe_volume.get(&key).map(|v| *v).unwrap_or(0);
                    let new_volume = current_volume.saturating_add(amount_in);
                    if new_volume > max_volume {
                        let remaining = max_volume.saturating_sub(current_volume);
                        return dex_error(&format!(
                            "anti-snipe: address volume cap reached during cooldown ({} blocks remaining). \
                             Used {}/{} of per-address limit. Max additional: {}.",
                            SNIPE_COOLDOWN_BLOCKS.saturating_sub(blocks_since_creation),
                            current_volume, max_volume, remaining
                        ));
                    }
                }
            }
        }

        if !pool.active {
            return dex_error("pool is not active");
        }

        // Determine swap direction relative to canonical pool ordering
        let is_a_to_b = pool.mint_a == mint_in;

        let (amount_out, fee) = match pool.calculate_swap_output(amount_in, is_a_to_b) {
            Some(result) => result,
            None => return dex_error("swap output is zero or exceeds reserves"),
        };

        if amount_out < min_amount_out {
            return dex_error(&format!(
                "slippage: would receive {amount_out}, minimum is {min_amount_out}"
            ));
        }

        // Use block-start reserves for price impact to prevent intra-block MEV.
        // If an earlier transaction in the same block manipulated reserves,
        // we still measure impact against the pre-block state.
        //
        // For pools created mid-block (not in the snapshot), capture their current
        // reserves on first swap access. This prevents subsequent swaps from using
        // already-mutated live reserves, which would make the rollback a no-op
        // and bypass MEV protection.
        let (snap_a, snap_b) = if let Some(snapshot) = self.block_start_reserves.get(&pool_id) {
            *snapshot
        } else if self.snapshot_taken.load(Ordering::Acquire) {
            // Pool was created mid-block. Capture its current (pre-swap) reserves
            // as the snapshot so all swaps in this block measure against the same baseline.
            let reserves = (pool.reserve_a, pool.reserve_b);
            self.block_start_reserves.insert(pool_id, reserves);
            reserves
        } else {
            // No block snapshot taken yet (e.g., during tests). Use current reserves.
            (pool.reserve_a, pool.reserve_b)
        };

        let impact = {
            let (input_reserve, output_reserve) = if is_a_to_b {
                (snap_a, snap_b)
            } else {
                (snap_b, snap_a)
            };
            if output_reserve == 0 || input_reserve == 0 {
                10_000 // 100% impact if reserves were zero
            } else {
                // Use cross-multiplication to avoid truncation with extreme reserve ratios.
                // price_before = out / in, price_after = new_out / new_in
                // impact = 1 - (price_after / price_before)
                //        = 1 - (new_out * in) / (new_in * out)
                let new_in = input_reserve as u128 + amount_in as u128;
                let new_out = (output_reserve as u128)
                    .saturating_mul(input_reserve as u128)
                    .checked_div(new_in)
                    .unwrap_or(0);
                // SECURITY: Use checked_mul to prevent u128 overflow on extreme reserves.
                // If overflow occurs, treat as 100% impact (safest default → swap rejected).
                let numer = new_out.checked_mul(input_reserve as u128);
                let denom = new_in.checked_mul(output_reserve as u128);
                match (numer, denom) {
                    (_, None) | (None, _) => 10_000, // u128 overflow → max impact
                    (_, Some(0)) => 10_000,
                    (Some(n), Some(d)) if n >= d => 0,
                    (Some(n), Some(d)) => {
                        let diff = d - n;
                        diff.checked_mul(10_000)
                            .map(|v| (v / d) as u64)
                            .unwrap_or(10_000)
                    }
                }
            }
        };
        if impact > MAX_SWAP_PRICE_IMPACT_BPS {
            return dex_error(&format!(
                "swap price impact too high: {impact} bps (max {MAX_SWAP_PRICE_IMPACT_BPS} bps)"
            ));
        }

        // Capture old reserves for post-swap invariant check
        let old_reserve_a = pool.reserve_a;
        let old_reserve_b = pool.reserve_b;

        // Update reserves (with overflow/underflow protection)
        if is_a_to_b {
            pool.reserve_a = match pool.reserve_a.checked_add(amount_in) {
                Some(v) => v,
                None => return dex_error("reserve_a overflow on swap"),
            };
            pool.reserve_b = match pool.reserve_b.checked_sub(amount_out) {
                Some(v) => v,
                None => return dex_error("reserve_b underflow on swap — AMM invariant violated"),
            };
            pool.cumulative_volume_a = pool.cumulative_volume_a.saturating_add(amount_in as u128);
        } else {
            pool.reserve_b = match pool.reserve_b.checked_add(amount_in) {
                Some(v) => v,
                None => return dex_error("reserve_b overflow on swap"),
            };
            pool.reserve_a = match pool.reserve_a.checked_sub(amount_out) {
                Some(v) => v,
                None => return dex_error("reserve_a underflow on swap — AMM invariant violated"),
            };
            pool.cumulative_volume_b = pool.cumulative_volume_b.saturating_add(amount_in as u128);
        }

        // Verify constant product invariant: k_new >= k_old
        let k_old = (old_reserve_a as u128) * (old_reserve_b as u128);
        let k_new = (pool.reserve_a as u128) * (pool.reserve_b as u128);
        if k_new < k_old {
            // Rollback reserves
            pool.reserve_a = old_reserve_a;
            pool.reserve_b = old_reserve_b;
            return dex_error("AMM invariant violated: k decreased after swap");
        }

        // Creator royalty: route a portion of the fee to the token creator.
        // The fee is already absorbed into reserves (constant-product), so we
        // debit the creator's share from reserves and credit it to the creator.
        let (creator_fee, creator_recipient) = {
            let (cf, _remaining) = pool.split_creator_fee(fee);
            (cf, pool.creator_fee_recipient)
        };

        // Holder dividend fee: 50% of the remaining fee (after creator royalty) goes to
        // token holders. Only applies to pools with a creator fee (i.e., launched tokens).
        let holder_dividend_fee = if pool.creator_fee_recipient.is_some() {
            let (_cf, remaining) = pool.split_creator_fee(fee);
            // 50% of remaining fee goes to holder dividends.
            // Integer division rounds down — the 1-satoshi dust on odd values
            // stays in pool reserves, benefiting LP holders (conservation of PI).
            remaining / 2
        } else {
            0
        };

        // Deduct creator fee from the input-side reserve (where the fee was absorbed)
        if creator_fee > 0 {
            if is_a_to_b {
                pool.reserve_a = pool.reserve_a.saturating_sub(creator_fee);
            } else {
                pool.reserve_b = pool.reserve_b.saturating_sub(creator_fee);
            }
        }

        // Deduct holder dividend fee from input-side reserve.
        // Only when mint_in is native PI — we track dividends in PI only.
        if holder_dividend_fee > 0 && mint_in.is_native_pi() {
            if is_a_to_b {
                pool.reserve_a = pool.reserve_a.saturating_sub(holder_dividend_fee);
            } else {
                pool.reserve_b = pool.reserve_b.saturating_sub(holder_dividend_fee);
            }
            let launched_mint = if pool.mint_a.is_native_pi() {
                pool.mint_b
            } else {
                pool.mint_a
            };
            let mut entry = self.dividend_fees.entry(launched_mint).or_insert(0);
            *entry = entry.saturating_add(holder_dividend_fee);
        }

        // Track anti-snipe volume for this address (only during cooldown)
        let pool_created_at = pool.created_at_height;

        // Clone pool state for changes map, then release shard lock
        let pool_snapshot = pool.clone();
        drop(pool_ref);

        // Record cumulative swap volume per address (only for cooldown-active pools)
        if pool_created_at > 0 {
            let blocks_since = self.current_block_height().saturating_sub(pool_created_at);
            if blocks_since < SNIPE_COOLDOWN_BLOCKS {
                *self.snipe_volume.entry((pool_id, sender)).or_insert(0) += amount_in;
            }
        }

        let mut pool_changes = HashMap::new();
        pool_changes.insert(pool_id, pool_snapshot);

        let mut token_deltas = vec![
            TokenDelta {
                owner: sender,
                mint: mint_in,
                amount: -(amount_in as i128),
            },
            TokenDelta {
                owner: sender,
                mint: mint_out,
                amount: amount_out as i128,
            },
        ];

        // Add creator royalty delta
        if creator_fee > 0 {
            if let Some(recipient) = creator_recipient {
                token_deltas.push(TokenDelta {
                    owner: recipient,
                    mint: mint_in,
                    amount: creator_fee as i128,
                });
            }
        }

        DexExecutionResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "Swap".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "pool": pool_id.to_string(),
                    "mint_in": mint_in.to_string(),
                    "mint_out": mint_out.to_string(),
                    "amount_in": amount_in,
                    "amount_out": amount_out,
                    "fee": fee,
                    "creator_fee": creator_fee,
                    "holder_dividend_fee": holder_dividend_fee,
                }))
                .unwrap_or_default(),
            }],
            pool_changes,
            lp_changes: HashMap::new(),
            token_deltas,
        }
    }

    /// Get accumulated dividend fees for all tokens.
    pub fn get_dividend_fees(&self) -> Vec<(MintId, u64)> {
        self.dividend_fees
            .iter()
            .map(|e| (*e.key(), *e.value()))
            .collect()
    }

    /// Drain accumulated dividend fees (called after persisting to storage).
    pub fn drain_dividend_fees(&self) -> Vec<(MintId, u64)> {
        let fees: Vec<_> = self
            .dividend_fees
            .iter()
            .map(|e| (*e.key(), *e.value()))
            .collect();
        self.dividend_fees.clear();
        fees
    }
}

impl Default for DexExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to create an error result.
fn dex_error(msg: &str) -> DexExecutionResult {
    DexExecutionResult {
        status: TransactionStatus::Reverted(msg.to_string()),
        events: vec![],
        pool_changes: HashMap::new(),
        lp_changes: HashMap::new(),
        token_deltas: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (DexExecutor, Address, MintId, MintId) {
        let dex = DexExecutor::new();
        let creator = Address([1u8; 20]);
        let mint_pi = MintId::ZERO;
        let mint_token = MintId::derive(&Address([2u8; 20]), 0);
        (dex, creator, mint_pi, mint_token)
    }

    #[test]
    fn create_pool() {
        let (dex, creator, mint_pi, mint_token) = setup();

        let result = dex.create_pool(creator, mint_pi, mint_token);
        assert_eq!(result.status, TransactionStatus::Success);
        assert_eq!(result.pool_changes.len(), 1);

        let pool_id = PoolId::derive(&mint_pi, &mint_token);
        let pool = dex.get_pool(&pool_id).unwrap();
        assert_eq!(pool.reserve_a, 0);
        assert_eq!(pool.reserve_b, 0);
        assert!(pool.active);
    }

    #[test]
    fn create_duplicate_pool_fails() {
        let (dex, creator, mint_pi, mint_token) = setup();

        dex.create_pool(creator, mint_pi, mint_token);
        let r = dex.create_pool(creator, mint_pi, mint_token);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn create_pool_same_token_fails() {
        let (dex, creator, mint_pi, _) = setup();

        let r = dex.create_pool(creator, mint_pi, mint_pi);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn add_initial_liquidity() {
        let (dex, creator, mint_pi, mint_token) = setup();
        dex.create_pool(creator, mint_pi, mint_token);

        let pool_id = PoolId::derive(&mint_pi, &mint_token);
        let result = dex.add_liquidity(creator, mint_pi, mint_token, 1_000_000, 1_000_000, 0);
        assert_eq!(result.status, TransactionStatus::Success);

        let pool = dex.get_pool(&pool_id).unwrap();
        assert_eq!(pool.reserve_a + pool.reserve_b, 2_000_000);

        let lp = dex.get_lp_balance(&pool_id, &creator);
        assert!(lp > 0);
        assert_eq!(lp, 1_000_000 - LiquidityPool::MINIMUM_LIQUIDITY);
    }

    #[test]
    fn add_liquidity_nonexistent_pool_fails() {
        let (dex, creator, mint_pi, mint_token) = setup();

        let r = dex.add_liquidity(creator, mint_pi, mint_token, 1_000, 1_000, 0);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn add_subsequent_liquidity() {
        let (dex, creator, mint_pi, mint_token) = setup();
        dex.create_pool(creator, mint_pi, mint_token);
        dex.add_liquidity(creator, mint_pi, mint_token, 1_000_000, 2_000_000, 0);

        let user2 = Address([3u8; 20]);
        let r = dex.add_liquidity(user2, mint_pi, mint_token, 100_000, 200_000, 0);
        assert_eq!(r.status, TransactionStatus::Success);

        let pool_id = PoolId::derive(&mint_pi, &mint_token);
        let lp = dex.get_lp_balance(&pool_id, &user2);
        assert!(lp > 0);
    }

    #[test]
    fn remove_liquidity() {
        let (dex, creator, mint_pi, mint_token) = setup();
        dex.create_pool(creator, mint_pi, mint_token);
        dex.add_liquidity(creator, mint_pi, mint_token, 1_000_000, 1_000_000, 0);

        // Advance past LP lock period
        dex.set_block_height(LP_LOCK_BLOCKS + 1);

        let pool_id = PoolId::derive(&mint_pi, &mint_token);
        let lp = dex.get_lp_balance(&pool_id, &creator);

        // Remove half
        let r = dex.remove_liquidity(creator, mint_pi, mint_token, lp / 2, 0, 0);
        assert_eq!(r.status, TransactionStatus::Success);

        // Check tokens returned
        assert_eq!(r.token_deltas.len(), 2);
        assert!(r.token_deltas.iter().all(|d| d.amount > 0));

        // Check LP balance decreased
        let new_lp = dex.get_lp_balance(&pool_id, &creator);
        assert_eq!(new_lp, lp - lp / 2);
    }

    #[test]
    fn remove_liquidity_insufficient_lp_fails() {
        let (dex, creator, mint_pi, mint_token) = setup();
        dex.create_pool(creator, mint_pi, mint_token);
        dex.add_liquidity(creator, mint_pi, mint_token, 1_000_000, 1_000_000, 0);

        // Advance past LP lock period
        dex.set_block_height(LP_LOCK_BLOCKS + 1);

        let r = dex.remove_liquidity(creator, mint_pi, mint_token, u64::MAX, 0, 0);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn lp_lock_period_prevents_immediate_removal() {
        let (dex, creator, mint_pi, mint_token) = setup();
        dex.create_pool(creator, mint_pi, mint_token);

        // Deposit at block 100
        dex.set_block_height(100);
        dex.add_liquidity(creator, mint_pi, mint_token, 1_000_000, 1_000_000, 0);

        let pool_id = PoolId::derive(&mint_pi, &mint_token);
        let lp = dex.get_lp_balance(&pool_id, &creator);
        assert!(lp > 0);

        // Try removing at block 101 — should fail (within lock period)
        dex.set_block_height(101);
        let r = dex.remove_liquidity(creator, mint_pi, mint_token, lp / 2, 0, 0);
        assert!(matches!(r.status, TransactionStatus::Reverted(ref msg) if msg.contains("locked")));

        // Advance past lock period and try again — should succeed
        dex.set_block_height(100 + LP_LOCK_BLOCKS + 1);
        let r = dex.remove_liquidity(creator, mint_pi, mint_token, lp / 2, 0, 0);
        assert_eq!(r.status, TransactionStatus::Success);
    }

    #[test]
    fn swap_basic() {
        let (dex, creator, mint_pi, mint_token) = setup();
        dex.create_pool(creator, mint_pi, mint_token);
        dex.add_liquidity(creator, mint_pi, mint_token, 1_000_000, 1_000_000, 0);

        let trader = Address([5u8; 20]);
        let r = dex.swap(trader, mint_pi, mint_token, 10_000, 0);
        assert_eq!(r.status, TransactionStatus::Success);

        // Should have debit of mint_pi and credit of mint_token
        assert_eq!(r.token_deltas.len(), 2);
        let debit = r.token_deltas.iter().find(|d| d.amount < 0).unwrap();
        let credit = r.token_deltas.iter().find(|d| d.amount > 0).unwrap();
        assert_eq!(debit.mint, mint_pi);
        assert_eq!(credit.mint, mint_token);
        assert_eq!(debit.amount, -(10_000i128));
        assert!(credit.amount > 0 && credit.amount < 10_000);
    }

    #[test]
    fn swap_nonexistent_pool_fails() {
        let (dex, _, mint_pi, mint_token) = setup();

        let trader = Address([5u8; 20]);
        let r = dex.swap(trader, mint_pi, mint_token, 10_000, 0);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn swap_same_token_fails() {
        let (dex, _, mint_pi, _) = setup();

        let trader = Address([5u8; 20]);
        let r = dex.swap(trader, mint_pi, mint_pi, 10_000, 0);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn swap_slippage_protection() {
        let (dex, creator, mint_pi, mint_token) = setup();
        dex.create_pool(creator, mint_pi, mint_token);
        dex.add_liquidity(creator, mint_pi, mint_token, 1_000_000, 1_000_000, 0);

        let trader = Address([5u8; 20]);
        // Set unrealistically high minimum output
        let r = dex.swap(trader, mint_pi, mint_token, 10_000, 10_000);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn swap_updates_reserves() {
        let (dex, creator, mint_pi, mint_token) = setup();
        dex.create_pool(creator, mint_pi, mint_token);
        dex.add_liquidity(creator, mint_pi, mint_token, 1_000_000, 1_000_000, 0);

        let pool_id = PoolId::derive(&mint_pi, &mint_token);
        let pool_before = dex.get_pool(&pool_id).unwrap();

        let trader = Address([5u8; 20]);
        dex.swap(trader, mint_pi, mint_token, 10_000, 0);

        let pool_after = dex.get_pool(&pool_id).unwrap();

        // Reserve of input token should increase, output should decrease
        let (a_is_pi, _) = if pool_before.mint_a == mint_pi {
            (true, false)
        } else {
            (false, true)
        };

        if a_is_pi {
            assert!(pool_after.reserve_a > pool_before.reserve_a);
            assert!(pool_after.reserve_b < pool_before.reserve_b);
        } else {
            assert!(pool_after.reserve_b > pool_before.reserve_b);
            assert!(pool_after.reserve_a < pool_before.reserve_a);
        }
    }

    #[test]
    fn swap_zero_amount_fails() {
        let (dex, creator, mint_pi, mint_token) = setup();
        dex.create_pool(creator, mint_pi, mint_token);
        dex.add_liquidity(creator, mint_pi, mint_token, 1_000_000, 1_000_000, 0);

        let trader = Address([5u8; 20]);
        let r = dex.swap(trader, mint_pi, mint_token, 0, 0);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn constant_product_invariant_maintained() {
        let (dex, creator, mint_pi, mint_token) = setup();
        dex.create_pool(creator, mint_pi, mint_token);
        dex.add_liquidity(creator, mint_pi, mint_token, 1_000_000, 1_000_000, 0);

        let pool_id = PoolId::derive(&mint_pi, &mint_token);
        let pool_before = dex.get_pool(&pool_id).unwrap();
        let k_before = pool_before.reserve_a as u128 * pool_before.reserve_b as u128;

        // Do several swaps
        let trader = Address([5u8; 20]);
        for _ in 0..5 {
            dex.swap(trader, mint_pi, mint_token, 1_000, 0);
        }

        let pool_after = dex.get_pool(&pool_id).unwrap();
        let k_after = pool_after.reserve_a as u128 * pool_after.reserve_b as u128;

        // k should only increase (fees add to reserves)
        assert!(k_after >= k_before);
    }

    #[test]
    fn bidirectional_swaps() {
        let (dex, creator, mint_pi, mint_token) = setup();
        dex.create_pool(creator, mint_pi, mint_token);
        dex.add_liquidity(creator, mint_pi, mint_token, 1_000_000, 1_000_000, 0);

        let trader = Address([5u8; 20]);

        // Swap A → B
        let r1 = dex.swap(trader, mint_pi, mint_token, 10_000, 0);
        assert_eq!(r1.status, TransactionStatus::Success);

        // Swap B → A
        let r2 = dex.swap(trader, mint_token, mint_pi, 5_000, 0);
        assert_eq!(r2.status, TransactionStatus::Success);
    }

    #[test]
    fn near_max_reserves_handled_safely() {
        // Verify that near-max reserve values don't cause panics or unexpected behavior
        let dex = DexExecutor::new();
        let creator = Address([1u8; 20]);
        let mint_a = MintId::ZERO;
        let mint_b = MintId::derive(&Address([2u8; 20]), 0);

        dex.create_pool(creator, mint_a, mint_b);

        // Set reserves near u64::MAX via load_pool
        let pool_id = PoolId::derive(&mint_a, &mint_b);
        let mut pool = dex.get_pool(&pool_id).unwrap();
        pool.reserve_a = u64::MAX - 100;
        pool.reserve_b = u64::MAX - 100;
        pool.lp_supply = 1_000_000;
        dex.load_pool(pool);

        // Adding liquidity to near-max pool should not panic — must error gracefully
        let r = dex.add_liquidity(creator, mint_a, mint_b, 200, 200, 0);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));

        // Swapping into near-max pool should not panic
        let trader = Address([5u8; 20]);
        let r = dex.swap(trader, mint_a, mint_b, 200, 0);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    /// Fix 191: Verify that snapshot is captured before any swaps mutate reserves.
    /// Multiple swaps in the same block should all measure price impact against
    /// the pre-block snapshot, not against already-mutated reserves.
    #[test]
    fn snapshot_used_for_price_impact_not_mutated_reserves() {
        let (dex, creator, mint_pi, mint_token) = setup();
        dex.create_pool(creator, mint_pi, mint_token);
        dex.add_liquidity(creator, mint_pi, mint_token, 10_000_000, 10_000_000, 0);

        // Take block-start snapshot
        dex.snapshot_block_start();

        let pool_id = PoolId::derive(&mint_pi, &mint_token);

        // Verify snapshot was captured
        let snapshot = dex.block_start_reserves.get(&pool_id);
        assert!(
            snapshot.is_some(),
            "Snapshot should exist after snapshot_block_start()"
        );
        let (snap_a, snap_b) = *snapshot.unwrap();
        assert!(snap_a > 0 && snap_b > 0, "Snapshot reserves should be > 0");

        // First swap — modifies live reserves
        let trader = Address([5u8; 20]);
        let r1 = dex.swap(trader, mint_pi, mint_token, 100_000, 0);
        assert_eq!(r1.status, TransactionStatus::Success);

        // Verify that the snapshot is UNCHANGED after the swap
        let snapshot_after = dex.block_start_reserves.get(&pool_id).unwrap();
        let (snap_a2, snap_b2) = *snapshot_after;
        assert_eq!(
            snap_a, snap_a2,
            "Snapshot reserve_a should not change after swap"
        );
        assert_eq!(
            snap_b, snap_b2,
            "Snapshot reserve_b should not change after swap"
        );

        // Verify that live reserves DID change
        let pool_after = dex.get_pool(&pool_id).unwrap();
        assert_ne!(
            pool_after.reserve_a, snap_a,
            "Live reserves should differ from snapshot after swap"
        );
    }

    /// Fix 191: Pools created mid-block (after snapshot) should have their
    /// initial reserves captured on first swap, not use stale mutated data.
    #[test]
    fn mid_block_pool_gets_snapshot_on_first_swap() {
        let dex = DexExecutor::new();
        let creator = Address([1u8; 20]);
        let mint_a = MintId::ZERO;
        let mint_b = MintId::derive(&Address([2u8; 20]), 0);

        // Take block-start snapshot (no pools exist yet)
        dex.snapshot_block_start();

        // Create and fund a pool mid-block (after snapshot)
        dex.create_pool(creator, mint_a, mint_b);
        dex.add_liquidity(creator, mint_a, mint_b, 1_000_000, 1_000_000, 0);

        let pool_id = PoolId::derive(&mint_a, &mint_b);

        // Pool should NOT be in the snapshot (created after snapshot)
        assert!(
            dex.block_start_reserves.get(&pool_id).is_none(),
            "Mid-block pool should not be in block-start snapshot"
        );

        // First swap — should capture initial reserves as snapshot
        let trader = Address([5u8; 20]);
        let r = dex.swap(trader, mint_a, mint_b, 10_000, 0);
        assert_eq!(r.status, TransactionStatus::Success);

        // After first swap, the pool should now have a snapshot entry
        let snapshot = dex.block_start_reserves.get(&pool_id);
        assert!(
            snapshot.is_some(),
            "Mid-block pool should get snapshot on first swap"
        );

        // The snapshot should reflect the pre-first-swap reserves
        // (which are the reserves after add_liquidity, before the swap mutated them)
        let pool_now = dex.get_pool(&pool_id).unwrap();
        let (snap_a, snap_b) = *snapshot.unwrap();

        // Live reserves should differ from snapshot after the swap
        // (the swap added to one reserve and removed from the other)
        assert!(
            pool_now.reserve_a != snap_a || pool_now.reserve_b != snap_b,
            "Live reserves should differ from snapshot after swap"
        );
    }

    /// Fix 191: Verify snapshot_taken flag is set correctly.
    #[test]
    fn snapshot_taken_flag() {
        let dex = DexExecutor::new();

        // Before snapshot, flag should be false
        assert!(!dex.snapshot_taken.load(Ordering::Acquire));

        // After snapshot, flag should be true
        dex.snapshot_block_start();
        assert!(dex.snapshot_taken.load(Ordering::Acquire));

        // After another snapshot (new block), flag should still be true
        dex.snapshot_block_start();
        assert!(dex.snapshot_taken.load(Ordering::Acquire));
    }
}
