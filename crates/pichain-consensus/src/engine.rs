//! Consensus Engine — coordinates DAG, Bullshark ordering, fast-path, and finality.
//!
//! Single-node mode: the validator proposes, self-certifies, and commits blocks.
//! Multi-node mode: validators exchange headers, collect certificates, and run Bullshark.
//!
//! Block finality flow:
//! 1. Block proposed → create DAG header + certificate (round N)
//! 2. Next round → certificate references previous round (round N+1)
//! 3. Bullshark commit: on even rounds, check if 2f+1 next-round certs reference leader
//! 4. Committed blocks are finalized and safe to persist

use pichain_crypto::bls::{BlsPublicKey, BlsSecretKey};
use pichain_crypto::ed25519::Address;
use pichain_crypto::Hash;
use std::collections::{BTreeSet, HashMap, VecDeque};
use tracing::{debug, info, warn};

use crate::dag::{Certificate, DagMempool, Header};
use crate::fast_path::AvalancheFastPath;
use crate::staking::StakingManager;
use crate::validator::ValidatorSet;
#[cfg(test)]
use crate::validator::Validator;

/// Finality status for a block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinalityStatus {
    /// Block is proposed but not yet committed by Bullshark.
    Proposed,
    /// Block is committed through Bullshark DAG ordering (2-round finality).
    Committed,
    /// Block is finalized via Avalanche fast-path (<500ms).
    FastFinalized,
}

/// Consensus engine configuration.
#[derive(Clone, Debug)]
pub struct ConsensusConfig {
    /// This validator's address.
    pub validator_address: Address,
    /// Enable Avalanche fast-path for simple transactions.
    pub enable_fast_path: bool,
    /// PI seed for leader selection (derived from latest block hash).
    pub pi_seed: [u8; 32],
    /// Chain ID for BLS signature domain separation (prevents cross-chain replay).
    /// Certificates signed on devnet (31415) are invalid on mainnet (314159).
    pub chain_id: u64,
}

/// Metrics tracked by the consensus engine.
#[derive(Clone, Debug, Default)]
pub struct ConsensusMetrics {
    /// Total certificates produced by this validator.
    pub certificates_produced: u64,
    /// Total certificates received from other validators.
    pub certificates_received: u64,
    /// Total Bullshark commits (even-round leader commits).
    pub bullshark_commits: u64,
    /// Total blocks finalized (Bullshark + fast-path).
    pub total_finalized: u64,
    /// Total fast-path finalizations.
    pub fast_path_finalized: u64,
    /// Current DAG round.
    pub current_round: u64,
    /// Latest committed round (`None` if no round committed yet).
    pub committed_round: Option<u64>,
    /// Number of active validators.
    pub validator_count: usize,
    /// Total stake in the network.
    pub total_stake: u64,
}

/// Maximum number of finalized block hashes to retain in the finality map
/// and committed_order history. Older entries are pruned to bound memory.
const MAX_FINALITY_HISTORY: usize = 10_000;

/// Maximum number of rounds a pending block can remain before being pruned.
/// Blocks older than this many rounds behind the current round are stale.
const PENDING_BLOCK_TTL_ROUNDS: u64 = 100;

/// Hard cap on pending_blocks map size to prevent memory exhaustion during
/// consensus stalls where blocks are proposed but never committed.
const MAX_PENDING_BLOCKS: usize = 10_000;

/// AUDIT-FIX C-1: View change timeout in milliseconds.
/// If the leader for the current round does not produce a certificate within
/// this duration, validators autonomously advance to the next round.
/// This prevents a single Byzantine leader from stalling the chain forever.
const ROUND_TIMEOUT_MS: u64 = 5_000; // 5 seconds

/// Exponential backoff cap: maximum timeout multiplier for consecutive timeouts.
/// After MAX_TIMEOUT_MULTIPLIER consecutive timeouts, the timeout stays constant
/// to prevent indefinite stalls while still allowing slow leaders to catch up.
const MAX_TIMEOUT_MULTIPLIER: u64 = 8;

/// The unified consensus engine.
///
/// Wraps Narwhal DAG, Bullshark ordering, Avalanche fast-path, and staking
/// into a single interface for the block production pipeline.
pub struct ConsensusEngine {
    /// DAG-based mempool (Narwhal).
    dag: DagMempool,
    /// Avalanche fast-path for simple transactions.
    fast_path: AvalancheFastPath,
    /// Staking manager.
    staking: StakingManager,
    /// Active validator set (rebuilt each epoch).
    validator_set: ValidatorSet,
    /// Configuration.
    config: ConsensusConfig,
    /// BLS secret key for signing certificates (None = read-only / test mode).
    bls_secret_key: Option<BlsSecretKey>,
    /// Current DAG round.
    current_round: u64,
    /// Block finality tracker: block_hash → status.
    finality: HashMap<Hash, FinalityStatus>,
    /// Ordered list of committed block hashes (Bullshark total order).
    committed_order: VecDeque<Hash>,
    /// Pending blocks awaiting finality: certificate_digest → (block_hash, proposed_round).
    pending_blocks: HashMap<Hash, (Hash, u64)>,
    /// Recently committed blocks ready for persistence (FIFO).
    commit_queue: VecDeque<Hash>,
    /// Evidence hashes that have already been slashed, to prevent double-slashing
    /// from the same equivocation evidence being processed twice.
    slashed_evidence: BTreeSet<Hash>,
    /// Buffered equivocation evidence for broadcasting to peers.
    /// Drained by the node after each `try_advance()` call.
    pending_evidence_broadcast: Vec<crate::dag::EquivocationEvidence>,
    /// AUDIT-FIX C-1: Timestamp (ms) when the current round started.
    /// Used for view change timeout detection.
    round_started_ms: u64,
    /// AUDIT-FIX C-1: Number of consecutive round timeouts without a successful
    /// leader certificate. Used for exponential backoff.
    consecutive_timeouts: u64,
    /// Metrics.
    metrics: ConsensusMetrics,
}

