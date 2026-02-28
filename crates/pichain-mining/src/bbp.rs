//! Bailey-Borwein-Plouffe (BBP) formula for computing hexadecimal digits of PI.
//!
//! The BBP formula allows computing the n-th hexadecimal digit of PI directly,
//! without computing all preceding digits. This is the key property that makes
//! PI digit computation naturally parallelizable for mining.
//!
//! Formula: PI = SUM(k=0 to inf) [1/16^k * (4/(8k+1) - 2/(8k+4) - 1/(8k+5) - 1/(8k+6))]
//!
//! To extract hex digit at position n, we compute the fractional part of 16^n * PI
//! using modular exponentiation to keep numbers manageable.

use rayon::prelude::*;

/// BBP PI digit computer.
pub struct BbpComputer;

/// Scaled fixed-point arithmetic with 64 fractional bits.
/// All values represent fractions in [0, 1) as integers in [0, 2^64).
const FRAC_BITS: u32 = 64;
const FRAC_ONE: u128 = 1u128 << FRAC_BITS;

impl BbpComputer {
    /// Compute a single hexadecimal digit of PI at the given position (0-indexed).
    ///
    /// Uses the BBP (Bailey-Borwein-Plouffe) spigot algorithm with deterministic
    /// fixed-point integer arithmetic (no f64) to ensure identical results across
    /// all CPU architectures (ARM, x86, etc.).
    ///
    /// Position 0 = first hex digit after the decimal point of PI in hex.
    ///
    /// PI in hex = 3.243F6A8885A308D3...
    /// Position 0 → 2, Position 1 → 4, Position 2 → 3, etc.
    pub fn compute_hex_digit(position: u64) -> u8 {
        let n = position;

        let s1 = Self::series_sum_fixed(n, 1);
        let s4 = Self::series_sum_fixed(n, 4);
        let s5 = Self::series_sum_fixed(n, 5);
        let s6 = Self::series_sum_fixed(n, 6);

        // Combine: {4*S1 - 2*S4 - S5 - S6} mod 1
        // Work in i128 to handle negative intermediate values
        let combined = (4i128 * s1 as i128) - (2i128 * s4 as i128) - (s5 as i128) - (s6 as i128);
        // Reduce to [0, FRAC_ONE)
        let frac = combined.rem_euclid(FRAC_ONE as i128) as u128;
        // Extract hex digit: top 4 bits of fractional part
        ((frac >> (FRAC_BITS - 4)) & 0xF) as u8
    }

    /// Compute a batch of hexadecimal digits starting at the given position.
    /// Uses single-threaded computation.
    pub fn compute_hex_digits(start_position: u64, count: u32) -> Vec<u8> {
        (0..count as u64)
            .map(|i| Self::compute_hex_digit(start_position + i))
            .collect()
    }

    /// Compute a batch of hexadecimal digits in parallel across all available CPU cores.
    /// Each digit is independent (BBP property), making this embarrassingly parallel.
    pub fn compute_hex_digits_parallel(start_position: u64, count: u32) -> Vec<u8> {
        (0..count as u64)
            .into_par_iter()
            .map(|i| Self::compute_hex_digit(start_position + i))
            .collect()
    }

    /// Compute the BBP series sum using deterministic fixed-point integer arithmetic.
    ///
    /// Returns a u64 representing the fractional part of {16^n * SUM(k=0..inf) 1/(16^k * (8k+j))}
    /// scaled by 2^64. All arithmetic is in the integer domain (u128/i128) to avoid
    /// platform-dependent f64 rounding differences between ARM and x86 FPUs.
    ///
    /// Split into two parts:
    /// 1. k = 0 to n: use modular exponentiation (exact integers)
    /// 2. k = n+1 to inf: scaled fixed-point division (converges rapidly)
    fn series_sum_fixed(n: u64, j: u64) -> u64 {
        let mut sum: u128 = 0;

        // Part 1: k = 0 to n (modular exponentiation -- exact)
        for k in 0..=n {
            let denom = 8 * k + j;
            if denom == 0 {
                continue;
            }
            let power = n - k;
            let mod_exp = Self::mod_pow_16(power, denom) as u128;
            // Add (mod_exp / denom) in fixed-point: (mod_exp << 64) / denom
            let term = (mod_exp << FRAC_BITS) / denom as u128;
            sum = sum.wrapping_add(term);
            // Keep fractional part only
            sum &= FRAC_ONE - 1;
        }

        // Part 2: k = n+1 onwards (converges rapidly)
        // Here 16^(n-k) = 16^(-(k-n)) = 1/16^(k-n)
        // In fixed-point: start with scale = FRAC_ONE / 16, divide by 16 each iteration
        let mut scale: u128 = FRAC_ONE / 16;
        for k in (n + 1).. {
            let denom = 8 * k + j;
            if denom == 0 {
                continue;
            }
            let term = scale / denom as u128;
            if term == 0 {
                break;
            }
            sum = sum.wrapping_add(term);
            sum &= FRAC_ONE - 1;
            scale /= 16;
        }

        (sum & (FRAC_ONE - 1)) as u64
    }

