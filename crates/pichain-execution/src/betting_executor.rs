//! Betting executor — manages match lifecycle for PVP, Poker, and Arcade games.
//!
//! Handles: CreateMatch, JoinMatch, StartMatch, ResolveMatch, CancelMatch.
//!
//! All games are player-vs-player. A 2% house fee is burned on resolution.

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use pichain_crypto::ed25519::Address;
use pichain_types::betting::{
    BettingMatch, GameCategory, MatchId, MatchState,
    HOUSE_FEE_BPS, MATCH_EXPIRY_BLOCKS, MAX_GAME_ID_LEN, MAX_MATCHES_PER_CREATOR,
    MAX_PLAYERS, MAX_WAGER, MIN_PLAYERS, MIN_WAGER, RESOLVE_DEADLINE_BLOCKS,
};
use pichain_types::transaction::{TransactionEvent, TransactionStatus};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Result of executing a betting operation.
#[derive(Clone, Debug)]
pub struct BettingResult {
    /// Execution status.
    pub status: TransactionStatus,
    /// Events emitted.
    pub events: Vec<TransactionEvent>,
    /// Match changes to persist.
    pub match_changes: HashMap<MatchId, BettingMatch>,
    /// PI to debit from sender (escrow). 0 if no debit.
    pub debit_sender: u64,
    /// PI credits: (address, amount) for winners/refunds.
    pub credits: Vec<(Address, u64)>,
    /// House fee to burn.
    pub house_fee_burn: u64,
}

fn betting_error(msg: &str) -> BettingResult {
    BettingResult {
        status: TransactionStatus::Reverted(format!("betting: {msg}")),
        events: vec![],
        match_changes: HashMap::new(),
        debit_sender: 0,
        credits: vec![],
        house_fee_burn: 0,
    }
}

/// Betting executor — manages matches.
pub struct BettingExecutor {
    /// In-memory match cache.
    matches: DashMap<MatchId, BettingMatch>,
    /// Per-creator active match count (anti-spam).
    creator_match_count: DashMap<Address, u64>,
    /// Match creation nonces per address.
    match_nonces: DashMap<Address, u64>,
    /// Block timestamp for deterministic state.
    block_timestamp_ms: AtomicU64,
    /// Current block height.
    block_height: AtomicU64,
}

impl BettingExecutor {
    pub fn new() -> Self {
        Self {
            matches: DashMap::new(),
            creator_match_count: DashMap::new(),
            match_nonces: DashMap::new(),
            block_timestamp_ms: AtomicU64::new(0),
            block_height: AtomicU64::new(0),
        }
    }

    /// Clear all cached state (used when re-executing after gas-limit trimming).
    pub fn clear_state(&self) {
        self.matches.clear();
        self.creator_match_count.clear();
        self.match_nonces.clear();
    }

    pub fn set_block_timestamp(&self, ts: u64) {
        self.block_timestamp_ms.store(ts, Ordering::Release);
    }

    pub fn set_block_height(&self, h: u64) {
        self.block_height.store(h, Ordering::Release);
    }

    fn block_timestamp(&self) -> u64 {
        self.block_timestamp_ms.load(Ordering::Acquire)
    }

    fn block_height_val(&self) -> u64 {
        self.block_height.load(Ordering::Acquire)
    }

    /// Load a match into the cache (from storage on startup).
    pub fn load_match(&self, m: BettingMatch) {
        // Track creator counts for active matches
        if matches!(m.state, MatchState::Open | MatchState::InProgress) {
            *self.creator_match_count.entry(m.creator).or_insert(0) += 1;
        }
        // Track nonces
        let nonce = m.creator_nonce + 1;
        self.match_nonces
            .entry(m.creator)
            .and_modify(|n| *n = (*n).max(nonce))
            .or_insert(nonce);
        self.matches.insert(m.id, m);
    }

    /// Get a match from the cache.
    pub fn get_match(&self, id: &MatchId) -> Option<BettingMatch> {
        self.matches.get(id).map(|v| v.clone())
    }