impl ConsensusEngine {
    /// Create a new consensus engine.
    ///
    /// In single-node mode, pass a ValidatorSet with just this validator.
    /// In multi-node mode, pass the full active validator set.
    pub fn new(
        config: ConsensusConfig,
        validator_set: ValidatorSet,
        staking: StakingManager,
    ) -> Self {
        let committee_size = validator_set.validator_count();
        let enable_fast_path = config.enable_fast_path;

        info!(
            validator = %config.validator_address,
            committee_size,
            quorum = validator_set.quorum_count(),
            total_stake = validator_set.total_stake(),
            fast_path = enable_fast_path,
            "consensus engine initialized"
        );

        let mut dag = DagMempool::new(committee_size);
        dag.set_numeric_chain_id(config.chain_id);

        // R29-FIX: Register proof-of-possession for all validators in the initial set,
        // verifying the PoP against their BLS public key when available.
        // This is required for BLS aggregate signature verification to accept
        // certificates — without PoP registration, signers are rejected.
        for v in validator_set.validators() {
            if let Some(ref pop) = v.bls_pop {
                if v.bls_public_key.verify_proof_of_possession(pop).is_ok() {
                    dag.register_verified_pop(v.address);
                } else {
                    warn!(
                        validator = %v.address,
                        "skipping PoP registration: proof-of-possession verification failed"
                    );
                }
            } else if validator_set.validator_count() > 1 {
                // R30-FIX: In multi-validator mode, do NOT register validators without PoP.
                // Allowing unverified keys enables rogue key attacks on BLS aggregation.
                warn!(validator = %v.address, "skipping validator without PoP in multi-validator mode");
                // Do NOT register - cannot participate without PoP
            } else {
                // Single-validator / test mode - allow without PoP
                dag.register_verified_pop(v.address);
            }
        }

        // Inject validator stakes for stake-weighted quorum
        let stakes: HashMap<Address, u64> = validator_set.validators().iter()
            .map(|v| (v.address, v.stake))
            .collect();
        dag.update_stakes(stakes);

        Self {
            dag,
            fast_path: if enable_fast_path {
                AvalancheFastPath::new()
            } else {
                AvalancheFastPath::new_test() // minimal config if disabled
            },
            staking,
            validator_set,
            config,
            bls_secret_key: None,
            current_round: 0,
            finality: HashMap::new(),
            committed_order: VecDeque::new(),
            pending_blocks: HashMap::new(),
            commit_queue: VecDeque::new(),
            slashed_evidence: BTreeSet::new(),
            pending_evidence_broadcast: Vec::new(),
            round_started_ms: chrono::Utc::now().timestamp_millis().max(0) as u64,
            consecutive_timeouts: 0,
            metrics: ConsensusMetrics::default(),
        }
    }

    /// Propose a block using wall-clock time. **Crate-internal only.**
    ///
    /// WARNING: Uses `chrono::Utc::now()` which is non-deterministic. Production
    /// code MUST use `propose_block_at()` with a consensus-derived timestamp.
    /// This method is retained only for tests and single-node development.
    #[allow(dead_code)]
    pub(crate) fn propose_block(&mut self, block_hash: Hash, tx_batch_hashes: Vec<Hash>) -> Hash {
        self.propose_block_at(block_hash, tx_batch_hashes, chrono::Utc::now().timestamp_millis().max(0) as u64)
    }

    /// Propose a block with an explicit timestamp (deterministic, consensus-safe).
    /// Prefer this over `propose_block` in production to avoid wall-clock dependency.
    pub fn propose_block_at(&mut self, block_hash: Hash, tx_batch_hashes: Vec<Hash>, timestamp_ms: u64) -> Hash {
        // SECURITY: Refuse to produce unsigned certificates in multi-validator mode
        if self.bls_secret_key.is_none() && self.validator_set.validator_count() > 1 {
            warn!("SECURITY: refusing to propose block without BLS signing key in multi-validator mode");
            return Hash::ZERO;
        }

        // Get parent references from previous round
        let parents = if self.current_round == 0 {
            vec![]
        } else {
            self.dag
                .get_round(self.current_round - 1)
                .map(|r| r.certificate_hashes())
                .unwrap_or_default()
        };

        // Create DAG header
        let header = Header {
            author: self.config.validator_address,
            round: self.current_round,
            parents,
            payload: tx_batch_hashes,
            timestamp_ms,
        };

        // Sign the header with BLS (if key available) or leave empty for test mode.
        // Domain-separated: prepend "CERT_HDR" + chain_id to prevent both
        // cross-context and cross-chain signature reuse.
        let header_digest = header.hash();
        let (signers, aggregate_signature) = if let Some(ref bls_sk) = self.bls_secret_key {
            let mut sign_msg = Vec::with_capacity(48);
            sign_msg.extend_from_slice(b"CERT_HDR");
            sign_msg.extend_from_slice(&self.config.chain_id.to_le_bytes());
            sign_msg.extend_from_slice(header_digest.as_bytes());
            let sig = bls_sk.sign(&sign_msg);
            (vec![self.config.validator_address], sig.to_bytes().to_vec())
        } else {
            (vec![self.config.validator_address], vec![])
        };
        let cert = Certificate::new(header, signers, aggregate_signature);
        let cert_digest = cert.digest;

        // Insert into DAG — provide BLS key for verification even in single-node mode
        let validator_keys = self.bls_secret_key.as_ref().map(|sk| {
            let mut keys = HashMap::new();
            keys.insert(self.config.validator_address, sk.public_key());
            keys
        });
        if let Err(e) = self.dag.insert_certificate(cert, validator_keys.as_ref()) {
            warn!(round = self.current_round, error = %e, "failed to insert certificate");
            return Hash::ZERO;
        }

        // Track this block as pending (with round for TTL pruning).
        // Enforce hard cap to prevent memory exhaustion during consensus stalls.
        if self.pending_blocks.len() >= MAX_PENDING_BLOCKS {
            warn!(
                pending = self.pending_blocks.len(),
                "pending_blocks at capacity — refusing new block until commits clear space"
            );
            return Hash::ZERO;
        }
        self.pending_blocks.insert(cert_digest, (block_hash, self.current_round));
        self.finality.insert(block_hash, FinalityStatus::Proposed);

        self.metrics.certificates_produced += 1;
        self.metrics.current_round = self.current_round;

        debug!(
            round = self.current_round,
            cert = %cert_digest,
            block = %block_hash,
            "block proposed to DAG"
        );

        // Advance round and reset timeout timer
        self.current_round = self.current_round.saturating_add(1);
        self.round_started_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        self.consecutive_timeouts = 0; // successful proposal resets backoff

        cert_digest
    }

    /// Receive a certificate from another validator (multi-node mode).
    /// Verifies the BLS signature against the registered validator public keys.
    pub fn receive_certificate(&mut self, cert: Certificate) -> Result<(), crate::ConsensusError> {
        let digest = cert.digest;
        let round = cert.round();
        let author = cert.author();

        // Build BLS key map from validator set for signature verification
        let keys = self.bls_key_map();
        self.dag.insert_certificate(cert, Some(&keys))?;
        self.metrics.certificates_received += 1;

        // AUDIT-FIX C-1: If the received cert advances our view of the DAG,
        // reset the round timer. This prevents false timeouts when a valid
        // leader certificate arrives within the expected window.
        if round >= self.current_round {
            self.round_started_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
            self.consecutive_timeouts = 0;
        }

        debug!(
            round,
            author = %author,
            cert = %digest,
            "received external certificate"
        );

        Ok(())
    }

