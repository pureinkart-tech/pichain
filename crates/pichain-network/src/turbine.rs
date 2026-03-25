//! Turbine-style erasure-coded block propagation.
//!
//! Blocks are split into shreds, erasure-coded (32 data + 32 recovery = 64 shreds),
//! and distributed in a logarithmic fanout tree. Any 32 of 64 shreds can
//! reconstruct the full block, tolerating Byzantine nodes withholding shreds.
//!
//! Uses Reed-Solomon erasure coding (via `reed-solomon-erasure` crate) for
//! production-grade block recovery from any sufficient subset of shreds.

use reed_solomon_erasure::galois_8::ReedSolomon;
use tracing::warn;

/// Configuration for Turbine-style block propagation.
pub struct TurbineConfig {
    /// Number of data shreds per block.
    pub data_shreds: u32,
    /// Number of recovery/parity shreds.
    pub recovery_shreds: u32,
    /// Fanout factor (how many peers each node forwards to).
    pub fanout: u32,
}

impl Default for TurbineConfig {
    fn default() -> Self {
        Self {
            data_shreds: 32,
            recovery_shreds: 32,
            fanout: 8,
        }
    }
}

/// A single shred (piece of an erasure-coded block).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Shred {
    /// Block height this shred belongs to.
    pub block_height: u64,
    /// Index within the shred set (0..data_shreds+recovery_shreds).
    pub index: u32,
    /// Total number of shreds (data + recovery).
    pub total: u32,
    /// Number of data shreds (needed for RS config verification).
    pub data_shred_count: u32,
    /// Is this a data shred or recovery shred?
    pub is_data: bool,
    /// The shred payload.
    pub data: Vec<u8>,
    /// Original block size (for trimming on reconstruction).
    pub original_size: u64,
    /// BLAKE3 hash of the complete shred set (all data before RS encoding).
    /// Used to verify that all shreds belong to the same block.
    pub block_hash: [u8; 32],
}

/// Maximum size per individual shred (1 MB).
/// A full block is split into 32 data shreds, so max block = 32 MB.
pub const MAX_SHRED_SIZE: usize = 1_048_576;

/// Maximum allowed original block size (16 MB).
///
/// SECURITY (Fix 195): The `original_size` field in a `Shred` is
/// attacker-controlled. Without bounds checking, a malicious value (e.g.,
/// u64::MAX) could cause OOM during reassembly buffer allocation or
/// corrupt the truncation logic. This cap matches the expected maximum
/// block size (32 data shreds * ~512KB typical = ~16MB).
pub const MAX_BLOCK_SIZE: usize = 16 * 1024 * 1024;

/// Split a block into shreds for Turbine propagation.
///
/// Uses Reed-Solomon erasure coding: `data_shreds` data shreds + `recovery_shreds`
/// parity shreds. Any `data_shreds` out of the total can reconstruct the block.
pub fn shred_block(block_data: &[u8], config: &TurbineConfig, block_height: u64) -> Vec<Shred> {
    let data_count = config.data_shreds as usize;
    let recovery_count = config.recovery_shreds as usize;
    let total = data_count + recovery_count;
    let original_size = block_data.len() as u64;

    // Compute a BLAKE3 hash binding all shreds to this block
    let block_hash = pichain_crypto::hash_concat(&[block_data, &block_height.to_le_bytes()]);

    // Calculate chunk size (padded to equal sizes)
    let chunk_size = block_data.len().div_ceil(data_count);
    let chunk_size = chunk_size.max(1); // at least 1 byte per shard

    // Create data shards (padded with zeros if needed)
    let mut all_shards: Vec<Vec<u8>> = Vec::with_capacity(total);
    for i in 0..data_count {
        let start = i * chunk_size;
        let end = (start + chunk_size).min(block_data.len());
        let mut shard = vec![0u8; chunk_size];
        if start < block_data.len() {
            let copy_len = end - start;
            shard[..copy_len].copy_from_slice(&block_data[start..end]);
        }
        all_shards.push(shard);
    }

    // Add empty parity shards
    for _ in 0..recovery_count {
        all_shards.push(vec![0u8; chunk_size]);
    }

    // Create Reed-Solomon encoder and generate parity shards
    let rs = match ReedSolomon::new(data_count, recovery_count) {
        Ok(rs) => rs,
        Err(e) => {
            warn!(data_count, recovery_count, error = %e, "invalid RS parameters, skipping shred creation");
            return vec![];
        }
    };
    if let Err(e) = rs.encode(&mut all_shards) {
        warn!(error = %e, "RS encoding failed, skipping shred creation");
        return vec![];
    }

    // Build shred structs
    let mut shreds = Vec::with_capacity(total);
    for (i, shard) in all_shards.into_iter().enumerate() {
        shreds.push(Shred {
            block_height,
            index: i as u32,
            total: total as u32,
            data_shred_count: config.data_shreds,
            is_data: i < data_count,
            data: shard,
            original_size,
            block_hash: *block_hash.as_bytes(),
        });
    }

    shreds
}

