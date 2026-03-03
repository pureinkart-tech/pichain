//! Betting/gaming types — PVP matches, poker tables, and arcade games.
//!
//! All games are player-vs-player (no house edge model). A small house fee
//! is burned on resolution for deflation. Provably fair randomness uses a
//! commit-reveal scheme with future block hashes.

use pichain_crypto::ed25519::Address;
use serde::{Deserialize, Serialize};

/// Unique identifier for a betting match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct MatchId(pub [u8; 32]);

impl MatchId {
    /// Derive a match ID from creator address and nonce.
    pub fn derive(creator: &Address, nonce: u64) -> Self {
        let mut data = Vec::with_capacity(20 + 8 + 12);
        data.extend_from_slice(&creator.0);
        data.extend_from_slice(&nonce.to_le_bytes());
        data.extend_from_slice(b"pichain-bet");
        let hash = pichain_crypto::hash(&data);
        Self(*hash.as_bytes())
    }
}

impl std::fmt::Display for MatchId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Game category.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameCategory {
    /// PVP match betting (FPS, racing, fighting, custom).
    Pvp,
    /// Poker (Texas Hold'em).
    Poker,
    /// Arcade browser games.
    Arcade,
}

impl GameCategory {
    /// From u8 tag.
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(GameCategory::Pvp),
            1 => Some(GameCategory::Poker),
            2 => Some(GameCategory::Arcade),
            _ => None,
        }
    }

    /// To u8 tag.
    pub fn to_tag(&self) -> u8 {
        match self {
            GameCategory::Pvp => 0,
            GameCategory::Poker => 1,
            GameCategory::Arcade => 2,
        }
    }
}

/// Match lifecycle state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchState {
    /// Accepting players.
    Open,
    /// Game in progress, no more joins.
    InProgress,
    /// Completed with winners.
    Completed {
        /// Winner addresses.
        winners: Vec<Address>,
        /// Payouts: (address, amount).
        payouts: Vec<(Address, u64)>,
    },
    /// Cancelled, all players refunded.
    Cancelled,
}

/// A betting match / game session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BettingMatch {
    /// Unique match ID.
    pub id: MatchId,
    /// Game category (PVP, Poker, Arcade).
    pub category: GameCategory,
    /// Game type identifier (e.g., "fps", "poker", "snake").
    pub game_id: String,
    /// Current state.
    pub state: MatchState,
    /// Match creator.
    pub creator: Address,
    /// Wager per player (base PI units).
    pub wager: u64,
    /// Maximum players.
    pub max_players: u8,
    /// Participants who have joined.
    pub participants: Vec<Address>,
    /// Total PI escrowed.
    pub escrow_total: u64,
    /// House fee in basis points (200 = 2%).
    pub house_fee_bps: u16,
    /// Block height when created.
    pub created_at_height: u64,
    /// Block height deadline for resolution.
    pub resolve_deadline_height: u64,
    /// Creation timestamp (ms).
    pub created_at_ms: u64,

    // --- Provably fair ---
    /// Server seed commitment: blake3(server_seed).
    pub server_seed_hash: [u8; 32],
    /// Client seeds from each participant.
    pub client_seeds: Vec<(Address, [u8; 32])>,
    /// Revealed server seed (set on resolution).
    #[serde(default)]
    pub server_seed_revealed: Option<Vec<u8>>,
    /// Block hash used as entropy (captured on StartMatch).
    #[serde(default)]
    pub entropy_block_hash: Option<[u8; 32]>,

    /// Creator's match nonce (for deterministic MatchId).
    pub creator_nonce: u64,
}

// ── Constants ──────────────────────────────────────────────────────

/// Minimum wager: 0.01 PI.
pub const MIN_WAGER: u64 = 10_000_000;

/// Maximum wager: 1000 PI.
pub const MAX_WAGER: u64 = 1_000_000_000_000;

/// House fee: 2% (200 bps), burned on resolution.
pub const HOUSE_FEE_BPS: u16 = 200;

/// Match expiry: ~30 minutes at 314ms blocks = 5,732 blocks.
pub const MATCH_EXPIRY_BLOCKS: u64 = 5_732;

/// Resolution deadline: ~2 hours = 22,930 blocks.
pub const RESOLVE_DEADLINE_BLOCKS: u64 = 22_930;

/// Maximum active matches per creator (anti-spam).
pub const MAX_MATCHES_PER_CREATOR: u64 = 50;

/// Minimum players.
pub const MIN_PLAYERS: u8 = 2;

/// Maximum players in a match.
pub const MAX_PLAYERS: u8 = 10;

/// Maximum game_id string length.
pub const MAX_GAME_ID_LEN: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_id_deterministic() {
        let addr = Address([1u8; 20]);
        let id1 = MatchId::derive(&addr, 0);
        let id2 = MatchId::derive(&addr, 0);
        assert_eq!(id1, id2);
    }

    #[test]
    fn match_id_different_nonces() {
        let addr = Address([1u8; 20]);
        let id1 = MatchId::derive(&addr, 0);
        let id2 = MatchId::derive(&addr, 1);
        assert_ne!(id1, id2);
    }

    #[test]
    fn game_category_roundtrip() {
        for tag in 0..=2u8 {
            let cat = GameCategory::from_tag(tag).unwrap();
            assert_eq!(cat.to_tag(), tag);
        }
        assert!(GameCategory::from_tag(3).is_none());
    }
}
