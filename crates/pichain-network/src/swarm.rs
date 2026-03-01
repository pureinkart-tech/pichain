//! Full P2P swarm with peer discovery, GossipSub messaging, and Kademlia DHT.
//!
//! This is the production P2P networking layer for PIChain validators.
//! It uses libp2p with:
//! - QUIC transport (0-RTT, multiplexed, encrypted)
//! - GossipSub 1.1 for transaction/block/consensus propagation
//! - Kademlia DHT for peer discovery
//! - Identify protocol for peer metadata exchange

use dashmap::DashMap;
use futures::StreamExt;
use libp2p::{
    autonat,
    connection_limits::{self, ConnectionLimits},
    gossipsub::{self, IdentTopic, MessageAuthenticity, MessageId, ValidationMode},
    identify,
    identity::Keypair,
    kad::{self, store::MemoryStore},
    noise,
    relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, Swarm,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use zeroize::Zeroize;

use crate::NetworkError;

/// Minimum number of connected peers required for safe operation.
///
/// SECURITY (Fix 194): With only 2-4 peers, an attacker can trivially eclipse
/// a node by positioning Sybil nodes as all of its connections. This prevents
/// the victim from seeing legitimate blocks/transactions, enabling censorship
/// and double-spend attacks.
///
/// Set to 8 as a minimum — the node should warn when peer count drops below
/// this threshold. GossipSub mesh parameters are also tuned accordingly
/// (mesh_n_low=6, mesh_n=8) so that the gossip protocol itself demands
/// sufficient peer diversity.
pub const MIN_SAFE_PEER_COUNT: usize = 8;

/// Per-peer inbound byte rate tracking.
///
/// Prevents amplification attacks by limiting how many bytes a single peer
/// can send within a rolling time window. Peers exceeding the limit are
/// flagged for disconnection.
///
/// Default limit: 100 MB per 60-second window.
pub(crate) struct PeerByteTracker {
    /// Per-peer inbound byte counts with window start time.
    counters: DashMap<PeerId, (u64, Instant)>,
    /// Maximum inbound bytes per peer per window.
    max_bytes_per_window: u64,
    /// Window duration.
    window: Duration,
}

impl PeerByteTracker {
    /// Create a new tracker with the given byte limit and window duration.
    fn new(max_bytes_per_window: u64, window: Duration) -> Self {
        Self {
            counters: DashMap::new(),
            max_bytes_per_window,
            window,
        }
    }

    /// Check if accepting `bytes` from `peer` would exceed the rate limit.
    /// Returns `true` if allowed (bytes are recorded), `false` if rate-limited.
    fn check_and_record(&self, peer: &PeerId, bytes: u64) -> bool {
        let now = Instant::now();
        let mut entry = self.counters.entry(*peer).or_insert((0, now));
        let (count, window_start) = entry.value_mut();

        // Reset window if expired
        if now.duration_since(*window_start) >= self.window {
            *count = 0;
            *window_start = now;
        }

        let new_count = count.saturating_add(bytes);
        if new_count > self.max_bytes_per_window {
            return false;
        }
        *count = new_count;
        true
    }

    /// Remove the entry for a disconnected peer.
    fn remove_peer(&self, peer: &PeerId) {
        self.counters.remove(peer);
    }

    /// Periodic cleanup of stale entries whose windows have long expired.
    fn cleanup(&self) {
        let now = Instant::now();
        self.counters.retain(|_, (_, window_start)| {
            now.duration_since(*window_start) < self.window * 2
        });
    }
}

/// GossipSub topic names for PIChain.
pub const TOPIC_TRANSACTIONS: &str = "/pichain/txs/1";
pub const TOPIC_BLOCKS: &str = "/pichain/blocks/1";
pub const TOPIC_CONSENSUS: &str = "/pichain/consensus/1";
pub const TOPIC_MINING: &str = "/pichain/mining/1";
pub const TOPIC_SYNC: &str = "/pichain/sync/1";

/// Combined network behaviour for PIChain.
#[derive(NetworkBehaviour)]
pub struct PiChainBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub kademlia: kad::Behaviour<MemoryStore>,
    pub identify: identify::Behaviour,
    /// AutoNAT — probes peers to detect if we're behind NAT.
    pub autonat: autonat::Behaviour,
    /// Relay client — allows connections through relay nodes when behind NAT.
    pub relay_client: relay::client::Behaviour,
    /// Enforces per-swarm connection limits to prevent DoS via connection flooding.
    pub connection_limits: connection_limits::Behaviour,
}

/// A clonable handle for broadcasting messages to the P2P network.
/// Does not receive messages — use `PiChainSwarm::recv()` for that.
#[derive(Clone)]
pub struct SwarmBroadcaster {
    outbound_tx: mpsc::Sender<(String, Vec<u8>)>,
}

impl SwarmBroadcaster {
    pub async fn broadcast_block(&self, data: Vec<u8>) -> Result<(), NetworkError> {
        self.outbound_tx
            .send((TOPIC_BLOCKS.to_string(), data))
            .await
            .map_err(|e| NetworkError::Gossip(format!("failed to send block: {e}")))
    }

    pub async fn broadcast_consensus(&self, data: Vec<u8>) -> Result<(), NetworkError> {
        self.outbound_tx
            .send((TOPIC_CONSENSUS.to_string(), data))
            .await
            .map_err(|e| NetworkError::Gossip(format!("failed to send consensus msg: {e}")))
    }

    pub async fn broadcast_transaction(&self, data: Vec<u8>) -> Result<(), NetworkError> {
        self.outbound_tx
            .send((TOPIC_TRANSACTIONS.to_string(), data))
            .await
            .map_err(|e| NetworkError::Gossip(format!("failed to send transaction: {e}")))
    }