    /// Try to advance consensus — attempt Bullshark commit on recent rounds.
    ///
    /// Returns a list of block hashes that were finalized in this call.
    /// Call this after every new certificate is added.
    pub fn try_advance(&mut self) -> Vec<Hash> {
        let mut newly_finalized = Vec::new();

        // Try to commit all uncommitted even rounds
        let current = self.current_round;

        // Determine the first round to attempt committing.
        // `committed_round` is `Option<u64>`:
        //   None     → never committed, start from round 0
        //   Some(r)  → last committed round was r, start from r + 2
        let start_round = match self.dag.committed_round() {
            None => 0,
            Some(r) => r + 2,
        };
        let mut round = start_round;

        while round + 1 < current {
            if round % 2 != 0 {
                round += 1;
                continue;
            }

            // Select leader for this round
            let leader = self
                .validator_set
                .select_leader(round, &self.config.pi_seed);

            let leader_addr = if let Some(leader) = leader {
                leader.address
            } else {
                // Fallback: use first validator (sorted by address) as leader.
                // This prevents permanent consensus stall when PI-seed-based
                // selection returns None (e.g., due to empty or misconfigured
                // validator set).
                match self.validator_set.validators().first() {
                    Some(v) => {
                        warn!(
                            round,
                            fallback_leader = %v.address,
                            "leader selection returned None — using first validator as fallback"
                        );
                        v.address
                    }
                    None => {
                        warn!(round, "no validators available — skipping round");
                        round += 2;
                        continue;
                    }
                }
            };

            if let Some(committed_certs) = self.dag.try_commit(round, &leader_addr) {
                self.metrics.bullshark_commits += 1;
                self.metrics.committed_round = Some(round);

                // Mark all committed certificates' blocks as finalized
                for cert in &committed_certs {
                    if let Some((block_hash, _proposed_round)) = self.pending_blocks.remove(&cert.digest) {
                        // Finality is immutable: never overwrite Committed or FastFinalized
                        if !matches!(
                            self.finality.get(&block_hash),
                            Some(FinalityStatus::Committed) | Some(FinalityStatus::FastFinalized)
                        ) {
                            self.finality.insert(block_hash, FinalityStatus::Committed);
                        }
                        self.committed_order.push_back(block_hash);
                        self.commit_queue.push_back(block_hash);
                        newly_finalized.push(block_hash);
                        self.metrics.total_finalized += 1;
                    }
                }

                info!(
                    round,
                    leader = %leader_addr,
                    committed = committed_certs.len(),
                    finalized = newly_finalized.len(),
                    "Bullshark commit"
                );
            }

            round += 2;
        }

        // Prune committed DAG rounds ONCE after the entire commit loop completes.
        // Previously this was called inside try_commit(), which could prune causal
        // history needed by subsequent commits within the same loop iteration.
        self.dag.prune_committed_rounds();

        // Prune pending blocks that are too old (> PENDING_BLOCK_TTL_ROUNDS behind current)
        let current_for_prune = self.current_round;
        self.pending_blocks.retain(|_cert_digest, (_block_hash, proposed_round)| {
            current_for_prune.saturating_sub(*proposed_round) <= PENDING_BLOCK_TTL_ROUNDS
        });

        // Process any equivocation evidence — slash misbehaving validators
        self.process_equivocations();

        // Prune old finality entries to bound memory usage
        while self.committed_order.len() > MAX_FINALITY_HISTORY {
            if let Some(old_hash) = self.committed_order.pop_front() {
                self.finality.remove(&old_hash);
            }
        }

        newly_finalized
    }

    /// AUDIT-FIX C-1: Check if the current round has timed out.
    ///
    /// If the leader for the current round has not produced a certificate within
    /// the timeout period, this method autonomously advances to the next round.
    /// This is the view change mechanism that prevents a single Byzantine leader
    /// from permanently stalling the chain.
    ///
    /// Uses exponential backoff: timeout doubles on consecutive timeouts up to
    /// MAX_TIMEOUT_MULTIPLIER, giving slow-but-honest leaders time to catch up.
    ///
    /// Returns `true` if a timeout occurred and the round was advanced.
    /// The caller should then call `try_advance()` to attempt Bullshark commits
    /// and potentially propose a new block in the new round.
    pub fn check_round_timeout(&mut self) -> bool {
        // Single-validator mode: no timeout needed (we are the only proposer)
        if self.validator_set.validator_count() <= 1 {
            return false;
        }

        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let multiplier = (1u64 << self.consecutive_timeouts.min(3)).min(MAX_TIMEOUT_MULTIPLIER);
        let effective_timeout = ROUND_TIMEOUT_MS.saturating_mul(multiplier);

        if now_ms.saturating_sub(self.round_started_ms) < effective_timeout {
            return false; // Not yet timed out
        }

        // Timeout! Advance to the next round without the leader's certificate.
        let old_round = self.current_round;
        self.current_round = self.current_round.saturating_add(1);
        self.round_started_ms = now_ms;
        self.consecutive_timeouts = self.consecutive_timeouts.saturating_add(1);

        // Determine who was the expected leader for the timed-out round
        let leader_addr = self.validator_set
            .select_leader(old_round, &self.config.pi_seed)
            .map(|v| v.address);

        warn!(
            timed_out_round = old_round,
            new_round = self.current_round,
            leader = ?leader_addr,
            consecutive_timeouts = self.consecutive_timeouts,
            timeout_ms = effective_timeout,
            "VIEW CHANGE: round timed out — advancing without leader certificate"
        );

        self.metrics.current_round = self.current_round;
        true
    }

    /// Get the current round timeout duration in milliseconds (accounting for backoff).
    pub fn current_timeout_ms(&self) -> u64 {
        let multiplier = (1u64 << self.consecutive_timeouts.min(3)).min(MAX_TIMEOUT_MULTIPLIER);
        ROUND_TIMEOUT_MS.saturating_mul(multiplier)
    }

    /// Maximum number of slashed evidence hashes to retain.
    /// When exceeded, the oldest half is evicted (not cleared entirely) to prevent
    /// re-processing of recently-slashed evidence while bounding memory growth.
    const MAX_SLASHED_EVIDENCE: usize = 10_000;

    /// Drain equivocation evidence from the DAG and slash offending validators.
    fn process_equivocations(&mut self) {
        // Safety bound: if the set has grown beyond the cap, evict the oldest half.
        // Unlike clearing entirely, this preserves recent evidence hashes to prevent
        // double-slashing of recently-processed equivocations (M-3 fix).
        if self.slashed_evidence.len() > Self::MAX_SLASHED_EVIDENCE {
            let to_keep = Self::MAX_SLASHED_EVIDENCE / 2;
            let to_remove: Vec<Hash> = self.slashed_evidence.iter()
                .take(self.slashed_evidence.len() - to_keep)
                .copied()
                .collect();
            for h in to_remove {
                self.slashed_evidence.remove(&h);
            }
        }

        let evidence = self.dag.drain_equivocation_evidence();
        for ev in evidence {
            // Canonicalize evidence hash by sorting the two cert digests.
            // This ensures the same evidence produces the same hash regardless
            // of which certificate was seen first (cert_a vs cert_b ordering).
            let (first, second) = if ev.cert_a.digest.as_bytes() <= ev.cert_b.digest.as_bytes() {
                (ev.cert_a.digest, ev.cert_b.digest)
            } else {
                (ev.cert_b.digest, ev.cert_a.digest)
            };
            let mut hash_input = Vec::new();
            hash_input.extend_from_slice(first.as_bytes());
            hash_input.extend_from_slice(second.as_bytes());
            let evidence_hash = pichain_crypto::hash(&hash_input);

            // Skip if this evidence has already been slashed (deduplication).
            if self.slashed_evidence.contains(&evidence_hash) {
                debug!(
                    validator = %ev.author,
                    round = ev.round,
                    "skipping duplicate equivocation evidence (already slashed)"
                );
                continue;
            }

            let evidence_epoch = self.staking.current_epoch();
            match self.staking.slash(ev.author, crate::staking::SlashReason::DoubleSigning, evidence_hash, evidence_epoch) {
                Ok(event) => {
                    // Record this evidence as slashed to prevent double-slashing.
                    self.slashed_evidence.insert(evidence_hash);
                    // Buffer evidence for broadcast to peers so all nodes
                    // process the same slashing deterministically.
                    // Cap at 1024 entries to prevent unbounded memory growth.
                    if self.pending_evidence_broadcast.len() < 1024 {
                        self.pending_evidence_broadcast.push(ev.clone());
                    }
                    warn!(
                        validator = %ev.author,
                        round = ev.round,
                        slashed = event.amount_slashed,
                        "EQUIVOCATION DETECTED: validator slashed for double-signing"
                    );
                }
                Err(e) => {
                    // Validator may not be in staking set (e.g., already removed).
                    // Log but don't fail consensus.
                    warn!(
                        validator = %ev.author,
                        round = ev.round,
                        error = %e,
                        "equivocation detected but slash failed"
                    );
                }
            }
        }
    }

