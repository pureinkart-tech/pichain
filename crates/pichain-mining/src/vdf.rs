//! Quantum-Safe Verifiable Delay Function (VDF) for PIChain mining.
//!
//! Uses an iterated Blake3 hash chain — the most conservative sequential primitive.
//! Each step depends on the previous, so the computation is inherently sequential.
//! No technology (quantum, classical, ASIC) can parallelize it.
//!
//! # Purpose
//!
//! The VDF gates mining proof submission rate. Even if a quantum computer
//! computes BBP digits instantly, it still must wait for the VDF to complete.
//! This caps the quantum mining advantage to ≤1.5x.
//!
//! # Verification
//!
//! For now, verification re-computes the hash chain (O(T) cost).
//! A future upgrade can use a STARK proof for O(log T) verification.
//!
//! # Security
//!
//! Based entirely on Blake3 preimage resistance. Quantum computers provide
//! at most a sqrt speedup on hash preimage (Grover's algorithm), giving
//! 128-bit quantum security with 256-bit Blake3 output. This is sufficient
//! for any foreseeable physics.

/// VDF parameters.
pub const DEFAULT_VDF_ITERATIONS: u64 = 500_000;

/// Minimum VDF iterations (prevents miners from skipping VDF with very fast hardware).
/// This is the absolute floor — even at chain genesis, VDF must do at least this many.
pub const MIN_VDF_ITERATIONS: u64 = 100_000;

/// Maximum VDF iterations (prevents DoS via excessive verification cost).
pub const MAX_VDF_ITERATIONS: u64 = 10_000_000;

/// Blocks per year (matching pichain-mining/src/reward.rs)
const VDF_BLOCKS_PER_YEAR: u64 = 365 * 24 * 3600 * 1000 / 314;

/// Compute the VDF: iterate Blake3 hash `iterations` times.
///
/// `output = Blake3(Blake3(Blake3(...Blake3(input)...)))` (iterations times)
///
/// This is inherently sequential — each step depends on the previous.
/// No parallelism, no quantum speedup, no ASIC shortcut.
///
/// # Arguments
///
/// * `input` - Initial seed (typically: Blake3(digits || anchor_block_hash || miner))
/// * `iterations` - Number of sequential hash iterations
///
/// # Returns
///
/// The final hash output after all iterations.
pub fn vdf_compute(input: &[u8; 32], iterations: u64) -> VdfProof {
    let mut state = *input;
    for _ in 0..iterations {
        let h = pichain_crypto::hash(&state);
        state = *h.as_bytes();
    }
    VdfProof {
        input: *input,
        output: state,
        iterations,
    }
}

/// Verify a VDF proof by re-computing the hash chain.
///
/// Returns `true` if `vdf_compute(proof.input, proof.iterations).output == proof.output`.
///
/// # Performance
///
/// Verification cost is O(iterations). At 500K iterations, this takes ~50ms on modern hardware.
/// Blake3 processes ~10M hashes/sec per core.
pub fn vdf_verify(proof: &VdfProof) -> bool {
    if proof.iterations > MAX_VDF_ITERATIONS {
        return false;
    }
    // SECURITY: Reject zero/trivial iteration counts. The VDF's purpose is to
    // enforce a minimum sequential computation time. Accepting zero iterations
    // would defeat this entirely.
    if proof.iterations < MIN_VDF_ITERATIONS {
        return false;
    }
    let recomputed = vdf_compute(&proof.input, proof.iterations);
    recomputed.output == proof.output
}

/// Compute the VDF input seed from mining proof components.
///
/// The seed binds the VDF to the specific proof, miner, and block context,
/// preventing VDF pre-computation and proof reuse.
pub fn vdf_seed(
    digits: &[u8],
    anchor_block_hash: &[u8; 32],
    miner: &[u8; 20],
) -> [u8; 32] {
    let h = pichain_crypto::hash_concat(&[
        digits,
        anchor_block_hash,
        miner,
        b"pichain-vdf-v1",
    ]);
    *h.as_bytes()
}

/// Calculate the required VDF iterations based on network parameters.
///
/// Uses **progressive strengthening**: starts loose for bootstrapping (letting
/// anyone mine from a laptop), then gradually tightens as the chain matures.
/// This ensures user-friendliness at launch while building toward full quantum
/// resistance over time.
///
/// # Progressive schedule
///
/// The VDF strength is determined by the larger of two factors:
/// - **Frontier depth**: as PI computation goes deeper, VDF scales up
/// - **Chain age**: VDF strengthens over time regardless of frontier
///
/// | Phase | Frontier | Chain Age | VDF Iterations | ~Wall Time |
/// |-------|----------|-----------|----------------|------------|
/// | Genesis | 0-1K | Year 1 | 100K | ~10ms |
/// | Bootstrap | 1K-10K | Year 1 | 200K | ~20ms |
/// | Early | 10K-100K | Year 1-2 | 500K | ~50ms |
/// | Growing | 100K-1M | Year 2-5 | 1M | ~100ms |
/// | Mature | 1M-10M | Year 5-10 | 2M | ~200ms |
/// | Hardened | 10M+ | Year 10+ | 4M | ~400ms |
///
/// At no point does VDF take more than ~1 second, keeping mining accessible.
/// The quantum resistance comes from the combination of VDF + sqrt scaling + epoch cap,
/// not from VDF alone being extremely long.
pub fn required_iterations(frontier: u64) -> u64 {
    required_iterations_with_age(frontier, 0)
}

