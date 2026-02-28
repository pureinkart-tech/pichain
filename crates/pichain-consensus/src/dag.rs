//! Narwhal-style DAG mempool and Bullshark ordering.
//!
//! All validators contribute transaction data simultaneously (no single leader bottleneck).
//! Certificates form a DAG. Bullshark provides total ordering with zero additional messages.

use pichain_crypto::ed25519::Address;
use pichain_crypto::Hash;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::ConsensusError;

/// Evidence of equivocation: a validator produced two different certificates
/// in the same round. This is cryptographic proof of misbehavior for slashing.
#[derive(Clone, Debug)]
pub struct EquivocationEvidence {
    /// The validator who equivocated.
    pub author: Address,
    /// The DAG round where equivocation occurred.
    pub round: u64,
    /// The first (existing) certificate.
    pub cert_a: Certificate,
    /// The second (conflicting) certificate.
    pub cert_b: Certificate,
}

/// A DAG header proposed by a validator.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Header {
    /// Author (validator address).
    pub author: Address,
    /// DAG round number.
    pub round: u64,
    /// References to certificates from the previous round (must have 2f+1).
    pub parents: Vec<Hash>,
    /// References to transaction batches (certificates of availability).
    pub payload: Vec<Hash>,
    /// Timestamp (milliseconds).
    pub timestamp_ms: u64,
}

impl Header {
    /// Compute a deterministic hash of this header using canonical binary encoding.
    /// This avoids serde_json's non-deterministic field ordering and formatting
    /// which would break consensus across different nodes/platforms.
    pub fn hash(&self) -> Hash {
        let mut bytes = Vec::with_capacity(128);
        bytes.extend_from_slice(&self.author.0);
        bytes.extend_from_slice(&self.round.to_le_bytes());
        // Length-prefix parent and payload lists to prevent ambiguous concatenation.
        // Without this, different parent/payload splits could produce the same hash.
        bytes.extend_from_slice(&(self.parents.len() as u64).to_le_bytes());
        for parent in &self.parents {
            bytes.extend_from_slice(parent.as_bytes());
        }
        bytes.extend_from_slice(&(self.payload.len() as u64).to_le_bytes());
        for payload_hash in &self.payload {
            bytes.extend_from_slice(payload_hash.as_bytes());
        }
        bytes.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        pichain_crypto::hash(&bytes)
    }
}

/// A certified header — a header with 2f+1 validator signatures.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Certificate {
    /// The header being certified.
    pub header: Header,
    /// Hash of the header (cached).
    pub digest: Hash,
    /// Validators who signed this certificate (by address).
    pub signers: Vec<Address>,
    /// Aggregate BLS signature (serialized).
    pub aggregate_signature: Vec<u8>,
}

impl Certificate {
    /// Create a new certificate (in production, would verify aggregate BLS sig).
    pub fn new(header: Header, signers: Vec<Address>, aggregate_signature: Vec<u8>) -> Self {
        let digest = header.hash();
        Self {
            header,
            digest,
            signers,
            aggregate_signature,
        }
    }

    pub fn round(&self) -> u64 {
        self.header.round
    }

    pub fn author(&self) -> Address {
        self.header.author
    }
}

/// A single round in the DAG.
#[derive(Clone, Debug, Default)]
pub struct DagRound {
    /// Certificates in this round, keyed by author.
    pub certificates: HashMap<Address, Certificate>,
}

impl DagRound {
    /// Insert a certificate into this round.
    /// Returns `Ok(())` on success, or `Err(existing_cert)` if this author
    /// already has a certificate in this round (equivocation evidence).
    pub fn insert(&mut self, cert: Certificate) -> Result<(), Certificate> {
        use std::collections::hash_map::Entry;
        match self.certificates.entry(cert.author()) {
            Entry::Occupied(existing) => Err(existing.get().clone()), // equivocation
            Entry::Vacant(v) => {
                v.insert(cert);
                Ok(())
            }
        }
    }

    pub fn get(&self, author: &Address) -> Option<&Certificate> {
        self.certificates.get(author)
    }

    pub fn len(&self) -> usize {
        self.certificates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.certificates.is_empty()
    }

    pub fn certificate_hashes(&self) -> Vec<Hash> {
        self.certificates.values().map(|c| c.digest).collect()
    }
}

