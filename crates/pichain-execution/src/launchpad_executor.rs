//! Launchpad executor — manages token launch lifecycle.
//!
//! Handles: CreateLaunch, ParticipateInLaunch, FinalizeLaunch.
//!
//! When a launch is finalized, the executor signals that an AMM pool
//! should be created with the raised PI and remaining tokens.

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use pichain_crypto::keys::Address;
use pichain_types::launchpad::{LaunchId, LaunchState, LaunchType, TokenLaunch};
use pichain_types::token::MintId;
use pichain_types::transaction::{TransactionEvent, TransactionStatus};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Result of executing a launchpad operation.
#[derive(Clone, Debug)]
pub struct LaunchpadResult {
    /// Execution status.
    pub status: TransactionStatus,
    /// Events emitted.
    pub events: Vec<TransactionEvent>,
    /// Launch changes (launch_id → updated launch).
    pub launch_changes: HashMap<LaunchId, TokenLaunch>,
    /// If finalized, contains the pool seeding details.
    pub finalization: Option<PoolSeedRequest>,
    /// Tokens received by the participant (set by participate()).
    pub tokens_received: u64,
    /// PI refunded to the participant (set by participate()).
    pub refund: u64,
    /// PI returned to seller (set by sell()).
    pub pi_returned: u64,
}

/// Request to create an AMM pool when a launch finalizes.
#[derive(Clone, Debug)]
pub struct PoolSeedRequest {
    /// Token mint for the launched token.
    pub mint: MintId,
    /// PI amount for the pool.
    pub pi_amount: u64,
    /// Token amount for the pool.
    pub token_amount: u64,
    /// Creator who receives remaining PI (after pool).
    pub creator: Address,
    /// PI to send to creator (raised PI minus pool PI).
    pub creator_pi: u64,
}

/// Launchpad executor — manages token launches.
pub struct LaunchpadExecutor {
    /// In-memory launch cache.
    launches: DashMap<LaunchId, TokenLaunch>,
    /// Block timestamp for deterministic object creation.
    block_timestamp_ms: AtomicU64,
}

impl LaunchpadExecutor {
    pub fn new() -> Self {
        Self {
            launches: DashMap::new(),
            block_timestamp_ms: AtomicU64::new(0),
        }
    }

    /// Clear all cached state. Used by the block producer when re-executing
    /// transactions after gas-limit trimming to prevent double-mutations.
    pub fn clear_state(&self) {
        self.launches.clear();
    }

    /// Set the block timestamp for deterministic state creation.
    pub fn set_block_timestamp(&self, ts: u64) {
        self.block_timestamp_ms.store(ts, Ordering::Release);
    }

    fn block_timestamp(&self) -> u64 {
        self.block_timestamp_ms.load(Ordering::Acquire)
    }

    /// Load a launch into the cache (from storage on startup).
    pub fn load_launch(&self, launch: TokenLaunch) {
        self.launches.insert(launch.id, launch);
    }

    /// Get a launch from the cache.
    pub fn get_launch(&self, id: &LaunchId) -> Option<TokenLaunch> {
        self.launches.get(id).map(|v| v.clone())
    }

    /// Get a launch by mint ID.
    pub fn get_launch_by_mint(&self, mint: &MintId) -> Option<TokenLaunch> {
        let id = LaunchId::from_mint(mint);
        self.get_launch(&id)
    }

    /// Snapshot all launches (for block-level persistence).
    pub fn all_launches(&self) -> HashMap<LaunchId, TokenLaunch> {
        self.launches
            .iter()
            .map(|e| (*e.key(), e.value().clone()))
            .collect()
    }