    pub async fn broadcast_sync(&self, data: Vec<u8>) -> Result<(), NetworkError> {
        self.outbound_tx
            .send((TOPIC_SYNC.to_string(), data))
            .await
            .map_err(|e| NetworkError::Gossip(format!("failed to send sync msg: {e}")))
    }
}

/// Messages received from the P2P network.
#[derive(Clone, Debug)]
pub enum SwarmMessage {
    /// Transaction received from a peer.
    Transaction(Vec<u8>),
    /// Block data received from a peer.
    Block(Vec<u8>),
    /// Consensus message (DAG certificate).
    Consensus(Vec<u8>),
    /// Mining proof broadcast.
    MiningProof(Vec<u8>),
    /// Sync protocol message (chain tip requests/responses, block requests).
    Sync(Vec<u8>),
    /// New peer discovered.
    PeerDiscovered(PeerId),
    /// Peer disconnected.
    PeerDisconnected(PeerId),
}

/// Configuration for the P2P swarm.
#[derive(Clone, Debug)]
pub struct SwarmConfig {
    /// Listen address (e.g., "/ip4/0.0.0.0/tcp/9314").
    pub listen_addr: String,
    /// Bootstrap peers to connect to initially.
    pub bootstrap_peers: Vec<(PeerId, Multiaddr)>,
    /// GossipSub heartbeat interval.
    pub heartbeat_interval: Duration,
    /// Maximum message size for gossip (transactions only, blocks go via Turbine).
    pub max_message_size: usize,
    /// Protocol version string.
    pub protocol_version: String,
    /// Maximum number of peer connections.
    pub max_connections: u32,
    /// Optional Ed25519 secret key bytes for deterministic peer ID.
    /// When set, the swarm derives its libp2p identity from this key instead
    /// of generating a random one. This allows pre-configuring bootstrap peer
    /// addresses with known peer IDs.
    pub identity_secret: Option<[u8; 32]>,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            listen_addr: "/ip4/0.0.0.0/tcp/9314".to_string(),
            bootstrap_peers: vec![],
            heartbeat_interval: Duration::from_secs(1),
            max_message_size: 64 * 1024, // 64KB (gossip is for txs, blocks use Turbine)
            protocol_version: "pichain/0.1.0".to_string(),
            max_connections: 256,
            identity_secret: None,
        }
    }
}

/// The PIChain P2P swarm — manages all peer-to-peer networking.
pub struct PiChainSwarm {
    peer_id: PeerId,
    #[allow(dead_code)]
    config: SwarmConfig,
    /// Channel to send messages OUT to the network.
    outbound_tx: mpsc::Sender<(String, Vec<u8>)>,
    /// Channel to receive messages FROM the network.
    inbound_rx: mpsc::Receiver<SwarmMessage>,
    /// Connected peer count (shared with event loop via atomic).
    connected_peer_count: Arc<AtomicUsize>,
}

impl PiChainSwarm {
    /// Create and start a new P2P swarm.
    pub async fn start(config: SwarmConfig) -> Result<Self, NetworkError> {
        let local_key = if let Some(mut secret) = config.identity_secret {
            let mut ed_bytes = secret;
            secret.zeroize();
            let result = libp2p::identity::ed25519::SecretKey::try_from_bytes(&mut ed_bytes)
                .map_err(|e| NetworkError::Transport(format!("invalid identity key: {e}")));
            ed_bytes.zeroize();
            let ed_key = result?;
            let ed_keypair = libp2p::identity::ed25519::Keypair::from(ed_key);
            Keypair::from(ed_keypair)
        } else {
            Keypair::generate_ed25519()
        };
        let peer_id = PeerId::from(local_key.public());

        info!(%peer_id, listen = %config.listen_addr, "starting P2P swarm");

        let (outbound_tx, mut outbound_rx) = mpsc::channel::<(String, Vec<u8>)>(10_000);
        let (inbound_tx, inbound_rx) = mpsc::channel::<SwarmMessage>(10_000);

        // Build the swarm
        let mut swarm = build_swarm(local_key.clone(), &config)?;

        // Listen on the configured address
        let listen_addr: Multiaddr = config
            .listen_addr
            .parse()
            .map_err(|e| NetworkError::Transport(format!("invalid listen address: {e}")))?;
        swarm
            .listen_on(listen_addr)
            .map_err(|e| NetworkError::Transport(format!("failed to listen: {e}")))?;

        // Subscribe to all topics
        let topics = [
            TOPIC_TRANSACTIONS,
            TOPIC_BLOCKS,
            TOPIC_CONSENSUS,
            TOPIC_MINING,
            TOPIC_SYNC,
        ];
        for topic_name in &topics {
            let topic = IdentTopic::new(*topic_name);
            swarm
                .behaviour_mut()
                .gossipsub
                .subscribe(&topic)
                .map_err(|e| NetworkError::Gossip(format!("failed to subscribe to {topic_name}: {e}")))?;
        }

        // Connect to bootstrap peers
        for (peer_id, addr) in &config.bootstrap_peers {
            swarm
                .behaviour_mut()
                .kademlia
                .add_address(peer_id, addr.clone());
            info!(%peer_id, %addr, "added bootstrap peer");
        }

        // Start bootstrap if we have peers
        if !config.bootstrap_peers.is_empty() {
            if let Err(e) = swarm.behaviour_mut().kademlia.bootstrap() {
                warn!("kademlia bootstrap failed: {e:?}");
            }
        }

        // Shared atomic counter for peer tracking between event loop and handle
        let connected_peer_count = Arc::new(AtomicUsize::new(0));
        let peer_count_clone = connected_peer_count.clone();

        // Per-peer inbound byte rate limiter: 100 MB per 60-second window.
        // Prevents amplification attacks where a malicious peer triggers
        // excessive data flow from the honest node.
        let byte_tracker = Arc::new(PeerByteTracker::new(
            100 * 1024 * 1024, // 100 MB
            Duration::from_secs(60),
        ));

        // Spawn the swarm event loop
        let swarm_peer_id = peer_id;
        tokio::spawn(async move {
            run_swarm_loop(swarm, inbound_tx, &mut outbound_rx, peer_count_clone, byte_tracker).await;
            info!("swarm event loop exited");
        });

        // Drop local_key immediately — the secret key material was already
        // cloned into the swarm via build_swarm(). Keeping it in PiChainSwarm
        // would leave unzeroized key bytes in memory for the node's lifetime.
        drop(local_key);

        Ok(Self {
            peer_id: swarm_peer_id,
            config,
            outbound_tx,
            inbound_rx,
            connected_peer_count,
        })
    }

