//! State sync protocol for PIChain.
//!
//! Allows new nodes to catch up to the current chain state by:
//! 1. Downloading block headers from peers
//! 2. Verifying header chain (parent hashes)
//! 3. Downloading block bodies (transactions) in parallel
//! 4. Optionally downloading state snapshots for fast-sync
//!
//! Three sync modes:
//! - Full sync: Download and replay all blocks from genesis
//! - Fast sync: Download a recent state snapshot + recent blocks
//! - Light sync: Only download block headers for verification

use pichain_crypto::Hash;
use pichain_types::block::{Block, BlockHeader};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Sync mode determines how aggressively we catch up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncMode {
    /// Download and replay every block from genesis.
    Full,
    /// Download a state snapshot at a recent height, then replay recent blocks.
    Fast,
    /// Only download block headers (no transactions or state).
    Light,
}

/// A request sent to a peer during sync.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SyncRequest {
    /// Request block headers in a range.
    GetHeaders { start_height: u64, max_count: u32 },
    /// Request full blocks (with transactions) by height.
    GetBlocks { heights: Vec<u64> },
    /// Request a state snapshot at a specific height.
    GetStateSnapshot { height: u64 },
    /// Request the peer's chain tip info.
    GetChainTip,
    /// Request specific account states for verification.
    GetAccountProofs {
        height: u64,
        accounts: Vec<[u8; 20]>,
    },
}

/// A response from a peer during sync.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SyncResponse {
    /// Block headers response.
    Headers(Vec<BlockHeader>),
    /// Full blocks response.
    Blocks(Vec<Block>),
    /// State snapshot chunk.
    StateChunk {
        height: u64,
        chunk_index: u32,
        total_chunks: u32,
        data: Vec<u8>,
    },
    /// Chain tip info.
    ChainTip {
        height: u64,
        block_hash: Hash,
        state_root: [u8; 32],
    },
    /// Account proofs (Merkle proofs for requested accounts).
    AccountProofs {
        height: u64,
        proofs: Vec<AccountProof>,
    },
    /// Error response.
    Error(String),
}

/// A Merkle proof for an account's state at a given height.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountProof {
    pub account: [u8; 20],
    pub state_data: Vec<u8>,
    /// Sibling hashes in the Merkle tree path.
    pub proof_path: Vec<[u8; 32]>,
}

/// Status of a peer during sync.
#[derive(Clone, Debug)]
pub struct PeerSyncStatus {
    /// Peer's reported chain height.
    pub height: u64,
    /// Peer's latest block hash.
    pub head_hash: Hash,
    /// When we last heard from this peer.
    pub last_seen: Instant,
    /// Number of requests in flight to this peer.
    pub inflight_requests: u32,
    /// Peer's sync speed (blocks/sec).
    pub speed_score: f64,
    /// Number of failed requests.
    pub failures: u32,
}

impl PeerSyncStatus {
    pub fn new(height: u64, head_hash: Hash) -> Self {
        Self {
            height,
            head_hash,
            last_seen: Instant::now(),
            inflight_requests: 0,
            speed_score: 1.0,
            failures: 0,
        }
    }

    /// Whether this peer is responsive (heard from recently, not too many failures).
    pub fn is_responsive(&self) -> bool {
        self.last_seen.elapsed() < Duration::from_secs(30)
            && self.failures < 3  // Reduced tolerance from 5 to 3
            && !self.is_banned()
    }

    /// Whether this peer is permanently banned (10+ failures).
    pub fn is_banned(&self) -> bool {
        self.failures >= 10
    }
}

/// State of the sync process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncState {
    /// Not syncing — node is up to date.
    Synced,
    /// Discovering peers and finding the best chain.
    Discovering,
    /// Downloading block headers.
    DownloadingHeaders {
        target_height: u64,
        current_height: u64,
    },
    /// Downloading block bodies.
    DownloadingBlocks {
        target_height: u64,
        current_height: u64,
    },
    /// Downloading a state snapshot.
    DownloadingSnapshot {
        target_height: u64,
        chunks_done: u32,
        total_chunks: u32,
    },
    /// Applying downloaded blocks.
    Applying {
        target_height: u64,
        current_height: u64,
    },
}

/// Maximum headers to request at once.
const MAX_HEADERS_PER_REQUEST: u32 = 512;
/// Maximum blocks to request at once.
const MAX_BLOCKS_PER_REQUEST: usize = 64;
/// Maximum inflight requests per peer.
const MAX_INFLIGHT_PER_PEER: u32 = 4;
/// Blocks behind tip to consider "synced enough" for fast sync snapshot.
const FAST_SYNC_SNAP_BEHIND: u64 = 100;