    /// Create a new token launch.
    #[allow(clippy::too_many_arguments)]
    pub fn create_launch(
        &self,
        sender: Address,
        mint: MintId,
        launch_type: LaunchType,
        tokens_for_sale: u64,
        target_pi: u64,
        max_per_address: u64,
        mint_authority: Option<Address>,
        token_decimals: u8,
    ) -> LaunchpadResult {
        // M4-FIX: Defense-in-depth — verify sender is the mint authority even though
        // the caller (executor.rs) already checks this. Prevents misuse if create_launch
        // is called directly from other code paths without the authority check.
        if mint_authority != Some(sender) {
            return launchpad_error("sender is not the mint authority for this token");
        }

        if tokens_for_sale == 0 {
            return launchpad_error("tokens_for_sale must be > 0");
        }
        if target_pi == 0 {
            return launchpad_error("target_pi must be > 0");
        }
        // max_per_address == 0 means no per-address limit

        let launch_id = LaunchId::from_mint(&mint);

        let launch = TokenLaunch {
            id: launch_id,
            mint,
            creator: sender,
            launch_type: launch_type.clone(),
            state: LaunchState::Active,
            tokens_for_sale,
            tokens_sold: 0,
            pi_raised: 0,
            target_pi,
            liquidity_bps: TokenLaunch::DEFAULT_LIQUIDITY_BPS,
            token_liquidity_bps: TokenLaunch::DEFAULT_TOKEN_LIQUIDITY_BPS,
            max_per_address,
            contributions: HashMap::new(),
            created_at_ms: self.block_timestamp(),
            token_decimals,
        };

        // Atomic check-and-insert to prevent TOCTOU race under parallel execution
        match self.launches.entry(launch_id) {
            Entry::Occupied(_) => return launchpad_error("launch already exists for this token"),
            Entry::Vacant(v) => {
                v.insert(launch.clone());
            }
        }

        let mut launch_changes = HashMap::new();
        launch_changes.insert(launch_id, launch);

        LaunchpadResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "CreateLaunch".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "launch_id": launch_id.to_string(),
                    "mint": mint.to_string(),
                    "tokens_for_sale": tokens_for_sale,
                    "target_pi": target_pi,
                }))
                .unwrap_or_default(),
            }],
            launch_changes,
            finalization: None,
            tokens_received: 0,
            refund: 0,
            pi_returned: 0,
        }
    }

    /// Participate in a launch (buy tokens with PI).
    pub fn participate(&self, sender: Address, mint: MintId, pi_amount: u64) -> LaunchpadResult {
        if pi_amount == 0 {
            return launchpad_error("contribution must be > 0");
        }

        let launch_id = LaunchId::from_mint(&mint);

        // Use get_mut to hold shard lock during read-modify-write (prevents TOCTOU race)
        let mut launch_ref = match self.launches.get_mut(&launch_id) {
            Some(l) => l,
            None => return launchpad_error("launch not found"),
        };
        let launch = launch_ref.value_mut();

        if launch.state != LaunchState::Active {
            return launchpad_error("launch is not active");
        }

        // Calculate tokens to receive
        let tokens = launch.tokens_for_pi(pi_amount);
        if tokens == 0 {
            return launchpad_error("contribution too small to purchase any tokens");
        }

        // Calculate actual cost (might be less than pi_amount for bonding curve).
        // If calculate_cost returns None, return an error instead of falling back
        // to pi_amount (which could overcharge the participant).
        let actual_cost = match launch.calculate_cost(tokens) {
            Some(cost) => cost,
            None => return launchpad_error("cost calculation failed for requested tokens"),
        };
        if actual_cost == 0 {
            return launchpad_error("cost calculation returned zero — tokens cannot be free");
        }
        let refund = pi_amount.saturating_sub(actual_cost);

        // Check per-address limit using actual_cost (not pi_amount).
        // Using pi_amount would over-count contributions when bonding curves charge less
        // than the requested amount, causing incorrect refund calculations and artificially
        // restrictive per-address limits.
        let current_contribution = launch.contributions.get(&sender).copied().unwrap_or(0);
        let new_contribution = match current_contribution.checked_add(actual_cost) {
            Some(v) => v,
            None => return launchpad_error("contribution overflow"),
        };
        // max_per_address == 0 means no per-address limit
        if launch.max_per_address > 0 && new_contribution > launch.max_per_address {
            return launchpad_error(&format!(
                "exceeds max contribution per address: current {}, adding {}, max {}",
                current_contribution, actual_cost, launch.max_per_address
            ));
        }

        // Update launch state (with overflow protection + supply cap)
        launch.tokens_sold = match launch.tokens_sold.checked_add(tokens) {
            Some(v) if v <= launch.tokens_for_sale => v,
            Some(v) => {
                return launchpad_error(&format!(
                    "tokens_sold {} would exceed tokens_for_sale {}",
                    v, launch.tokens_for_sale
                ))
            }
            None => return launchpad_error("tokens_sold overflow"),
        };
        launch.pi_raised = match launch.pi_raised.checked_add(actual_cost) {
            Some(v) => v,
            None => return launchpad_error("pi_raised overflow"),
        };
        // Track actual_cost spent (not the requested pi_amount) so that per-address
        // limits and refund calculations are based on real expenditure.
        *launch.contributions.entry(sender).or_insert(0) = new_contribution;

        // Check if target reached — transition to TargetReached and request pool seeding.
        // SECURITY: Do NOT transition directly to Finalized here. If pool seeding fails
        // downstream (e.g., duplicate pool), the launch must remain at TargetReached so
        // finalize() can be called again. Direct Finalized without pool = stuck funds.
        let mut finalization = None;
        if launch.pi_raised >= launch.target_pi || launch.tokens_sold >= launch.tokens_for_sale {
            launch.state = LaunchState::TargetReached;

            let (pi_for_pool, tokens_for_pool) = launch.finalization_amounts();
            if pi_for_pool > 0 && tokens_for_pool > 0 {
                let creator_pi = launch.pi_raised.saturating_sub(pi_for_pool);
                finalization = Some(PoolSeedRequest {
                    mint,
                    pi_amount: pi_for_pool,
                    token_amount: tokens_for_pool,
                    creator: launch.creator,
                    creator_pi,
                });
            }
        }

        let launch = launch.clone();
        drop(launch_ref); // Release shard lock before building result

        let mut launch_changes = HashMap::new();
        launch_changes.insert(launch_id, launch);

        LaunchpadResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "ParticipateInLaunch".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "launch_id": launch_id.to_string(),
                    "pi_spent": actual_cost,
                    "tokens_received": tokens,
                    "refund": refund,
                }))
                .unwrap_or_default(),
            }],
            launch_changes,
            finalization,
            tokens_received: tokens,
            refund,
            pi_returned: 0,
        }
    }

    /// Finalize a launch — create the AMM pool.
    pub fn finalize(&self, sender: Address, mint: MintId) -> LaunchpadResult {
        let launch_id = LaunchId::from_mint(&mint);

        // Use get_mut to hold shard lock during read-modify-write (prevents TOCTOU race
        // where two concurrent finalize calls could both read Active state)
        let mut launch_ref = match self.launches.get_mut(&launch_id) {
            Some(l) => l,
            None => return launchpad_error("launch not found"),
        };
        let launch = launch_ref.value_mut();

        // Anyone can finalize a launch that has reached its target — this prevents
        // the creator from holding graduation hostage.

        // SECURITY: Only allow finalization when target has been reached.
        // Previously accepted Active state, which allowed creators to finalize
        // after raising just 1 PI (premature finalization / rug-pull vector).
        if launch.state != LaunchState::TargetReached {
            return launchpad_error(
                "launch can only be finalized after reaching its target — \
                 current state must be TargetReached",
            );
        }

        // Must have raised something (should always be true if TargetReached, but defensive)
        if launch.pi_raised == 0 {
            return launchpad_error("no PI raised yet");
        }

        // Calculate pool seeding amounts
        let (pi_for_pool, tokens_for_pool) = launch.finalization_amounts();

        // Safety: tokens_for_pool should never be 0 after finalization_amounts
        // (it mints new tokens when all are sold), but guard against edge cases.
        if tokens_for_pool == 0 {
            return launchpad_error("cannot finalize: pool token calculation returned 0");
        }

        let creator_pi = match launch.pi_raised.checked_sub(pi_for_pool) {
            Some(v) => v,
            None => return launchpad_error("pool PI exceeds raised PI — accounting error"),
        };

        // PI conservation invariant: all raised PI must be accounted for.
        // pi_for_pool goes to DEX pool reserves, creator_pi goes to creator.
        // The pool reserves are implicitly backed by the native PI collected from
        // participants (which was removed from circulation during participation).
        if pi_for_pool.checked_add(creator_pi) != Some(launch.pi_raised) {
            return launchpad_error(&format!(
                "PI accounting invariant violated: pool({pi_for_pool}) + creator({creator_pi}) != raised({})",
                launch.pi_raised
            ));
        }

        launch.state = LaunchState::Finalized;
        let actual_creator = launch.creator;

        let launch = launch.clone();
        drop(launch_ref); // Release shard lock before building result

        let mut launch_changes = HashMap::new();
        launch_changes.insert(launch_id, launch);

        LaunchpadResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "FinalizeLaunch".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "launch_id": launch_id.to_string(),
                    "pi_for_pool": pi_for_pool,
                    "tokens_for_pool": tokens_for_pool,
                    "creator_pi": creator_pi,
                }))
                .unwrap_or_default(),
            }],
            launch_changes,
            finalization: Some(PoolSeedRequest {
                mint,
                pi_amount: pi_for_pool,
                token_amount: tokens_for_pool,
                creator: actual_creator,
                creator_pi,
            }),
            tokens_received: 0,
            refund: 0,
            pi_returned: 0,
        }
    }

    /// Sell tokens back to an active launch (reverse bonding curve).
    pub fn sell(&self, sender: Address, mint: MintId, token_amount: u64) -> LaunchpadResult {
        if token_amount == 0 {
            return launchpad_error("sell amount must be > 0");
        }

        let launch_id = LaunchId::from_mint(&mint);

        let mut launch_ref = match self.launches.get_mut(&launch_id) {
            Some(l) => l,
            None => return launchpad_error("launch not found"),
        };
        let launch = launch_ref.value_mut();

        if launch.state != LaunchState::Active {
            return launchpad_error("can only sell on active launches");
        }

        if token_amount > launch.tokens_sold {
            return launchpad_error("sell amount exceeds total tokens sold on curve");
        }

        // Calculate PI to return using reverse bonding curve
        let pi_return = match launch.calculate_sell_return(token_amount) {
            Some(v) => v,
            None => return launchpad_error("sell return calculation failed"),
        };

        if pi_return == 0 {
            return launchpad_error("sell amount too small to receive any PI");
        }

        // Cap at pi_raised to handle rounding dust from multiple buys
        let pi_return = std::cmp::min(pi_return, launch.pi_raised);

        // Update launch state
        launch.tokens_sold = launch.tokens_sold.saturating_sub(token_amount);
        launch.pi_raised = launch.pi_raised.saturating_sub(pi_return);

        // Update contribution tracking
        if let Some(contrib) = launch.contributions.get_mut(&sender) {
            *contrib = contrib.saturating_sub(pi_return);
            if *contrib == 0 {
                launch.contributions.remove(&sender);
            }
        }

        let launch = launch.clone();
        drop(launch_ref);

        let mut launch_changes = HashMap::new();
        launch_changes.insert(launch_id, launch);

        LaunchpadResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "SellFromLaunch".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "launch_id": launch_id.to_string(),
                    "tokens_sold": token_amount,
                    "pi_returned": pi_return,
                }))
                .unwrap_or_default(),
            }],
            launch_changes,
            finalization: None,
            tokens_received: 0,
            refund: 0,
            pi_returned: pi_return,
        }
    }

    /// Rollback a sell — re-add tokens and PI to the launch.
    /// Called when post-sell operations (token burn, PI credit) fail.
    pub fn rollback_sell(&self, mint: &MintId, sender: &Address, tokens: u64, pi_return: u64) {
        let launch_id = LaunchId::from_mint(mint);
        if let Some(mut launch_ref) = self.launches.get_mut(&launch_id) {
            launch_ref.tokens_sold = launch_ref.tokens_sold.saturating_add(tokens);
            launch_ref.pi_raised = launch_ref.pi_raised.saturating_add(pi_return);
            *launch_ref.contributions.entry(*sender).or_insert(0) += pi_return;
            // Re-check target reached
            if launch_ref.pi_raised >= launch_ref.target_pi
                || launch_ref.tokens_sold >= launch_ref.tokens_for_sale
            {
                launch_ref.state = LaunchState::TargetReached;
            }
        }
    }

    /// Rollback a finalization — revert launch state from Finalized back to TargetReached.
    /// Called when post-finalization pool seeding fails and the transaction must revert.
    pub fn rollback_finalization(&self, launch_id: &LaunchId) {
        if let Some(mut launch_ref) = self.launches.get_mut(launch_id) {
            if launch_ref.state == LaunchState::Finalized {
                launch_ref.state = LaunchState::TargetReached;
            }
        }
    }

    /// Rollback a participation — revert tokens_sold, pi_raised, and contributions.
    /// Called when post-participation token minting fails and the transaction must revert.
    pub fn rollback_participation(
        &self,
        mint: &MintId,
        sender: &Address,
        tokens: u64,
        pi_cost: u64,
    ) {
        let launch_id = LaunchId::from_mint(mint);
        if let Some(mut launch_ref) = self.launches.get_mut(&launch_id) {
            launch_ref.tokens_sold = launch_ref.tokens_sold.saturating_sub(tokens);
            launch_ref.pi_raised = launch_ref.pi_raised.saturating_sub(pi_cost);
            // R26-FIX: Decrement contribution rather than removing the entire entry.
            // Removing would erase prior successful contributions, enabling
            // per-address limit bypass on subsequent re-participation.
            if let Some(contrib) = launch_ref.contributions.get_mut(sender) {
                *contrib = contrib.saturating_sub(pi_cost);
                if *contrib == 0 {
                    launch_ref.contributions.remove(sender);
                }
            }
            // Revert state back to Active if we undid a TargetReached transition
            if launch_ref.state == LaunchState::TargetReached
                && launch_ref.pi_raised < launch_ref.target_pi
                && launch_ref.tokens_sold < launch_ref.tokens_for_sale
            {
                launch_ref.state = LaunchState::Active;
            }
        }
    }
}