    /// Snapshot all matches (for block-level persistence).
    pub fn all_matches(&self) -> HashMap<MatchId, BettingMatch> {
        self.matches.iter().map(|e| (*e.key(), e.value().clone())).collect()
    }

    /// Snapshot all match nonces.
    pub fn all_nonces(&self) -> HashMap<Address, u64> {
        self.match_nonces.iter().map(|e| (*e.key(), *e.value())).collect()
    }

    /// Get all active matches (for API queries).
    pub fn active_matches(&self) -> Vec<BettingMatch> {
        self.matches
            .iter()
            .filter(|e| matches!(e.value().state, MatchState::Open | MatchState::InProgress))
            .map(|e| e.value().clone())
            .collect()
    }

    /// Get matches by category.
    pub fn matches_by_category(&self, category: &GameCategory) -> Vec<BettingMatch> {
        self.matches
            .iter()
            .filter(|e| &e.value().category == category)
            .map(|e| e.value().clone())
            .collect()
    }

    /// Get matches where address is a participant.
    pub fn matches_by_player(&self, addr: &Address) -> Vec<BettingMatch> {
        self.matches
            .iter()
            .filter(|e| e.value().participants.contains(addr))
            .map(|e| e.value().clone())
            .collect()
    }

    /// Total wagered across all matches.
    pub fn total_wagered(&self) -> u64 {
        self.matches.iter().map(|e| e.value().escrow_total).sum()
    }

    /// Total active matches.
    pub fn active_count(&self) -> u64 {
        self.matches
            .iter()
            .filter(|e| matches!(e.value().state, MatchState::Open | MatchState::InProgress))
            .count() as u64
    }

