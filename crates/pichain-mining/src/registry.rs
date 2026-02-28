//! PI Digit Registry — on-chain record of all computed PI digits.
//!
//! The registry tracks:
//! - Which digit ranges have been computed and verified
//! - Who computed them (miner address)
//! - The block height where they were committed
//! - The running frontier (highest consecutive verified position)
//!
//! This enables the "Computing the Infinite" mission by maintaining
//! a permanent record of humanity's progress computing PI digits.

use pichain_crypto::ed25519::Address;
use pichain_crypto::Hash;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tracing::{debug, info};

/// A committed range of verified PI digits.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DigitRange {
    /// Starting hex digit position.
    pub start: u64,
    /// Number of hex digits in this range.
    pub count: u32,
    /// Blake3 hash of the digit data.
    pub commitment: Hash,
    /// Address of the miner who computed these digits.
    pub miner: Address,
    /// Block height where this range was committed.
    pub committed_at_height: u64,
    /// Timestamp of commitment.
    pub committed_at_ms: u64,
}

/// Statistics about PI digit computation progress.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DigitStats {
    /// Total hex digits verified across all ranges.
    pub total_digits_verified: u64,
    /// Highest consecutive verified position (the "frontier").
    pub frontier_position: u64,
    /// Total number of verified ranges.
    pub total_ranges: u64,
    /// Unique miners who have contributed.
    pub unique_miners: u64,
    /// Total PI tokens distributed as mining rewards.
    pub total_rewards_distributed: u64,
}

/// The PI Digit Registry — manages on-chain records of computed PI digits.
#[derive(Clone)]
pub struct DigitRegistry {
    /// All verified digit ranges, indexed by start position.
    ranges: BTreeMap<u64, DigitRange>,
    /// Per-miner contribution tracking: address → total digits computed.
    miner_contributions: BTreeMap<Address, u64>,
    /// The frontier: highest position where all preceding digits are verified.
    frontier: u64,
    /// Total digits verified.
    total_verified: u64,
}