/// The DAG mempool — stores all rounds of certificates.
pub struct DagMempool {
    /// All rounds, keyed by round number.
    rounds: HashMap<u64, DagRound>,
    /// Hash-indexed certificate lookup for O(1) find_certificate().
    cert_index: HashMap<Hash, Certificate>,
    /// Current round.
    current_round: u64,
    /// Committed round (Bullshark has ordered up to this round).
    /// `None` means no round has been committed yet, distinguishing from
    /// the valid case of having committed round 0.
    committed_round: Option<u64>,
    /// Number of validators.
    committee_size: usize,
    /// Collected equivocation evidence for slashing.
    equivocation_evidence: Vec<EquivocationEvidence>,
    /// Cache of verified proof-of-possession for BLS keys.
    /// Once a validator's PoP is verified, we don't need to check it again.
    verified_pops: HashSet<Address>,
    /// Chain identifier to prevent cross-network PoP replay attacks.
    /// A PoP proved on testnet should not be valid on mainnet.
    chain_id: String,
    /// Numeric chain ID for BLS certificate signature domain separation.
    /// Certificates signed on devnet (31415) are invalid on mainnet (314159).
    numeric_chain_id: u64,
    /// Validator stake map for stake-weighted quorum (address → stake).
    validator_stakes: HashMap<Address, u64>,
    /// Total stake across all validators.
    total_stake: u64,
    /// Stake-weighted quorum threshold (2/3+1 of total stake).
    quorum_stake: u64,
}

impl DagMempool {
    pub fn new(committee_size: usize) -> Self {
        Self::new_with_chain_id(committee_size, "pichain-mainnet-1")
    }

    pub fn new_with_chain_id(committee_size: usize, chain_id: &str) -> Self {
        Self {
            rounds: HashMap::new(),
            cert_index: HashMap::new(),
            current_round: 0,
            committed_round: None,
            committee_size,
            equivocation_evidence: Vec::new(),
            verified_pops: HashSet::new(),
            chain_id: chain_id.to_string(),
            numeric_chain_id: 0,
            validator_stakes: HashMap::new(),
            total_stake: 0,
            quorum_stake: 0,
        }
    }

    /// Set the numeric chain ID for BLS certificate signature domain separation.
    pub fn set_numeric_chain_id(&mut self, chain_id: u64) {
        self.numeric_chain_id = chain_id;
    }

    /// Get the chain ID.
    pub fn chain_id(&self) -> &str {
        &self.chain_id
    }

    /// Update the committee size (used during epoch transitions when validator set changes).
    pub fn update_committee_size(&mut self, new_size: usize) {
        self.committee_size = new_size;
    }

    /// Update the validator stake map (used during epoch transitions).
    /// Also updates the stake-weighted quorum threshold.
    pub fn update_stakes(&mut self, stakes: HashMap<Address, u64>) {
        self.total_stake = stakes.values().copied().fold(0u64, |acc, s| acc.saturating_add(s));
        self.quorum_stake = (self.total_stake as u128 * 2 / 3 + 1) as u64;
        self.validator_stakes = stakes;
    }

    /// Get the stake-weighted quorum threshold.
    pub fn quorum_stake(&self) -> u64 {
        self.quorum_stake
    }

    /// Get the total stake.
    pub fn total_stake(&self) -> u64 {
        self.total_stake
    }

    /// Get the current committee size.
    pub fn committee_size(&self) -> usize {
        self.committee_size
    }

    /// Register a validator's BLS key as having a verified proof-of-possession.
    /// Call this during node startup when validators register with their PoP.
    pub fn register_verified_pop(&mut self, validator: Address) {
        self.verified_pops.insert(validator);
    }

    /// Clear all verified proof-of-possession entries.
    /// Must be called during epoch transitions before re-registering the new
    /// validator set's PoPs, to prevent stale validators from the old epoch
    /// from being accepted as valid signers.
    pub fn reset_verified_pops(&mut self) {
        self.verified_pops.clear();
    }

    pub fn current_round(&self) -> u64 {
        self.current_round
    }

    pub fn committed_round(&self) -> Option<u64> {
        self.committed_round
    }