    /// Create a new match.
    #[allow(clippy::too_many_arguments)]
    pub fn create_match(
        &self,
        sender: Address,
        game_category: u8,
        game_id: &str,
        wager: u64,
        max_players: u8,
        server_seed_hash: [u8; 32],
    ) -> BettingResult {
        // Validate category
        let category = match GameCategory::from_tag(game_category) {
            Some(c) => c,
            None => return betting_error("invalid game category"),
        };

        // Validate game_id
        if game_id.is_empty() || game_id.len() > MAX_GAME_ID_LEN {
            return betting_error("game_id must be 1-32 characters");
        }
        for b in game_id.bytes() {
            if !(0x20..=0x7E).contains(&b) || b == b'<' || b == b'>' || b == b'&' {
                return betting_error("game_id contains invalid characters");
            }
        }

        // Validate wager
        if wager < MIN_WAGER {
            return betting_error("wager below minimum (0.01 PI)");
        }
        if wager > MAX_WAGER {
            return betting_error("wager exceeds maximum (1000 PI)");
        }

        // Validate players
        if !(MIN_PLAYERS..=MAX_PLAYERS).contains(&max_players) {
            return betting_error("max_players must be 2-10");
        }

        // Anti-spam: check creator's active match count
        let count = self.creator_match_count.get(&sender).map(|v| *v).unwrap_or(0);
        if count >= MAX_MATCHES_PER_CREATOR {
            return betting_error("too many active matches (max 50)");
        }

        // Get and increment match nonce
        let mut nonce = self.match_nonces.entry(sender).or_insert(0);
        let match_nonce = *nonce;
        *nonce += 1;

        let match_id = MatchId::derive(&sender, match_nonce);
        let height = self.block_height_val();

        let betting_match = BettingMatch {
            id: match_id,
            category,
            game_id: game_id.to_string(),
            state: MatchState::Open,
            creator: sender,
            wager,
            max_players,
            participants: vec![sender], // Creator is first participant
            escrow_total: wager,
            house_fee_bps: HOUSE_FEE_BPS,
            created_at_height: height,
            resolve_deadline_height: height + MATCH_EXPIRY_BLOCKS + RESOLVE_DEADLINE_BLOCKS,
            created_at_ms: self.block_timestamp(),
            server_seed_hash,
            client_seeds: vec![],
            server_seed_revealed: None,
            entropy_block_hash: None,
            creator_nonce: match_nonce,
        };

        // Atomic insert
        match self.matches.entry(match_id) {
            Entry::Occupied(_) => return betting_error("match ID collision"),
            Entry::Vacant(v) => {
                v.insert(betting_match.clone());
            }
        }

        *self.creator_match_count.entry(sender).or_insert(0) += 1;

        let mut match_changes = HashMap::new();
        match_changes.insert(match_id, betting_match);

        BettingResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "CreateMatch".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "match_id": match_id.to_string(),
                    "game_id": game_id,
                    "wager": wager,
                    "max_players": max_players,
                }))
                .unwrap_or_default(),
            }],
            match_changes,
            debit_sender: wager, // Escrow creator's wager
            credits: vec![],
            house_fee_burn: 0,
        }
    }

    /// Join an existing match.
    pub fn join_match(
        &self,
        sender: Address,
        match_id: MatchId,
        client_seed: [u8; 32],
    ) -> BettingResult {
        let mut match_ref = match self.matches.get_mut(&match_id) {
            Some(m) => m,
            None => return betting_error("match not found"),
        };
        let m = match_ref.value_mut();

        if m.state != MatchState::Open {
            return betting_error("match is not open for joining");
        }

        if m.participants.contains(&sender) {
            return betting_error("already joined this match");
        }

        if m.participants.len() >= m.max_players as usize {
            return betting_error("match is full");
        }

        // Check expiry
        let height = self.block_height_val();
        if height > m.created_at_height + MATCH_EXPIRY_BLOCKS {
            return betting_error("match has expired");
        }

        m.participants.push(sender);
        m.escrow_total = m.escrow_total.saturating_add(m.wager);
        m.client_seeds.push((sender, client_seed));

        let match_snapshot = m.clone();
        let wager_amount = match_snapshot.wager;
        drop(match_ref);

        let mut match_changes = HashMap::new();
        match_changes.insert(match_id, match_snapshot);

        BettingResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "JoinMatch".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "match_id": match_id.to_string(),
                    "player": sender.0.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                }))
                .unwrap_or_default(),
            }],
            match_changes,
            debit_sender: wager_amount,
            credits: vec![],
            house_fee_burn: 0,
        }
    }

    /// Start a match (creator only, Open -> InProgress).
    pub fn start_match(
        &self,
        sender: Address,
        match_id: MatchId,
        block_hash: [u8; 32],
    ) -> BettingResult {
        let mut match_ref = match self.matches.get_mut(&match_id) {
            Some(m) => m,
            None => return betting_error("match not found"),
        };
        let m = match_ref.value_mut();

        if sender != m.creator {
            return betting_error("only the creator can start the match");
        }

        if m.state != MatchState::Open {
            return betting_error("match is not open");
        }

        if m.participants.len() < MIN_PLAYERS as usize {
            return betting_error("need at least 2 players to start");
        }

        m.state = MatchState::InProgress;
        m.entropy_block_hash = Some(block_hash);
        // Reset resolve deadline from now
        let height = self.block_height_val();
        m.resolve_deadline_height = height + RESOLVE_DEADLINE_BLOCKS;

        let match_snapshot = m.clone();
        let player_count = match_snapshot.participants.len();
        drop(match_ref);

        let mut match_changes = HashMap::new();
        match_changes.insert(match_id, match_snapshot);

        BettingResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "StartMatch".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "match_id": match_id.to_string(),
                    "players": player_count,
                }))
                .unwrap_or_default(),
            }],
            match_changes,
            debit_sender: 0,
            credits: vec![],
            house_fee_burn: 0,
        }
    }

    /// Resolve a match — verify server seed, distribute pot to winners.
    pub fn resolve_match(
        &self,
        sender: Address,
        match_id: MatchId,
        winners: &[Address],
        server_seed: &[u8],
    ) -> BettingResult {
        let mut match_ref = match self.matches.get_mut(&match_id) {
            Some(m) => m,
            None => return betting_error("match not found"),
        };
        let m = match_ref.value_mut();

        // Only creator can resolve (they committed the server seed)
        if sender != m.creator {
            return betting_error("only the creator can resolve the match");
        }

        if !matches!(m.state, MatchState::Open | MatchState::InProgress) {
            return betting_error("match is not active");
        }

        if winners.is_empty() {
            return betting_error("must have at least one winner");
        }

        // Verify all winners are participants
        for w in winners {
            if !m.participants.contains(w) {
                return betting_error("winner is not a participant");
            }
        }

        // Verify server seed: blake3(server_seed) must equal committed hash
        let seed_hash = pichain_crypto::hash(server_seed);
        if seed_hash.as_bytes() != &m.server_seed_hash {
            return betting_error("server seed does not match commitment");
        }

        // Calculate payouts
        let total_pot = m.escrow_total;
        let house_fee = match total_pot.checked_mul(m.house_fee_bps as u64) {
            Some(v) => v / 10_000,
            None => {
                // Use u128 for overflow-safe calculation rather than silently zeroing
                let fee_128 = (total_pot as u128) * (m.house_fee_bps as u128) / 10_000;
                if fee_128 > u64::MAX as u128 {
                    return betting_error("house fee calculation overflow");
                }
                fee_128 as u64
            }
        };
        let winner_pot = total_pot.saturating_sub(house_fee);

        // Distribute evenly among winners
        let per_winner = if winners.is_empty() {
            0
        } else {
            winner_pot / winners.len() as u64
        };
        let remainder = winner_pot.saturating_sub(per_winner * winners.len() as u64);

        let mut credits = Vec::new();
        let mut payouts = Vec::new();
        for (i, w) in winners.iter().enumerate() {
            let amount = if i == 0 {
                per_winner + remainder // First winner gets rounding dust
            } else {
                per_winner
            };
            credits.push((*w, amount));
            payouts.push((*w, amount));
        }

        m.state = MatchState::Completed {
            winners: winners.to_vec(),
            payouts: payouts.clone(),
        };
        m.server_seed_revealed = Some(server_seed.to_vec());

        // Decrement creator's active match count
        if let Some(mut count) = self.creator_match_count.get_mut(&m.creator) {
            *count = count.saturating_sub(1);
        }

        let match_snapshot = m.clone();
        drop(match_ref);

        let mut match_changes = HashMap::new();
        match_changes.insert(match_id, match_snapshot);

        BettingResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "ResolveMatch".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "match_id": match_id.to_string(),
                    "winners": winners.iter().map(|w| w.0.iter().map(|b| format!("{b:02x}")).collect::<String>()).collect::<Vec<_>>(),
                    "total_pot": total_pot,
                    "house_fee": house_fee,
                }))
                .unwrap_or_default(),
            }],
            match_changes,
            debit_sender: 0,
            credits,
            house_fee_burn: house_fee,
        }
    }

    /// Remove a single participant from a match — refund their wager.
    /// Creator-only. Cannot remove self (use CancelMatch instead).
    /// Auto-cancels the match if fewer than 2 participants remain.
    pub fn remove_participant(
        &self,
        sender: Address,
        match_id: MatchId,
        participant: Address,
    ) -> BettingResult {
        let mut match_ref = match self.matches.get_mut(&match_id) {
            Some(m) => m,
            None => return betting_error("match not found"),
        };
        let m = match_ref.value_mut();

        if sender != m.creator {
            return betting_error("only the creator can remove participants");
        }

        if participant == m.creator {
            return betting_error("creator cannot remove self (use CancelMatch)");
        }

        if !matches!(m.state, MatchState::Open | MatchState::InProgress) {
            return betting_error("match is not active");
        }

        let idx = match m.participants.iter().position(|p| *p == participant) {
            Some(i) => i,
            None => return betting_error("participant not in match"),
        };

        // Remove participant and adjust escrow
        m.participants.remove(idx);
        m.escrow_total = m.escrow_total.saturating_sub(m.wager);

        // Remove their client seed if present
        m.client_seeds.retain(|(addr, _)| *addr != participant);

        let wager = m.wager;

        // Auto-cancel if fewer than 2 participants remain
        if m.participants.len() < MIN_PLAYERS as usize {
            // Refund remaining participants + the removed one
            let mut credits: Vec<(Address, u64)> = m
                .participants
                .iter()
                .map(|p| (*p, wager))
                .collect();
            credits.push((participant, wager));

            m.state = MatchState::Cancelled;

            // Decrement creator's active match count
            if let Some(mut count) = self.creator_match_count.get_mut(&m.creator) {
                *count = count.saturating_sub(1);
            }

            let match_snapshot = m.clone();
            drop(match_ref);

            let mut match_changes = HashMap::new();
            match_changes.insert(match_id, match_snapshot);

            return BettingResult {
                status: TransactionStatus::Success,
                events: vec![TransactionEvent {
                    emitter: sender,
                    event_type: "RemoveParticipant".to_string(),
                    data: serde_json::to_vec(&serde_json::json!({
                        "match_id": match_id.to_string(),
                        "removed": participant.0.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                        "auto_cancelled": true,
                    }))
                    .unwrap_or_default(),
                }],
                match_changes,
                debit_sender: 0,
                credits,
                house_fee_burn: 0,
            };
        }

        // Normal removal — just refund the removed participant
        let match_snapshot = m.clone();
        drop(match_ref);

        let mut match_changes = HashMap::new();
        match_changes.insert(match_id, match_snapshot);

        BettingResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "RemoveParticipant".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "match_id": match_id.to_string(),
                    "removed": participant.0.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                    "auto_cancelled": false,
                }))
                .unwrap_or_default(),
            }],
            match_changes,
            debit_sender: 0,
            credits: vec![(participant, wager)],
            house_fee_burn: 0,
        }
    }

    /// Cancel a match — refund all participants.
    pub fn cancel_match(
        &self,
        sender: Address,
        match_id: MatchId,
    ) -> BettingResult {
        let mut match_ref = match self.matches.get_mut(&match_id) {
            Some(m) => m,
            None => return betting_error("match not found"),
        };
        let m = match_ref.value_mut();

        let height = self.block_height_val();
        let expired = height > m.resolve_deadline_height;

        // Only creator can cancel, OR anyone can cancel if expired
        if sender != m.creator && !expired {
            return betting_error("only the creator can cancel (or wait for expiry)");
        }

        if !matches!(m.state, MatchState::Open | MatchState::InProgress) {
            return betting_error("match is not active");
        }

        // Refund all participants
        let mut credits = Vec::new();
        for p in &m.participants {
            credits.push((*p, m.wager));
        }

        m.state = MatchState::Cancelled;

        // Decrement creator's active match count
        if let Some(mut count) = self.creator_match_count.get_mut(&m.creator) {
            *count = count.saturating_sub(1);
        }

        let match_snapshot = m.clone();
        let refunded_count = match_snapshot.participants.len();
        drop(match_ref);

        let mut match_changes = HashMap::new();
        match_changes.insert(match_id, match_snapshot);

        BettingResult {
            status: TransactionStatus::Success,
            events: vec![TransactionEvent {
                emitter: sender,
                event_type: "CancelMatch".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "match_id": match_id.to_string(),
                    "refunded_players": refunded_count,
                }))
                .unwrap_or_default(),
            }],
            match_changes,
            debit_sender: 0,
            credits,
            house_fee_burn: 0,
        }
    }
}