impl DigitRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            ranges: BTreeMap::new(),
            miner_contributions: BTreeMap::new(),
            frontier: 0,
            total_verified: 0,
        }
    }

    /// Register a new verified digit range.
    /// Returns true if the range was new (not overlapping with existing).
    pub fn register(
        &mut self,
        range: DigitRange,
    ) -> Result<bool, RegistryError> {
        let start = range.start;
        let end = match start.checked_add(range.count as u64) {
            Some(e) => e,
            None => return Err(RegistryError::InvalidRange("digit range overflows u64".to_string())),
        };

        // Check for overlap with existing ranges
        if self.has_overlap(start, end) {
            return Err(RegistryError::OverlappingRange { start, end });
        }

        // Record the range
        let miner = range.miner;
        let count = range.count as u64;

        self.ranges.insert(start, range);
        *self.miner_contributions.entry(miner).or_insert(0) =
            self.miner_contributions.get(&miner).copied().unwrap_or(0).saturating_add(count);
        self.total_verified = self.total_verified.saturating_add(count);

        // Update frontier
        self.update_frontier();

        info!(
            start,
            count,
            %miner,
            frontier = self.frontier,
            total = self.total_verified,
            "registered new PI digit range"
        );

        Ok(true)
    }

    /// Check if a range [start, end) overlaps with any existing verified range.
    /// R37-FIX: O(log n) using BTreeMap::range instead of O(n) scan.
    /// Only need to check the range whose start is immediately <= start (could extend past it)
    /// and ranges whose start is in [start, end) (overlap by definition if they exist).
    pub fn has_overlap(&self, start: u64, end: u64) -> bool {
        // Check the last range starting at or before `start` — it could extend past `start`.
        if let Some((&range_start, range)) = self.ranges.range(..=start).next_back() {
            let range_end = range_start + range.count as u64;
            if range_end > start {
                return true;
            }
        }
        // Check if any range starts within [start+1, end) — these overlap by definition.
        // (start itself was already covered by the check above)
        self.ranges.range((start.saturating_add(1))..end).next().is_some()
    }

    /// Update the frontier position.
    /// The frontier is the highest position where all digits from 0..frontier are verified.
    fn update_frontier(&mut self) {
        let mut pos = self.frontier;
        loop {
            if let Some(range) = self.ranges.get(&pos) {
                pos += range.count as u64;
            } else {
                break;
            }
        }
        if pos > self.frontier {
            debug!(old = self.frontier, new = pos, "frontier advanced");
            self.frontier = pos;
        }
    }

    /// Get the current frontier position.
    pub fn frontier(&self) -> u64 {
        self.frontier
    }

    /// Get the next uncomputed position (where a miner should start).
    pub fn next_uncomputed_position(&self) -> u64 {
        // Start from frontier and find first gap
        let mut pos = self.frontier;
        loop {
            if let Some(range) = self.ranges.get(&pos) {
                pos += range.count as u64;
            } else {
                return pos;
            }
        }
    }

    /// Get the total number of verified digits.
    pub fn total_verified(&self) -> u64 {
        self.total_verified
    }

    /// Get the contribution of a specific miner.
    pub fn miner_contribution(&self, miner: &Address) -> u64 {
        self.miner_contributions.get(miner).copied().unwrap_or(0)
    }

    /// Get the top miners by contribution.
    pub fn top_miners(&self, count: usize) -> Vec<(Address, u64)> {
        let mut sorted: Vec<(Address, u64)> = self
            .miner_contributions
            .iter()
            .map(|(&addr, &count)| (addr, count))
            .collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        sorted.truncate(count);
        sorted
    }

    /// Check if a specific position has been computed.
    pub fn is_computed(&self, position: u64) -> bool {
        for (&start, range) in self.ranges.range(..=position).rev() {
            let end = start + range.count as u64;
            if start <= position && position < end {
                return true;
            }
        }
        false
    }

    /// Check if an entire range [range_start, range_start + count) is fully covered
    /// by already-registered ranges. Used as a pre-check before expensive spot-checking
    /// to reject duplicate/overlapping submissions early.
    pub fn is_range_fully_computed(&self, range_start: u64, count: u32) -> bool {
        if count == 0 {
            return true;
        }
        let range_end = match range_start.checked_add(count as u64) {
            Some(e) => e,
            None => return false, // overflow — cannot be fully computed
        };

        // Walk through existing ranges that could cover [range_start, range_end)
        let mut pos = range_start;
        // Find the first registered range whose start <= pos
        for (&start, range) in self.ranges.range(..=pos).rev() {
            let end = start + range.count as u64;
            if start <= pos && pos < end {
                pos = end; // advance past this range
                break;
            }
        }

        // Continue scanning forward through ranges
        while pos < range_end {
            if let Some(range) = self.ranges.get(&pos) {
                pos += range.count as u64;
            } else {
                // Gap found — not fully computed
                return false;
            }
        }

        pos >= range_end
    }

    /// Get statistics about the registry.
    pub fn stats(&self) -> DigitStats {
        DigitStats {
            total_digits_verified: self.total_verified,
            frontier_position: self.frontier,
            total_ranges: self.ranges.len() as u64,
            unique_miners: self.miner_contributions.len() as u64,
            total_rewards_distributed: 0, // Set by caller
        }
    }

    /// Get all ranges for serialization/persistence.
    pub fn all_ranges(&self) -> Vec<&DigitRange> {
        self.ranges.values().collect()
    }
}