    /// Insert a certificate into the DAG.
    ///
    /// If `validator_keys` is provided, the BLS aggregate signature is verified
    /// against the registered validator public keys. Pass `None` to skip
    /// key-based verification (self-signed inserts, tests, single-node mode).
    pub fn insert_certificate(
        &mut self,
        cert: Certificate,
        validator_keys: Option<&HashMap<Address, pichain_crypto::bls::BlsPublicKey>>,
    ) -> Result<(), ConsensusError> {
        let round = cert.round();

        // Verify the digest actually matches the header hash — never trust pre-computed values.
        let computed_digest = cert.header.hash();
        if computed_digest != cert.digest {
            return Err(ConsensusError::InvalidCertificate(format!(
                "certificate digest mismatch: claimed {}, computed {}",
                cert.digest, computed_digest
            )));
        }

        // Verify it has enough unique parent references (2f+1)
        if round > 0 {
            let required_parents = self.quorum_threshold();
            let unique_parents: std::collections::HashSet<_> =
                cert.header.parents.iter().collect();
            if unique_parents.len() < required_parents {
                return Err(ConsensusError::InsufficientVotes {
                    got: unique_parents.len(),
                    need: required_parents,
                });
            }

            // Verify all parent hashes reference certificates that exist in the DAG,
            // enforce that parents are from the previous round (round - 1),
            // and enforce timestamp monotonicity (cert must be >= all parent timestamps).
            let expected_parent_round = round - 1;
            let mut max_parent_ts: u64 = 0;
            for parent_hash in &cert.header.parents {
                match self.cert_index.get(parent_hash) {
                    Some(parent_cert) => {
                        // Verify parent is from the immediately preceding round.
                        // This prevents cross-round references that could break
                        // DAG structure and Bullshark ordering assumptions.
                        if parent_cert.round() != expected_parent_round {
                            return Err(ConsensusError::InvalidCertificate(format!(
                                "parent certificate is from round {}, expected round {}",
                                parent_cert.round(), expected_parent_round
                            )));
                        }
                        max_parent_ts = max_parent_ts.max(parent_cert.header.timestamp_ms);
                    }
                    None => {
                        return Err(ConsensusError::InvalidCertificate(format!(
                            "parent certificate {} not found in DAG",
                            parent_hash.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>()
                        )));
                    }
                }
            }
            if cert.header.timestamp_ms < max_parent_ts {
                return Err(ConsensusError::InvalidCertificate(format!(
                    "certificate timestamp {}ms is before parent timestamp {}ms",
                    cert.header.timestamp_ms, max_parent_ts
                )));
            }
        }

        // Verify BLS aggregate signature
        if let Some(keys) = validator_keys {
            // Multi-validator mode: MANDATORY signature verification
            if cert.aggregate_signature.is_empty() || cert.signers.is_empty() {
                return Err(ConsensusError::InvalidCertificate(
                    "certificate must have signers and aggregate signature in multi-validator mode".to_string()
                ));
            }

            if cert.aggregate_signature.len() != 96 {
                return Err(ConsensusError::InvalidCertificate(
                    "BLS aggregate signature must be 96 bytes".to_string()
                ));
            }

            // Reject duplicate signers — prevents double-counting in aggregate verification
            let unique_signers: HashSet<_> = cert.signers.iter().collect();
            if unique_signers.len() != cert.signers.len() {
                return Err(ConsensusError::InvalidCertificate(format!(
                    "duplicate signers detected: {} unique out of {}",
                    unique_signers.len(), cert.signers.len()
                )));
            }

            // R29-FIX: Verify author is among the signers
            if !cert.signers.contains(&cert.author()) {
                return Err(ConsensusError::InvalidCertificate(
                    "certificate author must be among the signers".to_string()
                ));
            }

            // Verify signer quorum: stake-weighted if stakes configured, else count-based
            let signer_addrs: Vec<&Address> = cert.signers.iter().collect();
            if !self.has_stake_quorum(&signer_addrs) {
                let _got_stake: u64 = cert.signers.iter()
                    .map(|a| self.validator_stakes.get(a).copied().unwrap_or(0))
                    .sum();
                return Err(ConsensusError::InsufficientVotes {
                    got: cert.signers.len(),
                    need: self.quorum_threshold(),
                });
            }

            let sig_bytes: [u8; 96] = cert.aggregate_signature[..96].try_into()
                .map_err(|_| ConsensusError::InvalidCertificate(
                    "malformed BLS aggregate signature".to_string()
                ))?;
            let bls_sig = pichain_crypto::bls::BlsSignature::from_bytes(&sig_bytes)
                .map_err(|_| ConsensusError::InvalidCertificate(
                    "invalid BLS aggregate signature (not on curve)".to_string()
                ))?;

            // Domain-separated message: prepend "CERT_HDR" + chain_id to prevent both
            // cross-context and cross-chain signature reuse. A certificate from devnet
            // (chain_id=31415) cannot be replayed on mainnet (chain_id=314159).
            let mut message = Vec::with_capacity(48);
            message.extend_from_slice(b"CERT_HDR");
            message.extend_from_slice(&self.numeric_chain_id.to_le_bytes());
            message.extend_from_slice(cert.digest.as_bytes());
            let signer_pks: Vec<&pichain_crypto::bls::BlsPublicKey> = cert.signers.iter()
                .filter_map(|addr| keys.get(addr))
                .collect();

            if signer_pks.len() != cert.signers.len() {
                return Err(ConsensusError::InvalidCertificate(format!(
                    "unknown signers: {} of {} not in validator set",
                    cert.signers.len() - signer_pks.len(),
                    cert.signers.len(),
                )));
            }

            // Verify the aggregate BLS signature
            pichain_crypto::bls::AggregateSignature::verify(&signer_pks, &message, &bls_sig)
                .map_err(|_| ConsensusError::InvalidCertificate(
                    "BLS aggregate signature verification failed".to_string()
                ))?;

            // Verify all signers have proven possession of their BLS keys.
            // This prevents rogue key attacks where an attacker crafts a key
            // K_atk = -(K1+K2+...) to forge aggregate signatures.
            for signer in &cert.signers {
                if !self.verified_pops.contains(signer) {
                    return Err(ConsensusError::InvalidCertificate(format!(
                        "signer {} has not provided proof-of-possession",
                        signer
                    )));
                }
            }
        } else if self.committee_size > 1 {
            // SECURITY: In multi-validator mode, validator_keys=None is never acceptable.
            // This prevents unsigned certificate injection during single-to-multi transitions.
            return Err(ConsensusError::InvalidCertificate(
                "validator_keys required in multi-validator mode (committee_size > 1)".to_string()
            ));
        } else if !cert.aggregate_signature.is_empty() && !cert.signers.is_empty() {
            // Single-validator mode: still verify signature format if present
            if cert.aggregate_signature.len() != 96 {
                return Err(ConsensusError::InvalidCertificate(
                    "BLS aggregate signature must be 96 bytes".to_string()
                ));
            }
        }
        // Note: In single-validator mode (committee_size=1, validator_keys=None),
        // empty sigs are allowed. This path is ONLY used for self-proposed certs
        // in development/test or single-node genesis environments.

        // Timestamp bounds check: reject certificates too far in the future
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        const MAX_TIMESTAMP_DRIFT_MS: u64 = 30_000; // 30 seconds
        if cert.header.timestamp_ms > now_ms.saturating_add(MAX_TIMESTAMP_DRIFT_MS) {
            return Err(ConsensusError::InvalidCertificate(format!(
                "certificate timestamp {}ms is too far in the future (now: {}ms, max drift: {}ms)",
                cert.header.timestamp_ms, now_ms, MAX_TIMESTAMP_DRIFT_MS
            )));
        }

        let author = cert.author();
        let digest = cert.digest;
        let dag_round = self.rounds.entry(round).or_default();
        if let Err(existing) = dag_round.insert(cert.clone()) {
            // Collect equivocation evidence for slashing (two different certs from same author in same round)
            // R32-FIX: Cap evidence vec to prevent unbounded memory growth from
            // an attacker flooding the network with equivocating certificates.
            if existing.digest != cert.digest && self.equivocation_evidence.len() < 1024 {
                self.equivocation_evidence.push(EquivocationEvidence {
                    author,
                    round,
                    cert_a: existing,
                    cert_b: cert,
                });
            }
            return Err(ConsensusError::InvalidCertificate(format!(
                "duplicate certificate from {} in round {}",
                author, round
            )));
        }
        // Index by hash for O(1) lookups in causal history collection
        self.cert_index.insert(digest, cert);

        if round > self.current_round {
            self.current_round = round;
        }

        Ok(())
    }