/// Calculate required VDF iterations considering both frontier depth and chain age.
///
/// `block_height` is used to determine chain age. Pass 0 if unknown (uses frontier only).
pub fn required_iterations_with_age(frontier: u64, block_height: u64) -> u64 {
    // Factor 1: Frontier depth
    let frontier_scale = if frontier < 1_000 {
        1u64 // Genesis: minimal VDF
    } else if frontier < 10_000 {
        2 // Bootstrap
    } else if frontier < 100_000 {
        5 // Early
    } else if frontier < 1_000_000 {
        10 // Growing
    } else if frontier < 10_000_000 {
        20 // Mature
    } else {
        40 // Hardened (10M+ digits computed)
    };

    // Factor 2: Chain age (progressive strengthening over time)
    let age_years = if block_height > 0 {
        (block_height / VDF_BLOCKS_PER_YEAR) as u64
    } else {
        0
    };
    let age_scale = match age_years {
        0 => 1u64,        // Year 1: no age-based increase
        1 => 2,           // Year 2: 2x
        2..=4 => 4,       // Years 3-5: 4x
        5..=9 => 8,       // Years 6-10: 8x
        _ => 16,          // Year 11+: 16x (full quantum hardening)
    };

    // Use the LARGER of the two factors — ensures VDF strengthens even if
    // frontier stalls, and scales with frontier even on a young chain.
    let effective_scale = frontier_scale.max(age_scale);
    let iterations = MIN_VDF_ITERATIONS.saturating_mul(effective_scale);
    iterations.clamp(MIN_VDF_ITERATIONS, MAX_VDF_ITERATIONS)
}

/// A VDF proof — evidence that the sequential hash chain was computed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VdfProof {
    /// Input seed to the hash chain.
    pub input: [u8; 32],
    /// Final output after `iterations` sequential hashes.
    pub output: [u8; 32],
    /// Number of sequential hash iterations performed.
    pub iterations: u64,
}

