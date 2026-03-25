//! Native token types — SPL-like token system built into the execution layer.
//!
//! Each custom token has a `TokenMint` (the token definition) and multiple
//! `TokenAccount` entries (per-user balances). This enables parallel execution
//! since different token pairs have independent state.
//!
//! Native PI is represented as `NATIVE_PI_MINT = MintId::ZERO`, allowing
//! unified treatment in pools and transfers.

use pichain_crypto::keys::Address;
use serde::{Deserialize, Serialize};

/// Maximum length of a token symbol (e.g., "DOPI", "USDC").
pub const MAX_SYMBOL_LEN: usize = 10;
/// Maximum length of a token name.
pub const MAX_TOKEN_NAME_LEN: usize = 64;
/// Maximum length of metadata URI.
pub const MAX_METADATA_URI_LEN: usize = 256;

/// Unique identifier for a token mint.
/// Derived from `hash(creator || nonce || "pichain-mint")`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct MintId(pub [u8; 32]);

impl MintId {
    /// The zero mint ID represents native PI token.
    pub const ZERO: Self = Self([0u8; 32]);

    /// Derive a deterministic mint ID from creator and nonce.
    pub fn derive(creator: &Address, nonce: u64) -> Self {
        let mut data = Vec::with_capacity(20 + 8 + 13);
        data.extend_from_slice(&creator.0);
        data.extend_from_slice(&nonce.to_le_bytes());
        data.extend_from_slice(b"pichain-mint");
        let hash = pichain_crypto::hash(&data);
        Self(*hash.as_bytes())
    }

    /// Check if this is the native PI token.
    pub fn is_native_pi(&self) -> bool {
        *self == Self::ZERO
    }
}

impl std::fmt::Display for MintId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Token mint — defines a custom token.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenMint {
    /// Unique mint identifier.
    pub id: MintId,
    /// Token name (e.g., "Doge PIChain").
    pub name: String,
    /// Token symbol (e.g., "DOPI").
    pub symbol: String,
    /// Decimal places (9 = same as PI, 6 = USDC-like).
    pub decimals: u8,
    /// Total supply minted so far.
    pub total_supply: u64,
    /// Maximum supply (0 = unlimited).
    pub max_supply: u64,
    /// Creator address.
    pub creator: Address,
    /// Current mint authority (can mint new tokens). None = frozen supply.
    pub mint_authority: Option<Address>,
    /// Freeze authority (can freeze/thaw accounts). None = no freezing.
    pub freeze_authority: Option<Address>,
    /// Whether the token is currently active.
    pub active: bool,
    /// Creation timestamp.
    pub created_at_ms: u64,
    /// URI for token metadata (logo, description, etc).
    pub metadata_uri: String,
}

impl TokenMint {
    /// Validate token mint fields. Returns Err with description on invalid input.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.symbol.is_empty() || self.symbol.len() > MAX_SYMBOL_LEN {
            return Err("symbol must be 1-10 characters");
        }
        if self.name.is_empty() || self.name.len() > MAX_TOKEN_NAME_LEN {
            return Err("name must be 1-64 characters");
        }
        if self.metadata_uri.len() > MAX_METADATA_URI_LEN {
            return Err("metadata_uri must be <= 256 characters");
        }
        if self.decimals > 18 {
            return Err("decimals must be <= 18");
        }
        Ok(())
    }

    /// Create a new token mint.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: MintId,
        name: String,
        symbol: String,
        decimals: u8,
        max_supply: u64,
        creator: Address,
        metadata_uri: String,
        created_at_ms: u64,
    ) -> Self {
        Self {
            id,
            name,
            symbol,
            decimals,
            total_supply: 0,
            max_supply,
            creator,
            mint_authority: Some(creator),
            freeze_authority: Some(creator),
            active: true,
            created_at_ms,
            metadata_uri,
        }
    }

    /// Check if more tokens can be minted.
    pub fn can_mint(&self, amount: u64) -> bool {
        if self.mint_authority.is_none() {
            return false;
        }
        if self.max_supply == 0 {
            return true; // Unlimited supply
        }
        self.total_supply
            .checked_add(amount)
            .map(|new_supply| new_supply <= self.max_supply)
            .unwrap_or(false)
    }
}

/// Deterministic key for a token account: hash(owner || mint_id).
/// This enables O(1) lookups and parallel execution.
pub fn token_account_key(owner: &Address, mint: &MintId) -> [u8; 32] {
    let mut data = Vec::with_capacity(72);
    data.extend_from_slice(&owner.0);
    data.extend_from_slice(&mint.0);
    data.extend_from_slice(b"pichain-token-account");
    *pichain_crypto::hash(&data).as_bytes()
}