impl Default for DigitRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors from the digit registry.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("overlapping digit range: {start}..{end}")]
    OverlappingRange { start: u64, end: u64 },
    #[error("range too large: {size} digits (max {max})")]
    RangeTooLarge { size: u64, max: u64 },
    #[error("invalid range: {0}")]
    InvalidRange(String),
    #[error("verification failed: {0}")]
    VerificationFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_range(start: u64, count: u32, miner: u8) -> DigitRange {
        DigitRange {
            start,
            count,
            commitment: pichain_crypto::hash(&[miner]),
            miner: Address([miner; 20]),
            committed_at_height: 1,
            committed_at_ms: 1000,
        }
    }

    #[test]
    fn empty_registry() {
        let reg = DigitRegistry::new();
        assert_eq!(reg.frontier(), 0);
        assert_eq!(reg.total_verified(), 0);
        assert_eq!(reg.next_uncomputed_position(), 0);
    }

    #[test]
    fn register_first_range() {
        let mut reg = DigitRegistry::new();
        reg.register(make_range(0, 1000, 1)).unwrap();

        assert_eq!(reg.frontier(), 1000);
        assert_eq!(reg.total_verified(), 1000);
        assert_eq!(reg.next_uncomputed_position(), 1000);
    }

    #[test]
    fn frontier_advances_with_contiguous_ranges() {
        let mut reg = DigitRegistry::new();
        reg.register(make_range(0, 1000, 1)).unwrap();
        reg.register(make_range(1000, 1000, 2)).unwrap();
        reg.register(make_range(2000, 500, 3)).unwrap();

        assert_eq!(reg.frontier(), 2500);
        assert_eq!(reg.total_verified(), 2500);
    }

    #[test]
    fn gap_stops_frontier() {
        let mut reg = DigitRegistry::new();
        reg.register(make_range(0, 1000, 1)).unwrap();
        // Gap at 1000-2000
        reg.register(make_range(2000, 1000, 2)).unwrap();

        assert_eq!(reg.frontier(), 1000); // Stops at gap
        assert_eq!(reg.total_verified(), 2000);
        assert_eq!(reg.next_uncomputed_position(), 1000);
    }

    #[test]
    fn filling_gap_advances_frontier() {
        let mut reg = DigitRegistry::new();
        reg.register(make_range(0, 1000, 1)).unwrap();
        reg.register(make_range(2000, 1000, 2)).unwrap();
        assert_eq!(reg.frontier(), 1000);

        // Fill the gap
        reg.register(make_range(1000, 1000, 3)).unwrap();
        assert_eq!(reg.frontier(), 3000);
    }

    #[test]
    fn overlapping_range_rejected() {
        let mut reg = DigitRegistry::new();
        reg.register(make_range(0, 1000, 1)).unwrap();

        // Overlapping range
        let result = reg.register(make_range(500, 1000, 2));
        assert!(result.is_err());
    }

    #[test]
    fn is_computed() {
        let mut reg = DigitRegistry::new();
        reg.register(make_range(0, 100, 1)).unwrap();

        assert!(reg.is_computed(0));
        assert!(reg.is_computed(50));
        assert!(reg.is_computed(99));
        assert!(!reg.is_computed(100));
        assert!(!reg.is_computed(200));
    }

    #[test]
    fn miner_contributions() {
        let mut reg = DigitRegistry::new();
        reg.register(make_range(0, 1000, 1)).unwrap();
        reg.register(make_range(1000, 2000, 1)).unwrap();
        reg.register(make_range(3000, 500, 2)).unwrap();

        assert_eq!(reg.miner_contribution(&Address([1; 20])), 3000);
        assert_eq!(reg.miner_contribution(&Address([2; 20])), 500);
    }

    #[test]
    fn top_miners() {
        let mut reg = DigitRegistry::new();
        reg.register(make_range(0, 3000, 1)).unwrap();
        reg.register(make_range(3000, 1000, 2)).unwrap();
        reg.register(make_range(4000, 2000, 3)).unwrap();

        let top = reg.top_miners(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, Address([1; 20])); // Miner 1: 3000
        assert_eq!(top[1].0, Address([3; 20])); // Miner 3: 2000
    }

    #[test]
    fn stats() {
        let mut reg = DigitRegistry::new();
        reg.register(make_range(0, 1000, 1)).unwrap();
        reg.register(make_range(1000, 500, 2)).unwrap();

        let stats = reg.stats();
        assert_eq!(stats.total_digits_verified, 1500);
        assert_eq!(stats.frontier_position, 1500);
        assert_eq!(stats.total_ranges, 2);
        assert_eq!(stats.unique_miners, 2);
    }
}
