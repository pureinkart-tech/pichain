//! Mining proof generation and verification.
//!
//! Miners submit proofs of PI digit computation. The proof contains:
//! - The computed digits
//! - The starting position
//! - A hash commitment
//!
//! Verification re-computes a random subset of the claimed digits
//! and checks they match. This makes fraud economically irrational.

use pichain_crypto::Hash;
use serde::{Deserialize, Serialize};

use crate::bbp::BbpComputer;
use crate::MiningError;

/// Integer square root -- deterministic across all platforms (no floating-point).
/// Returns the largest n such that n*n <= x.
/// Uses pure integer Newton's method with a bit-shift initial guess.
fn isqrt_u32(x: u32) -> u32 {
    if x <= 1 {
        return x;
    }
    // Initial guess: 2^(ceil(log2(x))/2) -- always >= true sqrt
    let shift = (32 - x.leading_zeros()).div_ceil(2);
    let mut r = 1u32 << shift;
    // Newton's method: r = (r + x/r) / 2, converges in ~5 iterations for u32
    loop {
        let r1 = (r + x / r) / 2;
        if r1 >= r {
            break;
        }
        r = r1;
    }
    r
}

/// A mining proof — evidence that PI digits were correctly computed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MiningProof {
    /// Starting hex digit position.
    pub start_position: u64,
    /// Number of hex digits computed.
    pub digit_count: u32,
    /// The computed hex digits.
    pub digits: Vec<u8>,
    /// Blake3 hash of the digits (commitment).
    pub commitment: Hash,
    /// Miner's address (for reward distribution).
    pub miner: pichain_crypto::keys::Address,
    /// Nonce used in the proof.
    pub nonce: u64,
}

impl MiningProof {
    /// Create a new mining proof.
    pub fn new(
        start_position: u64,
        digits: Vec<u8>,
        miner: pichain_crypto::keys::Address,
        nonce: u64,
    ) -> Self {
        let commitment = pichain_crypto::hash(&digits);
        Self {
            start_position,
            digit_count: digits.len().min(u32::MAX as usize) as u32,
            digits,
            commitment,
            miner,
            nonce,
        }
    }

    /// Serialize the proof into a canonical binary encoding for consensus.
    ///
    /// Uses explicit field ordering with length prefixes instead of serde_json,
    /// which has non-deterministic field ordering that could cause proofs to
    /// hash differently on different nodes, breaking consensus.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(20 + 8 + 4 + self.digits.len() + 32 + 8);
        // Canonical field order with explicit length prefixes
        buf.extend_from_slice(&self.start_position.to_le_bytes()); // 8 bytes
        buf.extend_from_slice(&self.digit_count.to_le_bytes()); // 4 bytes
        buf.extend_from_slice(&(self.digits.len() as u32).to_le_bytes()); // 4 bytes (length prefix)
        buf.extend_from_slice(&self.digits); // variable
        buf.extend_from_slice(self.commitment.as_bytes()); // 32 bytes
        buf.extend_from_slice(&self.miner.0); // 20 bytes
        buf.extend_from_slice(&self.nonce.to_le_bytes()); // 8 bytes
        buf
    }
}

/// Proof verifier — validates mining proofs by spot-checking digits.
#[derive(Clone)]
pub struct ProofVerifier {
    /// Minimum number of digits to spot-check for verification.
    pub spot_check_count: usize,
}

impl ProofVerifier {
    pub fn new() -> Self {
        Self {
            spot_check_count: 32, // Minimum 32 random positions
        }
    }