/// Reconstruct a block from any sufficient subset of shreds.
///
/// Requires at least `data_shreds` total shreds (data or recovery) to reconstruct.
/// Uses Reed-Solomon decoding for recovery from mixed data/parity shreds.
/// Validates that all shreds have consistent metadata (total, block_hash, block_height).
pub fn reconstruct_block(shreds: &[Shred], config: &TurbineConfig) -> Option<Vec<u8>> {
    let data_count = config.data_shreds as usize;
    let recovery_count = config.recovery_shreds as usize;
    let total = data_count + recovery_count;

    if shreds.is_empty() {
        return None;
    }

    let first = &shreds[0];
    let original_size = first.original_size as usize;
    let chunk_size = first.data.len();
    let expected_block_hash = first.block_hash;
    let expected_height = first.block_height;
    let expected_total = first.total;

    // Validate shred size to prevent OOM from malicious oversized shreds
    if chunk_size > MAX_SHRED_SIZE || chunk_size == 0 {
        return None;
    }

    // SECURITY (Fix 195): Validate original_size to prevent OOM.
    // An attacker could set original_size to u64::MAX, causing a multi-GB
    // allocation or incorrect truncation. Cap at MAX_BLOCK_SIZE.
    if original_size > MAX_BLOCK_SIZE || original_size == 0 {
        return None;
    }

    // Also verify original_size is consistent with the shred geometry:
    // it cannot exceed data_count * chunk_size (the maximum possible payload)
    if original_size > data_count * chunk_size {
        return None;
    }

    // Validate total matches config
    if expected_total != total as u32 {
        return None;
    }

    // Build shard array tracking which are present
    let mut shard_opts: Vec<Option<Vec<u8>>> = vec![None; total];
    let mut available = 0;

    for shred in shreds {
        // Validate consistency: all shreds must agree on metadata
        if shred.total != expected_total
            || shred.block_hash != expected_block_hash
            || shred.block_height != expected_height
            || shred.data.len() != chunk_size
        {
            continue; // Skip inconsistent shreds
        }

        let idx = shred.index as usize;
        if idx < total && shard_opts[idx].is_none() {
            shard_opts[idx] = Some(shred.data.clone());
            available += 1;
        }
    }

    // Need at least data_count shards to reconstruct
    if available < data_count {
        return None;
    }

    // Check if we have all data shreds (fast path — no RS decode needed)
    let all_data_present = (0..data_count).all(|i| shard_opts[i].is_some());

    if all_data_present {
        let mut block = Vec::with_capacity(data_count * chunk_size);
        for shard in shard_opts.iter().take(data_count) {
            block.extend_from_slice(shard.as_ref().unwrap());
        }
        block.truncate(original_size);

        // Verify block hash matches
        let computed_hash = pichain_crypto::hash_concat(&[&block, &expected_height.to_le_bytes()]);
        if *computed_hash.as_bytes() != expected_block_hash {
            return None;
        }

        return Some(block);
    }

    // Need Reed-Solomon reconstruction
    let rs = ReedSolomon::new(data_count, recovery_count).ok()?;

    // Reconstruct missing shards using Option<Vec<u8>> interface
    rs.reconstruct(&mut shard_opts).ok()?;

    // Concatenate data shreds
    let mut block = Vec::with_capacity(data_count * chunk_size);
    for shard in shard_opts.iter().take(data_count) {
        if let Some(ref data) = shard {
            block.extend_from_slice(data);
        } else {
            return None; // Should not happen after successful reconstruct
        }
    }
    block.truncate(original_size);

    // Verify reconstructed block hash matches
    let computed_hash = pichain_crypto::hash_concat(&[&block, &expected_height.to_le_bytes()]);
    if *computed_hash.as_bytes() != expected_block_hash {
        return None;
    }

    Some(block)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shred_and_reconstruct_all_data() {
        let block_data = b"PIChain genesis block data with transactions and state changes";
        let config = TurbineConfig {
            data_shreds: 4,
            recovery_shreds: 4,
            fanout: 2,
        };

        let shreds = shred_block(block_data, &config, 1);
        assert_eq!(shreds.len(), 8); // 4 data + 4 recovery
        assert_eq!(shreds[0].block_height, 1);
        assert_eq!(shreds[0].data_shred_count, 4);
        assert!(shreds.iter().all(|s| s.block_hash == shreds[0].block_hash));

        let reconstructed = reconstruct_block(&shreds, &config).unwrap();
        assert_eq!(&reconstructed[..], &block_data[..]);
    }

    #[test]
    fn reconstruct_from_data_shreds_only() {
        let block_data = b"PIChain block data test for Reed-Solomon erasure coding";
        let config = TurbineConfig {
            data_shreds: 4,
            recovery_shreds: 4,
            fanout: 2,
        };

        let shreds = shred_block(block_data, &config, 42);
        let data_only: Vec<Shred> = shreds.into_iter().filter(|s| s.is_data).collect();
        assert_eq!(data_only.len(), 4);

        let reconstructed = reconstruct_block(&data_only, &config).unwrap();
        assert_eq!(&reconstructed[..], &block_data[..]);
    }

    #[test]
    fn reconstruct_with_missing_data_shreds() {
        let block_data = b"Testing Reed-Solomon recovery with missing data shreds in Turbine";
        let config = TurbineConfig {
            data_shreds: 4,
            recovery_shreds: 4,
            fanout: 2,
        };

        let shreds = shred_block(block_data, &config, 100);

        // Drop 2 data shreds (indices 0 and 2), keep 2 data + 4 recovery = 6 shreds
        let partial: Vec<Shred> = shreds
            .into_iter()
            .filter(|s| s.index != 0 && s.index != 2)
            .collect();
        assert_eq!(partial.len(), 6);

        let reconstructed = reconstruct_block(&partial, &config).unwrap();
        assert_eq!(&reconstructed[..], &block_data[..]);
    }

    #[test]
    fn reconstruct_with_mixed_missing() {
        let block_data = b"Reed-Solomon allows reconstruction from any k-of-n shreds";
        let config = TurbineConfig {
            data_shreds: 4,
            recovery_shreds: 4,
            fanout: 2,
        };

        let shreds = shred_block(block_data, &config, 200);

        // Keep exactly 4 shreds: 2 data + 2 recovery (minimum for reconstruction)
        let minimal: Vec<Shred> = shreds
            .into_iter()
            .filter(|s| s.index == 1 || s.index == 3 || s.index == 4 || s.index == 6)
            .collect();
        assert_eq!(minimal.len(), 4);

        let reconstructed = reconstruct_block(&minimal, &config).unwrap();
        assert_eq!(&reconstructed[..], &block_data[..]);
    }

    #[test]
    fn insufficient_shreds_fails() {
        let block_data = b"test block";
        let config = TurbineConfig {
            data_shreds: 4,
            recovery_shreds: 4,
            fanout: 2,
        };

        let shreds = shred_block(block_data, &config, 1);
        // Only 3 shreds (need 4)
        let partial: Vec<Shred> = shreds.into_iter().take(3).collect();
        assert!(reconstruct_block(&partial, &config).is_none());
    }

    #[test]
    fn large_block_roundtrip() {
        // Test with a realistic block size (10KB)
        let block_data: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
        let config = TurbineConfig::default(); // 32 data + 32 recovery

        let shreds = shred_block(&block_data, &config, 500);
        assert_eq!(shreds.len(), 64);

        // Reconstruct from all shreds
        let reconstructed = reconstruct_block(&shreds, &config).unwrap();
        assert_eq!(reconstructed, block_data);

        // Reconstruct after dropping half the shreds (every other one)
        let half: Vec<Shred> = shreds.into_iter().step_by(2).collect();
        assert_eq!(half.len(), 32);
        let reconstructed = reconstruct_block(&half, &config).unwrap();
        assert_eq!(reconstructed, block_data);
    }

    #[test]
    fn inconsistent_shreds_rejected() {
        let block_data = b"test block for inconsistency check";
        let config = TurbineConfig {
            data_shreds: 4,
            recovery_shreds: 4,
            fanout: 2,
        };

        let mut shreds = shred_block(block_data, &config, 1);
        // Tamper with one shred's block_hash
        shreds[2].block_hash = [0xff; 32];

        // Should still work with 7 valid shreds (only need 4)
        let reconstructed = reconstruct_block(&shreds, &config).unwrap();
        assert_eq!(&reconstructed[..], &block_data[..]);
    }

    #[test]
    fn original_size_overflow_rejected() {
        // Fix 195: Attacker-controlled original_size could cause OOM
        let block_data = b"test block for OOM prevention";
        let config = TurbineConfig {
            data_shreds: 4,
            recovery_shreds: 4,
            fanout: 2,
        };

        let mut shreds = shred_block(block_data, &config, 1);

        // Set original_size to a huge value (would OOM without the check)
        for shred in &mut shreds {
            shred.original_size = u64::MAX;
        }
        assert!(
            reconstruct_block(&shreds, &config).is_none(),
            "should reject shreds with original_size > MAX_BLOCK_SIZE"
        );
    }

    #[test]
    fn original_size_exceeds_max_block_size_rejected() {
        let block_data = b"test block";
        let config = TurbineConfig {
            data_shreds: 4,
            recovery_shreds: 4,
            fanout: 2,
        };

        let mut shreds = shred_block(block_data, &config, 1);

        // Set original_size just above MAX_BLOCK_SIZE
        for shred in &mut shreds {
            shred.original_size = (MAX_BLOCK_SIZE as u64) + 1;
        }
        assert!(
            reconstruct_block(&shreds, &config).is_none(),
            "should reject shreds with original_size > MAX_BLOCK_SIZE"
        );
    }

    #[test]
    fn original_size_zero_rejected() {
        let block_data = b"test block";
        let config = TurbineConfig {
            data_shreds: 4,
            recovery_shreds: 4,
            fanout: 2,
        };

        let mut shreds = shred_block(block_data, &config, 1);
        for shred in &mut shreds {
            shred.original_size = 0;
        }
        assert!(
            reconstruct_block(&shreds, &config).is_none(),
            "should reject shreds with zero original_size"
        );
    }

    #[test]
    fn original_size_exceeds_geometry_rejected() {
        let block_data = b"small block data";
        let config = TurbineConfig {
            data_shreds: 4,
            recovery_shreds: 4,
            fanout: 2,
        };

        let mut shreds = shred_block(block_data, &config, 1);
        let chunk_size = shreds[0].data.len();
        // Set original_size to more than data_count * chunk_size
        let max_payload = 4 * chunk_size;
        for shred in &mut shreds {
            shred.original_size = (max_payload + 1) as u64;
        }
        assert!(
            reconstruct_block(&shreds, &config).is_none(),
            "should reject shreds where original_size > data_count * chunk_size"
        );
    }

    #[test]
    fn wrong_total_rejected() {
        let block_data = b"test block";
        let config = TurbineConfig {
            data_shreds: 4,
            recovery_shreds: 4,
            fanout: 2,
        };
        let shreds = shred_block(block_data, &config, 1);

        // Try to reconstruct with wrong config (mismatched total)
        let wrong_config = TurbineConfig {
            data_shreds: 8,
            recovery_shreds: 8,
            fanout: 2,
        };
        assert!(reconstruct_block(&shreds, &wrong_config).is_none());
    }
}
