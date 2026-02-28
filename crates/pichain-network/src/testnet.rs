//! Multi-node testnet harness for PIChain.
//!
//! Provides utilities for spawning and coordinating multiple PIChain nodes
//! in a local test environment. Supports:
//! - Automatic peer discovery between local nodes
//! - Block propagation verification
//! - Transaction submission and confirmation
//! - Network partition simulation
//! - Performance benchmarking

use pichain_crypto::ed25519::Address;
use pichain_crypto::Keypair;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tracing::info;

/// Configuration for a testnet node.
#[derive(Clone, Debug)]
pub struct TestNodeConfig {
    /// Node index in the testnet (0, 1, 2, ...).
    pub index: usize,
    /// P2P listen port.
    pub p2p_port: u16,
    /// RPC listen port.
    pub rpc_port: u16,
    /// Validator address (derived from a generated keypair).
    pub address: Address,
    /// Validator public key bytes (for re-creating identity).
    pub public_key: [u8; 32],
    /// Initial stake amount.
    pub stake: u64,
    /// Bootstrap peers (p2p multiaddrs).
    pub bootstrap_peers: Vec<String>,
    /// Chain ID.
    pub chain_id: u64,
}

impl TestNodeConfig {
    /// Create a config for node at given index.
    pub fn new(index: usize) -> Self {
        let keypair = Keypair::generate();
        let address = keypair.address();
        let public_key = keypair.public.to_bytes();
        Self {
            index,
            p2p_port: 9314u16.saturating_add(u16::try_from(index).unwrap_or(u16::MAX)),
            rpc_port: 8314u16.saturating_add(u16::try_from(index).unwrap_or(u16::MAX)),
            address,
            public_key,
            stake: 10_000 * 1_000_000_000, // 10,000 PI
            bootstrap_peers: vec![],
            chain_id: 31415, // devnet
        }
    }

    /// P2P multiaddr for this node.
    pub fn p2p_multiaddr(&self) -> String {
        format!("/ip4/127.0.0.1/tcp/{}", self.p2p_port)
    }

    /// RPC URL for this node.
    pub fn rpc_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.rpc_port)
    }

    /// Validator address.
    pub fn validator_address(&self) -> Address {
        self.address
    }
}

/// A simulated testnet node state (for in-process testing).
#[derive(Clone, Debug)]
pub struct SimulatedNode {
    /// Node config.
    pub config: TestNodeConfig,
    /// Current block height.
    pub height: u64,
    /// Blocks produced by this node.
    pub blocks_produced: u64,
    /// Transactions processed.
    pub txs_processed: u64,
    /// Connected peers (by index).
    pub connected_peers: Vec<usize>,
    /// Whether the node is running.
    pub running: bool,
    /// Whether the node is partitioned (isolated from network).
    pub partitioned: bool,
}

impl SimulatedNode {
    pub fn new(config: TestNodeConfig) -> Self {
        Self {
            config,
            height: 0,
            blocks_produced: 0,
            txs_processed: 0,
            connected_peers: vec![],
            running: false,
            partitioned: false,
        }
    }
}

/// Configuration for a local testnet.
#[derive(Clone, Debug)]
pub struct TestnetConfig {
    /// Number of validator nodes.
    pub num_validators: usize,
    /// Chain ID.
    pub chain_id: u64,
    /// Initial balance per validator (in PI base units).
    pub initial_balance: u64,
    /// Target block time.
    pub block_time_ms: u64,
    /// Whether to enable mining.
    pub enable_mining: bool,
}

impl Default for TestnetConfig {
    fn default() -> Self {
        Self {
            num_validators: 4,
            chain_id: 31415,
            initial_balance: 100_000 * 1_000_000_000, // 100K PI
            block_time_ms: 1000, // 1 second for testing
            enable_mining: false,
        }
    }
}

/// Genesis allocations for testnet.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenesisAllocation {
    pub address: Address,
    pub balance: u64,
    pub stake: u64,
}

/// A local testnet orchestrator.
pub struct LocalTestnet {
    /// Testnet configuration.
    config: TestnetConfig,
    /// Node configurations.
    node_configs: Vec<TestNodeConfig>,
    /// Simulated node states.
    nodes: Vec<SimulatedNode>,
    /// Genesis allocations.
    genesis_allocations: Vec<GenesisAllocation>,
    /// Global block height (consensus view).
    global_height: u64,
    /// Blocks propagated to all nodes.
    finalized_blocks: u64,
    /// When the testnet was started.
    started_at: Option<Instant>,
}