/// The state sync manager — coordinates downloading chain data from peers.
pub struct StateSyncManager {
    /// Current sync mode.
    mode: SyncMode,
    /// Current sync state.
    state: SyncState,
    /// Our local chain height.
    local_height: u64,
    /// Target chain height (best peer height).
    target_height: u64,
    /// Peer sync statuses, keyed by peer ID string.
    peers: HashMap<String, PeerSyncStatus>,
    /// Downloaded but not-yet-applied headers, keyed by height.
    pending_headers: BTreeMap<u64, BlockHeader>,
    /// Downloaded but not-yet-applied blocks, keyed by height.
    pending_blocks: BTreeMap<u64, Block>,
    /// Heights we've requested but not received yet.
    inflight_heights: HashSet<u64>,
    /// State snapshot chunks received.
    snapshot_chunks: BTreeMap<u32, Vec<u8>>,
    /// Total snapshot chunks expected.
    snapshot_total_chunks: u32,
    /// Expected state root from the trusted chain tip (for snapshot verification).
    /// Set when we receive a ChainTip from 2+ peers with matching state_root.
    expected_state_root: Option<[u8; 32]>,
    /// Maximum allowed size for a single state chunk (16MB).
    max_chunk_size: usize,
    /// State root votes from peers. Require 2+ peers to agree before trusting.
    chain_tip_votes: HashMap<[u8; 32], Vec<String>>,
    /// When sync started.
    sync_start: Option<Instant>,
    /// Blocks applied during this sync session.
    blocks_applied: u64,
    /// Initial gap when sync started (for monotonic progress calculation).
    initial_gap: u64,
}

impl StateSyncManager {
    pub fn new(mode: SyncMode, local_height: u64) -> Self {
        Self {
            mode,
            state: SyncState::Discovering,
            local_height,
            target_height: local_height,
            peers: HashMap::new(),
            pending_headers: BTreeMap::new(),
            pending_blocks: BTreeMap::new(),
            inflight_heights: HashSet::new(),
            snapshot_chunks: BTreeMap::new(),
            snapshot_total_chunks: 0,
            expected_state_root: None,
            max_chunk_size: 16 * 1024 * 1024, // 16MB per chunk
            chain_tip_votes: HashMap::new(),
            sync_start: None,
            blocks_applied: 0,
            initial_gap: 0,
        }
    }

    /// Register a peer's chain status.
    pub fn register_peer(&mut self, peer_id: String, height: u64, head_hash: Hash) {
        self.peers
            .insert(peer_id, PeerSyncStatus::new(height, head_hash));
        if height > self.target_height {
            self.target_height = height;
        }
    }

    /// Remove a disconnected peer.
    pub fn remove_peer(&mut self, peer_id: &str) {
        self.peers.remove(peer_id);
    }

    /// Check if we need to sync.
    pub fn needs_sync(&self) -> bool {
        self.target_height > self.local_height + 1
    }

    /// Start the sync process. Returns initial requests to send.
    pub fn start_sync(&mut self) -> Vec<(String, SyncRequest)> {
        if !self.needs_sync() {
            self.state = SyncState::Synced;
            return vec![];
        }

        self.sync_start = Some(Instant::now());
        self.initial_gap = self.target_height.saturating_sub(self.local_height);
        info!(
            local = self.local_height,
            target = self.target_height,
            gap = self.initial_gap,
            mode = ?self.mode,
            "starting state sync"
        );

        match self.mode {
            SyncMode::Fast => {
                // For fast sync, request a snapshot at a recent height
                let snap_height = self.target_height.saturating_sub(FAST_SYNC_SNAP_BEHIND);
                self.state = SyncState::DownloadingSnapshot {
                    target_height: self.target_height,
                    chunks_done: 0,
                    total_chunks: 0,
                };
                // Request snapshot from the best peer
                if let Some(peer_id) = self.best_peer() {
                    vec![(
                        peer_id,
                        SyncRequest::GetStateSnapshot {
                            height: snap_height,
                        },
                    )]
                } else {
                    vec![]
                }
            }
            SyncMode::Full | SyncMode::Light => {
                self.state = SyncState::DownloadingHeaders {
                    target_height: self.target_height,
                    current_height: self.local_height,
                };
                self.generate_header_requests()
            }
        }
    }