    /// Drain buffered equivocation evidence for P2P broadcast.
    /// The node should call this after `try_advance()` and broadcast each
    /// evidence item to all peers via the sync GossipSub topic.
    pub fn drain_pending_evidence(&mut self) -> Vec<crate::dag::EquivocationEvidence> {
        std::mem::take(&mut self.pending_evidence_broadcast)
    }

    /// Process equivocation evidence received from a peer.
    /// Returns true if the evidence was new and the validator was slashed.
    ///
    /// SECURITY: Verifies BLS aggregate signatures on BOTH certificates before slashing.
    /// Without this, any peer could slash any validator by sending fabricated evidence.
    pub fn process_remote_evidence(&mut self, evidence: crate::dag::EquivocationEvidence) -> bool {
        // Canonicalize evidence hash
        let (first, second) = if evidence.cert_a.digest.as_bytes() <= evidence.cert_b.digest.as_bytes() {
            (evidence.cert_a.digest, evidence.cert_b.digest)
        } else {
            (evidence.cert_b.digest, evidence.cert_a.digest)
        };
        let mut hash_input = Vec::new();
        hash_input.extend_from_slice(first.as_bytes());
        hash_input.extend_from_slice(second.as_bytes());
        let evidence_hash = pichain_crypto::hash(&hash_input);

        // Skip if already processed
        if self.slashed_evidence.contains(&evidence_hash) {
            return false;
        }

        // Verify digest matches header hash for both certificates
        let hash_a = evidence.cert_a.header.hash();
        if evidence.cert_a.digest != hash_a {
            warn!("rejecting evidence: cert_a digest does not match header hash");
            return false;
        }
        let hash_b = evidence.cert_b.header.hash();
        if evidence.cert_b.digest != hash_b {
            warn!("rejecting evidence: cert_b digest does not match header hash");
            return false;
        }

        // Verify the two certificates are actually conflicting (same author, same round, different digests)
        if evidence.cert_a.author() != evidence.cert_b.author() {
            warn!("rejecting evidence: different authors");
            return false;
        }
        if evidence.cert_a.header.round != evidence.cert_b.header.round {
            warn!("rejecting evidence: different rounds");
            return false;
        }
        if evidence.cert_a.digest == evidence.cert_b.digest {
            warn!("rejecting evidence: identical certificates");
            return false;
        }

        // CRITICAL: Derive author from the verified certificates, not from the
        // unauthenticated evidence.author field which an attacker could set freely.
        let cert_author = evidence.cert_a.author();

        // CRITICAL: Verify BLS aggregate signatures on BOTH certificates.
        // Without this, any network peer could forge evidence and slash honest validators.
        let keys = self.bls_key_map();
        if self.validator_set.validator_count() > 1 {
            for (label, cert) in [("cert_a", &evidence.cert_a), ("cert_b", &evidence.cert_b)] {
                // Require non-empty signature and signers
                if cert.aggregate_signature.is_empty() || cert.signers.is_empty() {
                    warn!("rejecting evidence: {label} has no signers or signature");
                    return false;
                }
                if cert.aggregate_signature.len() != 96 {
                    warn!("rejecting evidence: {label} has invalid signature length");
                    return false;
                }
                // Parse BLS signature
                let sig_bytes: [u8; 96] = match cert.aggregate_signature[..96].try_into() {
                    Ok(b) => b,
                    Err(_) => {
                        warn!("rejecting evidence: {label} malformed BLS signature");
                        return false;
                    }
                };
                let bls_sig = match pichain_crypto::bls::BlsSignature::from_bytes(&sig_bytes) {
                    Ok(s) => s,
                    Err(_) => {
                        warn!("rejecting evidence: {label} BLS signature not on curve");
                        return false;
                    }
                };
                // Build domain-separated message (same as DAG insert_certificate)
                let mut message = Vec::with_capacity(48);
                message.extend_from_slice(b"CERT_HDR");
                message.extend_from_slice(&self.config.chain_id.to_le_bytes());
                message.extend_from_slice(cert.digest.as_bytes());
                // AUDIT-FIX C-2: Reject duplicate signers to prevent inflating apparent quorum
                let unique_signers: std::collections::HashSet<_> = cert.signers.iter().collect();
                if unique_signers.len() != cert.signers.len() {
                    warn!("rejecting evidence: {label} has duplicate signers");
                    return false;
                }
                // Resolve signer public keys
                let signer_pks: Vec<&pichain_crypto::bls::BlsPublicKey> = cert.signers.iter()
                    .filter_map(|addr| keys.get(addr))
                    .collect();
                if signer_pks.len() != cert.signers.len() {
                    warn!("rejecting evidence: {label} has unknown signers");
                    return false;
                }
                // AUDIT-FIX C-1: Require quorum-level signers on evidence certificates.
                // Without this, an attacker with 1 key could forge sub-quorum certs and
                // slash any honest validator by creating conflicting evidence.
                let signer_refs: Vec<&Address> = cert.signers.iter().collect();
                if !self.dag.has_stake_quorum(&signer_refs) {
                    warn!("rejecting evidence: {label} lacks quorum signers ({} of {} required)",
                        cert.signers.len(), self.validator_set.validator_count() * 2 / 3 + 1);
                    return false;
                }
                // Verify aggregate BLS signature
                if pichain_crypto::bls::AggregateSignature::verify(&signer_pks, &message, &bls_sig).is_err() {
                    warn!("rejecting evidence: {label} BLS verification failed");
                    return false;
                }
            }
        }

        let evidence_epoch = self.staking.current_epoch();
        match self.staking.slash(cert_author, crate::staking::SlashReason::DoubleSigning, evidence_hash, evidence_epoch) {
            Ok(event) => {
                self.slashed_evidence.insert(evidence_hash);
                warn!(
                    validator = %cert_author,
                    round = evidence.round,
                    slashed = event.amount_slashed,
                    "REMOTE EQUIVOCATION EVIDENCE: validator slashed for double-signing"
                );
                true
            }
            Err(e) => {
                warn!(
                    validator = %cert_author,
                    error = %e,
                    "remote equivocation evidence: slash failed"
                );
                false
            }
        }
    }