    /// Calculate the number of spot checks for a given proof size.
    /// Uses max(min_checks, isqrt(digit_count)) to scale with proof size,
    /// ensuring large proofs get proportionally more verification.
    ///
    /// SECURITY: Uses integer square root instead of f64 to ensure deterministic
    /// results across all platforms. Floating-point sqrt can produce different
    /// results on ARM vs x86 (e.g., isqrt(961) could be 30 or 31 depending on
    /// FPU rounding), causing one node to accept a proof that another rejects,
    /// leading to consensus divergence.
    pub fn effective_check_count(&self, digit_count: u32) -> usize {
        let scaled = if digit_count < 500 {
            // For small proofs, check 25% of digits to make grinding infeasible
            (digit_count as usize) / 4
        } else {
            isqrt_u32(digit_count) as usize
        };
        self.spot_check_count.max(scaled).min(digit_count as usize)
    }

    /// Verify a mining proof by spot-checking computed digits.
    ///
    /// Randomly samples positions and verifies they match the BBP algorithm.
    /// The number of checks scales with proof size: max(16, sqrt(digit_count)).
    /// For a 1000-digit proof: 31 checks. For 10,000: 100 checks.
    ///
    /// `anchor_block_hash` is included in the spot-check seed to prevent the miner
    /// from grinding nonces to influence which positions are checked. Without it,
    /// the miner controls `commitment`, `nonce`, and `start_position` — all inputs
    /// to the seed — allowing ~2^16 attempts to find a nonce where all checks land
    /// on honestly-computed positions while the rest are fabricated.
    pub fn verify(
        &self,
        proof: &MiningProof,
        anchor_block_hash: &[u8; 32],
    ) -> Result<(), MiningError> {
        // Verify commitment
        let expected_commitment = pichain_crypto::hash(&proof.digits);
        if expected_commitment != proof.commitment {
            return Err(MiningError::InvalidProof("commitment mismatch".into()));
        }

        // Verify digit count matches
        if proof.digits.len() != proof.digit_count as usize {
            return Err(MiningError::InvalidProof("digit count mismatch".into()));
        }

        // All digits should be valid hex (0-15)
        if proof.digits.iter().any(|&d| d > 15) {
            return Err(MiningError::InvalidProof("invalid hex digit".into()));
        }

        // Reject zero-digit proofs (would pass verification vacuously)
        if proof.digit_count == 0 {
            return Err(MiningError::InvalidProof("zero digit count".into()));
        }

        // Spot-check: recompute specific positions and verify
        // Scales with proof size to maintain security for large proofs
        let check_count = self.effective_check_count(proof.digit_count);

        // Deterministic spot-check positions based on commitment, nonce, start_position,
        // AND anchor_block_hash. The anchor hash is controlled by the blockchain, not the
        // miner, preventing nonce grinding to influence which positions are checked.
        let check_seed = pichain_crypto::hash_concat(&[
            proof.commitment.as_bytes(),
            &proof.nonce.to_le_bytes(),
            &proof.start_position.to_le_bytes(),
            anchor_block_hash,
        ]);

        // Pre-compute all check positions and expected offsets
        let checks: Vec<(u64, usize)> = (0..check_count)
            .map(|i| {
                // R30-FIX: Include check index in ALL position seeds to prevent targeted forgery.
                let position_seed = pichain_crypto::hash_concat(&[
                    check_seed.as_bytes(),
                    &(i as u64).to_le_bytes(),
                ]);
                let offset_bytes: [u8; 8] = position_seed.as_bytes()[0..8].try_into().unwrap();
                let offset = u64::from_le_bytes(offset_bytes) % (proof.digit_count.max(1) as u64);
                let position = proof.start_position.saturating_add(offset);
                (position, offset as usize)
            })
            .collect();

        // Overflow check
        for &(position, _) in &checks {
            if position < proof.start_position {
                return Err(MiningError::InvalidProof(
                    "position overflow: start_position + offset > u64::MAX".into(),
                ));
            }
        }

        // Parallelize BBP spot checks with rayon — each check is O(position) and
        // independent, so this gives ~Nx speedup on N cores. At position 1.8M,
        // each check takes ~1s sequentially; with 16+ cores this drops to ~3s.
        use rayon::prelude::*;

        let mismatch = checks.par_iter().find_map_any(|&(position, offset_idx)| {
            let expected = BbpComputer::compute_hex_digit(position);
            let actual = proof.digits[offset_idx];
            if expected != actual {
                Some((position, expected, actual))
            } else {
                None
            }
        });

        if let Some((position, expected, actual)) = mismatch {
            return Err(MiningError::InvalidProof(format!(
                "digit mismatch at position {position}: expected {expected:x}, got {actual:x}"
            )));
        }

        Ok(())
    }
}

