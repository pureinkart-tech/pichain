//! PIChain account model.
//!
//! Every account is inherently a smart contract (native account abstraction).
//! Accounts can be externally-owned (default logic) or fully programmable.

use pichain_crypto::ed25519::Address;
use serde::{Deserialize, Serialize};

use crate::{Nonce, PiAmount};

/// State of an account on the PIChain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Account address.
    pub address: Address,
    /// Account state data.
    pub state: AccountState,
}

/// Core account state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountState {
    /// Balance in base PI units (1 PI = 10^9 base units).
    pub balance: PiAmount,
    /// Transaction nonce (monotonically increasing).
    pub nonce: Nonce,
    /// Optional contract code hash (None = externally-owned account with default logic).
    pub code_hash: Option<pichain_crypto::Hash>,
    /// Storage root hash for contract state (Jellyfish Merkle Tree root).
    pub storage_root: pichain_crypto::poseidon::PoseidonHash,
    /// Amount of PI staked.
    pub staked: PiAmount,
    /// Delegated staking: validator this account delegates to.
    pub delegate: Option<Address>,
    /// Amount of PI in the unbonding period (not yet available for withdrawal).
    #[serde(default)]
    pub unbonding: PiAmount,
    /// Block height at which the unbonding period started.
    /// Funds become available at `unbonding_height + UNBONDING_BLOCKS`.
    #[serde(default)]
    pub unbonding_height: u64,
    /// Data size in bytes (for state rent calculation).
    pub data_size: u64,
    /// Non-transferable PI balance, usable only for gas fees.
    /// Granted on wallet activation (3.14 PI) and cannot be sent to other accounts.
    #[serde(default)]
    pub locked_balance: PiAmount,
}

impl AccountState {
    /// Create a new empty account state.
    /// Unbonding period in blocks (~7 days at 314ms block time).
    /// 7 * 24 * 3600 * 1000 / 314 ≈ 1,927,389 blocks.
    pub const UNBONDING_BLOCKS: u64 = 1_927_389;

    pub fn new() -> Self {
        Self {
            balance: 0,
            nonce: 0,
            code_hash: None,
            storage_root: pichain_crypto::poseidon::PoseidonHash::ZERO,
            staked: 0,
            delegate: None,
            unbonding: 0,
            unbonding_height: 0,
            data_size: 0,
            locked_balance: 0,
        }
    }

    /// Create an account with a genesis balance.
    pub fn with_balance(balance: PiAmount) -> Self {
        Self {
            balance,
            ..Self::new()
        }
    }

    /// Check if this account has contract code.
    pub fn is_contract(&self) -> bool {
        self.code_hash.is_some()
    }

    /// Minimum balance required for state rent.
    /// Formula: data_bytes * 1_000 base units (= data_bytes * 0.000001 PI)
    pub fn minimum_balance(&self) -> PiAmount {
        // Minimum 1 KB worth = 1_000_000 base units (0.001 PI)
        
        self.data_size.max(1024).saturating_mul(1_000)
    }

    /// Check if balance is above state rent minimum.
    pub fn is_rent_exempt(&self) -> bool {
        self.balance >= self.minimum_balance()
    }

    /// Validate the account state invariants.
    /// Returns Err if the state is inconsistent.
    pub fn validate(&self) -> Result<(), &'static str> {
        // staked + unbonding must not exceed balance
        let locked = self.staked.checked_add(self.unbonding)
            .ok_or("staked + unbonding overflows u64")?;
        if locked > self.balance {
            return Err("staked + unbonding exceeds balance");
        }
        Ok(())
    }

    /// Total available balance for transfers (balance minus staked and unbonding).
    /// Does NOT include locked_balance (which is fee-only, non-transferable).
    pub fn available_balance(&self) -> PiAmount {
        self.balance
            .saturating_sub(self.staked)
            .saturating_sub(self.unbonding)
    }

    /// Total balance available for paying gas fees (includes locked_balance).
    /// locked_balance can only be used for fees, never for transfers.
    pub fn fee_balance(&self) -> PiAmount {
        self.available_balance()
            .saturating_add(self.locked_balance)
    }
}

impl Default for AccountState {
    fn default() -> Self {
        Self::new()
    }
}

impl Account {
    pub fn new(address: Address) -> Self {
        Self {
            address,
            state: AccountState::new(),
        }
    }

    pub fn with_balance(address: Address, balance: PiAmount) -> Self {
        Self {
            address,
            state: AccountState::with_balance(balance),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_account_has_zero_balance() {
        let acct = AccountState::new();
        assert_eq!(acct.balance, 0);
        assert_eq!(acct.nonce, 0);
        assert!(!acct.is_contract());
    }

    #[test]
    fn state_rent_minimum() {
        let mut acct = AccountState::new();
        acct.data_size = 2048; // 2 KB
        assert_eq!(acct.minimum_balance(), 2_048_000);
    }

    #[test]
    fn available_balance_excludes_staked() {
        let mut acct = AccountState::with_balance(10_000);
        acct.staked = 3_000;
        assert_eq!(acct.available_balance(), 7_000);
    }

    #[test]
    fn available_balance_excludes_unbonding() {
        let mut acct = AccountState::with_balance(10_000);
        acct.staked = 2_000;
        acct.unbonding = 3_000;
        // Available = 10_000 - 2_000 (staked) - 3_000 (unbonding) = 5_000
        assert_eq!(acct.available_balance(), 5_000);
    }

    #[test]
    fn account_state_validate_ok() {
        let mut acct = AccountState::with_balance(10_000);
        acct.staked = 3_000;
        acct.unbonding = 2_000;
        assert!(acct.validate().is_ok());
    }

    #[test]
    fn account_state_validate_overflow() {
        let mut acct = AccountState::with_balance(u64::MAX);
        acct.staked = u64::MAX;
        acct.unbonding = 1;
        assert!(acct.validate().is_err());
    }

    #[test]
    fn account_state_validate_exceeds_balance() {
        let mut acct = AccountState::with_balance(1_000);
        acct.staked = 600;
        acct.unbonding = 500;
        assert!(acct.validate().is_err());
    }

    #[test]
    fn available_balance_saturates_to_zero() {
        let mut acct = AccountState::with_balance(1_000);
        acct.staked = 500;
        acct.unbonding = 600;
        // 1_000 - 500 - 600 = -100 -> saturates to 0
        assert_eq!(acct.available_balance(), 0);
    }

    #[test]
    fn fee_balance_includes_locked() {
        let mut acct = AccountState::with_balance(5_000);
        acct.staked = 1_000;
        acct.locked_balance = 3_140_000_000; // 3.14 PI locked
        assert_eq!(acct.available_balance(), 4_000); // locked NOT included
        assert_eq!(acct.fee_balance(), 4_000 + 3_140_000_000); // locked IS included
    }

    #[test]
    fn locked_balance_default_zero() {
        let acct = AccountState::new();
        assert_eq!(acct.locked_balance, 0);
        assert_eq!(acct.fee_balance(), 0);
    }

    #[test]
    fn locked_balance_backwards_compat() {
        // Simulate loading an old account without locked_balance field
        let json = r#"{"balance":1000,"nonce":0,"code_hash":null,"storage_root":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"staked":0,"delegate":null,"unbonding":0,"unbonding_height":0,"data_size":0}"#;
        let state: AccountState = serde_json::from_str(json).unwrap();
        assert_eq!(state.locked_balance, 0);
    }
}