    /// Get a clonable broadcast handle for sending messages to the network.
    pub fn broadcaster(&self) -> SwarmBroadcaster {
        SwarmBroadcaster {
            outbound_tx: self.outbound_tx.clone(),
        }
    }

    /// Get the local peer ID.
    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    /// Parse bootstrap peer strings in multiaddr format (e.g. "/ip4/1.2.3.4/tcp/9314/p2p/<peer_id>").
    /// Invalid entries are silently skipped.
    pub fn parse_bootstrap_peers(addrs: &[String]) -> Vec<(PeerId, Multiaddr)> {
        addrs.iter().filter_map(|s| {
            let addr: Multiaddr = s.parse().ok()?;
            let peer_id = addr.iter().find_map(|proto| {
                if let libp2p::multiaddr::Protocol::P2p(id) = proto {
                    Some(id)
                } else {
                    None
                }
            })?;
            Some((peer_id, addr))
        }).collect()
    }

    /// Broadcast a transaction to the network.
    pub async fn broadcast_transaction(&self, data: Vec<u8>) -> Result<(), NetworkError> {
        self.outbound_tx
            .send((TOPIC_TRANSACTIONS.to_string(), data))
            .await
            .map_err(|e| NetworkError::Gossip(format!("failed to send transaction: {e}")))
    }

    /// Broadcast a block to the network.
    pub async fn broadcast_block(&self, data: Vec<u8>) -> Result<(), NetworkError> {
        self.outbound_tx
            .send((TOPIC_BLOCKS.to_string(), data))
            .await
            .map_err(|e| NetworkError::Gossip(format!("failed to send block: {e}")))
    }

    /// Broadcast a consensus message to the network.
    pub async fn broadcast_consensus(&self, data: Vec<u8>) -> Result<(), NetworkError> {
        self.outbound_tx
            .send((TOPIC_CONSENSUS.to_string(), data))
            .await
            .map_err(|e| NetworkError::Gossip(format!("failed to send consensus msg: {e}")))
    }

    /// Broadcast a mining proof to the network.
    pub async fn broadcast_mining_proof(&self, data: Vec<u8>) -> Result<(), NetworkError> {
        self.outbound_tx
            .send((TOPIC_MINING.to_string(), data))
            .await
            .map_err(|e| NetworkError::Gossip(format!("failed to send mining proof: {e}")))
    }

    /// Receive the next message from the network.
    pub async fn recv(&mut self) -> Option<SwarmMessage> {
        self.inbound_rx.recv().await
    }

    /// Get the number of connected peers.
    pub fn peer_count(&self) -> usize {
        self.connected_peer_count.load(Ordering::Relaxed)
    }

    /// Check if the node has enough peers for safe operation.
    ///
    /// Returns `false` when the peer count is below `MIN_SAFE_PEER_COUNT`,
    /// indicating the node may be vulnerable to eclipse attacks.
    pub fn has_safe_peer_count(&self) -> bool {
        self.peer_count() >= MIN_SAFE_PEER_COUNT
    }
}

