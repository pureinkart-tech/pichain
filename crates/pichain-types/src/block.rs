//! PIChain block types.
//!
//! Blocks contain ordered transactions from the Bullshark consensus output,
//! with Blake3 hashes linking them in a chain.

use pichain_crypto::ed25519::Address;
use pichain_crypto::Hash;
use serde::{Deserialize, Serialize};

use crate::transaction::SignedTransaction;
use crate::{BlockHeight, Epoch, Gas, PiAmount, Round};

/// Block header — the minimal data that validators attest to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    /// Chain identifier (314159 mainnet, 31415 devnet).
    /// Prevents cross-chain replay attacks.
    #[serde(default)]
    pub chain_id: u64,
    /// Block height (sequential).
    pub height: BlockHeight,
    /// Consensus epoch.
    pub epoch: Epoch,
    /// DAG round this block corresponds to.
    pub round: Round,
    /// Hash of the previous block header.
    pub parent_hash: Hash,
    /// Merkle root of all transactions in this block.
    pub tx_root: Hash,
    /// State root after executing all transactions (Jellyfish Merkle Tree root).
    pub state_root: pichain_crypto::poseidon::PoseidonHash,
    /// Receipts/effects root.
    pub receipts_root: Hash,
    /// Block producer (validator address).
    pub proposer: Address,
    /// Timestamp (milliseconds since Unix epoch).
    pub timestamp_ms: u64,
    /// Total gas used by all transactions.
    pub gas_used: Gas,
    /// Base fee per gas unit (EIP-1559 style).
    pub base_fee: PiAmount,
    /// Number of transactions in this block.
    pub tx_count: u32,
    /// Total PI burned in this block.
    pub pi_burned: PiAmount,
    /// Total miner fee income (25% of base fees) flowing to mining pool.
    #[serde(default)]
    pub pi_miner_fee: PiAmount,
}

impl BlockHeader {
    /// Compute the hash of this block header using canonical binary encoding.
    /// This avoids serde_json's non-deterministic field ordering which would
    /// break consensus across different nodes/platforms.
    pub fn hash(&self) -> Hash {
        let mut bytes = Vec::with_capacity(256);
        bytes.extend_from_slice(&self.chain_id.to_le_bytes());
        bytes.extend_from_slice(&self.height.to_le_bytes());
        bytes.extend_from_slice(&self.epoch.to_le_bytes());
        bytes.extend_from_slice(&self.round.to_le_bytes());
        bytes.extend_from_slice(self.parent_hash.as_bytes());
        bytes.extend_from_slice(self.tx_root.as_bytes());
        bytes.extend_from_slice(self.state_root.as_bytes());
        bytes.extend_from_slice(self.receipts_root.as_bytes());
        bytes.extend_from_slice(&self.proposer.0);
        bytes.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        bytes.extend_from_slice(&self.gas_used.to_le_bytes());
        bytes.extend_from_slice(&self.base_fee.to_le_bytes());
        bytes.extend_from_slice(&self.tx_count.to_le_bytes());
        bytes.extend_from_slice(&self.pi_burned.to_le_bytes());
        bytes.extend_from_slice(&self.pi_miner_fee.to_le_bytes());
        pichain_crypto::hash(&bytes)
    }

    /// Check if this is the genesis block.
    pub fn is_genesis(&self) -> bool {
        self.height == 0
    }

    /// Minimum base fee floor — prevents near-zero fees that enable spam.
    /// At 200,000 gas per tx, this means minimum ~0.2 PI per transaction.
    pub const MIN_BASE_FEE: PiAmount = 1_000;

    /// Calculate the next block's base fee (EIP-1559 adjustment).
    /// Increases if gas_used > 50% of target, decreases otherwise.
    /// Max adjustment: 12.5% per block.
    /// Uses u128 intermediate math to prevent overflow.
    pub fn next_base_fee(&self, gas_target: Gas) -> PiAmount {
        if gas_target == 0 || self.gas_used == gas_target {
            return self.base_fee;
        }

        if self.gas_used > gas_target {
            // Increase base fee (use u128 to prevent overflow)
            let delta = self.gas_used - gas_target;
            let fee_delta: u64 = (self.base_fee as u128 * delta as u128 / gas_target as u128 / 8)
                .try_into()
                .unwrap_or(u64::MAX);
            self.base_fee.saturating_add(fee_delta.max(1))
        } else {
            // Decrease base fee
            let delta = gas_target - self.gas_used;
            let fee_delta: u64 = (self.base_fee as u128 * delta as u128 / gas_target as u128 / 8)
                .try_into()
                .unwrap_or(u64::MAX);
            self.base_fee
                .saturating_sub(fee_delta)
                .max(Self::MIN_BASE_FEE)
        }
    }
}