impl LocalTestnet {
    /// Create a new local testnet with the given configuration.
    pub fn new(config: TestnetConfig) -> Self {
        let mut node_configs = Vec::new();
        let mut genesis_allocations = Vec::new();

        // Create node configs
        for i in 0..config.num_validators {
            let mut node_config = TestNodeConfig::new(i);
            node_config.chain_id = config.chain_id;
            node_config.stake = config.initial_balance / 2; // Stake half

            // Each node bootstraps from node 0 (except node 0 itself)
            if i > 0 {
                node_config.bootstrap_peers.push(
                    format!("/ip4/127.0.0.1/tcp/{}", 9314),
                );
            }

            genesis_allocations.push(GenesisAllocation {
                address: node_config.validator_address(),
                balance: config.initial_balance,
                stake: node_config.stake,
            });

            node_configs.push(node_config);
        }

        let nodes: Vec<SimulatedNode> = node_configs.iter()
            .map(|c| SimulatedNode::new(c.clone()))
            .collect();

        Self {
            config,
            node_configs,
            nodes,
            genesis_allocations,
            global_height: 0,
            finalized_blocks: 0,
            started_at: None,
        }
    }

    /// Start the testnet (simulated in-process).
    pub fn start(&mut self) {
        info!(
            validators = self.config.num_validators,
            chain_id = self.config.chain_id,
            "starting local testnet"
        );

        self.started_at = Some(Instant::now());

        // Connect all nodes to each other
        for i in 0..self.nodes.len() {
            self.nodes[i].running = true;
            self.nodes[i].connected_peers = (0..self.nodes.len())
                .filter(|j| *j != i)
                .collect();
        }

        info!("all {} nodes started and connected", self.nodes.len());
    }

    /// Simulate producing a block at the given validator.
    pub fn produce_block(&mut self, validator_index: usize) -> Result<u64, TestnetError> {
        if validator_index >= self.nodes.len() {
            return Err(TestnetError::InvalidNode(validator_index));
        }
        if !self.nodes[validator_index].running {
            return Err(TestnetError::NodeNotRunning(validator_index));
        }
        if self.nodes[validator_index].partitioned {
            return Err(TestnetError::NodePartitioned(validator_index));
        }

        self.global_height += 1;
        let height = self.global_height;

        // Update the producing node
        self.nodes[validator_index].height = height;
        self.nodes[validator_index].blocks_produced += 1;

        // Propagate to connected, non-partitioned peers
        for i in 0..self.nodes.len() {
            if i == validator_index {
                continue;
            }
            if self.nodes[i].running && !self.nodes[i].partitioned {
                self.nodes[i].height = height;
            }
        }

        self.finalized_blocks += 1;

        Ok(height)
    }

    /// Simulate submitting a transaction through a specific node.
    pub fn submit_transaction(&mut self, node_index: usize) -> Result<(), TestnetError> {
        if node_index >= self.nodes.len() {
            return Err(TestnetError::InvalidNode(node_index));
        }
        if !self.nodes[node_index].running {
            return Err(TestnetError::NodeNotRunning(node_index));
        }

        self.nodes[node_index].txs_processed += 1;
        Ok(())
    }

    /// Partition a node from the network.
    pub fn partition_node(&mut self, node_index: usize) -> Result<(), TestnetError> {
        if node_index >= self.nodes.len() {
            return Err(TestnetError::InvalidNode(node_index));
        }
        self.nodes[node_index].partitioned = true;
        self.nodes[node_index].connected_peers.clear();

        // Remove this node from other nodes' peer lists
        for i in 0..self.nodes.len() {
            if i != node_index {
                self.nodes[i].connected_peers.retain(|p| *p != node_index);
            }
        }

        info!(node = node_index, "node partitioned from network");
        Ok(())
    }

    /// Heal a network partition for a node.
    pub fn heal_partition(&mut self, node_index: usize) -> Result<(), TestnetError> {
        if node_index >= self.nodes.len() {
            return Err(TestnetError::InvalidNode(node_index));
        }
        self.nodes[node_index].partitioned = false;

        // Reconnect to all running, non-partitioned peers
        self.nodes[node_index].connected_peers = (0..self.nodes.len())
            .filter(|j| *j != node_index && self.nodes[*j].running && !self.nodes[*j].partitioned)
            .collect();

        // Add this node back to other nodes' peer lists
        for i in 0..self.nodes.len() {
            if i != node_index && self.nodes[i].running && !self.nodes[i].partitioned {
                if !self.nodes[i].connected_peers.contains(&node_index) {
                    self.nodes[i].connected_peers.push(node_index);
                }
            }
        }

        // Catch up to global height
        self.nodes[node_index].height = self.global_height;

        info!(node = node_index, "partition healed, node rejoined");
        Ok(())
    }