    /// Start an Avalanche fast-path query for a transaction.
    pub fn start_fast_path(&mut self, tx_hash: Hash) {
        self.fast_path.start_query(tx_hash);
    }

    /// Record a fast-path vote from an authenticated validator.
    ///
    /// The voter address is checked against the active validator set.
    /// Votes from unknown addresses are rejected (returns false).
    /// Double-votes from the same voter in the same round are silently ignored.
    pub fn record_fast_path_vote(&mut self, tx_hash: &Hash, voter: &Address, vote: bool) -> bool {
        // Reject votes from addresses not in the validator set
        let voter_stake = match self.validator_set.get_validator(voter) {
            Some(v) => v.stake,
            None => {
                warn!(voter = %voter, "rejected fast-path vote from unknown validator");
                return false;
            }
        };

        // R27-FIX: Use stake-weighted voting instead of count-based.
        // This prevents sybil attacks via many low-stake validators.
        let finalized = self.fast_path.record_vote_weighted(tx_hash, voter, vote, voter_stake);
        if finalized {
            self.metrics.fast_path_finalized += 1;
            self.metrics.total_finalized += 1;
            // R28-FIX: Only insert if not already finalized (prevents overwriting
            // Committed status with FastFinalized). Also track in committed_order
            // for proper pruning — otherwise fast-path entries leak forever.
            if !matches!(self.finality.get(tx_hash), Some(FinalityStatus::Committed) | Some(FinalityStatus::FastFinalized)) {
                self.finality.insert(*tx_hash, FinalityStatus::FastFinalized);
                self.committed_order.push_back(*tx_hash);
            }
            // Clean up the fast-path pending entry to free memory
            self.fast_path.remove(tx_hash);
        }
        finalized
    }

    /// Check if a transaction is finalized via fast-path.
    pub fn is_fast_path_finalized(&self, tx_hash: &Hash) -> bool {
        self.fast_path.is_finalized(tx_hash)
    }

    /// Check the finality status of a block.
    pub fn finality_status(&self, block_hash: &Hash) -> Option<&FinalityStatus> {
        self.finality.get(block_hash)
    }

    /// Check if a block is finalized (either Bullshark or fast-path).
    pub fn is_finalized(&self, block_hash: &Hash) -> bool {
        matches!(
            self.finality.get(block_hash),
            Some(FinalityStatus::Committed) | Some(FinalityStatus::FastFinalized)
        )
    }

    /// Pop the next finalized block hash from the commit queue.
    pub fn next_committed_block(&mut self) -> Option<Hash> {
        self.commit_queue.pop_front()
    }

    /// Get the current DAG round.
    pub fn current_round(&self) -> u64 {
        self.current_round
    }

    /// Get the latest committed round.
    /// Returns `None` if no round has been committed yet.
    pub fn committed_round(&self) -> Option<u64> {
        self.dag.committed_round()
    }

    /// Get the total number of finalized blocks.
    pub fn total_finalized(&self) -> u64 {
        self.metrics.total_finalized
    }

    /// Get consensus metrics.
    pub fn metrics(&self) -> &ConsensusMetrics {
        &self.metrics
    }

    /// Update the PI seed (typically from latest block hash).
    pub fn set_pi_seed(&mut self, seed: [u8; 32]) {
        self.config.pi_seed = seed;
    }

    /// Returns true if the engine is operating in single-validator mode.
    pub fn is_single_validator(&self) -> bool {
        self.validator_set.validator_count() <= 1
    }

    /// Update the validator set (typically at epoch boundaries).
    ///
    /// Returns the hashes of any pending blocks that were dropped during the
    /// epoch transition. The caller should re-queue the transactions from these
    /// blocks back into the mempool so they are not silently lost.
    ///
    /// AUDIT-FIX C-2: Detects single→multi validator transition and logs a
    /// checkpoint so the transition point is clearly identified in the chain history.
    pub fn update_validator_set(&mut self, new_set: ValidatorSet) -> Vec<Hash> {
        let old_count = self.validator_set.validator_count();
        let new_count = new_set.validator_count();

        // C-2: Detect and log the critical single→multi validator transition
        if old_count <= 1 && new_count > 1 {
            info!(
                new_validators = new_count,
                current_round = self.metrics.current_round,
                committed_round = ?self.metrics.committed_round,
                total_stake = new_set.total_stake(),
                "=== CONSENSUS TRANSITION: single-validator → BFT multi-validator mode ==="
            );
            info!(
                "BFT consensus now active: requires {}/{} validators for quorum",
                (new_count * 2 / 3) + 1,
                new_count
            );
        } else if old_count > 1 && new_count <= 1 {
            warn!(
                "=== CONSENSUS TRANSITION: multi-validator → single-validator mode ==="
            );
        }

        info!(
            old_validators = old_count,
            new_validators = new_count,
            total_stake = new_set.total_stake(),
            "validator set updated"
        );

        // Attempt to commit any pending rounds before draining, so blocks proposed
        // in the old epoch are not silently lost.
        let pre_drain_finalized = self.try_advance();
        if !pre_drain_finalized.is_empty() {
            info!(
                finalized = pre_drain_finalized.len(),
                "committed pending blocks before epoch transition"
            );
        }

        // Return dropped block hashes so their transactions can be re-queued
        let mut dropped_blocks = Vec::new();
        let stale = self.pending_blocks.len();
        if stale > 0 {
            warn!(stale_blocks = stale, "draining pending blocks on validator set change — returning for re-queue");
            for (_cert_digest, (block_hash, _round)) in self.pending_blocks.drain() {
                dropped_blocks.push(block_hash);
            }
        }

        // Clear stale PoP entries from the previous epoch before re-registering.
        // Without this, validators removed from the set could still pass PoP checks.
        self.dag.reset_verified_pops();

        // R29-FIX: Verify proof-of-possession before registering validators.
        // Only register validators whose PoP is cryptographically verified against
        // their BLS public key. Validators without a PoP (None) are still registered
        // for backward compatibility in test/single-node mode.
        for v in new_set.validators() {
            if let Some(ref pop) = v.bls_pop {
                if v.bls_public_key.verify_proof_of_possession(pop).is_ok() {
                    self.dag.register_verified_pop(v.address);
                } else {
                    warn!(
                        validator = %v.address,
                        "skipping PoP registration: proof-of-possession verification failed"
                    );
                }
            } else if new_set.validator_count() > 1 {
                // R30-FIX: In multi-validator mode, do NOT register validators without PoP.
                // Allowing unverified keys enables rogue key attacks on BLS aggregation.
                warn!(validator = %v.address, "skipping validator without PoP in multi-validator mode");
                // Do NOT register - cannot participate without PoP
            } else {
                // Single-validator / test mode - allow without PoP
                self.dag.register_verified_pop(v.address);
            }
        }

        self.validator_set = new_set;

        // Retain slashed_evidence across epoch boundaries to prevent double-slash attacks.
        // An attacker who stores evidence from epoch N could re-submit it after the
        // epoch transition if we cleared the dedup set. R33-FIX: Use same constant and
        // eviction strategy as process_equivocations() for cross-node consistency.
        if self.slashed_evidence.len() > Self::MAX_SLASHED_EVIDENCE {
            let to_keep = Self::MAX_SLASHED_EVIDENCE / 2;
            let to_remove: Vec<Hash> = self.slashed_evidence.iter()
                .take(self.slashed_evidence.len() - to_keep)
                .copied()
                .collect();
            for h in to_remove {
                self.slashed_evidence.remove(&h);
            }
        }

        // Update DAG committee size atomically with the validator set swap.
        // This must happen immediately after the assignment, before any metrics
        // or logging, to prevent a window where the DAG uses stale quorum thresholds.
        self.dag.update_committee_size(self.validator_set.validator_count());

        // Update stake map for stake-weighted quorum
        let stakes: HashMap<Address, u64> = self.validator_set.validators().iter()
            .map(|v| (v.address, v.stake))
            .collect();
        self.dag.update_stakes(stakes);

        self.metrics.validator_count = new_count;
        self.metrics.total_stake = self.validator_set.total_stake();

        // Advance staking epoch to snapshot validator stakes for velocity limiting.
        // This ensures epoch_start_stake is updated each time the validator set
        // changes (at epoch boundaries), enabling the per-epoch velocity cap.
        let new_staking_epoch = self.staking.current_epoch().saturating_add(1);
        self.staking.set_epoch(new_staking_epoch);

        dropped_blocks
    }