/// A full block with header and transactions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    /// Block header.
    pub header: BlockHeader,
    /// Ordered transactions.
    pub transactions: Vec<SignedTransaction>,
    /// Proposer's Ed25519 public key (32 bytes, hex-encoded).
    /// Needed to verify signature since Ed25519 doesn't support key recovery.
    #[serde(default)]
    pub proposer_pubkey: Vec<u8>,
    /// Ed25519 signature over the header hash (64 bytes, hex-encoded).
    /// Proves the claimed proposer actually produced this block.
    /// Genesis blocks have an empty signature.
    #[serde(default)]
    pub proposer_sig: Vec<u8>,
}

impl Block {
    /// Create the genesis block.
    pub fn genesis(chain_id: u64, timestamp_ms: u64) -> Self {
        Self {
            header: BlockHeader {
                chain_id,
                height: 0,
                epoch: 0,
                round: 0,
                parent_hash: Hash::ZERO,
                tx_root: Hash::ZERO,
                state_root: pichain_crypto::poseidon::PoseidonHash::ZERO,
                receipts_root: Hash::ZERO,
                proposer: Address::ZERO,
                timestamp_ms,
                gas_used: 0,
                base_fee: 1_000, // 0.000001 PI initial base fee
                tx_count: 0,
                pi_burned: 0,
                pi_miner_fee: 0,
            },
            transactions: vec![],
            proposer_pubkey: vec![],
            proposer_sig: vec![],
        }
    }

    /// Sign this block with the proposer's Ed25519 keypair.
    pub fn sign(&mut self, keypair: &pichain_crypto::ed25519::Keypair) {
        self.proposer_pubkey = keypair.public.0.to_vec();
        let header_hash = self.header.hash();
        let sig = keypair.secret.sign(header_hash.as_bytes());
        self.proposer_sig = sig.to_bytes().to_vec();
    }

    /// Verify the proposer's Ed25519 signature over the header hash.
    /// Returns Ok(()) if valid, Err with description if invalid.
    /// Genesis blocks (height 0) are exempt from signature verification.
    pub fn verify_proposer_sig(&self) -> Result<(), String> {
        if self.header.height == 0 {
            return Ok(()); // Genesis block has no proposer
        }
        // Validate field lengths
        if self.proposer_pubkey.len() != 32 {
            return Err(format!(
                "proposer_pubkey must be 32 bytes, got {}",
                self.proposer_pubkey.len()
            ));
        }
        if self.proposer_sig.len() != 64 {
            return Err(format!(
                "proposer_sig must be 64 bytes, got {}",
                self.proposer_sig.len()
            ));
        }
        // Verify the embedded public key derives to the claimed proposer address
        let mut pk_bytes = [0u8; 32];
        pk_bytes.copy_from_slice(&self.proposer_pubkey);
        let pubkey = pichain_crypto::ed25519::PublicKey(pk_bytes);
        let derived_addr = pubkey.to_address();
        if derived_addr != self.header.proposer {
            return Err(format!(
                "proposer pubkey derives to {derived_addr}, but header claims {}",
                self.header.proposer
            ));
        }
        let header_hash = self.header.hash();
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&self.proposer_sig);
        let sig = pichain_crypto::ed25519::Signature::from_bytes(&sig_bytes);
        pubkey
            .verify(header_hash.as_bytes(), &sig)
            .map_err(|e| format!("proposer signature verification failed: {e}"))
    }

    /// Compute the transaction Merkle root.
    pub fn compute_tx_root(&self) -> Hash {
        if self.transactions.is_empty() {
            return Hash::ZERO;
        }

        let tx_hashes: Vec<Hash> = self.transactions.iter().map(|tx| tx.hash()).collect();
        merkle_root(&tx_hashes)
    }

    /// Block hash (hash of the header).
    pub fn hash(&self) -> Hash {
        self.header.hash()
    }
}