    /// Stop a node.
    pub fn stop_node(&mut self, node_index: usize) -> Result<(), TestnetError> {
        if node_index >= self.nodes.len() {
            return Err(TestnetError::InvalidNode(node_index));
        }
        self.nodes[node_index].running = false;
        self.nodes[node_index].connected_peers.clear();

        for i in 0..self.nodes.len() {
            if i != node_index {
                self.nodes[i].connected_peers.retain(|p| *p != node_index);
            }
        }

        info!(node = node_index, "node stopped");
        Ok(())
    }

    /// Restart a stopped node.
    pub fn restart_node(&mut self, node_index: usize) -> Result<(), TestnetError> {
        if node_index >= self.nodes.len() {
            return Err(TestnetError::InvalidNode(node_index));
        }
        self.nodes[node_index].running = true;
        self.nodes[node_index].partitioned = false;
        self.heal_partition(node_index)?;
        info!(node = node_index, "node restarted");
        Ok(())
    }

    /// Get node count.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get running node count.
    pub fn running_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.running).count()
    }

    /// Get a node's state.
    pub fn node(&self, index: usize) -> Option<&SimulatedNode> {
        self.nodes.get(index)
    }

    /// Get the global block height.
    pub fn global_height(&self) -> u64 {
        self.global_height
    }

    /// Get the total blocks finalized.
    pub fn finalized_blocks(&self) -> u64 {
        self.finalized_blocks
    }

    /// Get the genesis allocations.
    pub fn genesis_allocations(&self) -> &[GenesisAllocation] {
        &self.genesis_allocations
    }

    /// Check if all running nodes are at the same height.
    pub fn is_consistent(&self) -> bool {
        let running_heights: Vec<u64> = self.nodes.iter()
            .filter(|n| n.running && !n.partitioned)
            .map(|n| n.height)
            .collect();

        if running_heights.is_empty() {
            return true;
        }

        running_heights.iter().all(|h| *h == running_heights[0])
    }

    /// Get testnet uptime.
    pub fn uptime(&self) -> Duration {
        self.started_at.map(|s| s.elapsed()).unwrap_or(Duration::ZERO)
    }

    /// Get node configs (for spawning real processes).
    pub fn node_configs(&self) -> &[TestNodeConfig] {
        &self.node_configs
    }

    /// Generate a testnet genesis config.
    pub fn genesis_config(&self) -> TestnetGenesisConfig {
        TestnetGenesisConfig {
            chain_id: self.config.chain_id,
            allocations: self.genesis_allocations.clone(),
            block_time_ms: self.config.block_time_ms,
            validators: self.node_configs.iter()
                .map(|c| ValidatorConfig {
                    address: c.validator_address(),
                    stake: c.stake,
                    p2p_addr: c.p2p_multiaddr(),
                })
                .collect(),
        }
    }
}

/// Testnet genesis configuration (for persisting to disk).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestnetGenesisConfig {
    pub chain_id: u64,
    pub allocations: Vec<GenesisAllocation>,
    pub block_time_ms: u64,
    pub validators: Vec<ValidatorConfig>,
}

/// Validator configuration for genesis.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidatorConfig {
    pub address: Address,
    pub stake: u64,
    pub p2p_addr: String,
}