    /// Get a specific round.
    pub fn get_round(&self, round: u64) -> Option<&DagRound> {
        self.rounds.get(&round)
    }

    /// Bullshark ordering: try to commit the leader of an even round.
    ///
    /// A leader is committed if their certificate in the even round is
    /// referenced by 2f+1 certificates in the next round.
    pub fn try_commit(&mut self, round: u64, leader: &Address) -> Option<Vec<Certificate>> {
        // Bullshark only commits on even rounds
        if round % 2 != 0 {
            return None;
        }

        let leader_cert = self.rounds.get(&round)?.get(leader)?.clone();
        let leader_hash = leader_cert.digest;

        // Stake-weighted commit check: sum stake of next-round cert authors
        // that reference the leader. Falls back to count-based if no stakes.
        let next_round = self.rounds.get(&(round + 1))?;
        let referencing_authors: Vec<&Address> = next_round
            .certificates
            .values()
            .filter(|c| c.header.parents.contains(&leader_hash))
            .map(|c| &c.header.author)
            .collect();

        if !self.has_stake_quorum(&referencing_authors) {
            return None;
        }

        // Commit! Collect all causally preceding certificates.
        let mut committed = Vec::new();
        let mut visited = HashSet::new();
        self.collect_causal_history(&leader_cert, &mut committed, &mut visited);

        // Sort causal history deterministically by (round, author address) to ensure
        // all nodes produce the same commit ordering regardless of BFS traversal order.
        committed.sort_by(|a, b| {
            a.round()
                .cmp(&b.round())
                .then_with(|| a.author().0.cmp(&b.author().0))
        });

        committed.push(leader_cert);

        self.committed_round = Some(round);

        Some(committed)
    }

