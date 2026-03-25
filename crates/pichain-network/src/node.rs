//! P2P network node using libp2p with QUIC transport.

use libp2p::{identity, Multiaddr, PeerId};
use tokio::sync::mpsc;
use tracing::info;

use crate::NetworkError;

/// PIChain network node.
pub struct NetworkNode {
    pub peer_id: PeerId,
    pub local_key: identity::Keypair,
    pub listen_addr: Multiaddr,
    pub tx_sender: mpsc::Sender<Vec<u8>>,
    pub tx_receiver: mpsc::Receiver<Vec<u8>>,
}

impl NetworkNode {
    /// Create a new network node with a random identity.
    ///
    /// # P2P Identity — libp2p ed25519 (NOT transaction signing)
    ///
    /// libp2p requires ed25519 keys for peer identity (PeerId derivation).
    /// This is a **network routing** identity only — it does NOT sign transactions,
    /// blocks, or any chain state. All PIChain transaction and consensus signing
    /// uses post-quantum ML-DSA-65 + SLH-DSA-SHAKE-128f exclusively.
    ///
    /// Even if a quantum computer could forge a libp2p PeerId, it could NOT:
    /// - Sign any transaction (requires PQ dual signatures)
    /// - Produce a valid block (requires PQ consensus signatures)
    /// - Steal any wallet (PQ keys only)
    ///
    /// ## Future upgrade path
    ///
    /// When libp2p adds post-quantum identity support (active IETF research),
    /// replace `generate_ed25519()` here with the PQ equivalent. The rest of the
    /// PIChain stack is already PQ-native and requires no changes.
    /// Track: <https://github.com/libp2p/specs/issues/608>
    pub fn new(listen_addr: &str) -> Result<Self, NetworkError> {
        let local_key = identity::Keypair::generate_ed25519();
        let peer_id = PeerId::from(local_key.public());
        let addr: Multiaddr = listen_addr
            .parse()
            .map_err(|e| NetworkError::Transport(format!("invalid address: {e}")))?;

        let (tx_sender, tx_receiver) = mpsc::channel(10_000);

        info!(%peer_id, "PIChain network node created");

        Ok(Self {
            peer_id,
            local_key,
            listen_addr: addr,
            tx_sender,
            tx_receiver,
        })
    }

    /// Get the peer ID as a string.
    pub fn peer_id_str(&self) -> String {
        self.peer_id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_network_node() {
        let node = NetworkNode::new("/ip4/127.0.0.1/tcp/9000").unwrap();
        assert!(!node.peer_id_str().is_empty());
    }
}
