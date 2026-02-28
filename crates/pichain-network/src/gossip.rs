//! GossipSub topic definitions and utilities.
//!
//! Topic constants and GossipSub configuration are in `swarm.rs`.
//! This module re-exports them for convenience.

pub use crate::swarm::{TOPIC_BLOCKS, TOPIC_CONSENSUS, TOPIC_MINING, TOPIC_TRANSACTIONS};