    /// Remove rounds that are fully committed (below committed_round - 1 buffer).
    /// Called externally after the commit loop completes to avoid pruning causal
    /// history that subsequent commits in the same loop iteration may still need.
    pub fn prune_committed_rounds(&mut self) {
        let committed = match self.committed_round {
            Some(r) => r,
            None => return, // Nothing committed yet, nothing to prune
        };
        let cutoff = committed.saturating_sub(1);
        let stale_rounds: Vec<u64> = self.rounds.keys()
            .filter(|&&r| r < cutoff)
            .copied()
            .collect();
        for round in stale_rounds {
            if let Some(dag_round) = self.rounds.remove(&round) {
                for cert in dag_round.certificates.values() {
                    self.cert_index.remove(&cert.digest);
                }
            }
        }
    }

    /// Collect all causally preceding (uncommitted) certificates using iterative BFS.
    /// Avoids stack overflow on deep DAGs (recursive version blows stack at ~10K rounds).
    fn collect_causal_history(
        &self,
        cert: &Certificate,
        result: &mut Vec<Certificate>,
        visited: &mut HashSet<Hash>,
    ) {
        let mut queue = std::collections::VecDeque::new();
        for parent_hash in &cert.header.parents {
            if visited.insert(*parent_hash) {
                queue.push_back(*parent_hash);
            }
        }

        while let Some(hash) = queue.pop_front() {
            if let Some(parent_cert) = self.find_certificate(&hash) {
                // Include all uncommitted ancestors. If nothing has been committed yet
                // (committed_round == None), all ancestors are uncommitted.
                let dominated = match self.committed_round {
                    Some(cr) => parent_cert.round() > cr,
                    None => true,
                };
                if dominated {
                    // Enqueue this cert's parents for further traversal
                    for grandparent in &parent_cert.header.parents {
                        if visited.insert(*grandparent) {
                            queue.push_back(*grandparent);
                        }
                    }
                    result.push(parent_cert);
                }
            }
        }
    }

    /// Find a certificate by its hash using the digest index (O(1) lookup).
    fn find_certificate(&self, hash: &Hash) -> Option<Certificate> {
        self.cert_index.get(hash).cloned()
    }

    /// Number of certificates needed for quorum (2f+1).
    fn quorum_threshold(&self) -> usize {
        if self.committee_size == 0 {
            return usize::MAX; // impossible to meet -- no validators
        }
        self.committee_size * 2 / 3 + 1
    }

    /// Check if a set of addresses meets the stake-weighted quorum.
    /// Falls back to count-based quorum if no stakes are configured.
    pub(crate) fn has_stake_quorum(&self, addresses: &[&Address]) -> bool {
        if self.total_stake == 0 {
            // No stakes configured — fall back to count-based quorum
            let unique: std::collections::HashSet<_> = addresses.iter().collect();
            return unique.len() >= self.quorum_threshold();
        }
        // R27-FIX: Deduplicate addresses to prevent double-counting stake,
        // and use saturating_add to prevent overflow.
        let unique: std::collections::HashSet<_> = addresses.iter().collect();
        let stake_sum: u64 = unique.iter()
            .map(|a| self.validator_stakes.get(*a).copied().unwrap_or(0))
            .fold(0u64, |acc, s| acc.saturating_add(s));
        stake_sum >= self.quorum_stake
    }

    /// Get collected equivocation evidence (for forwarding to slashing logic).
    pub fn equivocation_evidence(&self) -> &[EquivocationEvidence] {
        &self.equivocation_evidence
    }