/// Compute a binary Merkle root from a list of hashes.
///
/// Uses domain separation to prevent second-preimage attacks:
/// - Leaf nodes are hashed with a 0x00 prefix
/// - Internal nodes are hashed with a 0x01 prefix
/// - Odd elements are promoted without re-hashing (no duplicate-last-tx vulnerability)
///
/// This MUST match `compute_merkle_root` in `block_producer.rs`.
fn merkle_root(hashes: &[Hash]) -> Hash {
    if hashes.is_empty() {
        return Hash::ZERO;
    }
    if hashes.len() == 1 {
        // Single leaf: hash with leaf prefix for domain separation
        return pichain_crypto::hash_concat(&[&[0x00], hashes[0].as_bytes()]);
    }

    // Hash each leaf with 0x00 prefix (domain separation: leaf vs internal)
    let mut current_level: Vec<Hash> = hashes
        .iter()
        .map(|h| pichain_crypto::hash_concat(&[&[0x00], h.as_bytes()]))
        .collect();

    while current_level.len() > 1 {
        let mut next_level = Vec::with_capacity(current_level.len().div_ceil(2));

        for chunk in current_level.chunks(2) {
            if chunk.len() == 2 {
                // Internal node: 0x01 prefix
                next_level.push(pichain_crypto::hash_concat(&[
                    &[0x01],
                    chunk[0].as_bytes(),
                    chunk[1].as_bytes(),
                ]));
            } else {
                // Odd element: promote to next level (avoids duplicate-last-tx attack)
                next_level.push(chunk[0]);
            }
        }

        current_level = next_level;
    }

    current_level[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_block() {
        let genesis = Block::genesis(31415, 1710403200000); // Pi Day 2025
        assert!(genesis.header.is_genesis());
        assert_eq!(genesis.header.height, 0);
        assert_eq!(genesis.header.parent_hash, Hash::ZERO);
        assert_eq!(genesis.transactions.len(), 0);
    }

    #[test]
    fn base_fee_adjustment() {
        let gas_target = 50_000_000u64; // 50% of 100M

        // Start above the floor so we can see decreases
        let mut h = Block::genesis(31415, 0).header;
        h.base_fee = 10_000;

        // Block used exactly target → no change
        h.gas_used = gas_target;
        assert_eq!(h.next_base_fee(gas_target), 10_000);

        // Block used more than target → fee increases
        h.gas_used = gas_target * 2;
        assert!(h.next_base_fee(gas_target) > 10_000);

        // Block used less than target → fee decreases
        h.gas_used = gas_target / 2;
        assert!(h.next_base_fee(gas_target) < 10_000);
    }

    #[test]
    fn base_fee_respects_minimum_floor() {
        let mut h = Block::genesis(31415, 0).header;
        let gas_target = 50_000_000u64;

        // Even with zero gas usage, base fee can't go below MIN_BASE_FEE
        h.base_fee = BlockHeader::MIN_BASE_FEE;
        h.gas_used = 0;
        let new_fee = h.next_base_fee(gas_target);
        assert_eq!(
            new_fee,
            BlockHeader::MIN_BASE_FEE,
            "base fee must not drop below floor"
        );
    }

    #[test]
    fn block_hash_deterministic() {
        let b = Block::genesis(31415, 12345);
        assert_eq!(b.hash(), b.hash());
    }

    #[test]
    fn merkle_root_works() {
        let h1 = pichain_crypto::hash(b"tx1");
        let h2 = pichain_crypto::hash(b"tx2");
        let h3 = pichain_crypto::hash(b"tx3");

        let root = merkle_root(&[h1, h2, h3]);
        assert_ne!(root, Hash::ZERO);

        // Same inputs → same root
        let root2 = merkle_root(&[h1, h2, h3]);
        assert_eq!(root, root2);

        // Different inputs → different root
        let root3 = merkle_root(&[h3, h2, h1]);
        assert_ne!(root, root3);
    }
}
