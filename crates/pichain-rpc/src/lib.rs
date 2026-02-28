//! PIChain REST API Server
//!
//! Provides REST API endpoints for querying blockchain state, submitting
//! transactions, and interacting with tokens, DEX, mining, and faucet.
//! Includes WebSocket subscriptions for real-time block and transaction events.

pub mod server;
pub mod ws;

pub use server::{MiningStatusData, RpcServer, StateProvider, SwapQuote};
pub use ws::{WsBroadcaster, WsEvent};

#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("server error: {0}")]
    Server(String),
    #[error("invalid params: {0}")]
    InvalidParams(String),
    #[error("method not found: {0}")]
    MethodNotFound(String),
    #[error("internal error: {0}")]
    Internal(String),
}