    /// Drain all collected equivocation evidence (moves it out for processing).
    pub fn drain_equivocation_evidence(&mut self) -> Vec<EquivocationEvidence> {
        std::mem::take(&mut self.equivocation_evidence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cert(author: u8, round: u64, parents: Vec<Hash>) -> Certificate {
        let header = Header {
            author: Address([author; 20]),
            round,
            parents,
            payload: vec![],
            timestamp_ms: 0,
        };
        Certificate::new(header, vec![Address([author; 20])], vec![])
    }

    #[test]
    fn dag_basic_operations() {
        // committee_size=1: single-validator mode allows unsigned certs (None keys)
        let mut dag = DagMempool::new(1);

        // Round 0: no parents needed
        let c0 = make_cert(1, 0, vec![]);
        dag.insert_certificate(c0, None).unwrap();
        assert_eq!(dag.current_round(), 0);
        assert_eq!(dag.get_round(0).unwrap().len(), 1);
    }

    #[test]
    fn dag_rejects_unsigned_in_multi_validator_mode() {
        // committee_size=4: multi-validator mode MUST provide validator_keys
        let mut dag = DagMempool::new(4);
        let c0 = make_cert(1, 0, vec![]);
        let result = dag.insert_certificate(c0, None);
        assert!(result.is_err(), "multi-validator DAG must reject None keys");
    }

    #[test]
    fn dag_requires_parents() {
        // committee_size=1 for unsigned cert insertion; round 1 still needs parents
        let mut dag = DagMempool::new(1);

        // Round 1 without parents should fail (needs at least 1 parent in round 0)
        let c1 = make_cert(1, 1, vec![]);
        assert!(dag.insert_certificate(c1, None).is_err());
    }

    #[test]
    fn equivocation_evidence_collected() {
        let mut dag = DagMempool::new(1);

        // First cert from author 1 in round 0
        let c0 = make_cert(1, 0, vec![]);
        dag.insert_certificate(c0.clone(), None).unwrap();

        // Second DIFFERENT cert from author 1 in round 0 (equivocation!)
        let header2 = Header {
            author: Address([1; 20]),
            round: 0,
            parents: vec![],
            payload: vec![pichain_crypto::hash(b"different_payload")],
            timestamp_ms: 0,
        };
        let c0_dup = Certificate::new(header2, vec![Address([1; 20])], vec![]);
        assert!(c0_dup.digest != c0.digest); // different content
        let result = dag.insert_certificate(c0_dup, None);
        assert!(result.is_err());

        // Evidence should be collected
        let evidence = dag.equivocation_evidence();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].author, Address([1; 20]));
        assert_eq!(evidence[0].round, 0);
    }

    #[test]
    fn dag_round_tracking() {
        let mut dag = DagMempool::new(1); // committee of 1 for simplicity

        let c0 = make_cert(1, 0, vec![]);
        dag.insert_certificate(c0.clone(), None).unwrap();

        let c1 = make_cert(1, 1, vec![c0.digest]);
        dag.insert_certificate(c1, None).unwrap();
        assert_eq!(dag.current_round(), 1);
    }

    fn make_cert_with_ts(author: u8, round: u64, parents: Vec<Hash>, timestamp_ms: u64) -> Certificate {
        let header = Header {
            author: Address([author; 20]),
            round,
            parents,
            payload: vec![],
            timestamp_ms,
        };
        Certificate::new(header, vec![Address([author; 20])], vec![])
    }

    #[test]
    fn timestamp_monotonicity_enforced() {
        let mut dag = DagMempool::new(1);

        // Round 0 at timestamp 1000
        let c0 = make_cert_with_ts(1, 0, vec![], 1000);
        dag.insert_certificate(c0.clone(), None).unwrap();

        // Round 1 with timestamp BEFORE parent — should be rejected
        let c1_bad = make_cert_with_ts(1, 1, vec![c0.digest], 500);
        let result = dag.insert_certificate(c1_bad, None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("before parent timestamp"), "unexpected error: {err_msg}");

        // Round 1 with valid timestamp >= parent — should succeed
        let c1_good = make_cert_with_ts(2, 1, vec![c0.digest], 1000);
        dag.insert_certificate(c1_good, None).unwrap();
    }

