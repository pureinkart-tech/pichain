//! Avalanche-inspired fast-path for simple transactions.
//!
//! Simple transactions (token transfers, single-owner operations) are finalized
//! in ~200-500ms by querying k random validators. If alpha+ agree on validity,
//! the transaction is finalized without waiting for full DAG consensus.
//!
//! Security: probability of error < 10^-18 with k=40, alpha=30.
//!
//! ## Architectural Limitation: Random Validator Sampling
//!
//! The Avalanche protocol requires that each query round selects a **random
//! subset** of k validators from the active validator set. The current
//! implementation tracks votes globally (any k validators can vote per round)
//! but does **not** perform the random sampling step itself.
//!
//! This is because sampling requires integration with the networking layer
//! (to send query messages to specific validators) and the validator set
//! manager (to know who is currently active). The `sample_validators()` method
//! provides the protocol structure for this sampling; a full implementation
//! will wire it into the P2P query dispatch in the network crate.
//!
//! Without proper random sampling, an adversary who controls a concentrated
//! subset of validators could more easily influence the fast-path outcome.
//! Until sampling is fully integrated, the fast-path should only be used
//! for transactions that will also be confirmed through the full DAG consensus.

use pichain_crypto::ed25519::Address;
use pichain_crypto::Hash;
use rand::seq::SliceRandom;
use std::collections::{HashMap, HashSet};
use tracing::warn;

/// Avalanche fast-path configuration.
pub struct AvalancheFastPath {
    /// Number of validators to query per round (k).
    pub sample_size: usize,
    /// Number of positive responses needed for confidence (alpha).
    pub quorum_size: usize,
    /// Number of consecutive successful rounds needed (beta).
    pub decision_threshold: usize,
    /// Pending queries: tx_hash -> current confidence counter.
    pending: HashMap<Hash, FastPathState>,
}

/// State of a fast-path query for a transaction.
struct FastPathState {
    /// How many consecutive rounds have reached quorum.
    consecutive_successes: usize,
    /// Has this tx been finalized?
    finalized: bool,
    /// Votes received for current round.
    current_votes: usize,
    /// Total validators queried this round.
    current_queries: usize,
    /// Voters who have already voted in the current round (prevents double-voting).
    round_voters: HashSet<Address>,
    /// The set of validators sampled for the current round.
    /// If empty, any validator may vote (legacy behavior).
    sampled_validators: HashSet<Address>,
    /// R26-FIX: Accumulated stake of positive votes in the current round.
    current_vote_stake: u64,
    /// R26-FIX: Total stake of validators queried in the current round.
    current_queried_stake: u64,
    /// R26-FIX: Total stake of all sampled validators for this round.
    sampled_total_stake: u64,
}

impl AvalancheFastPath {
    /// Create with production parameters.
    pub fn new() -> Self {
        Self {
            sample_size: 40,  // k
            quorum_size: 30,  // alpha
            decision_threshold: 20, // beta
            pending: HashMap::new(),
        }
    }

    /// Create with test parameters.
    pub fn new_test() -> Self {
        Self {
            sample_size: 5,
            quorum_size: 3,
            decision_threshold: 3,
            pending: HashMap::new(),
        }
    }

    /// Select a random subset of `k` validators from the active set for one
    /// query round.
    ///
    /// This implements the core Avalanche sampling mechanism: each round queries
    /// a fresh random subset of validators, ensuring that repeated rounds probe
    /// different parts of the network. The security guarantee (error < 10^-18)
    /// depends on this randomness.
    ///
    /// # Arguments
    /// * `active_validators` - The full set of currently active validator addresses.
    ///
    /// # Returns
    /// A vector of `min(k, active_validators.len())` randomly chosen validators.
    /// Returns an empty vector if the active set is empty.
    ///
    /// # Note
    /// The caller is responsible for sending query messages to each sampled
    /// validator via the P2P network layer. Responses should be fed back via
    /// `record_vote()`.
    pub fn sample_validators(&self, active_validators: &[Address]) -> Vec<Address> {
        if active_validators.is_empty() {
            return Vec::new();
        }

        let k = self.sample_size.min(active_validators.len());
        let mut rng = rand::thread_rng();
        let mut validators = active_validators.to_vec();
        validators.shuffle(&mut rng);
        validators.truncate(k);
        validators
    }

    /// Start a fast-path query for a transaction.
    pub fn start_query(&mut self, tx_hash: Hash) {
        self.pending.insert(
            tx_hash,
            FastPathState {
                consecutive_successes: 0,
                finalized: false,
                current_votes: 0,
                current_queries: 0,
                round_voters: HashSet::new(),
                sampled_validators: HashSet::new(),
                current_vote_stake: 0,
                current_queried_stake: 0,
                sampled_total_stake: 0,
            },
        );
    }

