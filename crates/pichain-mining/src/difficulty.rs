//! Mining difficulty adjustment using a hybrid BBP-PoW model.
//!
//! Miners must:
//! 1. Compute PI digits via BBP (useful work)
//! 2. Find a nonce where blake3(digits || nonce || anchor_block_hash) < difficulty_target
//!
//! The difficulty target adjusts dynamically to maintain a target proof rate,
//! using an exponential moving average (ASERT-style) similar to Bitcoin Cash.

use serde::{Deserialize, Serialize};

/// Mining difficulty parameters.
pub const TARGET_PROOF_INTERVAL_MS: u64 = 10_000; // 10 seconds between proofs
pub const DIFFICULTY_WINDOW: usize = 144; // ~24 minutes of proofs
/// Maximum difficulty adjustment factor as a numerator over 1000 denominator.
/// 2000/1000 = 2x max change per adjustment.
pub const MAX_DIFFICULTY_ADJUSTMENT_NUM: u64 = 2000;
pub const DIFFICULTY_ADJUSTMENT_DENOM: u64 = 1000;
/// Kept for backward compatibility in non-consensus code:
pub const MAX_DIFFICULTY_ADJUSTMENT: f64 = 2.0; // Max 2x change per adjustment
pub const ADJUSTMENT_INTERVAL: usize = 10; // Adjust every 10 proofs
pub const MIN_DIFFICULTY_BITS: u32 = 8; // Minimum difficulty (easiest)
pub const MAX_DIFFICULTY_BITS: u32 = 240; // Maximum difficulty (hardest)

/// Initial difficulty target — easy enough for devnet solo mining.
/// Leading zeros required: 1 byte (8 bits) = ~256 nonce attempts on average.
pub const INITIAL_DIFFICULTY: [u8; 32] = [
    0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0xFF, 0xFF,
];

/// Frontier scale divisor for PoW scaling: frontier / 1000 is used for log2.
pub const FRONTIER_SCALE_DIVISOR: u64 = 1000;

/// Moore's Law interval: PoW gains +1 bit every this many years.
pub const MOORE_LAW_INTERVAL_YEARS: u32 = 2;

/// Compute the minimum PoW difficulty bits based on frontier position and chain age.
///
/// Formula: `8 + floor(log2(frontier / 1000 + 1)) + floor((year - 1) / 2)`
///
/// - `frontier_component` grows as PI computation goes deeper (~+3 bits per 10x frontier)
/// - `moore_component` grows with chain age (+1 bit every 2 years, tracking Moore's Law)
/// - Combined with BBP's natural O(position) per-digit cost, mining becomes exponentially harder
///
/// Uses integer-only arithmetic (no floating point) for consensus safety.
pub fn frontier_pow_bits(frontier: u64, year: u32) -> u32 {
    let frontier_component = if frontier <= FRONTIER_SCALE_DIVISOR {
        0u32
    } else {
        // Integer log2: number of bits to represent (frontier / 1000)
        let scaled = frontier / FRONTIER_SCALE_DIVISOR;
        // 64 - leading_zeros gives the position of the highest set bit + 1;
        // subtract 1 to get floor(log2(scaled))
        (u64::BITS - scaled.leading_zeros()).saturating_sub(1)
    };

    let moore_component = year.saturating_sub(1) / MOORE_LAW_INTERVAL_YEARS;

    let total = MIN_DIFFICULTY_BITS + frontier_component + moore_component;
    total.clamp(MIN_DIFFICULTY_BITS, MAX_DIFFICULTY_BITS)
}

/// Convert a difficulty-bits value to a 256-bit target.
///
/// The target is a 256-bit big-endian number where `bits` leading zero bits
/// are required, and all remaining bits are set to 1.
/// E.g. bits=8 → 0x00FFFFFF...FF, bits=16 → 0x0000FFFF...FF
pub fn target_from_bits(bits: u32) -> [u8; 32] {
    if bits >= 256 {
        // Impossibly hard — require all zeros
        let mut t = [0u8; 32];
        t[31] = 1; // never all-zero
        return t;
    }

    let mut target = [0xFFu8; 32];
    let full_zero_bytes = (bits / 8) as usize;
    let remaining_bits = bits % 8;

    for byte in target.iter_mut().take(full_zero_bytes) {
        *byte = 0x00;
    }
    if full_zero_bytes < 32 && remaining_bits > 0 {
        // Mask off the top `remaining_bits` bits of this byte
        target[full_zero_bytes] = 0xFF >> remaining_bits;
    }
    target
}