    #[test]
    fn duplicate_signers_rejected() {
        use pichain_crypto::bls::BlsKeypair;

        let mut dag = DagMempool::new(4);
        let bls_kp = BlsKeypair::generate();
        let addr = Address([1; 20]);

        // Register proof-of-possession for the validator
        dag.register_verified_pop(addr);

        // Build a key map with the validator
        let mut keys = HashMap::new();
        keys.insert(addr, bls_kp.public.clone());

        // Create cert with duplicate signer addresses
        let header = Header {
            author: addr,
            round: 0,
            parents: vec![],
            payload: vec![],
            timestamp_ms: 0,
        };
        let cert_digest = pichain_crypto::hash(&serde_json::to_vec(&header).unwrap_or_default());
        let sig = bls_kp.secret.sign(cert_digest.as_bytes());

        let cert = Certificate::new(
            header,
            vec![addr, addr, addr], // 3 duplicates of same signer
            sig.to_bytes().to_vec(),
        );

        let result = dag.insert_certificate(cert, Some(&keys));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("duplicate signers"), "unexpected error: {err_msg}");
    }

    #[test]
    fn digest_mismatch_rejected() {
        // Fix 184: Certificate with mismatched digest must be rejected
        let mut dag = DagMempool::new(1);

        let header = Header {
            author: Address([1; 20]),
            round: 0,
            parents: vec![],
            payload: vec![],
            timestamp_ms: 0,
        };

        // Create a certificate with a valid digest, then tamper with it
        let mut cert = Certificate::new(header, vec![Address([1; 20])], vec![]);
        // Overwrite the digest with a fake value
        cert.digest = pichain_crypto::hash(b"fake_digest");

        let result = dag.insert_certificate(cert, None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("digest mismatch"), "unexpected error: {err_msg}");
    }

    #[test]
    fn digest_mismatch_does_not_pollute_cert_index() {
        // Fix 184: A certificate with a bad digest must not end up in the cert_index
        let mut dag = DagMempool::new(1);

        let header = Header {
            author: Address([1; 20]),
            round: 0,
            parents: vec![],
            payload: vec![],
            timestamp_ms: 0,
        };
        let mut cert = Certificate::new(header, vec![Address([1; 20])], vec![]);
        let fake_digest = pichain_crypto::hash(b"fake_digest");
        cert.digest = fake_digest;

        let _ = dag.insert_certificate(cert, None);

        // The fake digest should not be indexed
        assert!(dag.find_certificate(&fake_digest).is_none());
        // The DAG round should remain empty
        assert!(dag.get_round(0).is_none());
    }

    #[test]
    fn update_committee_size() {
        // Fix 185: committee_size must be updatable for epoch transitions
        let mut dag = DagMempool::new(4);
        assert_eq!(dag.committee_size(), 4);
        assert_eq!(dag.quorum_threshold(), 3); // 4*2/3+1 = 3

        dag.update_committee_size(7);
        assert_eq!(dag.committee_size(), 7);
        assert_eq!(dag.quorum_threshold(), 5); // 7*2/3+1 = 5
    }

    #[test]
    fn chain_id_default() {
        // Fix 187: default chain_id should be "pichain-mainnet-1"
        let dag = DagMempool::new(4);
        assert_eq!(dag.chain_id(), "pichain-mainnet-1");
    }

    #[test]
    fn chain_id_custom() {
        // Fix 187: custom chain_id via new_with_chain_id
        let dag = DagMempool::new_with_chain_id(4, "pichain-testnet-1");
        assert_eq!(dag.chain_id(), "pichain-testnet-1");
        assert_eq!(dag.committee_size(), 4);
    }

    #[test]
    fn parent_from_wrong_round_rejected() {
        // Fix 204: Parent certificates must be from round - 1.
        let mut dag = DagMempool::new(1);

        // Insert certs in round 0 and round 1
        let c0 = make_cert(1, 0, vec![]);
        dag.insert_certificate(c0.clone(), None).unwrap();

        let c1 = make_cert(2, 1, vec![c0.digest]);
        dag.insert_certificate(c1.clone(), None).unwrap();

        // Try to insert a round-2 cert that references the round-0 cert (should be round 1)
        let c2_bad = make_cert(1, 2, vec![c0.digest]);
        let result = dag.insert_certificate(c2_bad, None);
        assert!(result.is_err(), "should reject parent from wrong round");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("expected round 1"),
            "unexpected error: {err_msg}"
        );

        // A round-2 cert referencing the round-1 cert (correct) should succeed
        let c2_good = make_cert(1, 2, vec![c1.digest]);
        dag.insert_certificate(c2_good, None).unwrap();
    }

    #[test]
    fn causal_history_is_deterministically_ordered() {
        // Fix 205: Causal history must be sorted by (round, author address).
        let mut dag = DagMempool::new(1);

        // Build a small DAG: round 0 has two validators, round 1 references both,
        // round 2 (even) is the leader cert that triggers commit.
        let c0a = make_cert(0x10, 0, vec![]);
        dag.insert_certificate(c0a.clone(), None).unwrap();

        let c0b = make_cert(0x20, 0, vec![]);
        dag.insert_certificate(c0b.clone(), None).unwrap();

        // Round 1 cert references both round-0 certs
        let c1 = make_cert(0x10, 1, vec![c0a.digest, c0b.digest]);
        dag.insert_certificate(c1.clone(), None).unwrap();

        // Round 2 (even) references round-1
        let c2 = make_cert(0x10, 2, vec![c1.digest]);
        dag.insert_certificate(c2.clone(), None).unwrap();

        // Round 3 references round-2 (to provide 2f+1 for Bullshark commit)
        let c3 = make_cert(0x10, 3, vec![c2.digest]);
        dag.insert_certificate(c3.clone(), None).unwrap();

        // Commit round 2
        let leader = Address([0x10; 20]);
        let committed = dag.try_commit(2, &leader);
        assert!(committed.is_some(), "should commit round 2");

        let certs = committed.unwrap();
        // The last cert is the leader cert; the preceding ones are causal history
        // sorted by (round, author).
        let history = &certs[..certs.len() - 1];

        for i in 1..history.len() {
            let prev = &history[i - 1];
            let curr = &history[i];
            let ordering = prev
                .round()
                .cmp(&curr.round())
                .then_with(|| prev.author().0.cmp(&curr.author().0));
            assert!(
                ordering != std::cmp::Ordering::Greater,
                "causal history not sorted: round {} author {:?} came before round {} author {:?}",
                prev.round(), prev.author(), curr.round(), curr.author()
            );
        }
    }

    #[test]
    fn stake_weighted_quorum() {
        let mut dag = DagMempool::new(3);
        let mut stakes = HashMap::new();
        // Validator 1 has 90% stake, validators 2-3 have 5% each
        stakes.insert(Address([1; 20]), 900);
        stakes.insert(Address([2; 20]), 50);
        stakes.insert(Address([3; 20]), 50);
        dag.update_stakes(stakes);

        // Total stake = 1000, quorum = 667
        assert_eq!(dag.quorum_stake(), 667);
        assert_eq!(dag.total_stake(), 1000);

        // Validator 1 alone (900 stake) meets quorum
        assert!(dag.has_stake_quorum(&[&Address([1; 20])]));

        // Validators 2+3 (100 stake) do NOT meet quorum
        assert!(!dag.has_stake_quorum(&[&Address([2; 20]), &Address([3; 20])]));

        // Validator 1 + 2 (950 stake) meets quorum
        assert!(dag.has_stake_quorum(&[&Address([1; 20]), &Address([2; 20])]));
    }

    /// Integration test: multi-validator quorum requires >2/3 of total stake.
    /// Validates that the stake-weighted quorum logic correctly handles a
    /// 3-validator set with asymmetric stake distribution.
    #[test]
    fn multi_validator_quorum_requires_stake_majority() {
        let addr_a = Address([0x0A; 20]);
        let addr_b = Address([0x0B; 20]);
        let addr_c = Address([0x0C; 20]);

        let mut dag = DagMempool::new(3);
        let mut stakes = HashMap::new();
        stakes.insert(addr_a, 100);
        stakes.insert(addr_b, 200);
        stakes.insert(addr_c, 300);
        dag.update_stakes(stakes);

        // Total stake = 600
        assert_eq!(dag.total_stake(), 600);

        // Quorum threshold = 600 * 2/3 + 1 = 401
        assert_eq!(dag.quorum_stake(), 401);

        // A single low-stake validator (100) cannot form quorum
        assert!(!dag.has_stake_quorum(&[&addr_a]),
            "100 stake should not meet 401 quorum");

        // A single mid-stake validator (200) cannot form quorum
        assert!(!dag.has_stake_quorum(&[&addr_b]),
            "200 stake should not meet 401 quorum");

        // A single high-stake validator (300) cannot form quorum
        assert!(!dag.has_stake_quorum(&[&addr_c]),
            "300 stake should not meet 401 quorum");

        // Two validators with enough combined stake form quorum:
        // B + C = 200 + 300 = 500 >= 401
        assert!(dag.has_stake_quorum(&[&addr_b, &addr_c]),
            "500 stake should meet 401 quorum");

        // A + C = 100 + 300 = 400 < 401 — barely under threshold
        assert!(!dag.has_stake_quorum(&[&addr_a, &addr_c]),
            "400 stake should not meet 401 quorum");

        // A + B = 100 + 200 = 300 < 401
        assert!(!dag.has_stake_quorum(&[&addr_a, &addr_b]),
            "300 stake should not meet 401 quorum");

        // All three validators: 100 + 200 + 300 = 600 >= 401
        assert!(dag.has_stake_quorum(&[&addr_a, &addr_b, &addr_c]),
            "600 stake should meet 401 quorum");

        // Duplicate addresses should not double-count stake (R27-FIX)
        assert!(!dag.has_stake_quorum(&[&addr_b, &addr_b, &addr_b]),
            "duplicated 200 stake should only count once (200 < 401)");
    }

}