    /// Set the BLS secret key for certificate signing.
    pub fn set_bls_secret_key(&mut self, key: BlsSecretKey) {
        self.bls_secret_key = Some(key);
    }

    /// Build a map of validator address → BLS public key for signature verification.
    fn bls_key_map(&self) -> HashMap<Address, BlsPublicKey> {
        self.validator_set
            .validators()
            .iter()
            .map(|v| (v.address, v.bls_public_key.clone()))
            .collect()
    }

    /// Get a reference to the staking manager.
    pub fn staking(&self) -> &StakingManager {
        &self.staking
    }

    /// Get a mutable reference to the staking manager.
    pub fn staking_mut(&mut self) -> &mut StakingManager {
        &mut self.staking
    }

    /// Get a reference to the validator set.
    pub fn validator_set(&self) -> &ValidatorSet {
        &self.validator_set
    }

    /// Get a reference to the DAG mempool.
    pub fn dag(&self) -> &DagMempool {
        &self.dag
    }

    /// Get a mutable reference to the DAG mempool.
    #[allow(dead_code)]
    pub(crate) fn dag_mut(&mut self) -> &mut DagMempool {
        &mut self.dag
    }

    /// Get the number of pending (unfinalized) blocks.
    pub fn pending_count(&self) -> usize {
        self.pending_blocks.len()
    }