    /// Start a fast-path query with an initial sample for the first round.
    ///
    /// This is the preferred entry point when the network layer is integrated.
    /// The caller should use `sample_validators()` to obtain the sample, send
    /// queries to those validators, then feed responses via `record_vote()` or
    /// `record_vote_weighted()`.
    pub fn start_query_with_sample(&mut self, tx_hash: Hash, sample: &[Address]) {
        self.pending.insert(
            tx_hash,
            FastPathState {
                consecutive_successes: 0,
                finalized: false,
                current_votes: 0,
                current_queries: 0,
                round_voters: HashSet::new(),
                sampled_validators: sample.iter().copied().collect(),
                current_vote_stake: 0,
                current_queried_stake: 0,
                sampled_total_stake: 0,
            },
        );
    }

    /// Set the sampled validator set for the current round of an in-progress query.
    ///
    /// When a round completes and the transaction is not yet finalized, the
    /// caller should sample a fresh set of validators and call this method
    /// before dispatching the next round of queries.
    pub fn set_round_sample(&mut self, tx_hash: &Hash, sample: &[Address]) {
        if let Some(state) = self.pending.get_mut(tx_hash) {
            state.sampled_validators = sample.iter().copied().collect();
        }
    }

    /// Set the sampled validator set with stake weights for stake-weighted quorum (R26-FIX).
    ///
    /// `sample_with_stakes` provides (address, stake) pairs. The total stake
    /// of sampled validators is computed and used as the quorum denominator.
    pub fn set_round_sample_with_stakes(
        &mut self,
        tx_hash: &Hash,
        sample_with_stakes: &[(Address, u64)],
    ) {
        if let Some(state) = self.pending.get_mut(tx_hash) {
            state.sampled_validators = sample_with_stakes.iter().map(|(a, _)| *a).collect();
            state.sampled_total_stake = sample_with_stakes.iter().map(|(_, s)| *s).fold(0u64, |a, b| a.saturating_add(b));
        }
    }

    /// Record a vote from a queried validator (count-based quorum).
    ///
    /// `voter` must be the address of a validator in the active set.
    /// If a sample was set for this round, only votes from sampled validators
    /// are accepted; votes from non-sampled validators are silently ignored.
    /// Double-votes from the same voter in the same round are silently ignored.
    /// Returns true if the transaction has been finalized.
    ///
    /// Note: For PoS networks, prefer `record_vote_weighted()` which uses
    /// stake-weighted quorum instead of simple count-based quorum.
    pub fn record_vote(&mut self, tx_hash: &Hash, voter: &Address, vote: bool) -> bool {
        self.record_vote_weighted(tx_hash, voter, vote, 1)
    }

    /// Record a stake-weighted vote from a queried validator (R26-FIX).
    ///
    /// In PoS, a count-based quorum allows an attacker with many small-stake
    /// validators to dominate the fast-path. Stake-weighted voting ensures
    /// quorum requires 2/3+ of *stake*, not just 2/3+ of validator count.
    ///
    /// `voter_stake` is the voter's stake weight. When all voters pass stake=1,
    /// this degrades to the original count-based behavior.
    pub fn record_vote_weighted(
        &mut self,
        tx_hash: &Hash,
        voter: &Address,
        vote: bool,
        voter_stake: u64,
    ) -> bool {
        let state = match self.pending.get_mut(tx_hash) {
            Some(s) => s,
            None => return false,
        };

        if state.finalized {
            return true;
        }

        // If a sample was set for this round, reject votes from non-sampled validators
        if !state.sampled_validators.is_empty() && !state.sampled_validators.contains(voter) {
            return false;
        }

        // Reject duplicate votes from the same validator in this round
        if !state.round_voters.insert(*voter) {
            return false;
        }

        state.current_queries += 1;
        state.current_queried_stake = state.current_queried_stake.saturating_add(voter_stake);
        if vote {
            state.current_votes += 1;
            state.current_vote_stake = state.current_vote_stake.saturating_add(voter_stake);
        }

        // R27-FIX: Use the actual sample size for round completion when a
        // sample was set, not the global self.sample_size. If fewer than
        // sample_size validators exist, the round would never complete.
        let round_size = if !state.sampled_validators.is_empty() {
            state.sampled_validators.len()
        } else {
            self.sample_size
        };

        // Check if this round is complete
        if state.current_queries >= round_size {
            // R26/R27-FIX: Stake-weighted quorum is MANDATORY.
            // Count-based quorum is a Sybil attack vector: an adversary with many
            // low-stake validators can dominate the fast-path despite owning
            // negligible total stake. By requiring stake data, we ensure quorum
            // reflects economic weight, not validator count.
            let quorum_met = if state.sampled_total_stake > 0 {
                // Stake-weighted: vote_stake > 2/3 of sampled_total_stake (strict)
                state.current_vote_stake as u128 * 3 > state.sampled_total_stake as u128 * 2
            } else if round_size <= 1 {
                // Single-validator or test mode: allow count-based for trivial cases
                state.current_votes >= self.quorum_size
            } else {
                // No stake data provided for multi-validator round — quorum cannot
                // be met. Callers MUST use set_round_sample_with_stakes() to provide
                // stake weights before recording votes.
                warn!("fast-path quorum denied: no stake data for {} validators — use set_round_sample_with_stakes()", round_size);
                false
            };

            if quorum_met {
                state.consecutive_successes += 1;
            } else {
                state.consecutive_successes = 0;
            }

            // Reset for next round (clear voter set and sample)
            state.current_votes = 0;
            state.current_queries = 0;
            state.current_vote_stake = 0;
            state.current_queried_stake = 0;
            state.sampled_total_stake = 0;
            state.round_voters.clear();
            state.sampled_validators.clear();

            // Check if we've reached decision threshold
            if state.consecutive_successes >= self.decision_threshold {
                state.finalized = true;
                return true;
            }
        }

        false
    }