/// Compute the frontier-scaled difficulty target for PoW verification.
///
/// Returns a 256-bit target that gets harder (smaller) as frontier advances
/// and the chain ages. The DifficultyAdjuster's rate-based target acts as a
/// second layer — the effective target is the HARDER (smaller) of the two.
pub fn frontier_difficulty_target(frontier: u64, year: u32) -> [u8; 32] {
    let bits = frontier_pow_bits(frontier, year);
    target_from_bits(bits)
}

/// Difficulty adjuster — maintains target proof rate via rolling window.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DifficultyAdjuster {
    /// Current difficulty target (hash must be <= this value).
    pub current_target: [u8; 32],
    /// Timestamps of recent proof submissions (milliseconds since epoch).
    pub recent_proof_times: Vec<u64>,
    /// Target interval between proofs in milliseconds.
    pub target_interval_ms: u64,
    /// Rolling window size.
    pub window_size: usize,
    /// Number of proofs since last adjustment.
    #[serde(default)]
    pub proofs_since_adjustment: usize,
}

impl DifficultyAdjuster {
    pub fn new() -> Self {
        Self {
            current_target: INITIAL_DIFFICULTY,
            recent_proof_times: Vec::new(),
            target_interval_ms: TARGET_PROOF_INTERVAL_MS,
            window_size: DIFFICULTY_WINDOW,
            proofs_since_adjustment: 0,
        }
    }

    /// Record a proof submission timestamp and adjust difficulty periodically.
    pub fn record_proof(&mut self, timestamp_ms: u64) {
        self.recent_proof_times.push(timestamp_ms);

        // Keep only the window
        if self.recent_proof_times.len() > self.window_size {
            let excess = self.recent_proof_times.len() - self.window_size;
            self.recent_proof_times.drain(..excess);
        }

        self.proofs_since_adjustment = self.proofs_since_adjustment.saturating_add(1);

        // Only adjust every ADJUSTMENT_INTERVAL proofs to prevent runaway
        if self.proofs_since_adjustment >= ADJUSTMENT_INTERVAL
            && self.recent_proof_times.len() >= ADJUSTMENT_INTERVAL
        {
            self.adjust();
            self.proofs_since_adjustment = 0;
        }
    }

    /// Adjust the difficulty target based on recent proof times.
    ///
    /// SECURITY: Uses integer-only arithmetic to guarantee deterministic results
    /// across all platforms (ARM, x86, RISC-V). Floating-point operations can
    /// produce different results due to FPU rounding mode differences, x87 vs SSE
    /// precision, and fused multiply-add availability, which would cause consensus
    /// divergence between nodes on different architectures.
    fn adjust(&mut self) {
        let times = &self.recent_proof_times;
        if times.len() < 2 {
            return;
        }

        let first = times[0];
        let last = match times.last() {
            Some(v) => *v,
            None => return,
        };
        let elapsed = last.saturating_sub(first);
        let intervals = (times.len() - 1) as u64;

        if intervals == 0 || elapsed == 0 {
            return;
        }

        let actual_interval = elapsed / intervals;

        // Integer ratio: numerator / denominator = actual_interval / target_interval
        // Clamp to [DENOM/MAX_ADJ_NUM, MAX_ADJ_NUM/DENOM] i.e. [1/2, 2/1]
        //
        // We express the ratio as (numerator, denominator) = (actual_interval, target_interval)
        // clamped to [500/1000, 2000/1000].
        let num = actual_interval.min(
            self.target_interval_ms.saturating_mul(MAX_DIFFICULTY_ADJUSTMENT_NUM)
                / DIFFICULTY_ADJUSTMENT_DENOM
        ).max(
            self.target_interval_ms.saturating_mul(DIFFICULTY_ADJUSTMENT_DENOM)
                / MAX_DIFFICULTY_ADJUSTMENT_NUM
        );
        let den = self.target_interval_ms;

        if den == 0 {
            return;
        }

        self.current_target = multiply_target_int(&self.current_target, num, den);
    }