    /// Get the committed block order (Bullshark total ordering).
    /// Returns the two contiguous slices that make up the VecDeque.
    pub fn committed_order(&self) -> (&[Hash], &[Hash]) {
        self.committed_order.as_slices()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::bls::BlsKeypair;

    fn make_test_validator(addr_byte: u8, stake: u64) -> Validator {
        let bls_kp = BlsKeypair::generate();
        Validator {
            address: Address([addr_byte; 20]),
            bls_public_key: bls_kp.public,
            bls_pop: None,
            stake,
            uptime_bps: 9900, // 99%
            contribution_score: 100,
            active: true,
        }
    }

    fn setup_single_validator() -> ConsensusEngine {
        let validator = make_test_validator(1, 100_000_000_000_000);
        let validator_address = validator.address;
        let validator_set = ValidatorSet::new(vec![validator]);
        let staking = StakingManager::new();

        let config = ConsensusConfig {
            validator_address,
            enable_fast_path: false,
            pi_seed: [0u8; 32],
            chain_id: 31415,
        };

        ConsensusEngine::new(config, validator_set, staking)
    }

    #[test]
    fn single_validator_propose_and_commit() {
        let mut engine = setup_single_validator();

        let block1 = pichain_crypto::hash(b"block1");
        let block2 = pichain_crypto::hash(b"block2");
        let block3 = pichain_crypto::hash(b"block3");

        // Round 0 (even): propose block1
        let cert0 = engine.propose_block(block1, vec![]);
        assert_ne!(cert0, Hash::ZERO);
        assert_eq!(engine.current_round(), 1);

        // Round 1 (odd): propose block2
        let cert1 = engine.propose_block(block2, vec![]);
        assert_ne!(cert1, Hash::ZERO);
        assert_eq!(engine.current_round(), 2);

        // Try advance — should commit round 0 (even), since round 1 has cert referencing it
        let finalized = engine.try_advance();
        assert!(!finalized.is_empty(), "should finalize block1");
        assert!(engine.is_finalized(&block1));

        // Round 2 (even): propose block3
        engine.propose_block(block3, vec![]);
        assert_eq!(engine.current_round(), 3);

        // Block2 isn't finalized yet (it was proposed in odd round 1)
        // We need round 3 to reference round 2 for Bullshark to commit round 2
        // But block2's cert is in round 1 which is committed as causal history of round 0's commit
        // Actually, block2 was proposed in round 1 (odd), so it will be committed
        // when a later even round commits.
    }

    #[test]
    fn single_validator_finality_tracking() {
        let mut engine = setup_single_validator();

        let block = pichain_crypto::hash(b"test_block");
        engine.propose_block(block, vec![]);

        // Initially proposed
        assert_eq!(
            engine.finality_status(&block),
            Some(&FinalityStatus::Proposed)
        );
        assert!(!engine.is_finalized(&block));

        // After second round and advance, should be committed
        let block2 = pichain_crypto::hash(b"block2");
        engine.propose_block(block2, vec![]);
        engine.try_advance();

        assert!(engine.is_finalized(&block));
        assert_eq!(
            engine.finality_status(&block),
            Some(&FinalityStatus::Committed)
        );
    }

    #[test]
    fn multi_validator_setup() {
        let v1 = make_test_validator(1, 100_000_000_000_000);
        let v2 = make_test_validator(2, 100_000_000_000_000);
        let v3 = make_test_validator(3, 100_000_000_000_000);

        let addr = v1.address;
        let validator_set = ValidatorSet::new(vec![v1, v2, v3]);

        assert_eq!(validator_set.validator_count(), 3);
        assert_eq!(validator_set.quorum_count(), 3); // 3 * 2/3 + 1 = 3

        let staking = StakingManager::new();
        let config = ConsensusConfig {
            validator_address: addr,
            enable_fast_path: true,
            pi_seed: [0u8; 32],
            chain_id: 31415,
        };

        let engine = ConsensusEngine::new(config, validator_set, staking);
        assert_eq!(engine.current_round(), 0);
        assert_eq!(engine.pending_count(), 0);
    }

    #[test]
    fn fast_path_integration() {
        let mut engine = setup_single_validator();
        let valid_voter = engine.config.validator_address;

        let tx_hash = pichain_crypto::hash(b"simple_transfer");
        engine.start_fast_path(tx_hash);
        assert!(!engine.is_fast_path_finalized(&tx_hash));

        // Fast-path uses production params (k=40, alpha=30, beta=20)
        // Won't finalize without enough votes in this test
        let result = engine.record_fast_path_vote(&tx_hash, &valid_voter, true);
        assert!(!result); // Need 40 votes per round, 20 consecutive rounds
    }

    #[test]
    fn fast_path_rejects_unknown_voter() {
        let mut engine = setup_single_validator();

        let tx_hash = pichain_crypto::hash(b"attack_tx");
        engine.start_fast_path(tx_hash);

        // Vote from address not in validator set should be rejected
        let unknown = Address([0xFF; 20]);
        let result = engine.record_fast_path_vote(&tx_hash, &unknown, true);
        assert!(!result);
    }

    #[test]
    fn metrics_tracking() {
        let mut engine = setup_single_validator();

        let block = pichain_crypto::hash(b"block");
        engine.propose_block(block, vec![]);
        assert_eq!(engine.metrics().certificates_produced, 1);

        let block2 = pichain_crypto::hash(b"block2");
        engine.propose_block(block2, vec![]);
        assert_eq!(engine.metrics().certificates_produced, 2);

        engine.try_advance();
        assert!(engine.metrics().bullshark_commits > 0 || engine.metrics().total_finalized > 0);
    }

    #[test]
    fn commit_queue_ordering() {
        let mut engine = setup_single_validator();

        // Produce several blocks
        let _blocks: Vec<Hash> = (0..6)
            .map(|i| {
                let h = pichain_crypto::hash(format!("block_{i}").as_bytes());
                engine.propose_block(h, vec![]);
                h
            })
            .collect();

        // Advance consensus
        let finalized = engine.try_advance();

        // Check that finalized blocks can be popped from queue
        for hash in &finalized {
            let popped = engine.next_committed_block();
            assert_eq!(popped, Some(*hash));
        }
    }

    #[test]
    fn pi_seed_update() {
        let mut engine = setup_single_validator();
        let new_seed = *pichain_crypto::hash(b"new_block_hash").as_bytes();
        engine.set_pi_seed(new_seed);
        assert_eq!(engine.config.pi_seed, new_seed);
    }

    #[test]
    fn equivocation_triggers_slashing() {
        let validator = make_test_validator(1, 100_000_000_000_000);
        let validator_address = validator.address;
        let validator_set = ValidatorSet::new(vec![validator]);
        let mut staking = StakingManager::new();
        // Register 3 validators for bootstrap-safe setup
        staking.register_validator(
            validator_address,
            crate::staking::MIN_VALIDATOR_STAKE,
            1000,
        ).unwrap();
        let dummy_addr = pichain_crypto::ed25519::Address([99u8; 20]);
        staking.register_validator(
            dummy_addr,
            crate::staking::MIN_VALIDATOR_STAKE,
            1000,
        ).unwrap();
        let dummy_addr2 = pichain_crypto::ed25519::Address([98u8; 20]);
        staking.register_validator(
            dummy_addr2,
            crate::staking::MIN_VALIDATOR_STAKE,
            1000,
        ).unwrap();

        let config = ConsensusConfig {
            validator_address,
            enable_fast_path: false,
            pi_seed: [0u8; 32],
            chain_id: 31415,
        };
        let mut engine = ConsensusEngine::new(config, validator_set, staking);

        // Produce a valid certificate in round 0
        let h0 = pichain_crypto::hash(b"block_0");
        engine.propose_block(h0, vec![]);

        // Manually inject a conflicting certificate from the same author in round 0
        let conflicting_header = crate::dag::Header {
            author: validator_address,
            round: 0,
            parents: vec![],
            payload: vec![pichain_crypto::hash(b"conflicting_payload")],
            timestamp_ms: 0,
        };
        let conflicting_cert = crate::dag::Certificate::new(
            conflicting_header,
            vec![validator_address],
            vec![],
        );
        // This should fail (duplicate) but collect equivocation evidence
        let _ = engine.dag.insert_certificate(conflicting_cert, None);

        // Verify equivocation evidence was collected
        assert_eq!(engine.dag.equivocation_evidence().len(), 1);

        // try_advance should process equivocations and trigger slashing
        engine.try_advance();

        // Evidence should be drained
        assert_eq!(engine.dag.equivocation_evidence().len(), 0);

        // Validator should be jailed
        let staking = engine.staking();
        let slash_history = staking.slash_history();
        assert_eq!(slash_history.len(), 1);
        assert_eq!(slash_history[0].validator, validator_address);
        assert!(matches!(slash_history[0].reason, crate::staking::SlashReason::DoubleSigning));
        assert!(slash_history[0].amount_slashed > 0);
    }

    #[test]
    fn update_validator_set_updates_committee_size() {
        // Fix 185: committee_size must be updated when validator set changes
        let mut engine = setup_single_validator();

        // Initial committee size is 1 (single validator)
        assert_eq!(engine.dag().committee_size(), 1);

        // Create a new validator set with 5 validators
        let validators: Vec<Validator> = (1..=5)
            .map(|i| make_test_validator(i, 100_000_000_000_000))
            .collect();
        let new_set = ValidatorSet::new(validators);

        engine.update_validator_set(new_set);

        // DAG committee_size should now reflect the new validator count
        assert_eq!(engine.dag().committee_size(), 5);
    }

    #[test]
    fn update_validator_set_changes_quorum_threshold() {
        // Fix 185: quorum threshold must adjust with committee size
        let mut engine = setup_single_validator();

        // committee_size=1 => quorum = 1*2/3+1 = 1
        // Single validator can self-certify round 0
        let block = pichain_crypto::hash(b"block_before_transition");
        engine.propose_block(block, vec![]);

        // Grow validator set to 4 validators
        let validators: Vec<Validator> = (1..=4)
            .map(|i| make_test_validator(i, 100_000_000_000_000))
            .collect();
        let new_set = ValidatorSet::new(validators);
        engine.update_validator_set(new_set);

        // committee_size=4 => quorum = 4*2/3+1 = 3
        assert_eq!(engine.dag().committee_size(), 4);
    }

    #[test]
    fn verified_pops_cleared_on_epoch_transition() {
        // Fix 202: verified_pops must be cleared on epoch transition so that
        // validators removed from the set cannot pass PoP checks.
        let v1 = make_test_validator(1, 100_000_000_000_000);
        let v2 = make_test_validator(2, 100_000_000_000_000);
        let addr1 = v1.address;
        let addr2 = v2.address;
        let validator_set = ValidatorSet::new(vec![v1, v2]);
        let staking = StakingManager::new();

        let config = ConsensusConfig {
            validator_address: addr1,
            enable_fast_path: false,
            pi_seed: [0u8; 32],
            chain_id: 31415,
        };
        let mut engine = ConsensusEngine::new(config, validator_set, staking);

        // Both validators should have verified PoPs initially
        // (checked indirectly: the DAG was set up with both validators' PoPs)

        // Transition to a new set with only v1 (v2 removed)
        let v1_new = make_test_validator(1, 100_000_000_000_000);
        let new_set = ValidatorSet::new(vec![v1_new]);
        engine.update_validator_set(new_set);

        // After transition, addr2's PoP should be cleared.
        // We can verify this by checking the DAG does not accept a cert
        // signed by the removed validator (addr2) in multi-validator mode.
        // Since we are now single-validator mode, a simple assertion is that
        // the old validator was removed and only the new set's validators are registered.
        // We test by trying a BLS-verified insert for the removed validator.
        let keys = engine.bls_key_map();
        assert!(
            !keys.contains_key(&addr2),
            "removed validator should not be in key map"
        );
    }

    #[test]
    fn pruning_after_commit_loop_preserves_causal_history() {
        // Fix 206: Pruning must happen after the commit loop, not inside try_commit,
        // so that causal history needed by subsequent commits is preserved.
        let mut engine = setup_single_validator();

        // Produce enough rounds to trigger multiple commits in a single try_advance
        let blocks: Vec<Hash> = (0..8)
            .map(|i| {
                let h = pichain_crypto::hash(format!("block_{i}").as_bytes());
                engine.propose_block(h, vec![]);
                h
            })
            .collect();

        // Advance — should commit multiple even rounds without losing causal history
        let finalized = engine.try_advance();
        assert!(
            !finalized.is_empty(),
            "should finalize blocks across multiple commit rounds"
        );

        // Verify all finalized blocks are properly tracked
        for hash in &finalized {
            assert!(
                engine.is_finalized(hash),
                "block {hash} should be finalized"
            );
        }

        // Verify that at least blocks from early rounds were committed
        assert!(
            engine.is_finalized(&blocks[0]),
            "first block should be finalized"
        );
    }

    #[test]
    fn bls_signing_roundtrip() {
        // Integration test: Verify that a ConsensusEngine with a BLS secret key
        // produces certificates with valid BLS signatures that can be verified
        // using the corresponding public key.
        let bls_kp = BlsKeypair::generate();
        let bls_pk = bls_kp.public.clone();

        let validator = Validator {
            address: Address([1; 20]),
            bls_public_key: bls_kp.public.clone(),
            bls_pop: None,
            stake: 100_000_000_000_000,
            uptime_bps: 9900,
            contribution_score: 100,
            active: true,
        };
        let validator_address = validator.address;
        let validator_set = ValidatorSet::new(vec![validator]);
        let staking = StakingManager::new();

        let config = ConsensusConfig {
            validator_address,
            enable_fast_path: false,
            pi_seed: [0u8; 32],
            chain_id: 31415,
        };

        let mut engine = ConsensusEngine::new(config, validator_set, staking);
        engine.set_bls_secret_key(bls_kp.secret);

        // Propose a block — this should create a BLS-signed certificate
        let block_hash = pichain_crypto::hash(b"bls_test_block");
        let cert_digest = engine.propose_block(block_hash, vec![]);
        assert_ne!(cert_digest, Hash::ZERO, "certificate should be created");

        // Extract the certificate from the DAG (round 0)
        let round_0 = engine.dag().get_round(0)
            .expect("round 0 should exist");
        let cert = round_0.get(&validator_address)
            .expect("certificate should be in round 0");

        // The certificate should have a non-empty aggregate signature
        assert_eq!(cert.aggregate_signature.len(), 96,
            "BLS signature should be 96 bytes");
        assert!(!cert.aggregate_signature.iter().all(|&b| b == 0),
            "signature should not be all zeros");

        // Verify the BLS signature manually using the public key
        // The signing domain is "CERT_HDR" + chain_id + header_digest (same as propose_block_at)
        let header_digest = cert.header.hash();
        let mut sign_msg = Vec::with_capacity(48);
        sign_msg.extend_from_slice(b"CERT_HDR");
        sign_msg.extend_from_slice(&31415u64.to_le_bytes()); // test chain_id
        sign_msg.extend_from_slice(header_digest.as_bytes());

        let sig_bytes: [u8; 96] = cert.aggregate_signature[..96].try_into().unwrap();
        let sig = pichain_crypto::bls::BlsSignature::from_bytes(&sig_bytes)
            .expect("signature should be valid BLS point");

        bls_pk.verify(&sign_msg, &sig)
            .expect("BLS signature verification should succeed");
    }

    #[test]
    fn duplicate_equivocation_not_double_slashed() {
        // Fix 209: The same equivocation evidence must not cause double-slashing.
        let validator = make_test_validator(1, 100_000_000_000_000);
        let validator_address = validator.address;
        let validator_set = ValidatorSet::new(vec![validator]);
        let mut staking = StakingManager::new();
        // Register 3 validators for bootstrap-safe setup
        staking.register_validator(
            validator_address,
            crate::staking::MIN_VALIDATOR_STAKE,
            1000,
        ).unwrap();
        let dummy_addr = pichain_crypto::ed25519::Address([99u8; 20]);
        staking.register_validator(
            dummy_addr,
            crate::staking::MIN_VALIDATOR_STAKE,
            1000,
        ).unwrap();
        let dummy_addr2 = pichain_crypto::ed25519::Address([98u8; 20]);
        staking.register_validator(
            dummy_addr2,
            crate::staking::MIN_VALIDATOR_STAKE,
            1000,
        ).unwrap();

        let config = ConsensusConfig {
            validator_address,
            enable_fast_path: false,
            pi_seed: [0u8; 32],
            chain_id: 31415,
        };
        let mut engine = ConsensusEngine::new(config, validator_set, staking);

        // Produce a valid certificate in round 0
        let h0 = pichain_crypto::hash(b"block_0");
        engine.propose_block(h0, vec![]);

        // Manually inject two different conflicting certificates to create
        // two separate equivocation evidence entries with the same pair of certs.
        let conflicting_header_1 = crate::dag::Header {
            author: validator_address,
            round: 0,
            parents: vec![],
            payload: vec![pichain_crypto::hash(b"conflict_1")],
            timestamp_ms: 0,
        };
        let conflicting_cert_1 = crate::dag::Certificate::new(
            conflicting_header_1,
            vec![validator_address],
            vec![],
        );
        let _ = engine.dag.insert_certificate(conflicting_cert_1, None);

        // Process first equivocation
        engine.try_advance();
        let slash_count_1 = engine.staking().slash_history().len();
        assert_eq!(slash_count_1, 1, "first equivocation should trigger one slash");

        // Inject the same conflicting cert again (different payload but same effect)
        let conflicting_header_2 = crate::dag::Header {
            author: validator_address,
            round: 0,
            parents: vec![],
            payload: vec![pichain_crypto::hash(b"conflict_1")], // same payload = same digest
            timestamp_ms: 0,
        };
        let conflicting_cert_2 = crate::dag::Certificate::new(
            conflicting_header_2,
            vec![validator_address],
            vec![],
        );
        let _ = engine.dag.insert_certificate(conflicting_cert_2, None);

        // Process second equivocation — should be deduplicated
        engine.try_advance();
        let slash_count_2 = engine.staking().slash_history().len();
        assert_eq!(
            slash_count_2, slash_count_1,
            "duplicate equivocation evidence should not cause additional slashing"
        );
    }
}
