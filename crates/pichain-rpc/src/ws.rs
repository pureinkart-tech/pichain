//! WebSocket subscription system for real-time blockchain events.
//!
//! Supports subscriptions to:
//! - `newBlocks` — emitted when a new block is finalized
//! - `newTransactions` — emitted for each transaction in a new block
//! - `miningStatus` — periodic mining frontier updates

use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, warn};

/// Events that can be broadcast to WebSocket subscribers.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum WsEvent {
    /// New block finalized.
    #[serde(rename = "newBlock")]
    NewBlock {
        height: u64,
        tx_count: u32,
        gas_used: u64,
        base_fee: u64,
        pi_burned: u64,
        timestamp_ms: u64,
        proposer: String,
        block_hash: String,
    },
    /// Transaction included in a block.
    #[serde(rename = "newTransaction")]
    NewTransaction {
        tx_hash: String,
        sender: String,
        kind: String,
        block_height: u64,
        status: String,
    },
    /// Mining status update.
    #[serde(rename = "miningStatus")]
    MiningStatus {
        frontier_position: u64,
        total_digits_verified: u64,
        difficulty_bits: u32,
        unique_miners: u64,
    },
}

/// Subscription request from the client.
#[derive(Debug, Deserialize)]
struct SubscribeRequest {
    /// Subscription topics: "newBlocks", "newTransactions", "miningStatus"
    subscribe: Vec<String>,
}

/// Maximum concurrent WebSocket connections to prevent resource exhaustion.
const MAX_WS_CONNECTIONS: u64 = 500;

/// Manages WebSocket subscriptions and event broadcasting.
pub struct WsBroadcaster {
    sender: broadcast::Sender<WsEvent>,
    subscriber_count: AtomicU64,
}

impl WsBroadcaster {
    /// Create a new broadcaster with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            subscriber_count: AtomicU64::new(0),
        }
    }

    /// Broadcast an event to all connected subscribers.
    /// Returns the number of receivers that got the message.
    pub fn broadcast(&self, event: WsEvent) -> usize {
        self.sender.send(event).unwrap_or(0)
    }

    /// Get a new receiver for subscribing to events.
    pub fn subscribe(&self) -> broadcast::Receiver<WsEvent> {
        self.subscriber_count.fetch_add(1, Ordering::Relaxed);
        self.sender.subscribe()
    }

    /// Get the current number of active subscribers.
    pub fn subscriber_count(&self) -> u64 {
        self.subscriber_count.load(Ordering::Relaxed)
    }

    /// Check if a new connection would exceed the limit.
    pub fn at_capacity(&self) -> bool {
        self.subscriber_count.load(Ordering::Relaxed) >= MAX_WS_CONNECTIONS
    }

    /// Decrement subscriber count (called when a WebSocket disconnects).
    pub fn on_disconnect(&self) {
        let _ = self.subscriber_count.fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |c| Some(c.saturating_sub(1)),
        );
    }
}