/// Token account — holds a user's balance for a specific token.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenAccount {
    /// The account key (deterministic from owner + mint).
    #[serde(default)]
    pub key: [u8; 32],
    /// Owner address.
    pub owner: Address,
    /// Mint this account holds tokens for.
    pub mint: MintId,
    /// Token balance.
    pub balance: u64,
    /// Delegate who can spend on behalf of the owner.
    pub delegate: Option<Address>,
    /// Maximum amount the delegate can spend.
    pub delegate_amount: u64,
    /// Whether this account is frozen by the freeze authority.
    pub frozen: bool,
}

impl TokenAccount {
    /// Create a new token account with zero balance.
    pub fn new(owner: Address, mint: MintId) -> Self {
        let key = token_account_key(&owner, &mint);
        Self {
            key,
            owner,
            mint,
            balance: 0,
            delegate: None,
            delegate_amount: 0,
            frozen: false,
        }
    }

    /// Create a token account with an initial balance.
    pub fn with_balance(owner: Address, mint: MintId, balance: u64) -> Self {
        let mut account = Self::new(owner, mint);
        account.balance = balance;
        account
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_id_derive_deterministic() {
        let addr = Address([1u8; 20]);
        let id1 = MintId::derive(&addr, 0);
        let id2 = MintId::derive(&addr, 0);
        assert_eq!(id1, id2);

        // Different nonce = different ID
        let id3 = MintId::derive(&addr, 1);
        assert_ne!(id1, id3);

        // Different creator = different ID
        let addr2 = Address([2u8; 20]);
        let id4 = MintId::derive(&addr2, 0);
        assert_ne!(id1, id4);
    }

    #[test]
    fn native_pi_mint() {
        assert!(MintId::ZERO.is_native_pi());
        let custom = MintId::derive(&Address([1u8; 20]), 0);
        assert!(!custom.is_native_pi());
    }

    #[test]
    fn token_account_key_deterministic() {
        let owner = Address([1u8; 20]);
        let mint = MintId::derive(&Address([2u8; 20]), 0);

        let key1 = token_account_key(&owner, &mint);
        let key2 = token_account_key(&owner, &mint);
        assert_eq!(key1, key2);

        // Different owner = different key
        let key3 = token_account_key(&Address([3u8; 20]), &mint);
        assert_ne!(key1, key3);
    }

    #[test]
    fn token_mint_supply_limits() {
        let mut mint = TokenMint::new(
            MintId::derive(&Address([1u8; 20]), 0),
            "TestCoin".to_string(),
            "TST".to_string(),
            9,
            1_000_000, // max supply 1M
            Address([1u8; 20]),
            String::new(),
            0,
        );

        assert!(mint.can_mint(500_000));
        assert!(mint.can_mint(1_000_000));
        assert!(!mint.can_mint(1_000_001));

        mint.total_supply = 900_000;
        assert!(mint.can_mint(100_000));
        assert!(!mint.can_mint(100_001));

        // Revoke mint authority
        mint.mint_authority = None;
        assert!(!mint.can_mint(1));
    }

    #[test]
    fn unlimited_supply() {
        let mint = TokenMint::new(
            MintId::derive(&Address([1u8; 20]), 0),
            "InfiniteCoin".to_string(),
            "INF".to_string(),
            9,
            0, // unlimited
            Address([1u8; 20]),
            String::new(),
            0,
        );

        assert!(mint.can_mint(u64::MAX / 2));
    }

    #[test]
    fn token_mint_validation() {
        let addr = Address([1u8; 20]);
        let mint = TokenMint::new(
            MintId::derive(&addr, 0),
            "TestCoin".to_string(),
            "TST".to_string(),
            9,
            1_000_000,
            addr,
            String::new(),
            0,
        );
        assert!(mint.validate().is_ok());

        // Symbol too long
        let bad_sym = TokenMint::new(
            MintId::derive(&addr, 0),
            "TestCoin".to_string(),
            "TOOLONGSYMBOL".to_string(),
            9,
            1_000_000,
            addr,
            String::new(),
            0,
        );
        assert!(bad_sym.validate().is_err());

        // Empty symbol
        let empty_sym = TokenMint::new(
            MintId::derive(&addr, 0),
            "TestCoin".to_string(),
            "".to_string(),
            9,
            1_000_000,
            addr,
            String::new(),
            0,
        );
        assert!(empty_sym.validate().is_err());

        // Decimals too high
        let bad_dec = TokenMint::new(
            MintId::derive(&addr, 0),
            "TestCoin".to_string(),
            "TST".to_string(),
            19,
            1_000_000,
            addr,
            String::new(),
            0,
        );
        assert!(bad_dec.validate().is_err());
    }

    #[test]
    fn token_account_creation() {
        let owner = Address([1u8; 20]);
        let mint = MintId::derive(&Address([2u8; 20]), 0);

        let account = TokenAccount::new(owner, mint);
        assert_eq!(account.balance, 0);
        assert_eq!(account.owner, owner);
        assert_eq!(account.mint, mint);
        assert!(!account.frozen);
        assert!(account.delegate.is_none());

        let account2 = TokenAccount::with_balance(owner, mint, 1_000);
        assert_eq!(account2.balance, 1_000);
    }
}