    /// Check if a hash meets the current difficulty target.
    pub fn meets_difficulty(&self, hash: &[u8; 32]) -> bool {
        hash <= &self.current_target
    }

    /// Get the current difficulty as approximate number of leading zero bits.
    pub fn difficulty_bits(&self) -> u32 {
        let mut bits = 0u32;
        for &byte in &self.current_target {
            if byte == 0 {
                bits += 8;
            } else {
                bits += byte.leading_zeros();
                break;
            }
        }
        bits
    }

    /// Get the estimated hash attempts needed to find a valid nonce.
    pub fn estimated_attempts(&self) -> u64 {
        let bits = self.difficulty_bits();
        if bits >= 64 {
            return u64::MAX;
        }
        1u64 << bits
    }
}

impl Default for DifficultyAdjuster {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns the harder (smaller) of two 256-bit difficulty targets.
/// Used to combine the frontier-scaled target with the rate-based target
/// so that the effective difficulty is always at least as hard as either layer.
pub fn harder_target(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    if a <= b { *a } else { *b }
}

/// Check if a mining proof meets the difficulty requirement.
/// Computes blake3(digits || nonce_bytes || anchor_block_hash || miner) and checks against target.
/// The miner address is included in the hash to bind the PoW to a specific miner,
/// preventing proof-of-work theft where one miner's nonce is reused by another.
pub fn check_proof_difficulty(
    digits: &[u8],
    nonce: u64,
    anchor_block_hash: &[u8; 32],
    target: &[u8; 32],
    miner: &[u8; 20],
) -> bool {
    let hash = pichain_crypto::hash_concat(&[digits, &nonce.to_le_bytes(), anchor_block_hash, miner]);
    hash.as_bytes() <= target
}

/// Find a nonce that satisfies the difficulty target.
/// Returns None if max_attempts is exceeded.
pub fn find_nonce(
    digits: &[u8],
    anchor_block_hash: &[u8; 32],
    target: &[u8; 32],
    max_attempts: u64,
    miner: &[u8; 20],
) -> Option<u64> {
    (0..max_attempts).find(|&nonce| check_proof_difficulty(digits, nonce, anchor_block_hash, target, miner))
}

/// Find a nonce in parallel using rayon.
pub fn find_nonce_parallel(
    digits: &[u8],
    anchor_block_hash: &[u8; 32],
    target: &[u8; 32],
    max_attempts: u64,
    miner: &[u8; 20],
) -> Option<u64> {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    let found = AtomicBool::new(false);
    let result_nonce = AtomicU64::new(0);

    // Search in chunks to allow early termination
    let chunk_size = 10_000u64;
    let chunks = max_attempts.div_ceil(chunk_size);

    (0..chunks).into_par_iter().for_each(|chunk_idx| {
        if found.load(Ordering::Acquire) {
            return;
        }
        let start = chunk_idx * chunk_size;
        let end = (start + chunk_size).min(max_attempts);
        for nonce in start..end {
            if found.load(Ordering::Acquire) {
                return;
            }
            if check_proof_difficulty(digits, nonce, anchor_block_hash, target, miner) {
                // Use swap to ensure only the first finder sets the flag;
                // store nonce before setting found to prevent reading stale 0
                if !found.swap(true, Ordering::SeqCst) {
                    result_nonce.store(nonce, Ordering::SeqCst);
                }
                return;
            }
        }
    });

    if found.load(Ordering::SeqCst) {
        Some(result_nonce.load(Ordering::SeqCst))
    } else {
        None
    }
}

/// Multiply a 256-bit target by a floating-point factor (DEPRECATED for consensus use).
/// Kept for backward compatibility in non-consensus code.
/// For consensus-critical paths, use `multiply_target_int` instead.
#[allow(dead_code)]
fn multiply_target(target: &[u8; 32], factor: f64) -> [u8; 32] {
    // No-op for factor ~= 1.0
    if (factor - 1.0).abs() < 1e-12 {
        return *target;
    }

    // Find first non-zero byte
    let first_nz = match target.iter().position(|&b| b != 0) {
        Some(p) => p,
        None => {
            let mut r = [0u8; 32];
            r[31] = 1;
            return r;
        }
    };

    // Extract 6 significant bytes into the low end of a u64 buffer
    let extract_len = 6.min(32 - first_nz);
    let mut buf = [0u8; 8];
    buf[8 - extract_len..].copy_from_slice(&target[first_nz..first_nz + extract_len]);
    let value = u64::from_be_bytes(buf);

    // Scale (48-bit value * f64 is exact for factors in [0.25, 4.0])
    let scaled_f = value as f64 * factor;
    if !scaled_f.is_finite() || scaled_f < 1.0 {
        let mut r = [0u8; 32];
        r[31] = 1;
        return r;
    }
    // Clamp to u64::MAX to prevent undefined behavior from f64 → u64 cast
    let scaled = if scaled_f > u64::MAX as f64 { u64::MAX } else { scaled_f as u64 };

    // Determine where the significant bytes moved
    let scaled_be = scaled.to_be_bytes();
    let scaled_first = scaled_be.iter().position(|&b| b != 0).unwrap_or(7);
    let orig_first_in_buf = 8 - extract_len;

    // Position shift: negative = grew (easier), positive = shrunk (harder)
    let shift = scaled_first as i32 - orig_first_in_buf as i32;
    let new_pos = (first_nz as i32 + shift).clamp(0, 31) as usize;

    // Write the significant bytes of the scaled value
    let mut result = [0u8; 32];
    let sig_len = (8 - scaled_first).min(32 - new_pos);
    result[new_pos..new_pos + sig_len]
        .copy_from_slice(&scaled_be[scaled_first..scaled_first + sig_len]);

    // Clamp: never allow all-zero target
    if result == [0u8; 32] {
        result[31] = 1;
    }

    result
}

/// Multiply a 256-bit target by an integer ratio (numerator / denominator).
/// Deterministic across all platforms — no floating-point operations.
///
/// Uses u128 intermediate to avoid overflow when scaling 48-bit extracted
/// values by ratios up to 2x.
fn multiply_target_int(target: &[u8; 32], numerator: u64, denominator: u64) -> [u8; 32] {
    if denominator == 0 || numerator == denominator {
        return *target;
    }

    // Find first non-zero byte
    let first_nz = match target.iter().position(|&b| b != 0) {
        Some(p) => p,
        None => {
            let mut r = [0u8; 32];
            r[31] = 1;
            return r;
        }
    };

    // Extract 6 significant bytes into the low end of a u64 buffer
    let extract_len = 6.min(32 - first_nz);
    let mut buf = [0u8; 8];
    buf[8 - extract_len..].copy_from_slice(&target[first_nz..first_nz + extract_len]);
    let value = u64::from_be_bytes(buf);

    // Scale using u128 to prevent overflow: (value * numerator) / denominator
    let scaled_128 = (value as u128).saturating_mul(numerator as u128)
        / (denominator as u128);
    let scaled = if scaled_128 > u64::MAX as u128 {
        u64::MAX
    } else if scaled_128 == 0 {
        // Target can never be zero
        let mut r = [0u8; 32];
        r[31] = 1;
        return r;
    } else {
        scaled_128 as u64
    };

    // Determine where the significant bytes moved
    let scaled_be = scaled.to_be_bytes();
    let scaled_first = scaled_be.iter().position(|&b| b != 0).unwrap_or(7);
    let orig_first_in_buf = 8 - extract_len;

    // Position shift: negative = grew (easier), positive = shrunk (harder)
    let shift = scaled_first as i32 - orig_first_in_buf as i32;
    let new_pos = (first_nz as i32 + shift).clamp(0, 31) as usize;

    // Write the significant bytes of the scaled value
    let mut result = [0u8; 32];
    let sig_len = (8 - scaled_first).min(32 - new_pos);
    result[new_pos..new_pos + sig_len]
        .copy_from_slice(&scaled_be[scaled_first..scaled_first + sig_len]);

    // Clamp: never allow all-zero target
    if result == [0u8; 32] {
        result[31] = 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_difficulty_is_easy() {
        let adjuster = DifficultyAdjuster::new();
        // Initial target has 1 leading zero byte → ~256 attempts needed
        assert_eq!(adjuster.difficulty_bits(), 8);
        assert_eq!(adjuster.estimated_attempts(), 256);
    }

    #[test]
    fn check_difficulty_works() {
        let target = INITIAL_DIFFICULTY;
        let digits = b"test_digits";
        let anchor = [0u8; 32];
        let miner = [1u8; 20];

        // Try many nonces, at least one should work with easy difficulty
        let nonce = find_nonce(digits, &anchor, &target, 10_000, &miner);
        assert!(nonce.is_some(), "should find nonce with easy difficulty");

        // Verify the found nonce actually works
        let n = nonce.unwrap();
        assert!(check_proof_difficulty(digits, n, &anchor, &target, &miner));
    }

    #[test]
    fn parallel_nonce_search() {
        let target = INITIAL_DIFFICULTY;
        let digits = b"parallel_test";
        let anchor = [1u8; 32];
        let miner = [2u8; 20];

        let nonce = find_nonce_parallel(digits, &anchor, &target, 100_000, &miner);
        assert!(nonce.is_some());
        assert!(check_proof_difficulty(
            digits,
            nonce.unwrap(),
            &anchor,
            &target,
            &miner,
        ));
    }

    #[test]
    fn pow_hash_includes_miner_address() {
        let target = INITIAL_DIFFICULTY;
        let digits = b"miner_bind_test";
        let anchor = [0u8; 32];
        let miner_a = [1u8; 20];
        let miner_b = [2u8; 20];

        // Find a nonce valid for miner_a
        let nonce = find_nonce(digits, &anchor, &target, 10_000, &miner_a)
            .expect("should find nonce for miner_a");

        // The nonce must be valid for miner_a
        assert!(
            check_proof_difficulty(digits, nonce, &anchor, &target, &miner_a),
            "nonce should be valid for the miner that found it"
        );

        // The same nonce should (almost certainly) NOT be valid for miner_b
        // because the hash includes the miner address.
        // With 8-bit difficulty there is a 1/256 chance of false positive,
        // so we only assert the hash outputs differ (not necessarily that
        // miner_b fails difficulty). We check the actual hash differs.
        let hash_a = pichain_crypto::hash_concat(
            &[digits.as_slice(), &nonce.to_le_bytes(), &anchor, &miner_a],
        );
        let hash_b = pichain_crypto::hash_concat(
            &[digits.as_slice(), &nonce.to_le_bytes(), &anchor, &miner_b],
        );
        assert_ne!(
            hash_a, hash_b,
            "PoW hash must differ when miner address differs"
        );
    }

    #[test]
    fn difficulty_adjusts_down_when_too_slow() {
        let mut adjuster = DifficultyAdjuster::new();
        let initial_target = adjuster.current_target;

        // Simulate proofs coming in too slowly (20s instead of 10s target)
        for i in 0..10 {
            adjuster.record_proof(i * 20_000); // 20s intervals
        }

        // Target should increase (easier) because proofs are too slow
        assert!(
            adjuster.current_target >= initial_target,
            "difficulty should decrease (target increase) when proofs are slow"
        );
    }

    #[test]
    fn difficulty_adjusts_up_when_too_fast() {
        let mut adjuster = DifficultyAdjuster::new();
        let initial_target = adjuster.current_target;

        // Simulate proofs coming in too fast (2s instead of 10s target)
        for i in 0..10 {
            adjuster.record_proof(i * 2_000); // 2s intervals
        }

        // Target should decrease (harder) because proofs are too fast
        assert!(
            adjuster.current_target <= initial_target,
            "difficulty should increase (target decrease) when proofs are fast"
        );
    }

    #[test]
    fn difficulty_stable_at_target_rate() {
        let mut adjuster = DifficultyAdjuster::new();
        let initial_target = adjuster.current_target;

        // Simulate proofs at exactly the target rate
        for i in 0..10 {
            adjuster.record_proof(i * TARGET_PROOF_INTERVAL_MS);
        }

        // Target should remain approximately the same
        assert_eq!(
            adjuster.current_target, initial_target,
            "difficulty should stay stable at target rate"
        );
    }

    #[test]
    fn multiply_target_doubles() {
        let target: [u8; 32] = {
            let mut t = [0u8; 32];
            t[1] = 0x80; // 0x0080...
            t
        };
        let doubled = multiply_target(&target, 2.0);
        // Should roughly double (easier)
        assert!(doubled > target);
    }

    #[test]
    fn multiply_target_halves() {
        let target: [u8; 32] = {
            let mut t = [0u8; 32];
            t[1] = 0x80;
            t
        };
        let halved = multiply_target(&target, 0.5);
        // Should roughly halve (harder)
        assert!(halved < target);
    }

    #[test]
    fn frontier_pow_bits_base_case() {
        // Year 1, frontier 0: just the base 8 bits
        assert_eq!(frontier_pow_bits(0, 1), 8);
        // Year 1, frontier 500 (below FRONTIER_SCALE_DIVISOR): still 8
        assert_eq!(frontier_pow_bits(500, 1), 8);
    }

    #[test]
    fn frontier_pow_bits_scales_with_frontier() {
        // frontier 10K → scaled = 10, log2(10) = 3 → 8 + 3 = 11
        assert_eq!(frontier_pow_bits(10_000, 1), 11);
        // frontier 1M → scaled = 1000, log2(1000) = 9 → 8 + 9 = 17
        assert_eq!(frontier_pow_bits(1_000_000, 1), 17);
        // frontier 1B → scaled = 1M, log2(1M) = 19 → 8 + 19 = 27
        assert_eq!(frontier_pow_bits(1_000_000_000, 1), 27);
    }

    #[test]
    fn frontier_pow_bits_scales_with_year() {
        // Year 1: +0 moore bits
        assert_eq!(frontier_pow_bits(0, 1), 8);
        // Year 3: (3-1)/2 = 1 moore bit → 9
        assert_eq!(frontier_pow_bits(0, 3), 9);
        // Year 11: (11-1)/2 = 5 moore bits → 13
        assert_eq!(frontier_pow_bits(0, 11), 13);
    }

    #[test]
    fn frontier_pow_bits_combined() {
        // Year 10, frontier 10M: frontier_comp = log2(10000) ≈ 13, moore = (9)/2 = 4
        // Total = 8 + 13 + 4 = 25
        assert_eq!(frontier_pow_bits(10_000_000, 10), 25);
    }

    #[test]
    fn frontier_pow_bits_clamped() {
        // Should never go below MIN_DIFFICULTY_BITS
        assert!(frontier_pow_bits(0, 0) >= MIN_DIFFICULTY_BITS);
        // Should never exceed MAX_DIFFICULTY_BITS
        assert!(frontier_pow_bits(u64::MAX, 200) <= MAX_DIFFICULTY_BITS);
    }

    #[test]
    fn target_from_bits_8() {
        let target = target_from_bits(8);
        assert_eq!(target[0], 0x00);
        assert_eq!(target[1], 0xFF);
        assert_eq!(target[31], 0xFF);
    }

    #[test]
    fn target_from_bits_16() {
        let target = target_from_bits(16);
        assert_eq!(target[0], 0x00);
        assert_eq!(target[1], 0x00);
        assert_eq!(target[2], 0xFF);
    }

    #[test]
    fn target_from_bits_12() {
        let target = target_from_bits(12);
        assert_eq!(target[0], 0x00);
        // 12 bits = 1 full zero byte + 4 zero bits → 0x0F
        assert_eq!(target[1], 0x0F);
        assert_eq!(target[2], 0xFF);
    }

    #[test]
    fn target_from_bits_monotonic() {
        // More bits → harder (smaller target)
        for bits in 8..64 {
            let easier = target_from_bits(bits);
            let harder = target_from_bits(bits + 1);
            assert!(harder < easier, "bits {} should be easier than {}", bits, bits + 1);
        }
    }

    #[test]
    fn frontier_difficulty_target_matches_bits() {
        let target = frontier_difficulty_target(10_000, 1);
        let bits = frontier_pow_bits(10_000, 1);
        assert_eq!(target, target_from_bits(bits));
    }
}