/// How long a WebSocket connection can be idle (no messages sent or received)
/// before we consider it dead and close it. This prevents zombie connections
/// from accumulating when clients disconnect without a proper close frame.
const WS_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Interval for sending WebSocket ping frames to detect dead connections.
const WS_PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Handle a WebSocket connection — reads subscription requests, forwards matching events.
///
/// SECURITY (Fix 196): The receiver task is tied to a `CancellationToken` so it
/// is automatically aborted when the sender loop exits, preventing zombie tasks.
/// Additionally, idle connections are timed out via periodic pings.
pub async fn handle_ws(socket: WebSocket, broadcaster: Arc<WsBroadcaster>) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut rx = broadcaster.subscribe();

    // Default: subscribe to all topics
    let mut sub_blocks = true;
    let mut sub_txs = true;
    let mut sub_mining = true;

    // Spawn a task to read incoming messages (subscription filters)
    // SECURITY: Limit incoming message size to prevent memory exhaustion DoS.
    const MAX_WS_MESSAGE_SIZE: usize = 1024; // Subscription requests are tiny
    let (filter_tx, mut filter_rx) = tokio::sync::mpsc::channel::<(bool, bool, bool)>(4);

    // Use an AbortHandle to ensure the receiver task is cleaned up when
    // the sender loop exits (Fix 196: prevents zombie tasks).
    let receiver_handle = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            if let Message::Text(text) = msg {
                // Reject oversized messages to prevent memory exhaustion
                if text.len() > MAX_WS_MESSAGE_SIZE {
                    warn!(size = text.len(), "WS message too large, ignoring");
                    continue;
                }
                if let Ok(req) = serde_json::from_str::<SubscribeRequest>(&text) {
                    // Limit subscribe vector to prevent allocation attacks
                    if req.subscribe.len() > 10 {
                        continue;
                    }
                    let blocks = req.subscribe.iter().any(|s| s == "newBlocks");
                    let txs = req.subscribe.iter().any(|s| s == "newTransactions");
                    let mining = req.subscribe.iter().any(|s| s == "miningStatus");
                    // If the send fails, the main loop has exited — break.
                    if filter_tx.send((blocks, txs, mining)).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Ping interval to detect dead connections (Fix 196)
    let mut ping_interval = tokio::time::interval(WS_PING_INTERVAL);
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_activity = tokio::time::Instant::now();

    loop {
        tokio::select! {
            // Check for filter updates from client
            Some((b, t, m)) = filter_rx.recv() => {
                sub_blocks = b;
                sub_txs = t;
                sub_mining = m;
                last_activity = tokio::time::Instant::now();
                debug!(blocks = b, txs = t, mining = m, "WS subscription updated");
            }

            // Forward matching events
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        let should_send = match &event {
                            WsEvent::NewBlock { .. } => sub_blocks,
                            WsEvent::NewTransaction { .. } => sub_txs,
                            WsEvent::MiningStatus { .. } => sub_mining,
                        };
                        if should_send {
                            if let Ok(json) = serde_json::to_string(&event) {
                                if ws_sender.send(Message::Text(json.into())).await.is_err() {
                                    break; // Client disconnected
                                }
                                last_activity = tokio::time::Instant::now();
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "WS subscriber lagged behind");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            // Periodic ping / idle timeout check (Fix 196)
            _ = ping_interval.tick() => {
                if last_activity.elapsed() > WS_IDLE_TIMEOUT {
                    debug!("WS connection idle timeout, closing");
                    break;
                }
                // Send a ping frame to detect dead connections
                if ws_sender.send(Message::Ping(vec![].into())).await.is_err() {
                    break; // Client gone
                }
            }
        }
    }

    // SECURITY (Fix 196): Abort the receiver task to prevent zombie tasks.
    // If the receiver's WebSocket stream is blocking on read, this ensures
    // the task is cleaned up rather than hanging indefinitely.
    receiver_handle.abort();

    broadcaster.on_disconnect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_returns_zero_without_subscribers() {
        let broadcaster = WsBroadcaster::new(16);
        let count = broadcaster.broadcast(WsEvent::MiningStatus {
            frontier_position: 0,
            total_digits_verified: 0,
            difficulty_bits: 8,
            unique_miners: 0,
        });
        assert_eq!(count, 0);
    }

    #[test]
    fn subscriber_count_tracks_subscriptions() {
        let broadcaster = WsBroadcaster::new(16);
        assert_eq!(broadcaster.subscriber_count(), 0);
        let _rx1 = broadcaster.subscribe();
        assert_eq!(broadcaster.subscriber_count(), 1);
        let _rx2 = broadcaster.subscribe();
        assert_eq!(broadcaster.subscriber_count(), 2);
        broadcaster.on_disconnect();
        assert_eq!(broadcaster.subscriber_count(), 1);
    }

    #[tokio::test]
    async fn broadcast_delivers_to_subscribers() {
        let broadcaster = WsBroadcaster::new(16);
        let mut rx = broadcaster.subscribe();

        let event = WsEvent::NewBlock {
            height: 42,
            tx_count: 5,
            gas_used: 100_000,
            base_fee: 1_000,
            pi_burned: 500,
            timestamp_ms: 1000,
            proposer: "abc".to_string(),
            block_hash: "def".to_string(),
        };

        assert_eq!(broadcaster.broadcast(event), 1);

        let received = rx.recv().await.unwrap();
        match received {
            WsEvent::NewBlock { height, tx_count, .. } => {
                assert_eq!(height, 42);
                assert_eq!(tx_count, 5);
            }
            _ => panic!("wrong event type"),
        }
    }

    #[test]
    fn ws_event_serializes_correctly() {
        let event = WsEvent::NewBlock {
            height: 1,
            tx_count: 0,
            gas_used: 0,
            base_fee: 1000,
            pi_burned: 0,
            timestamp_ms: 12345,
            proposer: "aabb".to_string(),
            block_hash: "ccdd".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"newBlock\""));
        assert!(json.contains("\"height\":1"));
    }
}
