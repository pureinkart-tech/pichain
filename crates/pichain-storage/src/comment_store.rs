//! Comment store — persists per-token launch comments in RocksDB.
//!
//! Uses the `state` column family with key prefix:
//! - `C:` + mint_id(32) + inverted_timestamp(8) + comment_idx(2) → Comment JSON
//! - `Cc:` + mint_id(32) → comment count (u64 LE)

use pichain_crypto::ed25519::Address;
use pichain_types::token::MintId;
use serde::{Deserialize, Serialize};

use crate::db::PiChainDB;
use crate::StorageError;

/// A comment on a token launch.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comment {
    /// Comment author address.
    pub author: Address,
    /// Comment text (max 500 chars).
    pub text: String,
    /// Timestamp (milliseconds since epoch).
    pub timestamp_ms: u64,
    /// Token mint this comment is on.
    pub mint_id: MintId,
}

const COMMENT_PREFIX: u8 = b'C';
const COMMENT_COUNT_PREFIX: &[u8] = b"Cc";

/// Comment storage operations.
pub struct CommentStore<'a> {
    db: &'a PiChainDB,
}

impl<'a> CommentStore<'a> {
    pub fn new(db: &'a PiChainDB) -> Self {
        Self { db }
    }

    /// Add a comment for a token launch.
    pub fn put_comment(&self, comment: &Comment) -> Result<(), StorageError> {
        // Get and increment the comment count for this mint
        let count = self.get_comment_count(&comment.mint_id)?;

        // Store the comment with inverted timestamp for newest-first ordering
        let inverted_ts = u64::MAX - comment.timestamp_ms;
        let idx = (count % 65536) as u16;

        let mut key = Vec::with_capacity(1 + 32 + 8 + 2);
        key.push(COMMENT_PREFIX);
        key.extend_from_slice(&comment.mint_id.0);
        key.extend_from_slice(&inverted_ts.to_be_bytes());
        key.extend_from_slice(&idx.to_be_bytes());

        let data =
            serde_json::to_vec(comment).map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.db.put_state(&key, &data)?;

        // Update count
        let mut count_key = Vec::with_capacity(2 + 32);
        count_key.extend_from_slice(COMMENT_COUNT_PREFIX);
        count_key.extend_from_slice(&comment.mint_id.0);
        self.db.put_state(&count_key, &(count + 1).to_le_bytes())?;

        Ok(())
    }

    /// Get comment count for a mint.
    pub fn get_comment_count(&self, mint_id: &MintId) -> Result<u64, StorageError> {
        let mut key = Vec::with_capacity(2 + 32);
        key.extend_from_slice(COMMENT_COUNT_PREFIX);
        key.extend_from_slice(&mint_id.0);

        match self.db.get_state(&key)? {
            Some(data) if data.len() == 8 => Ok(u64::from_le_bytes(data[..8].try_into().unwrap())),
            _ => Ok(0),
        }
    }

    /// Get comments for a token launch (newest first).
    pub fn get_comments(
        &self,
        mint_id: &MintId,
        limit: usize,
    ) -> Result<Vec<Comment>, StorageError> {
        let mut prefix = Vec::with_capacity(1 + 32);
        prefix.push(COMMENT_PREFIX);
        prefix.extend_from_slice(&mint_id.0);

        let entries = self.db.scan_state_prefix(&prefix)?;
        let mut comments = Vec::new();
        for (_key, value) in entries {
            if comments.len() >= limit {
                break;
            }
            match serde_json::from_slice::<Comment>(&value) {
                Ok(c) => comments.push(c),
                Err(_) => continue, // Skip corrupted entries
            }
        }
        Ok(comments)
    }
}