    /// Compute 16^power mod denom using binary modular exponentiation.
    fn mod_pow_16(power: u64, denom: u64) -> u64 {
        if denom == 0 {
            return 0;
        }
        if power == 0 {
            return 1 % denom;
        }

        let mut result: u128 = 1;
        let mut base: u128 = 16 % denom as u128;
        let mut exp = power;
        let m = denom as u128;

        while exp > 0 {
            if exp & 1 == 1 {
                result = (result * base) % m;
            }
            exp >>= 1;
            if exp > 0 {
                base = (base * base) % m;
            }
        }

        result as u64
    }
}

/// Convert a vector of hex digits to a hex string.
pub fn digits_to_hex_string(digits: &[u8]) -> String {
    digits
        .iter()
        .map(|d| format!("{:x}", d))
        .collect::<String>()
}

/// Known PI hex digits for verification (after "3."):
/// PI = 3.243F6A8885A308D313198A2E03707344A4093822299F31D008...
pub const KNOWN_PI_HEX: &str = "243f6a8885a308d313198a2e03707344a4093822299f31d008";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_hex_digit() {
        // PI hex = 3.243F6A8...
        // Position 0 should be 2
        let digit = BbpComputer::compute_hex_digit(0);
        assert_eq!(digit, 2, "First hex digit of PI should be 2");
    }

    #[test]
    fn first_several_digits() {
        let digits = BbpComputer::compute_hex_digits(0, 8);
        let hex = digits_to_hex_string(&digits);
        assert!(
            KNOWN_PI_HEX.starts_with(&hex),
            "Expected PI hex to start with {hex}, known: {}",
            &KNOWN_PI_HEX[..8]
        );
    }

    #[test]
    fn specific_known_positions() {
        // Known hex digits of PI at specific positions
        let expected: Vec<(u64, u8)> = vec![
            (0, 0x2),  // 2
            (1, 0x4),  // 4
            (2, 0x3),  // 3
            (3, 0xF),  // F
        ];

        for (pos, expected_digit) in expected {
            let digit = BbpComputer::compute_hex_digit(pos);
            assert_eq!(
                digit, expected_digit,
                "PI hex digit at position {pos}: expected {expected_digit:x}, got {digit:x}"
            );
        }
    }

    #[test]
    fn deterministic() {
        let d1 = BbpComputer::compute_hex_digit(100);
        let d2 = BbpComputer::compute_hex_digit(100);
        assert_eq!(d1, d2);
    }

    #[test]
    fn batch_matches_individual() {
        let batch = BbpComputer::compute_hex_digits(0, 4);
        for (i, &digit) in batch.iter().enumerate() {
            assert_eq!(digit, BbpComputer::compute_hex_digit(i as u64));
        }
    }

    #[test]
    fn mod_pow_16_correctness() {
        // 16^0 mod 1 = 0
        assert_eq!(BbpComputer::mod_pow_16(0, 1), 0);
        // 16^1 mod 10 = 6
        assert_eq!(BbpComputer::mod_pow_16(1, 10), 6);
        // 16^2 mod 10 = 256 mod 10 = 6
        assert_eq!(BbpComputer::mod_pow_16(2, 10), 6);
        // 16^3 mod 100 = 4096 mod 100 = 96
        assert_eq!(BbpComputer::mod_pow_16(3, 100), 96);
    }

    #[test]
    fn hex_string_conversion() {
        let digits = vec![0x2, 0x4, 0x3, 0xF, 0x6, 0xA];
        assert_eq!(digits_to_hex_string(&digits), "243f6a");
    }

    #[test]
    fn parallel_matches_sequential() {
        let seq = BbpComputer::compute_hex_digits(0, 100);
        let par = BbpComputer::compute_hex_digits_parallel(0, 100);
        assert_eq!(seq, par, "Parallel computation must produce identical results");
    }

    #[test]
    fn parallel_larger_batch() {
        let par = BbpComputer::compute_hex_digits_parallel(0, 500);
        assert_eq!(par.len(), 500);
        // Verify first few digits match known PI
        assert_eq!(par[0], 0x2);
        assert_eq!(par[1], 0x4);
        assert_eq!(par[2], 0x3);
        assert_eq!(par[3], 0xF);
    }
}