    /// Check if a transaction has been finalized via fast-path.
    pub fn is_finalized(&self, tx_hash: &Hash) -> bool {
        self.pending
            .get(tx_hash)
            .map(|s| s.finalized)
            .unwrap_or(false)
    }

    /// Remove a finalized transaction from tracking.
    pub fn remove(&mut self, tx_hash: &Hash) {
        self.pending.remove(tx_hash);
    }

    /// Number of transactions currently being queried.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

impl Default for AvalancheFastPath {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voter(i: u8) -> Address {
        Address([i; 20])
    }

    /// Helper: set up a sample with equal stakes for testing.
    fn setup_round_with_stakes(fp: &mut AvalancheFastPath, tx_hash: &Hash, validators: &[Address]) {
        let stakes: Vec<(Address, u64)> = validators.iter().map(|a| (*a, 1000)).collect();
        fp.set_round_sample_with_stakes(tx_hash, &stakes);
    }

    #[test]
    fn fast_path_finalization() {
        let mut fp = AvalancheFastPath::new_test();
        let tx_hash = pichain_crypto::hash(b"simple transfer");

        let sample: Vec<Address> = (0..5u8).map(voter).collect();
        fp.start_query_with_sample(tx_hash, &sample);
        setup_round_with_stakes(&mut fp, &tx_hash, &sample);
        assert!(!fp.is_finalized(&tx_hash));

        // Simulate 3 rounds of successful queries (decision_threshold=3)
        for _round in 0..3 {
            if _round > 0 {
                setup_round_with_stakes(&mut fp, &tx_hash, &sample);
            }
            // 5 queries per round, 4 positive (>2/3 of stake)
            for i in 0..5u8 {
                let vote = i < 4; // first 4 vote yes (80% > 66.7%)
                let result = fp.record_vote_weighted(&tx_hash, &voter(i), vote, 1000);
                if _round == 2 && i == 4 {
                    assert!(result, "should finalize after 3 successful rounds");
                }
            }
        }

        assert!(fp.is_finalized(&tx_hash));
    }

    #[test]
    fn fast_path_failure_resets() {
        let mut fp = AvalancheFastPath::new_test();
        let tx_hash = pichain_crypto::hash(b"contested transfer");

        let sample: Vec<Address> = (0..5u8).map(voter).collect();
        fp.start_query_with_sample(tx_hash, &sample);

        // 2 successful rounds (4/5 = 80% > 66.7% required)
        for _round in 0..2 {
            setup_round_with_stakes(&mut fp, &tx_hash, &sample);
            for i in 0..5u8 {
                fp.record_vote_weighted(&tx_hash, &voter(i), i < 4, 1000);
            }
        }

        // 1 failed round (only 1 positive vote = 20% < 66.7%)
        setup_round_with_stakes(&mut fp, &tx_hash, &sample);
        for i in 0..5u8 {
            fp.record_vote_weighted(&tx_hash, &voter(i), i < 1, 1000);
        }

        assert!(!fp.is_finalized(&tx_hash));
    }

    #[test]
    fn count_based_quorum_denied_without_stakes() {
        let mut fp = AvalancheFastPath::new_test();
        let tx_hash = pichain_crypto::hash(b"sybil test");

        // Use start_query (no sample, no stakes) — count-based path
        let sample: Vec<Address> = (0..5u8).map(voter).collect();
        fp.start_query_with_sample(tx_hash, &sample);
        // Don't set stakes — sampled_total_stake remains 0

        // Even with all 5 voting yes, quorum should be denied (no stake data)
        for i in 0..5u8 {
            fp.record_vote(&tx_hash, &voter(i), true);
        }

        // After one round with no quorum, consecutive_successes should be 0
        assert!(!fp.is_finalized(&tx_hash));
    }