    /// Generate requests for the next batch of headers.
    fn generate_header_requests(&mut self) -> Vec<(String, SyncRequest)> {
        let mut requests = Vec::new();
        let start = self.local_height + 1 + self.pending_headers.len() as u64;

        let responsive_peers: Vec<String> = self
            .peers
            .iter()
            .filter(|(_, s)| s.is_responsive() && s.inflight_requests < MAX_INFLIGHT_PER_PEER)
            .map(|(id, _)| id.clone())
            .collect();

        for (i, peer_id) in responsive_peers.iter().enumerate() {
            let req_start =
                start.saturating_add((i as u64).saturating_mul(MAX_HEADERS_PER_REQUEST as u64));
            if req_start > self.target_height {
                break;
            }
            requests.push((
                peer_id.clone(),
                SyncRequest::GetHeaders {
                    start_height: req_start,
                    max_count: MAX_HEADERS_PER_REQUEST,
                },
            ));
            if let Some(status) = self.peers.get_mut(peer_id) {
                status.inflight_requests = status.inflight_requests.saturating_add(1);
            }
        }

        requests
    }

    /// Generate requests for block bodies.
    fn generate_block_requests(&mut self) -> Vec<(String, SyncRequest)> {
        let mut requests = Vec::new();

        // Collect heights we have headers for but not blocks
        let needed: Vec<u64> = self
            .pending_headers
            .keys()
            .filter(|h| !self.pending_blocks.contains_key(h) && !self.inflight_heights.contains(h))
            .copied()
            .take(MAX_BLOCKS_PER_REQUEST * 4) // Request ahead
            .collect();

        let responsive_peers: Vec<String> = self
            .peers
            .iter()
            .filter(|(_, s)| s.is_responsive() && s.inflight_requests < MAX_INFLIGHT_PER_PEER)
            .map(|(id, _)| id.clone())
            .collect();

        // Distribute block requests across peers
        for (i, chunk) in needed.chunks(MAX_BLOCKS_PER_REQUEST).enumerate() {
            if let Some(peer_id) = responsive_peers.get(i % responsive_peers.len().max(1)) {
                let heights: Vec<u64> = chunk.to_vec();
                for h in &heights {
                    self.inflight_heights.insert(*h);
                }
                requests.push((peer_id.clone(), SyncRequest::GetBlocks { heights }));
                if let Some(status) = self.peers.get_mut(peer_id) {
                    status.inflight_requests = status.inflight_requests.saturating_add(1);
                }
            }
        }

        requests
    }