/// Testnet errors.
#[derive(Debug, thiserror::Error)]
pub enum TestnetError {
    #[error("invalid node index: {0}")]
    InvalidNode(usize),
    #[error("node {0} is not running")]
    NodeNotRunning(usize),
    #[error("node {0} is partitioned")]
    NodePartitioned(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_testnet() {
        let config = TestnetConfig::default();
        let testnet = LocalTestnet::new(config);
        assert_eq!(testnet.node_count(), 4);
        assert_eq!(testnet.genesis_allocations().len(), 4);
    }

    #[test]
    fn start_and_produce_blocks() {
        let mut testnet = LocalTestnet::new(TestnetConfig::default());
        testnet.start();

        assert_eq!(testnet.running_count(), 4);

        // Produce blocks from different validators
        testnet.produce_block(0).unwrap();
        testnet.produce_block(1).unwrap();
        testnet.produce_block(2).unwrap();

        assert_eq!(testnet.global_height(), 3);
        assert!(testnet.is_consistent());
    }

    #[test]
    fn block_propagation() {
        let mut testnet = LocalTestnet::new(TestnetConfig::default());
        testnet.start();

        testnet.produce_block(0).unwrap();

        // All nodes should be at height 1
        for i in 0..4 {
            assert_eq!(testnet.node(i).unwrap().height, 1);
        }
    }

    #[test]
    fn network_partition() {
        let mut testnet = LocalTestnet::new(TestnetConfig::default());
        testnet.start();

        // Partition node 3
        testnet.partition_node(3).unwrap();

        // Produce a block
        testnet.produce_block(0).unwrap();

        // Node 3 should be behind
        assert_eq!(testnet.node(3).unwrap().height, 0);
        assert_eq!(testnet.node(0).unwrap().height, 1);
        // Consistency check excludes partitioned nodes, but the partitioned
        // node is definitively behind the rest
        assert!(testnet.node(3).unwrap().height < testnet.global_height());
    }

    #[test]
    fn heal_partition() {
        let mut testnet = LocalTestnet::new(TestnetConfig::default());
        testnet.start();

        testnet.partition_node(3).unwrap();
        testnet.produce_block(0).unwrap();
        assert_eq!(testnet.node(3).unwrap().height, 0);

        // Heal partition — node catches up
        testnet.heal_partition(3).unwrap();
        assert_eq!(testnet.node(3).unwrap().height, 1);
        assert!(testnet.is_consistent());
    }

    #[test]
    fn stop_and_restart_node() {
        let mut testnet = LocalTestnet::new(TestnetConfig::default());
        testnet.start();

        testnet.stop_node(2).unwrap();
        assert_eq!(testnet.running_count(), 3);

        testnet.produce_block(0).unwrap();
        assert_eq!(testnet.node(2).unwrap().height, 0);

        testnet.restart_node(2).unwrap();
        assert_eq!(testnet.running_count(), 4);
        assert_eq!(testnet.node(2).unwrap().height, 1);
    }

    #[test]
    fn partitioned_node_cannot_produce() {
        let mut testnet = LocalTestnet::new(TestnetConfig::default());
        testnet.start();

        testnet.partition_node(0).unwrap();
        assert!(testnet.produce_block(0).is_err());
    }

    #[test]
    fn stopped_node_cannot_produce() {
        let mut testnet = LocalTestnet::new(TestnetConfig::default());
        testnet.start();

        testnet.stop_node(0).unwrap();
        assert!(testnet.produce_block(0).is_err());
    }

    #[test]
    fn genesis_config_generation() {
        let testnet = LocalTestnet::new(TestnetConfig::default());
        let genesis = testnet.genesis_config();

        assert_eq!(genesis.chain_id, 31415);
        assert_eq!(genesis.validators.len(), 4);
        assert_eq!(genesis.allocations.len(), 4);
    }

    #[test]
    fn transaction_submission() {
        let mut testnet = LocalTestnet::new(TestnetConfig::default());
        testnet.start();

        testnet.submit_transaction(0).unwrap();
        testnet.submit_transaction(0).unwrap();
        assert_eq!(testnet.node(0).unwrap().txs_processed, 2);
    }

    #[test]
    fn multiple_partitions_and_heals() {
        let mut testnet = LocalTestnet::new(TestnetConfig {
            num_validators: 6,
            ..TestnetConfig::default()
        });
        testnet.start();

        testnet.produce_block(0).unwrap(); // Height 1

        // Partition nodes 4 and 5
        testnet.partition_node(4).unwrap();
        testnet.partition_node(5).unwrap();

        testnet.produce_block(1).unwrap(); // Height 2
        testnet.produce_block(2).unwrap(); // Height 3

        // Nodes 4,5 still at height 1
        assert_eq!(testnet.node(4).unwrap().height, 1);
        assert_eq!(testnet.node(5).unwrap().height, 1);

        // Heal both
        testnet.heal_partition(4).unwrap();
        testnet.heal_partition(5).unwrap();

        // Should catch up to height 3
        assert!(testnet.is_consistent());
        assert_eq!(testnet.node(4).unwrap().height, 3);
    }
}