impl Default for BettingExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_executor() -> BettingExecutor {
        let ex = BettingExecutor::new();
        ex.set_block_height(100);
        ex.set_block_timestamp(1000);
        ex
    }

    fn addr(n: u8) -> Address {
        Address([n; 20])
    }

    #[test]
    fn create_match_success() {
        let ex = make_executor();
        let seed_hash = [42u8; 32];
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, seed_hash);
        assert!(matches!(result.status, TransactionStatus::Success));
        assert_eq!(result.debit_sender, 1_000_000_000);
        assert_eq!(result.match_changes.len(), 1);
        let m = result.match_changes.values().next().unwrap();
        assert_eq!(m.participants.len(), 1);
        assert_eq!(m.participants[0], addr(1));
    }

    #[test]
    fn create_match_invalid_category() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 5, "fps", 1_000_000_000, 4, [0; 32]);
        assert!(matches!(result.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn create_match_wager_too_low() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 100, 4, [0; 32]); // 100 < MIN_WAGER
        assert!(matches!(result.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn join_match_success() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();

        let join_result = ex.join_match(addr(2), match_id, [99; 32]);
        assert!(matches!(join_result.status, TransactionStatus::Success));
        assert_eq!(join_result.debit_sender, 1_000_000_000);

        let m = ex.get_match(&match_id).unwrap();
        assert_eq!(m.participants.len(), 2);
        assert_eq!(m.escrow_total, 2_000_000_000);
    }

    #[test]
    fn join_match_already_joined() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();

        let join_result = ex.join_match(addr(1), match_id, [99; 32]);
        assert!(matches!(join_result.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn start_match_success() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();
        ex.join_match(addr(2), match_id, [99; 32]);

        let start_result = ex.start_match(addr(1), match_id, [0xAB; 32]);
        assert!(matches!(start_result.status, TransactionStatus::Success));

        let m = ex.get_match(&match_id).unwrap();
        assert!(matches!(m.state, MatchState::InProgress));
        assert_eq!(m.entropy_block_hash, Some([0xAB; 32]));
    }

    #[test]
    fn start_match_not_creator() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();
        ex.join_match(addr(2), match_id, [99; 32]);

        let start_result = ex.start_match(addr(2), match_id, [0xAB; 32]);
        assert!(matches!(start_result.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn resolve_match_success() {
        let ex = make_executor();
        let server_seed = b"my_secret_seed_value_for_testing!";
        let seed_hash = *pichain_crypto::hash(server_seed).as_bytes();

        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, seed_hash);
        let match_id = *result.match_changes.keys().next().unwrap();
        ex.join_match(addr(2), match_id, [99; 32]);
        ex.start_match(addr(1), match_id, [0xAB; 32]);

        let resolve_result = ex.resolve_match(
            addr(1),
            match_id,
            &[addr(2)],
            server_seed,
        );
        assert!(matches!(resolve_result.status, TransactionStatus::Success));

        // Total pot = 2 * 1e9 = 2e9
        // House fee = 2e9 * 200 / 10000 = 40_000_000
        // Winner gets 2e9 - 40_000_000 = 1_960_000_000
        assert_eq!(resolve_result.house_fee_burn, 40_000_000);
        assert_eq!(resolve_result.credits.len(), 1);
        assert_eq!(resolve_result.credits[0].0, addr(2));
        assert_eq!(resolve_result.credits[0].1, 1_960_000_000);
    }

    #[test]
    fn resolve_match_bad_seed() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();
        ex.join_match(addr(2), match_id, [99; 32]);

        let resolve_result = ex.resolve_match(addr(1), match_id, &[addr(2)], b"wrong_seed");
        assert!(matches!(resolve_result.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn cancel_match_refund() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();
        ex.join_match(addr(2), match_id, [99; 32]);

        let cancel_result = ex.cancel_match(addr(1), match_id);
        assert!(matches!(cancel_result.status, TransactionStatus::Success));
        assert_eq!(cancel_result.credits.len(), 2);
        assert_eq!(cancel_result.credits[0].1, 1_000_000_000);
        assert_eq!(cancel_result.credits[1].1, 1_000_000_000);
    }

    #[test]
    fn cancel_by_non_creator_fails_before_expiry() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();

        let cancel_result = ex.cancel_match(addr(2), match_id);
        assert!(matches!(cancel_result.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn cancel_by_anyone_after_expiry() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();

        // Advance block height past deadline
        ex.set_block_height(100 + MATCH_EXPIRY_BLOCKS + RESOLVE_DEADLINE_BLOCKS + 1);

        let cancel_result = ex.cancel_match(addr(3), match_id);
        assert!(matches!(cancel_result.status, TransactionStatus::Success));
    }

    // --- RemoveParticipant tests ---

    #[test]
    fn remove_participant_success() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();
        ex.join_match(addr(2), match_id, [99; 32]);
        ex.join_match(addr(3), match_id, [88; 32]);

        // 3 players, remove player 3
        let remove_result = ex.remove_participant(addr(1), match_id, addr(3));
        assert!(matches!(remove_result.status, TransactionStatus::Success));
        assert_eq!(remove_result.credits.len(), 1);
        assert_eq!(remove_result.credits[0].0, addr(3));
        assert_eq!(remove_result.credits[0].1, 1_000_000_000);

        let m = ex.get_match(&match_id).unwrap();
        assert_eq!(m.participants.len(), 2);
        assert_eq!(m.escrow_total, 2_000_000_000);
        assert!(matches!(m.state, MatchState::Open));
    }

    #[test]
    fn remove_participant_not_creator_fails() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();
        ex.join_match(addr(2), match_id, [99; 32]);
        ex.join_match(addr(3), match_id, [88; 32]);

        // Player 2 tries to remove player 3
        let remove_result = ex.remove_participant(addr(2), match_id, addr(3));
        assert!(matches!(remove_result.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn remove_participant_self_fails() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();
        ex.join_match(addr(2), match_id, [99; 32]);

        // Creator tries to remove self
        let remove_result = ex.remove_participant(addr(1), match_id, addr(1));
        assert!(matches!(remove_result.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn remove_participant_not_in_match_fails() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();
        ex.join_match(addr(2), match_id, [99; 32]);

        // Try to remove someone who never joined
        let remove_result = ex.remove_participant(addr(1), match_id, addr(5));
        assert!(matches!(remove_result.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn remove_participant_auto_cancels_below_minimum() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();
        ex.join_match(addr(2), match_id, [99; 32]);

        // 2 players, remove player 2 → only 1 left → auto-cancel
        let remove_result = ex.remove_participant(addr(1), match_id, addr(2));
        assert!(matches!(remove_result.status, TransactionStatus::Success));
        // Should refund both: remaining creator + removed player
        assert_eq!(remove_result.credits.len(), 2);

        let m = ex.get_match(&match_id).unwrap();
        assert!(matches!(m.state, MatchState::Cancelled));
    }

    #[test]
    fn remove_participant_completed_match_fails() {
        let ex = make_executor();
        let server_seed = b"my_secret_seed_value_for_testing!";
        let seed_hash = *pichain_crypto::hash(server_seed).as_bytes();

        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, seed_hash);
        let match_id = *result.match_changes.keys().next().unwrap();
        ex.join_match(addr(2), match_id, [99; 32]);
        ex.start_match(addr(1), match_id, [0xAB; 32]);
        ex.resolve_match(addr(1), match_id, &[addr(2)], server_seed);

        // Try to remove from completed match
        let remove_result = ex.remove_participant(addr(1), match_id, addr(2));
        assert!(matches!(remove_result.status, TransactionStatus::Reverted(_)));
    }

    #[test]
    fn remove_participant_in_progress_works() {
        let ex = make_executor();
        let result = ex.create_match(addr(1), 0, "fps", 1_000_000_000, 4, [42; 32]);
        let match_id = *result.match_changes.keys().next().unwrap();
        ex.join_match(addr(2), match_id, [99; 32]);
        ex.join_match(addr(3), match_id, [88; 32]);
        ex.start_match(addr(1), match_id, [0xAB; 32]);

        // 3 players, match in progress, remove player 3
        let remove_result = ex.remove_participant(addr(1), match_id, addr(3));
        assert!(matches!(remove_result.status, TransactionStatus::Success));
        assert_eq!(remove_result.credits.len(), 1);
        assert_eq!(remove_result.credits[0].0, addr(3));

        let m = ex.get_match(&match_id).unwrap();
        assert_eq!(m.participants.len(), 2);
        assert!(matches!(m.state, MatchState::InProgress));
    }
}
