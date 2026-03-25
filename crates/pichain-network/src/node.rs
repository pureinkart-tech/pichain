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