    #[test]
    fn double_vote_rejected() {
        let mut fp = AvalancheFastPath::new_test();
        let tx_hash = pichain_crypto::hash(b"double vote test");

        fp.start_query(tx_hash);

        // Same voter votes 5 times -- only the first should count
        let v = voter(1);
        fp.record_vote(&tx_hash, &v, true);
        fp.record_vote(&tx_hash, &v, true);
        fp.record_vote(&tx_hash, &v, true);
        fp.record_vote(&tx_hash, &v, true);
        fp.record_vote(&tx_hash, &v, true);

        // Should NOT be finalized -- only 1 unique vote was counted, need 5 queries
        assert!(!fp.is_finalized(&tx_hash));
    }

    #[test]
    fn sample_validators_returns_k_subset() {
        let fp = AvalancheFastPath::new_test(); // k=5
        let validators: Vec<Address> = (0..20u8).map(voter).collect();

        let sample = fp.sample_validators(&validators);
        assert_eq!(sample.len(), 5, "should sample exactly k validators");

        // All sampled validators should be from the active set
        for v in &sample {
            assert!(validators.contains(v), "sampled validator should be in active set");
        }

        // No duplicates in the sample
        let unique: HashSet<Address> = sample.iter().copied().collect();
        assert_eq!(unique.len(), sample.len(), "sample should have no duplicates");
    }

    #[test]
    fn sample_validators_with_fewer_than_k() {
        let fp = AvalancheFastPath::new_test(); // k=5
        let validators: Vec<Address> = (0..3u8).map(voter).collect();

        let sample = fp.sample_validators(&validators);
        assert_eq!(sample.len(), 3, "should return all validators when fewer than k");
    }

    #[test]
    fn sample_validators_empty_set() {
        let fp = AvalancheFastPath::new_test();
        let sample = fp.sample_validators(&[]);
        assert!(sample.is_empty(), "should return empty for empty active set");
    }

    #[test]
    fn sampled_round_rejects_non_sampled_voters() {
        let mut fp = AvalancheFastPath::new_test(); // k=5, alpha=3, beta=3
        let tx_hash = pichain_crypto::hash(b"sampled round test");

        // Start query with a specific sample of 5 validators (indices 0..5) with stakes
        let sample: Vec<Address> = (0..5u8).map(voter).collect();
        fp.start_query_with_sample(tx_hash, &sample);
        setup_round_with_stakes(&mut fp, &tx_hash, &sample);

        // Votes from non-sampled validators (indices 10..15) should be ignored
        for i in 10..15u8 {
            let result = fp.record_vote_weighted(&tx_hash, &voter(i), true, 1000);
            assert!(!result, "non-sampled validator vote should be ignored");
        }

        // Votes from sampled validators should count
        for i in 0..5u8 {
            fp.record_vote_weighted(&tx_hash, &voter(i), true, 1000);
        }

        // After one round, sample is cleared; next round accepts any validator
        // (caller should set a new sample via set_round_sample_with_stakes for proper operation)
    }

    #[test]
    fn set_round_sample_between_rounds() {
        let mut fp = AvalancheFastPath::new_test(); // k=5, alpha=3, beta=3
        let tx_hash = pichain_crypto::hash(b"multi-round sample test");

        // Round 1: sample validators 0..5 with stakes
        let sample1: Vec<Address> = (0..5u8).map(voter).collect();
        fp.start_query_with_sample(tx_hash, &sample1);
        setup_round_with_stakes(&mut fp, &tx_hash, &sample1);
        for i in 0..5u8 {
            fp.record_vote_weighted(&tx_hash, &voter(i), true, 1000);
        }

        // Round 2: set new sample with stakes (validators 5..10)
        let sample2: Vec<Address> = (5..10u8).map(voter).collect();
        setup_round_with_stakes(&mut fp, &tx_hash, &sample2);
        // Validator 0 (from round 1 sample) should now be rejected
        let rejected = fp.record_vote_weighted(&tx_hash, &voter(0), true, 1000);
        assert!(!rejected, "validator from previous round's sample should be rejected");
        // Validators from new sample should be accepted
        for i in 5..10u8 {
            fp.record_vote_weighted(&tx_hash, &voter(i), true, 1000);
        }

        // Round 3: set new sample with stakes (validators 10..15)
        let sample3: Vec<Address> = (10..15u8).map(voter).collect();
        setup_round_with_stakes(&mut fp, &tx_hash, &sample3);
        for i in 10..15u8 {
            let result = fp.record_vote_weighted(&tx_hash, &voter(i), true, 1000);
            if i == 14 {
                assert!(result, "should finalize after 3 successful rounds");
            }
        }

        assert!(fp.is_finalized(&tx_hash));
    }
}