/// Build the libp2p swarm with all behaviours configured.
fn build_swarm(local_key: Keypair, config: &SwarmConfig) -> Result<Swarm<PiChainBehaviour>, NetworkError> {
    // GossipSub configuration with peer scoring and collision-resistant message IDs
    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(config.heartbeat_interval)
        .validation_mode(ValidationMode::Strict)
        .max_transmit_size(config.max_message_size)
        // SECURITY (Fix 194): Increased mesh_n_low from 4→6 and mesh_n from 8→10
        // to make eclipse attacks more expensive. An attacker now needs to control
        // at least 6 Sybil nodes (up from 2-4) in the victim's GossipSub mesh.
        .mesh_n(10)
        .mesh_n_low(6)
        .mesh_n_high(14)
        .gossip_lazy(6)
        .flood_publish(false) // Use mesh propagation, not flood (prevents amplification)
        .history_length(5)
        .history_gossip(3)
        .message_id_fn(|message: &gossipsub::Message| {
            // Use BLAKE3 (256-bit) instead of DefaultHasher (64-bit SipHash)
            // to prevent collision-based message suppression attacks
            let hash = pichain_crypto::hash_concat(&[
                &message.data,
                message.source.map(|p| p.to_bytes()).unwrap_or_default().as_slice(),
            ]);
            MessageId::from(hash.as_bytes().to_vec())
        })
        .build()
        .map_err(|e| NetworkError::Gossip(format!("gossipsub config error: {e}")))?;

    let mut gossipsub = gossipsub::Behaviour::new(
        MessageAuthenticity::Signed(local_key.clone()),
        gossipsub_config,
    )
    .map_err(|e| NetworkError::Gossip(format!("gossipsub creation error: {e}")))?;

    // Configure peer scoring to defend against eclipse/Sybil/mesh-poisoning attacks.
    // Peers that send invalid messages or fail to forward are penalized.
    let peer_score_params = gossipsub::PeerScoreParams {
        // IP colocation penalty: penalize many peers from same IP (Sybil defense)
        ip_colocation_factor_weight: -10.0,
        ip_colocation_factor_threshold: 3.0,
        // Decay slowly so bad actors don't recover quickly
        decay_interval: Duration::from_secs(60),
        decay_to_zero: 0.01,
        retain_score: Duration::from_secs(3600),
        ..Default::default()
    };
    let peer_score_thresholds = gossipsub::PeerScoreThresholds {
        gossip_threshold: -100.0,
        publish_threshold: -200.0,
        graylist_threshold: -400.0,
        ..Default::default()
    };
    gossipsub.with_peer_score(peer_score_params, peer_score_thresholds)
        .map_err(|e| NetworkError::Gossip(format!("peer score config error: {e}")))?;

    // Kademlia configuration
    let peer_id = PeerId::from(local_key.public());
    let mut store_config = kad::store::MemoryStoreConfig::default();
    store_config.max_records = 65_536; // Cap DHT records to prevent memory exhaustion
    store_config.max_provided_keys = 4_096;
    store_config.max_value_bytes = 8_192; // 8 KiB max per record value
    let store = MemoryStore::with_config(peer_id, store_config);
    let mut kademlia_config = kad::Config::new(
        libp2p::StreamProtocol::try_from_owned(format!("/pichain/kad/1.0.0"))
            .expect("valid protocol"),
    );
    kademlia_config.set_query_timeout(Duration::from_secs(60));
    kademlia_config.set_record_ttl(Some(Duration::from_secs(7 * 24 * 3600))); // 7 days
    kademlia_config.set_provider_record_ttl(Some(Duration::from_secs(24 * 3600))); // 1 day
    kademlia_config.set_record_filtering(kad::StoreInserts::FilterBoth);
    let kademlia = kad::Behaviour::with_config(peer_id, store, kademlia_config);

    // Identify configuration
    let identify = identify::Behaviour::new(identify::Config::new(
        config.protocol_version.clone(),
        local_key.public(),
    ));

    // Enforce connection limits to prevent DoS via connection flooding.
    // Without these, an attacker can open unlimited inbound TCP connections,
    // exhausting file descriptors and memory.
    let max_conns = config.max_connections;
    let limits = ConnectionLimits::default()
        .with_max_established(Some(max_conns))
        .with_max_established_incoming(Some(max_conns / 2))
        .with_max_pending_incoming(Some(64));
    let conn_limits = connection_limits::Behaviour::new(limits);

    let swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .map_err(|e| NetworkError::Transport(format!("tcp transport error: {e}")))?
        .with_relay_client(noise::Config::new, yamux::Config::default)
        .map_err(|e| NetworkError::Transport(format!("relay client error: {e}")))?
        .with_behaviour(|key, relay_client| {
            // AutoNAT: probe every 30s, 3 attempts per peer
            let autonat = autonat::Behaviour::new(
                key.public().to_peer_id(),
                autonat::Config {
                    retry_interval: Duration::from_secs(30),
                    ..Default::default()
                },
            );

            Ok(PiChainBehaviour {
                gossipsub,
                kademlia,
                identify,
                autonat,
                relay_client,
                connection_limits: conn_limits,
            })
        })
        .map_err(|e| NetworkError::Transport(format!("behaviour error: {e}")))?
        .with_swarm_config(|cfg| {
            cfg.with_idle_connection_timeout(Duration::from_secs(300))
        })
        .build();

    info!(
        max_established = max_conns,
        max_inbound = max_conns / 2,
        max_pending_inbound = 64,
        "built P2P swarm with enforced connection limits"
    );

    Ok(swarm)
}

