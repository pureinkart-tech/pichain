//! Validator management and PI-weighted selection.

use pichain_crypto::bls::{BlsPublicKey, BlsSignature};
use pichain_crypto::ed25519::Address;


/// A validator in the PIChain network.
#[derive(Clone, Debug)]
pub struct Validator {
    /// Validator address (derived from Ed25519 key).
    pub address: Address,
    /// BLS public key for consensus attestations.
    pub bls_public_key: BlsPublicKey,
    /// BLS proof-of-possession (proves the validator holds the corresponding secret key).
    /// `None` means PoP verification is skipped (test/single-node mode).
    pub bls_pop: Option<BlsSignature>,
    /// Amount of PI staked.
    pub stake: u64,
    /// Uptime score in basis points (0 = 0%, 10000 = 100%).
    /// Using integer arithmetic for consensus determinism across platforms.
    pub uptime_bps: u64,
    /// Contribution score (blocks produced, attestations made).
    pub contribution_score: u64,
    /// Whether this validator is active.
    pub active: bool,
}

impl Validator {
    /// Calculate the PI-Weight for validator selection.
    ///
    /// PI-Weight = f(stake, uptime, contribution_score, pi_hash_position)
    pub fn pi_weight(&self, pi_seed: &[u8; 32]) -> u64 {
        let stake_weight = self.stake / 1_000_000_000; // normalize to PI units
        // uptime_bps is 0..10000; divide by 100 to get 0..100 range
        let uptime_weight = self.uptime_bps / 100;
        let contribution_weight = self.contribution_score.min(1000);

        // PI hash position — deterministic position from PI digits
        let pi_hash = pichain_crypto::hash_concat(&[
            pi_seed,
            &self.address.0,
        ]);
        let mut pi_bytes = [0u8; 8];
        pi_bytes.copy_from_slice(&pi_hash.as_bytes()[0..8]);
        let pi_position = u64::from_le_bytes(pi_bytes) % 1000;

        // Weighted combination (stake is dominant).
        // Single division to avoid premature truncation of each term.
        (stake_weight * 60
            + uptime_weight * 20
            + contribution_weight * 10
            + pi_position * 10) / 100
    }
}

/// The set of active validators for the current epoch.
pub struct ValidatorSet {
    validators: Vec<Validator>,
    /// Total stake across all validators.
    total_stake: u64,
    /// Quorum threshold (2/3 + 1 of total stake).
    quorum_stake: u64,
}

impl ValidatorSet {
    pub fn new(validators: Vec<Validator>) -> Self {
        let total_stake: u64 = validators.iter().map(|v| v.stake).fold(0u64, |acc, s| acc.saturating_add(s));
        let quorum_stake = (total_stake as u128 * 2 / 3 + 1) as u64;
        Self {
            validators,
            total_stake,
            quorum_stake,
        }
    }

    pub fn validators(&self) -> &[Validator] {
        &self.validators
    }

    pub fn total_stake(&self) -> u64 {
        self.total_stake
    }

    pub fn quorum_stake(&self) -> u64 {
        self.quorum_stake
    }

    pub fn validator_count(&self) -> usize {
        self.validators.len()
    }

    /// Check if a set of validator stakes forms a quorum (2/3+).
    pub fn has_quorum(&self, stake: u64) -> bool {
        stake >= self.quorum_stake
    }

    /// Get a validator by address.
    pub fn get_validator(&self, address: &Address) -> Option<&Validator> {
        self.validators.iter().find(|v| v.address == *address)
    }

    /// Select the leader for a given round using PI-weighted VRF.
    pub fn select_leader(&self, round: u64, pi_seed: &[u8; 32]) -> Option<&Validator> {
        if self.validators.is_empty() {
            return None;
        }

        // Compute selection hash from round + PI seed
        let selection_hash = pichain_crypto::hash_concat(&[
            &round.to_le_bytes(),
            pi_seed,
        ]);
        let mut selection_bytes = [0u8; 8];
        selection_bytes.copy_from_slice(&selection_hash.as_bytes()[0..8]);
        let selection_value = u64::from_le_bytes(selection_bytes);

        // Weighted random selection based on PI-Weight.
        // Use u128 for weight accumulation to prevent saturation when many
        // validators have large PI-weights (u64 saturating_add could clamp
        // to u64::MAX, biasing leader selection toward later validators).
        let weights: Vec<u64> = self
            .validators
            .iter()
            .map(|v| v.pi_weight(pi_seed).max(1))
            .collect();
        let total_weight: u128 = weights.iter().map(|w| *w as u128).sum();
        if total_weight == 0 {
            return self.validators.first();
        }
        let target = (selection_value as u128) % total_weight;

        let mut cumulative: u128 = 0;
        for (i, weight) in weights.iter().enumerate() {
            cumulative += *weight as u128;
            if target < cumulative {
                return Some(&self.validators[i]);
            }
        }

        self.validators.last()
    }

    /// Get the minimum number of validators needed for quorum.
    pub fn quorum_count(&self) -> usize {
        self.validators.len() * 2 / 3 + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::bls::BlsKeypair;

    fn make_validator(stake: u64, i: u8) -> Validator {
        let bls_kp = BlsKeypair::generate();
        Validator {
            address: Address([i; 20]),
            bls_public_key: bls_kp.public,
            bls_pop: None,
            stake,
            uptime_bps: 9900, // 99%
            contribution_score: 100,
            active: true,
        }
    }

    #[test]
    fn validator_set_quorum() {
        let validators = vec![
            make_validator(1000, 1),
            make_validator(1000, 2),
            make_validator(1000, 3),
        ];
        let set = ValidatorSet::new(validators);
        assert_eq!(set.total_stake(), 3000);
        assert!(set.has_quorum(2001));
        assert!(!set.has_quorum(2000));
    }

    #[test]
    fn leader_selection_deterministic() {
        let validators = vec![
            make_validator(1000, 1),
            make_validator(2000, 2),
            make_validator(3000, 3),
        ];
        let set = ValidatorSet::new(validators);
        let seed = [0u8; 32];

        let leader1 = set.select_leader(1, &seed).unwrap();
        let leader2 = set.select_leader(1, &seed).unwrap();
        assert_eq!(leader1.address, leader2.address);
    }

    #[test]
    fn different_rounds_may_select_different_leaders() {
        let validators = vec![
            make_validator(1000, 1),
            make_validator(1000, 2),
            make_validator(1000, 3),
            make_validator(1000, 4),
        ];
        let set = ValidatorSet::new(validators);
        let seed = [0u8; 32];

        let mut leaders = std::collections::HashSet::new();
        for round in 0..100 {
            let leader = set.select_leader(round, &seed).unwrap();
            leaders.insert(leader.address);
        }
        // Should select multiple different leaders across 100 rounds
        assert!(leaders.len() > 1);
    }
}