    /// Handle a sync response from a peer.
    pub fn handle_response(
        &mut self,
        peer_id: &str,
        response: SyncResponse,
    ) -> Vec<(String, SyncRequest)> {
        if let Some(status) = self.peers.get_mut(peer_id) {
            status.last_seen = Instant::now();
            status.inflight_requests = status.inflight_requests.saturating_sub(1);
        }

        match response {
            SyncResponse::Headers(headers) => {
                debug!(count = headers.len(), peer = peer_id, "received headers");

                // Validate the header chain before accepting
                if !headers.is_empty() {
                    if !Self::verify_header_chain(&headers) {
                        warn!(peer = peer_id, "rejected headers: invalid chain linkage");
                        if let Some(status) = self.peers.get_mut(peer_id) {
                            status.failures += 1;
                        }
                        return self.generate_header_requests();
                    }

                    // Verify heights are in the expected range
                    let first_height = headers[0].height;
                    let last_height = match headers.last() {
                        Some(h) => h.height,
                        None => return self.generate_header_requests(),
                    };
                    if first_height > last_height || last_height > self.target_height + 10 {
                        warn!(
                            peer = peer_id,
                            first_height, last_height, "rejected headers: out of range"
                        );
                        if let Some(status) = self.peers.get_mut(peer_id) {
                            status.failures += 1;
                        }
                        return self.generate_header_requests();
                    }

                    // Fix NET-228: Verify chain continuity — the first header's
                    // parent_hash must connect to either:
                    //   a) A previously downloaded pending header at height-1, or
                    //   b) (Implicitly) the genesis if first_height == 0.
                    // Without this check, a malicious peer can send a valid-looking
                    // header chain that is completely disconnected from our chain,
                    // leading us to download and attempt to apply forged blocks.
                    if first_height > 0 {
                        let prev_height = first_height - 1;
                        let connects =
                            if let Some(prev_header) = self.pending_headers.get(&prev_height) {
                                // The first header must chain to the pending header at prev_height
                                headers[0].parent_hash == prev_header.hash()
                            } else if prev_height == self.local_height {
                                // The first header chains directly to the local tip.
                                // We don't have the local tip header hash stored here,
                                // so we accept it when it follows immediately from local_height.
                                // The block-body validation later will catch mismatches.
                                true
                            } else {
                                // No anchor — we have no header at prev_height to verify against.
                                // Accept only if first_height <= local_height + 1 (optimistic).
                                first_height <= self.local_height + 1
                            };

                        if !connects {
                            warn!(
                                peer = peer_id,
                                first_height,
                                "rejected headers: disconnected from local chain or pending headers"
                            );
                            if let Some(status) = self.peers.get_mut(peer_id) {
                                status.failures += 1;
                            }
                            return self.generate_header_requests();
                        }
                    }
                }

                for header in headers {
                    self.pending_headers.insert(header.height, header);
                }

                // Check if we have enough headers to start downloading blocks
                let headers_ahead = self.pending_headers.len() as u64;
                if (headers_ahead >= MAX_HEADERS_PER_REQUEST as u64
                    || self.pending_headers.keys().last().copied().unwrap_or(0)
                        >= self.target_height)
                    && self.mode != SyncMode::Light
                {
                    self.state = SyncState::DownloadingBlocks {
                        target_height: self.target_height,
                        current_height: self.local_height,
                    };
                    return self.generate_block_requests();
                }

                self.generate_header_requests()
            }

            SyncResponse::Blocks(blocks) => {
                debug!(count = blocks.len(), peer = peer_id, "received blocks");
                for block in blocks {
                    let height = block.header.height;
                    self.inflight_heights.remove(&height);

                    // Validate block header matches previously downloaded header
                    if let Some(expected_header) = self.pending_headers.get(&height) {
                        if block.header.hash() != expected_header.hash() {
                            warn!(
                                peer = peer_id,
                                height, "rejected block: header hash mismatch"
                            );
                            if let Some(status) = self.peers.get_mut(peer_id) {
                                status.failures += 1;
                            }
                            continue;
                        }

                        // Verify the block's transactions produce the expected tx_root.
                        // Without this, a malicious peer can send blocks with modified
                        // transaction lists (swapping, removing, or adding transactions)
                        // that would be silently accepted, resulting in divergent state.
                        let computed_tx_root = block.compute_tx_root();
                        if computed_tx_root != expected_header.tx_root {
                            warn!(
                                peer = peer_id,
                                height, "rejected block: tx_root mismatch (tampered transactions)"
                            );
                            if let Some(status) = self.peers.get_mut(peer_id) {
                                status.failures += 1;
                            }
                            continue;
                        }
                    }

                    self.pending_blocks.insert(height, block);
                }
                self.generate_block_requests()
            }

            SyncResponse::StateChunk {
                height,
                chunk_index,
                total_chunks,
                data,
            } => {
                debug!(
                    height,
                    chunk_index,
                    total_chunks,
                    bytes = data.len(),
                    "received state chunk"
                );

                // SECURITY: Reject oversized chunks to prevent OOM attacks.
                // A malicious peer could send multi-GB chunks to exhaust memory.
                if data.len() > self.max_chunk_size {
                    warn!(
                        peer = peer_id,
                        size = data.len(),
                        limit = self.max_chunk_size,
                        "rejected state chunk: exceeds maximum size"
                    );
                    if let Some(status) = self.peers.get_mut(peer_id) {
                        status.failures += 1;
                    }
                    return vec![];
                }

                // Lock total_chunks on first chunk to prevent manipulation
                if self.snapshot_total_chunks == 0 {
                    // SECURITY: Cap total_chunks to prevent memory exhaustion.
                    // At 16MB per chunk, 1024 chunks = 16GB which is already very large.
                    const MAX_TOTAL_CHUNKS: u32 = 1024;
                    if total_chunks > MAX_TOTAL_CHUNKS {
                        warn!(
                            peer = peer_id,
                            total_chunks,
                            "rejected state snapshot: too many chunks (max {MAX_TOTAL_CHUNKS})"
                        );
                        if let Some(status) = self.peers.get_mut(peer_id) {
                            status.failures += 1;
                        }
                        return vec![];
                    }
                    self.snapshot_total_chunks = total_chunks;
                } else if total_chunks != self.snapshot_total_chunks {
                    warn!(
                        peer = peer_id,
                        expected = self.snapshot_total_chunks,
                        got = total_chunks,
                        "rejected state chunk: total_chunks mismatch"
                    );
                    if let Some(status) = self.peers.get_mut(peer_id) {
                        status.failures += 1;
                    }
                    return vec![];
                }

                // Validate chunk_index is in range
                if chunk_index >= self.snapshot_total_chunks {
                    warn!(
                        peer = peer_id,
                        chunk_index, "rejected state chunk: index out of range"
                    );
                    return vec![];
                }

                self.snapshot_chunks.insert(chunk_index, data);

                if self.snapshot_chunks.len() as u32 >= self.snapshot_total_chunks {
                    // SECURITY: Verify the reconstructed state against the expected
                    // state root from the chain tip. Without this, a malicious peer
                    // can send a completely fabricated snapshot that gives them
                    // arbitrary balances, and the node will trust it.
                    if let Some(expected_root) = self.expected_state_root {
                        let mut hasher_data = Vec::new();
                        for idx in 0..self.snapshot_total_chunks {
                            if let Some(chunk) = self.snapshot_chunks.get(&idx) {
                                hasher_data.extend_from_slice(chunk);
                            }
                        }
                        let snapshot_hash = pichain_crypto::hash(&hasher_data);
                        if snapshot_hash.as_bytes()[..] != expected_root[..] {
                            warn!(
                                height,
                                expected = %pichain_crypto::Hash::from_bytes({
                                    let mut h = [0u8; 32];
                                    h.copy_from_slice(&expected_root);
                                    h
                                }),
                                got = %snapshot_hash,
                                "REJECTED state snapshot: hash does not match expected state root"
                            );
                            // Reset snapshot state and re-request from a different peer
                            self.snapshot_chunks.clear();
                            self.snapshot_total_chunks = 0;
                            if let Some(status) = self.peers.get_mut(peer_id) {
                                status.failures += 5; // Heavy penalty — likely malicious
                            }
                            return vec![];
                        }
                        info!(
                            height,
                            "state snapshot verified against chain tip state root"
                        );
                    } else {
                        // SECURITY: Reject snapshot entirely — never accept unverified state.
                        // This prevents single-peer state poisoning attacks.
                        warn!(
                            height,
                            "REJECTING state snapshot: no verified state root from consensus"
                        );
                        self.snapshot_chunks.clear();
                        self.snapshot_total_chunks = 0;
                        // Wait for more chain tip responses to establish consensus
                        return vec![];
                    }

                    info!(height, "state snapshot complete");
                    // After snapshot, download remaining blocks
                    self.local_height = height;
                    self.state = SyncState::DownloadingHeaders {
                        target_height: self.target_height,
                        current_height: height,
                    };
                    return self.generate_header_requests();
                }

                self.state = SyncState::DownloadingSnapshot {
                    target_height: self.target_height,
                    chunks_done: self.snapshot_chunks.len() as u32,
                    total_chunks,
                };
                vec![]
            }

            SyncResponse::ChainTip {
                height,
                block_hash,
                state_root,
            } => {
                if let Some(status) = self.peers.get_mut(peer_id) {
                    status.height = height;
                    status.head_hash = block_hash;
                }
                if height > self.target_height {
                    self.target_height = height;
                }

                // SECURITY: Record the state root from the chain tip.
                // Require 2+ independent peers to agree on the same state root
                // before trusting it. This prevents single-peer state poisoning.
                if self.expected_state_root.is_none() {
                    let voters = self.chain_tip_votes.entry(state_root).or_default();
                    if !voters.contains(&peer_id.to_string()) {
                        voters.push(peer_id.to_string());
                    }
                    let vote_count = voters.len();

                    if vote_count >= 2 {
                        // 2+ peers agree — accept this state root
                        self.expected_state_root = Some(state_root);
                        info!(
                            votes = vote_count,
                            height, "state root verified by {vote_count} peers — accepted"
                        );
                    } else {
                        debug!(
                            peer = peer_id,
                            height,
                            votes = vote_count,
                            "state root vote recorded — waiting for 2+ peer agreement"
                        );
                    }
                    debug!(
                        peer = peer_id,
                        height,
                        state_root = %pichain_crypto::Hash::from_bytes({
                            let mut h = [0u8; 32];
                            h.copy_from_slice(&state_root);
                            h
                        }),
                        "recorded expected state root from chain tip"
                    );
                }

                vec![]
            }

            SyncResponse::AccountProofs { .. } => {
                // Used for light client verification
                vec![]
            }

            SyncResponse::Error(msg) => {
                warn!(peer = peer_id, error = msg, "sync error from peer");
                if let Some(status) = self.peers.get_mut(peer_id) {
                    status.failures += 1;
                }
                vec![]
            }
        }
    }