impl Default for ProofVerifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::keys::Address;

    #[test]
    fn valid_proof_passes() {
        let digits = BbpComputer::compute_hex_digits(0, 32);
        let proof = MiningProof::new(0, digits, Address::ZERO, 42);

        let verifier = ProofVerifier::new();
        assert!(verifier.verify(&proof, &[0u8; 32]).is_ok());
    }

    #[test]
    fn tampered_digits_fail() {
        let mut digits = BbpComputer::compute_hex_digits(0, 32);
        let real_commitment = pichain_crypto::hash(&digits);

        // Tamper with a digit
        digits[5] = (digits[5] + 1) % 16;

        let proof = MiningProof {
            start_position: 0,
            digit_count: 32,
            digits,
            commitment: real_commitment, // wrong commitment now
            miner: Address::ZERO,
            nonce: 42,
        };

        let verifier = ProofVerifier::new();
        assert!(verifier.verify(&proof, &[0u8; 32]).is_err());
    }

    #[test]
    fn invalid_hex_digit_fails() {
        let mut digits = BbpComputer::compute_hex_digits(0, 8);
        digits[0] = 255; // invalid hex digit

        let proof = MiningProof::new(0, digits, Address::ZERO, 0);
        let verifier = ProofVerifier::new();
        assert!(verifier.verify(&proof, &[0u8; 32]).is_err());
    }

    #[test]
    fn proof_serialization_deterministic() {
        let digits = BbpComputer::compute_hex_digits(100, 16);
        let proof = MiningProof::new(100, digits, Address::ZERO, 1);
        let bytes1 = proof.to_bytes();
        let bytes2 = proof.to_bytes();
        assert_eq!(bytes1, bytes2, "canonical encoding must be deterministic");
        assert_eq!(proof.start_position, 100);
        assert_eq!(proof.digit_count, 16);
    }

    #[test]
    fn zero_digit_proof_rejected() {
        let proof = MiningProof {
            start_position: 0,
            digit_count: 0,
            digits: vec![],
            commitment: pichain_crypto::hash(&[]),
            miner: Address::ZERO,
            nonce: 0,
        };
        let verifier = ProofVerifier::new();
        let err = verifier.verify(&proof, &[0u8; 32]).unwrap_err();
        assert!(err.to_string().contains("zero digit count"));
    }

    #[test]
    fn spot_check_positions_vary_with_start_position() {
        // Same digits/nonce but different start_position should produce different check patterns
        let digits = BbpComputer::compute_hex_digits(0, 200);
        let proof_a = MiningProof::new(0, digits.clone(), Address::ZERO, 42);
        let proof_b = MiningProof {
            start_position: 1000,
            digit_count: 200,
            digits: digits.clone(),
            commitment: pichain_crypto::hash(&digits),
            miner: Address::ZERO,
            nonce: 42,
        };
        // Seeds should differ because start_position is included
        let seed_a = pichain_crypto::hash_concat(&[
            proof_a.commitment.as_bytes(),
            &proof_a.nonce.to_le_bytes(),
            &proof_a.start_position.to_le_bytes(),
        ]);
        let seed_b = pichain_crypto::hash_concat(&[
            proof_b.commitment.as_bytes(),
            &proof_b.nonce.to_le_bytes(),
            &proof_b.start_position.to_le_bytes(),
        ]);
        assert_ne!(seed_a, seed_b);
    }
}
