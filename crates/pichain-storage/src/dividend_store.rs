//! Dividend store — tracks holder dividend pools and per-user claim state.
//!
//! Uses the `state` column family with key prefix:
//! - `Dv:` + mint_id(32) → DividendPool JSON (per-token accumulator)
//! - `Dd:` + mint_id(32) + address(20) → DividendDebt (u128 LE, reward_debt)

use pichain_crypto::ed25519::Address;
use pichain_types::token::MintId;
use serde::{Deserialize, Serialize};

use crate::db::PiChainDB;
use crate::StorageError;

/// Dividend pool for a token — tracks accumulated trading fees for holder distribution.
/// Uses reward-per-share pattern for O(1) claiming.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DividendPool {
    /// Token mint this pool distributes dividends for.
    pub mint_id: MintId,
    /// Total PI accumulated for distribution (in base units).
    pub total_accumulated: u64,
    /// Total PI already claimed by holders.
    pub total_claimed: u64,
    /// Reward per share, scaled by 1e12 for precision.
    /// Updated as: reward_per_share += (new_fee * 1e12) / total_supply
    pub reward_per_share_x1e12: u128,
    /// Total token supply used for reward calculation.
    pub total_supply: u64,
}

const POOL_PREFIX: &[u8] = b"Dv";
const DEBT_PREFIX: &[u8] = b"Dd";

/// Dividend storage operations.
pub struct DividendStore<'a> {
    db: &'a PiChainDB,
}

impl<'a> DividendStore<'a> {
    pub fn new(db: &'a PiChainDB) -> Self {
        Self { db }
    }

    /// Get or create dividend pool for a token.
    pub fn get_pool(&self, mint_id: &MintId) -> Result<Option<DividendPool>, StorageError> {
        let mut key = Vec::with_capacity(2 + 32);
        key.extend_from_slice(POOL_PREFIX);
        key.extend_from_slice(&mint_id.0);

        match self.db.get_state(&key)? {
            Some(data) => {
                let pool = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?;
                Ok(Some(pool))
            }
            None => Ok(None),
        }
    }

    /// Save dividend pool state.
    pub fn put_pool(&self, pool: &DividendPool) -> Result<(), StorageError> {
        let mut key = Vec::with_capacity(2 + 32);
        key.extend_from_slice(POOL_PREFIX);
        key.extend_from_slice(&pool.mint_id.0);

        let data =
            serde_json::to_vec(pool).map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.put_state(&key, &data)?;
        Ok(())
    }

    /// Get reward debt for a specific holder.
    pub fn get_reward_debt(
        &self,
        mint_id: &MintId,
        holder: &Address,
    ) -> Result<u128, StorageError> {
        let mut key = Vec::with_capacity(2 + 32 + 20);
        key.extend_from_slice(DEBT_PREFIX);
        key.extend_from_slice(&mint_id.0);
        key.extend_from_slice(&holder.0);

        match self.db.get_state(&key)? {
            Some(data) if data.len() == 16 => {
                Ok(u128::from_le_bytes(data[..16].try_into().unwrap()))
            }
            _ => Ok(0),
        }
    }

    /// Set reward debt for a holder (called after claiming).
    pub fn set_reward_debt(
        &self,
        mint_id: &MintId,
        holder: &Address,
        debt: u128,
    ) -> Result<(), StorageError> {
        let mut key = Vec::with_capacity(2 + 32 + 20);
        key.extend_from_slice(DEBT_PREFIX);
        key.extend_from_slice(&mint_id.0);
        key.extend_from_slice(&holder.0);

        self.db.put_state(&key, &debt.to_le_bytes())?;
        Ok(())
    }

    /// Calculate claimable dividends for a holder.
    /// Returns PI amount in base units.
    pub fn claimable(
        &self,
        mint_id: &MintId,
        holder: &Address,
        holder_balance: u64,
    ) -> Result<u64, StorageError> {
        let pool = match self.get_pool(mint_id)? {
            Some(p) => p,
            None => return Ok(0),
        };

        let debt = self.get_reward_debt(mint_id, holder)?;
        let entitled = (holder_balance as u128)
            .checked_mul(pool.reward_per_share_x1e12)
            .unwrap_or(0)
            / 1_000_000_000_000;

        Ok(entitled.saturating_sub(debt) as u64)
    }
}