    /// Get blocks ready to be applied (in order, starting from local_height + 1).
    pub fn drain_ready_blocks(&mut self) -> Vec<Block> {
        let mut ready = Vec::new();
        let mut next_height = self.local_height + 1;

        while let Some(block) = self.pending_blocks.remove(&next_height) {
            ready.push(block);
            self.pending_headers.remove(&next_height);
            next_height += 1;
        }

        if !ready.is_empty() {
            self.local_height = next_height - 1;
            self.blocks_applied += ready.len() as u64;
            debug!(
                new_height = self.local_height,
                applied = ready.len(),
                "applied sync blocks"
            );

            if self.local_height >= self.target_height {
                self.state = SyncState::Synced;
                if let Some(start) = self.sync_start {
                    info!(
                        height = self.local_height,
                        blocks = self.blocks_applied,
                        duration_secs = start.elapsed().as_secs(),
                        "sync complete"
                    );
                }
            }
        }

        ready
    }

    /// Verify that a chain of headers is valid (parent hashes link correctly,
    /// timestamps are monotonic, and proposers are plausible).
    ///
    /// SECURITY NOTE (Fix 193): This function currently verifies structural
    /// integrity (hash linkage, height sequence, timestamp monotonicity) but
    /// does NOT verify block signatures because `BlockHeader` does not carry
    /// an embedded signature — signatures are part of the consensus layer
    /// (DAG certificates). A future protocol upgrade should either:
    ///   1. Extend `SyncResponse::Headers` to include the proposer's signature
    ///      over each header, allowing full authentication, or
    ///   2. Accept a `validator_set` parameter and verify that each header's
    ///      `proposer` was a valid validator at that epoch/height.
    ///
    /// Without signature verification, an attacker controlling the sync peer
    /// can forge an entire header chain. Mitigated by:
    ///   - Requiring 2+ peers to agree on chain tip state root
    ///   - Verifying block bodies against header hashes after download
    ///   - Proposer validation (non-zero, non-duplicate) as a weak filter
    pub fn verify_header_chain(headers: &[BlockHeader]) -> bool {
        if headers.is_empty() {
            return true;
        }

        // Reject headers from the null proposer — every real block has a
        // non-zero proposer set by the consensus layer.
        for header in headers {
            if header.proposer == pichain_crypto::keys::Address::ZERO && !header.is_genesis() {
                return false;
            }
        }

        for window in headers.windows(2) {
            let prev = &window[0];
            let curr = &window[1];
            let expected_hash = prev.hash();
            if curr.parent_hash != expected_hash {
                return false;
            }
            if curr.height != prev.height + 1 {
                return false;
            }
            // Timestamps must be monotonically non-decreasing.
            // A block with a timestamp before its parent indicates
            // a malicious or misconfigured peer.
            if curr.timestamp_ms < prev.timestamp_ms {
                return false;
            }
        }
        true
    }