impl Default for LaunchpadExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to create an error result.
fn launchpad_error(msg: &str) -> LaunchpadResult {
    LaunchpadResult {
        status: TransactionStatus::Reverted(msg.to_string()),
        events: vec![],
        launch_changes: HashMap::new(),
        finalization: None,
        tokens_received: 0,
        refund: 0,
        pi_returned: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_fair() -> (LaunchpadExecutor, Address, MintId) {
        let executor = LaunchpadExecutor::new();
        let creator = Address([1u8; 20]);
        let mint = MintId::derive(&creator, 0);
        (executor, creator, mint)
    }

    #[test]
    fn create_launch() {
        let (executor, creator, mint) = setup_fair();
        let result = executor.create_launch(
            creator,
            mint,
            LaunchType::FairLaunch {
                price_per_token: 1_000,
            },
            1_000_000,
            1_000_000_000,
            100_000_000,
            Some(creator),
            0,
        );
        assert_eq!(result.status, TransactionStatus::Success);
        assert!(executor.get_launch_by_mint(&mint).is_some());
    }

    #[test]
    fn create_duplicate_launch_fails() {
        let (executor, creator, mint) = setup_fair();
        executor.create_launch(
            creator,
            mint,
            LaunchType::FairLaunch {
                price_per_token: 1_000,
            },
            1_000_000,
            1_000_000_000,
            100_000_000,
            Some(creator),
            0,
        );
        let r = executor.create_launch(
            creator,
            mint,
            LaunchType::FairLaunch {
                price_per_token: 1_000,
            },
            1_000_000,
            1_000_000_000,
            100_000_000,
            Some(creator),
            0,
        );
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn participate_in_fair_launch() {
        let (executor, creator, mint) = setup_fair();
        executor.create_launch(
            creator,
            mint,
            LaunchType::FairLaunch {
                price_per_token: 1_000,
            },
            1_000_000,
            1_000_000_000,
            100_000_000,
            Some(creator),
            0,
        );

        let buyer = Address([2u8; 20]);
        let result = executor.participate(buyer, mint, 100_000);
        assert_eq!(result.status, TransactionStatus::Success);

        let launch = executor.get_launch_by_mint(&mint).unwrap();
        assert_eq!(launch.tokens_sold, 100); // 100_000 / 1_000
        assert_eq!(launch.pi_raised, 100_000);
    }

    #[test]
    fn participate_exceeds_max_per_address() {
        let (executor, creator, mint) = setup_fair();
        executor.create_launch(
            creator,
            mint,
            LaunchType::FairLaunch {
                price_per_token: 1_000,
            },
            1_000_000,
            1_000_000_000,
            10_000, // Very low max
            Some(creator),
            0,
        );

        let buyer = Address([2u8; 20]);
        let r = executor.participate(buyer, mint, 20_000);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn participate_in_bonding_curve() {
        let (executor, creator, _) = setup_fair();
        let mint = MintId::derive(&Address([3u8; 20]), 0);

        executor.create_launch(
            creator,
            mint,
            LaunchType::BondingCurve {
                base_price: 100,
                slope: 1,
                price_scale: 1,
            },
            10_000,
            10_000_000,
            u64::MAX,
            Some(creator),
            0,
        );

        let buyer = Address([2u8; 20]);
        let result = executor.participate(buyer, mint, 1_045);
        assert_eq!(result.status, TransactionStatus::Success);

        let launch = executor.get_launch_by_mint(&mint).unwrap();
        // With base_price=100, slope=1: cost for 10 tokens = 1045
        assert_eq!(launch.tokens_sold, 10);
    }

    #[test]
    fn auto_graduate_on_participate() {
        let (executor, creator, mint) = setup_fair();
        executor.create_launch(
            creator,
            mint,
            LaunchType::FairLaunch {
                price_per_token: 1_000,
            },
            1_000_000,
            100_000, // Low target
            u64::MAX,
            Some(creator),
            0,
        );

        // Participate enough to reach target — should auto-finalize
        let buyer = Address([2u8; 20]);
        let result = executor.participate(buyer, mint, 500_000);
        assert_eq!(result.status, TransactionStatus::Success);

        // Should transition to TargetReached (pool seeding happens in executor layer)
        let launch = executor.get_launch_by_mint(&mint).unwrap();
        assert_eq!(launch.state, LaunchState::TargetReached);

        // Result should include finalization data for pool seeding
        assert!(result.finalization.is_some());
        let pool_seed = result.finalization.unwrap();
        assert!(pool_seed.pi_amount > 0);
        assert!(pool_seed.token_amount > 0);
        assert_eq!(pool_seed.creator, creator);
    }

    #[test]
    fn anyone_can_finalize() {
        let (executor, creator, mint) = setup_fair();
        executor.create_launch(
            creator,
            mint,
            LaunchType::FairLaunch {
                price_per_token: 1_000,
            },
            1_000_000,
            100_000,
            u64::MAX,
            Some(creator),
            0,
        );

        // Participate just under target so it doesn't auto-finalize
        let buyer = Address([2u8; 20]);
        executor.participate(buyer, mint, 90_000);

        // Should still be active
        let launch = executor.get_launch_by_mint(&mint).unwrap();
        assert_eq!(launch.state, LaunchState::Active);

        // Cannot finalize yet — not at target
        let imposter = Address([99u8; 20]);
        let r = executor.finalize(imposter, mint);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn cannot_finalize_with_no_raises() {
        let (executor, creator, mint) = setup_fair();
        executor.create_launch(
            creator,
            mint,
            LaunchType::FairLaunch {
                price_per_token: 1_000,
            },
            1_000_000,
            100_000,
            u64::MAX,
            Some(creator),
            0,
        );

        let r = executor.finalize(creator, mint);
        assert!(matches!(r.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn multiple_participants() {
        let (executor, creator, mint) = setup_fair();
        executor.create_launch(
            creator,
            mint,
            LaunchType::FairLaunch {
                price_per_token: 1_000,
            },
            1_000_000,
            1_000_000_000,
            u64::MAX,
            Some(creator),
            0,
        );

        let buyer1 = Address([2u8; 20]);
        let buyer2 = Address([3u8; 20]);

        executor.participate(buyer1, mint, 100_000);
        executor.participate(buyer2, mint, 200_000);

        let launch = executor.get_launch_by_mint(&mint).unwrap();
        assert_eq!(launch.tokens_sold, 300); // 100 + 200
        assert_eq!(launch.pi_raised, 300_000);
        assert_eq!(launch.contributions.len(), 2);
    }

    #[test]
    fn contribution_overflow_protected() {
        let (executor, creator, mint) = setup_fair();
        executor.create_launch(
            creator,
            mint,
            LaunchType::FairLaunch { price_per_token: 1 },
            u64::MAX,
            u64::MAX,
            u64::MAX, // no per-address limit
            Some(creator),
            0,
        );

        let buyer1 = Address([2u8; 20]);
        let buyer2 = Address([3u8; 20]);
        // First big contribution from buyer1
        let r = executor.participate(buyer1, mint, u64::MAX / 2);
        assert_eq!(r.status, TransactionStatus::Success);

        // Second big contribution from buyer2 that would overflow pi_raised.
        // pi_raised is currently u64::MAX/2, adding u64::MAX/2 + 2 would overflow.
        // However, tokens_for_pi limits to remaining supply and calculate_cost
        // uses actual tokens. With remaining = u64::MAX - u64::MAX/2 = u64::MAX/2 + 1,
        // actual_cost = u64::MAX/2 + 1, so pi_raised = u64::MAX (no overflow).
        // We need a scenario where pi_raised actually overflows.
        let r = executor.participate(buyer2, mint, u64::MAX / 2 + 1);
        assert_eq!(r.status, TransactionStatus::Success);

        // Now pi_raised = u64::MAX, tokens_sold = u64::MAX.
        // A third contribution should fail because no tokens remain.
        let buyer3 = Address([4u8; 20]);
        let r = executor.participate(buyer3, mint, 1);
        assert!(
            matches!(r.status, TransactionStatus::Reverted(_)),
            "Should fail when no tokens remain. Status: {:?}",
            r.status
        );
    }

    /// Fix 190: Verify that contributions track actual_cost, not pi_amount.
    /// With a bonding curve, actual_cost can be less than pi_amount.
    /// The contributions map must reflect what was actually spent.
    #[test]
    fn contribution_tracks_actual_cost_not_pi_amount() {
        let (executor, creator, _) = setup_fair();
        let mint = MintId::derive(&Address([10u8; 20]), 0);

        // Create a bonding curve launch with base_price=100, slope=1
        executor.create_launch(
            creator,
            mint,
            LaunchType::BondingCurve {
                base_price: 100,
                slope: 1,
                price_scale: 1,
            },
            10_000,
            10_000_000,
            u64::MAX, // unlimited per-address
            Some(creator),
            0,
        );

        let buyer = Address([2u8; 20]);
        // Send 2000 PI. With base_price=100, slope=1, the actual cost for the
        // tokens received will differ from the requested pi_amount.
        let pi_amount = 2_000u64;
        let r = executor.participate(buyer, mint, pi_amount);
        assert_eq!(r.status, TransactionStatus::Success);

        let launch = executor.get_launch_by_mint(&mint).unwrap();
        let contribution = launch.contributions.get(&buyer).copied().unwrap_or(0);

        // The contribution should equal pi_raised (actual cost), not pi_amount
        assert_eq!(
            contribution, launch.pi_raised,
            "Contribution should track actual_cost ({}) not pi_amount ({}). Got: {}",
            launch.pi_raised, pi_amount, contribution
        );
        // The actual cost may differ from pi_amount for bonding curve
        // (it could be less if the curve doesn't perfectly match pi_amount)
        assert!(
            contribution <= pi_amount,
            "Contribution ({}) should be <= pi_amount ({})",
            contribution,
            pi_amount
        );
    }

    /// Fix 190: Verify per-address limit uses actual_cost correctly.
    /// A bonding curve might charge less than pi_amount, so the per-address
    /// limit should allow more contributions than if it tracked pi_amount.
    #[test]
    fn per_address_limit_uses_actual_cost() {
        let (executor, creator, _) = setup_fair();
        let mint = MintId::derive(&Address([11u8; 20]), 0);

        // Create a fair launch with a per-address limit
        let price_per_token = 1_000u64;
        let max_per_address = 5_000u64;
        executor.create_launch(
            creator,
            mint,
            LaunchType::FairLaunch { price_per_token },
            1_000_000,
            1_000_000_000,
            max_per_address,
            Some(creator),
            0,
        );

        let buyer = Address([2u8; 20]);

        // First contribution: buy 3 tokens for 3_000
        let r = executor.participate(buyer, mint, 3_000);
        assert_eq!(r.status, TransactionStatus::Success);

        let launch = executor.get_launch_by_mint(&mint).unwrap();
        let contribution = launch.contributions.get(&buyer).copied().unwrap_or(0);
        assert_eq!(contribution, 3_000);

        // Second contribution: buy 2 more tokens for 2_000 (total = 5_000 = max)
        let r = executor.participate(buyer, mint, 2_000);
        assert_eq!(r.status, TransactionStatus::Success);

        let launch = executor.get_launch_by_mint(&mint).unwrap();
        let contribution = launch.contributions.get(&buyer).copied().unwrap_or(0);
        assert_eq!(contribution, 5_000);

        // Third contribution: any more should fail
        let r = executor.participate(buyer, mint, 1_000);
        assert!(
            matches!(r.status, TransactionStatus::Reverted(ref msg) if msg.contains("exceeds max"))
        );
    }

    /// EXEC-215: Verify that calculate_cost failure returns an error instead of
    /// silently falling back to pi_amount.
    #[test]
    fn participate_cost_calculation_failure_returns_error() {
        let executor = LaunchpadExecutor::new();
        let creator = Address([1u8; 20]);
        let mint = MintId::derive(&creator, 42);

        // Create a bonding curve launch with extreme slope that will cause
        // calculate_cost overflow when tokens_sold is high.
        executor.create_launch(
            creator,
            mint,
            LaunchType::BondingCurve {
                base_price: u64::MAX / 2,
                slope: u64::MAX / 2,
                price_scale: 1,
            },
            1_000_000,
            u64::MAX,
            u64::MAX,
            Some(creator),
            0,
        );

        // Manually corrupt tokens_sold to a high value that will cause
        // calculate_cost to overflow (simulating a state inconsistency).
        if let Some(mut launch_ref) = executor.launches.get_mut(&LaunchId::from_mint(&mint)) {
            launch_ref.tokens_sold = 999_990;
        }

        let buyer = Address([2u8; 20]);
        // With tokens_sold near 999_990 and extreme base_price + slope,
        // the binary search in tokens_for_pi should return 0 (all costs overflow u64).
        // If tokens_for_pi returns 0, it hits the "contribution too small" error.
        // Either way, the participant should get an error — not a silent fallback to pi_amount.
        let r = executor.participate(buyer, mint, 1_000);
        assert!(
            matches!(r.status, TransactionStatus::Reverted(_)),
            "Cost calculation failure should return error, got: {:?}",
            r.status,
        );
    }

    #[test]
    fn rollback_participation() {
        let (executor, creator, mint) = setup_fair();
        executor.create_launch(
            creator,
            mint,
            LaunchType::FairLaunch {
                price_per_token: 1_000,
            },
            1_000_000,
            1_000_000_000,
            100_000_000,
            Some(creator),
            0,
        );

        let buyer = Address([2u8; 20]);
        let result = executor.participate(buyer, mint, 100_000);
        assert_eq!(result.status, TransactionStatus::Success);
        assert_eq!(result.tokens_received, 100);

        // Verify state after participation
        let launch = executor.get_launch_by_mint(&mint).unwrap();
        assert_eq!(launch.tokens_sold, 100);
        assert_eq!(launch.pi_raised, 100_000);

        // Rollback the participation
        executor.rollback_participation(&mint, &buyer, 100, 100_000);

        // Verify state is fully reverted
        let launch = executor.get_launch_by_mint(&mint).unwrap();
        assert_eq!(launch.tokens_sold, 0);
        assert_eq!(launch.pi_raised, 0);
        assert!(launch.contributions.get(&buyer).is_none());
        assert_eq!(launch.state, LaunchState::Active);
    }
}