impl VdfProof {
    /// Serialize to bytes for inclusion in transactions.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(72);
        buf.extend_from_slice(&self.input);
        buf.extend_from_slice(&self.output);
        buf.extend_from_slice(&self.iterations.to_le_bytes());
        buf
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 72 {
            return None;
        }
        let mut input = [0u8; 32];
        let mut output = [0u8; 32];
        input.copy_from_slice(&bytes[..32]);
        output.copy_from_slice(&bytes[32..64]);
        let iterations = u64::from_le_bytes(bytes[64..72].try_into().ok()?);
        Some(Self {
            input,
            output,
            iterations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vdf_compute_deterministic() {
        let input = [42u8; 32];
        let p1 = vdf_compute(&input, 1000);
        let p2 = vdf_compute(&input, 1000);
        assert_eq!(p1.output, p2.output);
    }

    #[test]
    fn vdf_different_inputs_different_outputs() {
        let a = vdf_compute(&[1u8; 32], 1000);
        let b = vdf_compute(&[2u8; 32], 1000);
        assert_ne!(a.output, b.output);
    }

    #[test]
    fn vdf_different_iterations_different_outputs() {
        let input = [42u8; 32];
        let a = vdf_compute(&input, 1000);
        let b = vdf_compute(&input, 1001);
        assert_ne!(a.output, b.output);
    }

    #[test]
    fn vdf_verify_valid_proof() {
        let input = [42u8; 32];
        let proof = vdf_compute(&input, MIN_VDF_ITERATIONS);
        assert!(vdf_verify(&proof));
    }

    #[test]
    fn vdf_verify_rejects_wrong_output() {
        let input = [42u8; 32];
        let mut proof = vdf_compute(&input, MIN_VDF_ITERATIONS);
        proof.output[0] ^= 0xFF; // tamper
        assert!(!vdf_verify(&proof));
    }

    #[test]
    fn vdf_verify_rejects_wrong_iterations() {
        let input = [42u8; 32];
        let mut proof = vdf_compute(&input, MIN_VDF_ITERATIONS);
        proof.iterations = MIN_VDF_ITERATIONS - 1; // lie about iteration count
        assert!(!vdf_verify(&proof));
    }

    #[test]
    fn vdf_verify_rejects_excessive_iterations() {
        let proof = VdfProof {
            input: [0u8; 32],
            output: [0u8; 32],
            iterations: MAX_VDF_ITERATIONS + 1,
        };
        assert!(!vdf_verify(&proof));
    }

    #[test]
    fn vdf_seed_includes_all_components() {
        let digits = b"test_digits";
        let anchor = [1u8; 32];
        let miner = [2u8; 20];

        let s1 = vdf_seed(digits, &anchor, &miner);

        // Changing any component changes the seed
        let s2 = vdf_seed(b"other_digits", &anchor, &miner);
        assert_ne!(s1, s2);

        let s3 = vdf_seed(digits, &[3u8; 32], &miner);
        assert_ne!(s1, s3);

        let s4 = vdf_seed(digits, &anchor, &[4u8; 20]);
        assert_ne!(s1, s4);
    }

    #[test]
    fn vdf_proof_serialization_roundtrip() {
        let input = [42u8; 32];
        let proof = vdf_compute(&input, 5000);
        let bytes = proof.to_bytes();
        assert_eq!(bytes.len(), 72);
        let restored = VdfProof::from_bytes(&bytes).unwrap();
        assert_eq!(proof, restored);
    }

    #[test]
    fn required_iterations_scales_with_frontier() {
        let early = required_iterations(0);
        let mid = required_iterations(100_000);
        let mature = required_iterations(1_000_000);

        assert!(early <= mid);
        assert!(mid <= mature);
        assert!(early >= MIN_VDF_ITERATIONS);
        assert!(mature <= MAX_VDF_ITERATIONS);
    }

    #[test]
    fn progressive_strengthening_with_age() {
        // Year 1, frontier 0: minimal VDF
        let genesis = required_iterations_with_age(0, 0);
        assert_eq!(genesis, MIN_VDF_ITERATIONS);

        // Year 1, frontier 100K: frontier scaling = 10x
        let early = required_iterations_with_age(100_000, VDF_BLOCKS_PER_YEAR / 2);
        assert!(early > genesis);

        // Year 11, frontier 0: age-based strengthening alone = 16x
        let year11_no_frontier = required_iterations_with_age(0, VDF_BLOCKS_PER_YEAR * 11);
        assert!(
            year11_no_frontier >= MIN_VDF_ITERATIONS * 16,
            "year 11+ should have 16x age scaling, got {}",
            year11_no_frontier
        );

        // Age strengthening at LOW frontier (where frontier factor is small):
        // Year 1 at frontier 1K: max(2, 1) = 2x
        let year1_low = required_iterations_with_age(1_000, VDF_BLOCKS_PER_YEAR);
        // Year 5 at frontier 1K: max(2, 8) = 8x
        let year5_low = required_iterations_with_age(1_000, VDF_BLOCKS_PER_YEAR * 5);
        assert!(
            year5_low > year1_low,
            "year 5 at low frontier should be stronger than year 1: {} vs {}",
            year5_low, year1_low
        );
    }

    #[test]
    fn progressive_strengthening_uses_max_of_factors() {
        // High frontier but young chain: frontier factor dominates
        let high_frontier = required_iterations_with_age(10_000_000, 1);
        assert!(high_frontier >= MIN_VDF_ITERATIONS * 20);

        // Low frontier but old chain: age factor dominates
        let old_chain = required_iterations_with_age(0, VDF_BLOCKS_PER_YEAR * 11);
        assert!(old_chain >= MIN_VDF_ITERATIONS * 16);
    }

    #[test]
    fn vdf_never_exceeds_one_second() {
        // At ~10M hashes/sec, MAX_VDF_ITERATIONS (10M) = ~1 second
        // Verify no parameter combination exceeds this
        let max = required_iterations_with_age(u64::MAX, VDF_BLOCKS_PER_YEAR * 100);
        assert!(max <= MAX_VDF_ITERATIONS);
    }

    #[test]
    fn vdf_zero_iterations_rejected() {
        let input = [42u8; 32];
        let proof = vdf_compute(&input, 0);
        // Zero iterations: output == input but should be REJECTED by verify
        assert_eq!(proof.output, input);
        assert!(!vdf_verify(&proof), "zero-iteration VDF should be rejected");
    }

    #[test]
    fn vdf_single_iteration_rejected() {
        let input = [42u8; 32];
        let proof = vdf_compute(&input, 1);
        let expected = pichain_crypto::hash(&input);
        assert_eq!(proof.output, *expected.as_bytes());
        // Single iteration is far below MIN_VDF_ITERATIONS, should be rejected
        assert!(!vdf_verify(&proof), "single-iteration VDF should be rejected");
    }

    #[test]
    fn vdf_minimum_iterations_accepted() {
        let input = [42u8; 32];
        let proof = vdf_compute(&input, MIN_VDF_ITERATIONS);
        assert!(vdf_verify(&proof), "MIN_VDF_ITERATIONS should be accepted");
    }
}