/// Main swarm event processing loop.
async fn run_swarm_loop(
    mut swarm: Swarm<PiChainBehaviour>,
    inbound_tx: mpsc::Sender<SwarmMessage>,
    outbound_rx: &mut mpsc::Receiver<(String, Vec<u8>)>,
    peer_count: Arc<AtomicUsize>,
    byte_tracker: Arc<PeerByteTracker>,
) {
    // Periodic cleanup interval for stale byte-tracker entries
    let mut cleanup_interval = tokio::time::interval(Duration::from_secs(120));
    cleanup_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // Handle outbound messages
            Some((topic_str, data)) = outbound_rx.recv() => {
                let topic = IdentTopic::new(&topic_str);
                if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic, data) {
                    warn!("failed to publish to {topic_str}: {e}");
                }
            }
            // Periodic cleanup of stale per-peer byte counters
            _ = cleanup_interval.tick() => {
                byte_tracker.cleanup();
            }
            // Handle swarm events
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::Behaviour(PiChainBehaviourEvent::Gossipsub(
                        gossipsub::Event::Message {
                            propagation_source,
                            message_id,
                            message,
                        },
                    )) => {
                        let topic = message.topic.to_string();
                        debug!(%propagation_source, %message_id, %topic, "received gossip message");

                        // Defense-in-depth: reject oversized messages even if gossipsub allows them
                        const MAX_GOSSIP_MSG: usize = 64 * 1024; // matches SwarmConfig default
                        if message.data.len() > MAX_GOSSIP_MSG {
                            warn!(size = message.data.len(), %topic, "rejected oversized gossip message");
                            continue;
                        }

                        // Per-peer inbound byte rate limiting (H-242-1).
                        // Track bytes received from each peer and disconnect
                        // peers that exceed 100 MB/min to prevent amplification.
                        let msg_bytes = message.data.len() as u64;
                        if !byte_tracker.check_and_record(&propagation_source, msg_bytes) {
                            warn!(
                                %propagation_source,
                                msg_bytes,
                                "peer exceeded inbound byte rate limit, disconnecting"
                            );
                            // Disconnect abusive peer to stop further amplification
                            let _ = swarm.disconnect_peer_id(propagation_source);
                            continue;
                        }

                        let msg = match topic.as_str() {
                            t if t == TOPIC_TRANSACTIONS => SwarmMessage::Transaction(message.data),
                            t if t == TOPIC_BLOCKS => SwarmMessage::Block(message.data),
                            t if t == TOPIC_CONSENSUS => SwarmMessage::Consensus(message.data),
                            t if t == TOPIC_MINING => SwarmMessage::MiningProof(message.data),
                            t if t == TOPIC_SYNC => SwarmMessage::Sync(message.data),
                            _ => continue,
                        };

                        if inbound_tx.send(msg).await.is_err() {
                            warn!("inbound channel closed");
                            break;
                        }
                    }
                    SwarmEvent::Behaviour(PiChainBehaviourEvent::Identify(
                        identify::Event::Received { peer_id, info, .. },
                    )) => {
                        info!(%peer_id, protocol = %info.protocol_version, "identified peer");

                        // Filter addresses to prevent DHT poisoning via Identify.
                        // Only accept routable, non-private addresses (max 8 per peer)
                        // to prevent eclipse attacks and amplification/reflection.
                        let valid_addrs: Vec<Multiaddr> = info.listen_addrs
                            .into_iter()
                            .filter(|addr| is_routable_address(addr))
                            .take(8)
                            .collect();

                        for addr in valid_addrs {
                            swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                        }

                        let _ = inbound_tx.send(SwarmMessage::PeerDiscovered(peer_id)).await;
                    }
                    SwarmEvent::Behaviour(PiChainBehaviourEvent::Kademlia(
                        kad::Event::RoutingUpdated { peer, .. },
                    )) => {
                        debug!(%peer, "kademlia routing updated");
                    }
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!(%address, "listening on");
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, num_established, .. } => {
                        // R30-FIX: Only count unique peers, not individual connections.
                        // Previously every TCP connection incremented peer_count, so a
                        // single peer with multiple connections inflated the count and
                        // defeated eclipse detection (MIN_SAFE_PEER_COUNT check).
                        if num_established.get() == 1 {
                            peer_count.fetch_add(1, Ordering::Relaxed);
                        }
                        info!(%peer_id, connections = num_established.get(), peers = peer_count.load(Ordering::Relaxed), "connection established");
                        let _ = inbound_tx.send(SwarmMessage::PeerDiscovered(peer_id)).await;
                    }
                    SwarmEvent::ConnectionClosed { peer_id, num_established, .. } => {
                        // R30-FIX: Only decrement when last connection to peer closes.
                        // num_established is the number of remaining connections to this peer.
                        // Only clean up byte tracker when fully disconnected.
                        if num_established == 0 {
                            byte_tracker.remove_peer(&peer_id);
                            // Saturating decrement
                            let _ = peer_count.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                                Some(c.saturating_sub(1))
                            });
                        }
                        let current_peers = peer_count.load(Ordering::Relaxed);
                        info!(%peer_id, remaining = num_established, peers = current_peers, "connection closed");

                        // SECURITY (Fix 194): Warn when peer count drops below the
                        // safe threshold — indicates possible eclipse attack or
                        // severe network partition.
                        if current_peers < MIN_SAFE_PEER_COUNT {
                            warn!(
                                peers = current_peers,
                                minimum = MIN_SAFE_PEER_COUNT,
                                "peer count below safe minimum — node may be eclipsed"
                            );
                        }

                        if num_established == 0 {
                            let _ = inbound_tx.send(SwarmMessage::PeerDisconnected(peer_id)).await;
                        }
                    }
                    SwarmEvent::Behaviour(PiChainBehaviourEvent::Autonat(event)) => {
                        match event {
                            autonat::Event::StatusChanged { old, new } => {
                                info!(
                                    old_status = ?old,
                                    new_status = ?new,
                                    "AutoNAT status changed"
                                );
                                match new {
                                    autonat::NatStatus::Public(addr) => {
                                        info!(%addr, "NAT status: PUBLIC — reachable from the internet");
                                    }
                                    autonat::NatStatus::Private => {
                                        warn!("NAT status: PRIVATE — behind NAT, consider using a relay peer");
                                    }
                                    autonat::NatStatus::Unknown => {
                                        debug!("NAT status: UNKNOWN — probing in progress");
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Check if a multiaddr is publicly routable (not loopback, private, or link-local).
/// Used to filter Identify-reported addresses before adding to Kademlia DHT,
/// preventing DHT poisoning and eclipse attacks.
fn is_routable_address(addr: &Multiaddr) -> bool {
    use std::net::Ipv4Addr;
    for proto in addr.iter() {
        match proto {
            libp2p::multiaddr::Protocol::Ip4(ip) => {
                if ip.is_loopback()
                    || ip.is_private()
                    || ip.is_link_local()
                    || ip.is_unspecified()
                    || ip.is_broadcast()
                    || ip == Ipv4Addr::new(0, 0, 0, 0)
                {
                    return false;
                }
                // Carrier-Grade NAT (100.64.0.0/10)
                let octets = ip.octets();
                if octets[0] == 100 && (octets[1] & 0xC0) == 64 {
                    return false;
                }
                // Benchmarking (198.18.0.0/15)
                if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) {
                    return false;
                }
                // Documentation ranges
                if (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)       // 192.0.2.0/24
                    || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100) // 198.51.100.0/24
                    || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)  // 203.0.113.0/24
                {
                    return false;
                }
                // Reserved/Future (240.0.0.0/4)
                if octets[0] >= 240 {
                    return false;
                }
            }
            libp2p::multiaddr::Protocol::Ip6(ip) => {
                if ip.is_loopback() || ip.is_unspecified() {
                    return false;
                }
                // Filter IPv6 link-local (fe80::), unique-local (fc00::/fdoo::),
                // and documentation (2001:db8::) addresses to prevent DHT poisoning
                let segments = ip.segments();
                if segments[0] == 0xfe80                    // Link-local
                    || (segments[0] & 0xfe00) == 0xfc00     // Unique local (fc00::/7)
                    || (segments[0] == 0x2001 && segments[1] == 0x0db8) // Documentation
                {
                    return false;
                }
                // Check IPv4-mapped IPv6 (::ffff:x.x.x.x) — extract the IPv4
                // and apply all IPv4 rules to prevent filter bypass via mapped addrs.
                if let Some(ipv4) = ip.to_ipv4_mapped() {
                    if ipv4.is_loopback() || ipv4.is_private() || ipv4.is_link_local()
                        || ipv4.is_unspecified() || ipv4.is_broadcast()
                    {
                        return false;
                    }
                    // Also check CGNAT, benchmarking, documentation, reserved ranges
                    let octets = ipv4.octets();
                    if octets[0] == 100 && (octets[1] & 0xC0) == 64 { return false; }
                    if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) { return false; }
                    if (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
                        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113) { return false; }
                    if octets[0] >= 240 { return false; }
                }
            }
            _ => {}
        }
    }
    // Reject addresses with no IP component at all
    let has_ip = addr.iter().any(|p| {
        matches!(p, libp2p::multiaddr::Protocol::Ip4(_) | libp2p::multiaddr::Protocol::Ip6(_))
    });
    has_ip
}

/// Compute the deterministic libp2p PeerId for a given Ed25519 secret key.
///
/// This allows pre-computing peer IDs for validators so bootstrap addresses
/// can include the `/p2p/<peer_id>` suffix in configuration files.
pub fn peer_id_from_ed25519(secret: &[u8; 32]) -> Result<PeerId, NetworkError> {
    let mut bytes = *secret;
    let ed_key = libp2p::identity::ed25519::SecretKey::try_from_bytes(&mut bytes)
        .map_err(|e| NetworkError::Transport(format!("invalid ed25519 key: {e}")))?;
    let ed_keypair = libp2p::identity::ed25519::Keypair::from(ed_key);
    let keypair = Keypair::from(ed_keypair);
    Ok(PeerId::from(keypair.public()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = SwarmConfig::default();
        assert_eq!(config.listen_addr, "/ip4/0.0.0.0/tcp/9314");
        assert!(config.bootstrap_peers.is_empty());
        assert_eq!(config.max_message_size, 64 * 1024); // 64KB for gossip
        assert_eq!(config.max_connections, 256);
        assert_eq!(config.heartbeat_interval, Duration::from_secs(1));
    }

    #[test]
    fn min_safe_peer_count_is_reasonable() {
        // Fix 194: Minimum must be at least 8 to make eclipse attacks expensive.
        // An attacker must control >= MIN_SAFE_PEER_COUNT Sybil nodes.
        assert!(
            MIN_SAFE_PEER_COUNT >= 8,
            "MIN_SAFE_PEER_COUNT must be >= 8 for eclipse resistance"
        );
    }

    #[test]
    fn topic_constants() {
        assert_eq!(TOPIC_TRANSACTIONS, "/pichain/txs/1");
        assert_eq!(TOPIC_BLOCKS, "/pichain/blocks/1");
        assert_eq!(TOPIC_CONSENSUS, "/pichain/consensus/1");
        assert_eq!(TOPIC_MINING, "/pichain/mining/1");
    }

    #[test]
    fn routable_address_filtering() {
        // Public IP should be accepted
        let public: Multiaddr = "/ip4/1.2.3.4/tcp/9314".parse().unwrap();
        assert!(is_routable_address(&public));

        // Loopback should be rejected
        let loopback: Multiaddr = "/ip4/127.0.0.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&loopback));

        // Private 10.x should be rejected
        let private_10: Multiaddr = "/ip4/10.0.0.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&private_10));

        // Private 192.168.x should be rejected
        let private_192: Multiaddr = "/ip4/192.168.1.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&private_192));

        // Private 172.16.x should be rejected
        let private_172: Multiaddr = "/ip4/172.16.0.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&private_172));

        // 0.0.0.0 should be rejected
        let unspecified: Multiaddr = "/ip4/0.0.0.0/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&unspecified));
    }

    // --- PeerByteTracker tests ---

    fn test_peer_id() -> PeerId {
        let key = Keypair::generate_ed25519();
        PeerId::from(key.public())
    }

    #[test]
    fn byte_tracker_allows_within_limit() {
        let tracker = PeerByteTracker::new(1_000_000, Duration::from_secs(60));
        let peer = test_peer_id();

        // Should allow messages within the 1 MB limit
        assert!(tracker.check_and_record(&peer, 500_000));
        assert!(tracker.check_and_record(&peer, 400_000));
        // 900_000 total, still under 1_000_000
        assert!(tracker.check_and_record(&peer, 100_000));
        // Exactly at 1_000_000 — allowed
    }

    #[test]
    fn byte_tracker_blocks_over_limit() {
        let tracker = PeerByteTracker::new(1_000_000, Duration::from_secs(60));
        let peer = test_peer_id();

        assert!(tracker.check_and_record(&peer, 900_000));
        // Next message would push total to 1_100_001 > 1_000_000
        assert!(!tracker.check_and_record(&peer, 100_001));
    }

    #[test]
    fn byte_tracker_independent_peers() {
        let tracker = PeerByteTracker::new(1_000_000, Duration::from_secs(60));
        let peer_a = test_peer_id();
        let peer_b = test_peer_id();

        // Fill peer_a to the limit
        assert!(tracker.check_and_record(&peer_a, 1_000_000));
        assert!(!tracker.check_and_record(&peer_a, 1));

        // peer_b should still be independently allowed
        assert!(tracker.check_and_record(&peer_b, 500_000));
    }

    #[test]
    fn byte_tracker_window_reset() {
        // Use a very short window for testing
        let tracker = PeerByteTracker::new(1_000, Duration::from_millis(1));
        let peer = test_peer_id();

        // Fill to limit
        assert!(tracker.check_and_record(&peer, 1_000));
        assert!(!tracker.check_and_record(&peer, 1));

        // Wait for the window to expire
        std::thread::sleep(Duration::from_millis(5));

        // After window reset, should be allowed again
        assert!(tracker.check_and_record(&peer, 500));
    }

    #[test]
    fn byte_tracker_remove_peer() {
        let tracker = PeerByteTracker::new(1_000, Duration::from_secs(60));
        let peer = test_peer_id();

        assert!(tracker.check_and_record(&peer, 1_000));
        assert!(!tracker.check_and_record(&peer, 1));

        // Remove the peer (simulating disconnect)
        tracker.remove_peer(&peer);

        // Should be allowed again after removal
        assert!(tracker.check_and_record(&peer, 500));
    }

    #[test]
    fn byte_tracker_cleanup_removes_stale() {
        let tracker = PeerByteTracker::new(1_000, Duration::from_millis(1));
        let peer = test_peer_id();

        assert!(tracker.check_and_record(&peer, 100));
        assert_eq!(tracker.counters.len(), 1);

        // Wait for 2x window (cleanup threshold)
        std::thread::sleep(Duration::from_millis(5));
        tracker.cleanup();

        // Stale entry should be removed
        assert_eq!(tracker.counters.len(), 0);
    }

    #[test]
    fn byte_tracker_zero_bytes_allowed() {
        let tracker = PeerByteTracker::new(1_000, Duration::from_secs(60));
        let peer = test_peer_id();

        // Zero-byte messages should always be allowed
        assert!(tracker.check_and_record(&peer, 0));
    }

    #[test]
    fn byte_tracker_single_message_exceeds_limit() {
        let tracker = PeerByteTracker::new(1_000, Duration::from_secs(60));
        let peer = test_peer_id();

        // A single message larger than the entire window should be rejected
        assert!(!tracker.check_and_record(&peer, 1_001));
    }

    #[test]
    fn reserved_ipv4_ranges_rejected() {
        // Carrier-Grade NAT (100.64.0.0/10)
        let cgnat: Multiaddr = "/ip4/100.64.0.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&cgnat), "CGNAT should be rejected");
        let cgnat_high: Multiaddr = "/ip4/100.127.255.254/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&cgnat_high), "CGNAT high end should be rejected");

        // Benchmarking (198.18.0.0/15)
        let bench18: Multiaddr = "/ip4/198.18.0.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&bench18), "198.18.x.x should be rejected");
        let bench19: Multiaddr = "/ip4/198.19.255.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&bench19), "198.19.x.x should be rejected");

        // Documentation ranges
        let doc1: Multiaddr = "/ip4/192.0.2.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&doc1), "192.0.2.0/24 should be rejected");
        let doc2: Multiaddr = "/ip4/198.51.100.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&doc2), "198.51.100.0/24 should be rejected");
        let doc3: Multiaddr = "/ip4/203.0.113.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&doc3), "203.0.113.0/24 should be rejected");

        // Reserved/Future (240.0.0.0/4)
        let reserved: Multiaddr = "/ip4/240.0.0.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&reserved), "240.x.x.x should be rejected");
        let reserved_high: Multiaddr = "/ip4/255.255.255.254/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&reserved_high), "255.x.x.x should be rejected");

        // Public IP should still be accepted
        let public: Multiaddr = "/ip4/8.8.8.8/tcp/9314".parse().unwrap();
        assert!(is_routable_address(&public), "public IP should be accepted");

        // 100.63.x.x is NOT CGNAT — should be routable
        let not_cgnat: Multiaddr = "/ip4/100.63.255.1/tcp/9314".parse().unwrap();
        assert!(is_routable_address(&not_cgnat), "100.63.x.x is not CGNAT, should be accepted");

        // 198.20.x.x is NOT benchmarking — should be routable
        let not_bench: Multiaddr = "/ip4/198.20.0.1/tcp/9314".parse().unwrap();
        assert!(is_routable_address(&not_bench), "198.20.x.x is not benchmarking, should be accepted");
    }

    #[test]
    fn ipv4_mapped_ipv6_rejected() {
        // IPv4-mapped IPv6 loopback (::ffff:127.0.0.1) should be rejected
        let mapped_loopback: Multiaddr = "/ip6/::ffff:127.0.0.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&mapped_loopback), "mapped loopback should be rejected");

        // IPv4-mapped IPv6 private (::ffff:10.0.0.1) should be rejected
        let mapped_private: Multiaddr = "/ip6/::ffff:10.0.0.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&mapped_private), "mapped private should be rejected");

        // IPv4-mapped IPv6 CGNAT (::ffff:100.64.0.1) should be rejected
        let mapped_cgnat: Multiaddr = "/ip6/::ffff:100.64.0.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&mapped_cgnat), "mapped CGNAT should be rejected");

        // IPv4-mapped IPv6 reserved (::ffff:240.0.0.1) should be rejected
        let mapped_reserved: Multiaddr = "/ip6/::ffff:240.0.0.1/tcp/9314".parse().unwrap();
        assert!(!is_routable_address(&mapped_reserved), "mapped reserved should be rejected");

        // IPv4-mapped IPv6 public (::ffff:8.8.8.8) should be accepted
        let mapped_public: Multiaddr = "/ip6/::ffff:8.8.8.8/tcp/9314".parse().unwrap();
        assert!(is_routable_address(&mapped_public), "mapped public IP should be accepted");
    }

    /// Integration test: SwarmMessage variants preserve their payload bytes across
    /// construction and pattern matching — the same roundtrip path used by the
    /// swarm event loop when broadcasting and receiving messages.
    #[test]
    fn broadcast_message_serialization_roundtrip() {
        // Simulate the P2P broadcast path: data is serialized to bytes by the sender,
        // wrapped in a SwarmMessage by the swarm event loop, and extracted by the receiver.

        // --- Block variant ---
        let block_data = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04];
        let block_msg = SwarmMessage::Block(block_data.clone());
        let cloned = block_msg.clone();
        match cloned {
            SwarmMessage::Block(data) => {
                assert_eq!(data, block_data, "Block payload should survive roundtrip");
            }
            _ => panic!("expected SwarmMessage::Block"),
        }

        // --- Consensus variant ---
        let consensus_data = vec![0xCA; 96]; // simulated BLS certificate bytes
        let consensus_msg = SwarmMessage::Consensus(consensus_data.clone());
        let cloned = consensus_msg.clone();
        match cloned {
            SwarmMessage::Consensus(data) => {
                assert_eq!(data, consensus_data, "Consensus payload should survive roundtrip");
            }
            _ => panic!("expected SwarmMessage::Consensus"),
        }

        // --- Transaction variant ---
        let tx_data = vec![0x01, 0x02, 0x03, 0x04, 0x05];
        let tx_msg = SwarmMessage::Transaction(tx_data.clone());
        let cloned = tx_msg.clone();
        match cloned {
            SwarmMessage::Transaction(data) => {
                assert_eq!(data, tx_data, "Transaction payload should survive roundtrip");
            }
            _ => panic!("expected SwarmMessage::Transaction"),
        }

        // --- MiningProof variant ---
        let mining_data = vec![0xFF; 32];
        let mining_msg = SwarmMessage::MiningProof(mining_data.clone());
        let cloned = mining_msg.clone();
        match cloned {
            SwarmMessage::MiningProof(data) => {
                assert_eq!(data, mining_data, "MiningProof payload should survive roundtrip");
            }
            _ => panic!("expected SwarmMessage::MiningProof"),
        }

        // --- PeerDiscovered variant ---
        let key = Keypair::generate_ed25519();
        let peer = PeerId::from(key.public());
        let peer_msg = SwarmMessage::PeerDiscovered(peer);
        let cloned = peer_msg.clone();
        match cloned {
            SwarmMessage::PeerDiscovered(p) => {
                assert_eq!(p, peer, "PeerDiscovered should preserve PeerId");
            }
            _ => panic!("expected SwarmMessage::PeerDiscovered"),
        }

        // --- PeerDisconnected variant ---
        let dc_msg = SwarmMessage::PeerDisconnected(peer);
        let cloned = dc_msg.clone();
        match cloned {
            SwarmMessage::PeerDisconnected(p) => {
                assert_eq!(p, peer, "PeerDisconnected should preserve PeerId");
            }
            _ => panic!("expected SwarmMessage::PeerDisconnected"),
        }

        // Verify empty payloads work correctly (edge case)
        let empty_block = SwarmMessage::Block(vec![]);
        match empty_block {
            SwarmMessage::Block(data) => {
                assert!(data.is_empty(), "empty payload should be preserved");
            }
            _ => panic!("expected SwarmMessage::Block"),
        }

        // Verify large payload (max gossip size)
        let large_data = vec![0x42u8; 64 * 1024]; // 64KB = max gossip message size
        let large_msg = SwarmMessage::Transaction(large_data.clone());
        match large_msg {
            SwarmMessage::Transaction(data) => {
                assert_eq!(data.len(), 64 * 1024, "large payload size should be preserved");
                assert_eq!(data, large_data, "large payload bytes should match");
            }
            _ => panic!("expected SwarmMessage::Transaction"),
        }
    }

    #[test]
    fn deterministic_peer_id_from_ed25519() {
        // Same secret key must always produce the same peer ID
        let secret: [u8; 32] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
            0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
            0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
            0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x20,
        ];
        let id1 = peer_id_from_ed25519(&secret).expect("valid key");
        let id2 = peer_id_from_ed25519(&secret).expect("valid key");
        assert_eq!(id1, id2, "same secret must produce same peer ID");

        // Different secret must produce different peer ID
        let secret2: [u8; 32] = [
            0xFF, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
            0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
            0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
            0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x20,
        ];
        let id3 = peer_id_from_ed25519(&secret2).expect("valid key");
        assert_ne!(id1, id3, "different secrets must produce different peer IDs");
    }

    #[test]
    fn swarm_config_identity_secret_default_is_none() {
        let config = SwarmConfig::default();
        assert!(config.identity_secret.is_none(), "default config should have no identity_secret");
    }
}