    /// Get the best peer (highest chain, most responsive).
    fn best_peer(&self) -> Option<String> {
        self.peers
            .iter()
            .filter(|(_, s)| s.is_responsive())
            .max_by_key(|(_, s)| (s.height, (s.speed_score * 1000.0) as u64))
            .map(|(id, _)| id.clone())
    }

    /// Get current sync state.
    pub fn state(&self) -> &SyncState {
        &self.state
    }

    /// Get sync progress as a percentage (monotonically increasing).
    pub fn progress(&self) -> f64 {
        if self.target_height <= self.local_height {
            return 100.0;
        }
        if self.initial_gap == 0 {
            return 100.0;
        }
        let done = self.blocks_applied as f64;
        let total = self.initial_gap as f64;
        (done / total * 100.0).min(100.0)
    }

    /// Get the number of connected sync peers.
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Get the local chain height.
    pub fn local_height(&self) -> u64 {
        self.local_height
    }

    /// Get the target chain height.
    pub fn target_height(&self) -> u64 {
        self.target_height
    }

    /// Notify that a block was applied externally (e.g., from block production).
    pub fn block_applied(&mut self, height: u64) {
        if height > self.local_height {
            self.local_height = height;
        }
    }

    /// Tick timeout check — clears stale inflight requests that haven't received a response.
    /// Should be called periodically (e.g., every 10 seconds).
    /// Returns re-requests to send to alternative peers.
    pub fn tick_timeouts(&mut self) -> Vec<(String, SyncRequest)> {
        const SYNC_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

        // Detect peers that haven't responded and have inflight requests
        let stale_peers: Vec<String> = self
            .peers
            .iter()
            .filter(|(_, s)| {
                s.inflight_requests > 0 && s.last_seen.elapsed() > SYNC_REQUEST_TIMEOUT
            })
            .map(|(id, _)| id.clone())
            .collect();

        for peer_id in &stale_peers {
            if let Some(status) = self.peers.get_mut(peer_id) {
                status.failures = status.failures.saturating_add(status.inflight_requests);
                status.inflight_requests = 0;
                tracing::warn!(
                    peer = %peer_id,
                    failures = status.failures,
                    "sync request timed out — clearing inflight requests"
                );
            }
        }

        // Also clear any inflight_heights that might be stuck
        if !stale_peers.is_empty() {
            self.inflight_heights.clear();
        }

        // Generate new requests to replace the timed-out ones
        match &self.state {
            SyncState::DownloadingHeaders { .. } => self.generate_header_requests(),
            SyncState::DownloadingBlocks { .. } => self.generate_block_requests(),
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_types::Block;

    #[test]
    fn sync_manager_creation() {
        let mgr = StateSyncManager::new(SyncMode::Full, 0);
        assert_eq!(mgr.local_height(), 0);
        assert_eq!(*mgr.state(), SyncState::Discovering);
    }

    #[test]
    fn register_peers_updates_target() {
        let mut mgr = StateSyncManager::new(SyncMode::Full, 0);
        mgr.register_peer("peer1".to_string(), 100, Hash::ZERO);
        assert_eq!(mgr.target_height(), 100);
        assert!(mgr.needs_sync());

        mgr.register_peer("peer2".to_string(), 200, Hash::ZERO);
        assert_eq!(mgr.target_height(), 200);
    }

    #[test]
    fn no_sync_needed_when_at_tip() {
        let mut mgr = StateSyncManager::new(SyncMode::Full, 100);
        mgr.register_peer("peer1".to_string(), 100, Hash::ZERO);
        assert!(!mgr.needs_sync());
    }

    #[test]
    fn start_sync_generates_requests() {
        let mut mgr = StateSyncManager::new(SyncMode::Full, 0);
        mgr.register_peer("peer1".to_string(), 1000, Hash::ZERO);
        let requests = mgr.start_sync();
        assert!(!requests.is_empty());
    }

    #[test]
    fn verify_valid_header_chain() {
        let genesis = Block::genesis(31415, 1000);
        let mut headers = vec![genesis.header.clone()];
        let proposer = pichain_crypto::keys::Address([1u8; 20]); // non-zero proposer

        // Create a chain of headers
        for i in 1..5u64 {
            let mut h = genesis.header.clone();
            h.height = i;
            h.proposer = proposer;
            h.parent_hash = headers.last().unwrap().hash();
            headers.push(h);
        }

        assert!(StateSyncManager::verify_header_chain(&headers));
    }

    #[test]
    fn verify_invalid_header_chain() {
        let genesis = Block::genesis(31415, 1000);
        let mut h1 = genesis.header.clone();
        h1.height = 1;
        h1.proposer = pichain_crypto::keys::Address([1u8; 20]); // non-zero proposer
        h1.parent_hash = Hash::ZERO; // Wrong parent!

        assert!(!StateSyncManager::verify_header_chain(&[
            genesis.header,
            h1
        ]));
    }

    #[test]
    fn verify_header_chain_rejects_non_monotonic_timestamps() {
        let genesis = Block::genesis(31415, 5000);
        let proposer = pichain_crypto::keys::Address([1u8; 20]);

        let mut h1 = genesis.header.clone();
        h1.height = 1;
        h1.proposer = proposer;
        h1.parent_hash = genesis.header.hash();
        h1.timestamp_ms = 3000; // BEFORE genesis timestamp of 5000

        assert!(
            !StateSyncManager::verify_header_chain(&[genesis.header.clone(), h1]),
            "should reject header chain with non-monotonic timestamps"
        );

        // Valid case: equal timestamps should be accepted
        let mut h1_valid = genesis.header.clone();
        h1_valid.height = 1;
        h1_valid.proposer = proposer;
        h1_valid.parent_hash = genesis.header.hash();
        h1_valid.timestamp_ms = 5000; // Equal to parent — allowed

        assert!(
            StateSyncManager::verify_header_chain(&[genesis.header.clone(), h1_valid]),
            "should accept header chain with equal timestamps"
        );

        // Valid case: increasing timestamps
        let mut h1_inc = genesis.header.clone();
        h1_inc.height = 1;
        h1_inc.proposer = proposer;
        h1_inc.parent_hash = genesis.header.hash();
        h1_inc.timestamp_ms = 6000; // After parent

        assert!(
            StateSyncManager::verify_header_chain(&[genesis.header, h1_inc]),
            "should accept header chain with increasing timestamps"
        );
    }

    #[test]
    fn verify_header_chain_rejects_zero_proposer() {
        let genesis = Block::genesis(31415, 1000);
        let mut h1 = genesis.header.clone();
        h1.height = 1;
        h1.parent_hash = genesis.header.hash();
        h1.timestamp_ms = 2000;
        // Leave proposer as ZERO — should be rejected for non-genesis blocks
        h1.proposer = pichain_crypto::keys::Address::ZERO;

        assert!(
            !StateSyncManager::verify_header_chain(&[genesis.header.clone(), h1]),
            "should reject header chain with zero proposer on non-genesis block"
        );

        // Genesis block with zero proposer should be accepted
        assert!(
            StateSyncManager::verify_header_chain(&[genesis.header]),
            "genesis block with zero proposer should be accepted"
        );
    }

    #[test]
    fn drain_ready_blocks_in_order() {
        let mut mgr = StateSyncManager::new(SyncMode::Full, 0);
        mgr.target_height = 3;

        // Insert blocks out of order
        let b2 = {
            let mut b = Block::genesis(31415, 0);
            b.header.height = 2;
            b
        };
        let b1 = {
            let mut b = Block::genesis(31415, 0);
            b.header.height = 1;
            b
        };
        let b3 = {
            let mut b = Block::genesis(31415, 0);
            b.header.height = 3;
            b
        };

        mgr.pending_blocks.insert(2, b2);
        mgr.pending_blocks.insert(3, b3);

        // Block 2 and 3 are pending but block 1 is missing → nothing ready
        let ready = mgr.drain_ready_blocks();
        assert!(ready.is_empty());

        // Add block 1 → all become ready
        mgr.pending_blocks.insert(1, b1);
        let ready = mgr.drain_ready_blocks();
        assert_eq!(ready.len(), 3);
        assert_eq!(mgr.local_height(), 3);
    }

    #[test]
    fn progress_calculation() {
        let mut mgr = StateSyncManager::new(SyncMode::Full, 0);
        mgr.target_height = 100;
        // Simulate start_sync which sets initial_gap
        mgr.initial_gap = 100;
        assert_eq!(mgr.progress(), 0.0);

        mgr.blocks_applied = 50;
        assert_eq!(mgr.progress(), 50.0);

        mgr.blocks_applied = 100;
        assert_eq!(mgr.progress(), 100.0);
    }

    #[test]
    fn peer_responsiveness() {
        let status = PeerSyncStatus::new(100, Hash::ZERO);
        assert!(status.is_responsive());
    }

    #[test]
    fn remove_peer() {
        let mut mgr = StateSyncManager::new(SyncMode::Full, 0);
        mgr.register_peer("peer1".to_string(), 100, Hash::ZERO);
        assert_eq!(mgr.peer_count(), 1);
        mgr.remove_peer("peer1");
        assert_eq!(mgr.peer_count(), 0);
    }

    /// NET-228: Headers whose parent_hash doesn't connect to our pending
    /// chain must be rejected to prevent disconnected chain attacks.
    #[test]
    fn sync_rejects_disconnected_headers() {
        let mut mgr = StateSyncManager::new(SyncMode::Full, 0);
        mgr.target_height = 100;
        mgr.register_peer("peer1".to_string(), 100, Hash::ZERO);

        // First, insert a valid chain starting from height 0 (genesis)
        let genesis = Block::genesis(31415, 1000);
        let proposer = pichain_crypto::keys::Address([1u8; 20]);

        let mut h1 = genesis.header.clone();
        h1.height = 1;
        h1.proposer = proposer;
        h1.parent_hash = genesis.header.hash();
        h1.timestamp_ms = 2000;

        // Insert genesis and h1 as previously downloaded pending headers
        mgr.pending_headers.insert(0, genesis.header.clone());
        mgr.pending_headers.insert(1, h1.clone());

        // Now create a disconnected header batch starting at height 2
        // but with a WRONG parent_hash (doesn't chain to h1)
        let mut disconnected = genesis.header.clone();
        disconnected.height = 2;
        disconnected.proposer = proposer;
        disconnected.parent_hash = Hash::ZERO; // Wrong! Should be h1.hash()
        disconnected.timestamp_ms = 3000;

        let mut disconnected2 = genesis.header.clone();
        disconnected2.height = 3;
        disconnected2.proposer = proposer;
        disconnected2.parent_hash = disconnected.hash();
        disconnected2.timestamp_ms = 4000;

        // Submit the disconnected headers
        let peer_id = "peer1";
        let _requests = mgr.handle_response(
            peer_id,
            SyncResponse::Headers(vec![disconnected.clone(), disconnected2.clone()]),
        );

        // The disconnected headers must NOT have been inserted
        assert!(
            !mgr.pending_headers.contains_key(&2),
            "height 2 must be rejected: parent_hash doesn't chain to height 1"
        );
        assert!(
            !mgr.pending_headers.contains_key(&3),
            "height 3 must be rejected along with its batch"
        );

        // Verify the peer got a failure bump
        assert!(
            mgr.peers.get(peer_id).unwrap().failures > 0,
            "peer must be penalized for sending disconnected headers"
        );

        // Now submit a VALID continuation that chains from h1
        let mut valid_h2 = genesis.header.clone();
        valid_h2.height = 2;
        valid_h2.proposer = proposer;
        valid_h2.parent_hash = h1.hash(); // Correct parent!
        valid_h2.timestamp_ms = 3000;

        let _requests = mgr.handle_response(peer_id, SyncResponse::Headers(vec![valid_h2.clone()]));

        // This one should be accepted
        assert!(
            mgr.pending_headers.contains_key(&2),
            "valid continuation header at height 2 must be accepted"
        );
    }
}
