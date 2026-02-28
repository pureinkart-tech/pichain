//! Bridge infrastructure for cross-chain asset transfers.
//!
//! Supports bridging between PIChain and other networks:
//! - Ethereum (EVM) bridge — lock/mint mechanism
//! - Generic message passing — relay arbitrary data cross-chain
//! - Light client verification — verify source chain headers
//!
//! Bridge flow:
//! 1. User locks tokens on source chain
//! 2. Bridge relayers observe the lock event
//! 3. Relayers submit attestations to destination chain
//! 4. Once quorum is reached, wrapped tokens are minted on destination chain
//! 5. Reverse: burn wrapped tokens → release original on source chain

use pichain_crypto::ed25519::{Address, PublicKey, Signature};
use pichain_crypto::Hash;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Minimum blocks between transfer initiation and execution.
/// Prevents same-block bridge exploits where source chain finality hasn't been confirmed.
/// ~30 minutes at 314ms blocks ≈ 5,732 blocks.
const MIN_BRIDGE_CONFIRMATION_BLOCKS: u64 = 5_732;

/// Supported bridge chains.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BridgeChain {
    /// Ethereum mainnet.
    Ethereum,
    /// BNB Smart Chain.
    BnbChain,
    /// Arbitrum (L2).
    Arbitrum,
    /// Base (L2).
    Base,
    /// Solana.
    Solana,
    /// Generic chain identified by ID.
    Other(u64),
}

impl BridgeChain {
    /// Get the chain ID used for bridge operations.
    pub fn chain_id(&self) -> u64 {
        match self {
            BridgeChain::Ethereum => 1,
            BridgeChain::BnbChain => 56,
            BridgeChain::Arbitrum => 42161,
            BridgeChain::Base => 8453,
            BridgeChain::Solana => 1399811149, // Wormhole convention
            BridgeChain::Other(id) => *id,
        }
    }
}

/// A bridge transfer request (lock/burn on source chain).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeTransfer {
    /// Unique transfer ID (hash of transfer details).
    pub id: Hash,
    /// Source chain.
    pub source_chain: BridgeChain,
    /// Destination chain.
    pub dest_chain: BridgeChain,
    /// Sender address on source chain.
    pub sender: Vec<u8>,
    /// Recipient address on destination chain.
    pub recipient: Vec<u8>,
    /// Token being bridged (address on source chain).
    pub token: Vec<u8>,
    /// Amount being bridged.
    pub amount: u128,
    /// Source chain block number where the lock occurred.
    pub source_block: u64,
    /// Source chain transaction hash.
    pub source_tx_hash: Vec<u8>,
    /// Status of the transfer.
    pub status: TransferStatus,
    /// Timestamp (milliseconds).
    pub timestamp_ms: u64,
    /// Nonce (prevents replay).
    pub nonce: u64,
}

/// Status of a bridge transfer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferStatus {
    /// Observed on source chain, waiting for attestations.
    Pending,
    /// Has enough attestations, ready to be executed.
    Confirmed,
    /// Executed on destination chain.
    Completed,
    /// Failed or expired.
    Failed(String),
    /// Refunded on source chain.
    Refunded,
}

/// Attestation from a bridge relayer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attestation {
    /// Transfer ID being attested.
    pub transfer_id: Hash,
    /// Relayer address.
    pub relayer: Address,
    /// Relayer's Ed25519 signature over the transfer attestation message.
    pub signature: Signature,
    /// Block height when the attestation was submitted.
    pub submitted_at: u64,
}

/// Bridge relayer — a trusted entity that observes cross-chain events.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeRelayer {
    /// Relayer's PIChain address.
    pub address: Address,
    /// Relayer's Ed25519 public key (for signature verification).
    pub public_key: PublicKey,
    /// Relayer's stake (bonded for honest behavior).
    pub stake: u64,
    /// Whether this relayer is active.
    pub active: bool,
    /// Number of attestations submitted.
    pub attestation_count: u64,
    /// Number of incorrect attestations (can lead to slashing).
    pub incorrect_count: u64,
}

/// Configuration for bridge token pairs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeTokenPair {
    /// PIChain token (native PI address or contract address).
    pub pi_token: Address,
    /// External chain token address.
    pub external_token: Vec<u8>,
    /// External chain.
    pub chain: BridgeChain,
    /// Decimal conversion factor.
    pub decimal_factor: u128,
    /// Whether bridging is currently enabled.
    pub enabled: bool,
    /// Total locked on PIChain side.
    pub total_locked: u128,
    /// Total minted on external side.
    pub total_minted: u128,
    /// Minimum bridge amount.
    pub min_amount: u128,
    /// Bridge fee in basis points.
    pub fee_bps: u16,
}

/// Default cap on pending (unconfirmed) bridge transfers to prevent memory exhaustion.
const DEFAULT_MAX_PENDING_TRANSFERS: usize = 10_000;

/// The bridge manager — coordinates cross-chain transfers.
pub struct BridgeManager {
    /// Active bridge transfers, keyed by transfer ID.
    transfers: HashMap<Hash, BridgeTransfer>,
    /// Attestations per transfer.
    attestations: HashMap<Hash, Vec<Attestation>>,
    /// Registered bridge relayers.
    relayers: HashMap<Address, BridgeRelayer>,
    /// Configured token pairs.
    token_pairs: Vec<BridgeTokenPair>,
    /// Required attestations for confirmation (quorum).
    required_attestations: usize,
    /// Total value locked across all bridges.
    total_value_locked: u128,
    /// Transfer nonce counter.
    next_nonce: u64,
    /// Current block height.
    current_height: u64,
    /// Maximum pending (unconfirmed) transfers allowed (Fix NET-226).
    /// Prevents unbounded memory growth from spam or sustained attack.
    max_pending_transfers: usize,
}

impl BridgeManager {
    /// Create a new bridge manager.
    pub fn new(required_attestations: usize) -> Self {
        Self {
            transfers: HashMap::new(),
            attestations: HashMap::new(),
            relayers: HashMap::new(),
            token_pairs: Vec::new(),
            // Minimum quorum of 3 to prevent single-relayer compromise.
            // A quorum of 1-2 allows a single compromised relayer to
            // unilaterally mint unlimited wrapped tokens.
            required_attestations: required_attestations.max(3),
            total_value_locked: 0,
            next_nonce: 0,
            current_height: 0,
            max_pending_transfers: DEFAULT_MAX_PENDING_TRANSFERS,
        }
    }

    /// Set the current block height.
    pub fn set_height(&mut self, height: u64) {
        self.current_height = height;
    }

    /// Register a bridge relayer with their public key for signature verification.
    pub fn register_relayer(
        &mut self,
        address: Address,
        public_key: PublicKey,
        stake: u64,
    ) -> Result<(), BridgeError> {
        // Verify the public key derives to the claimed address
        if public_key.to_address() != address {
            return Err(BridgeError::PublicKeyMismatch(address));
        }
        if self.relayers.contains_key(&address) {
            return Err(BridgeError::RelayerAlreadyRegistered(address));
        }
        self.relayers.insert(address, BridgeRelayer {
            address,
            public_key,
            stake,
            active: true,
            attestation_count: 0,
            incorrect_count: 0,
        });
        Ok(())
    }

    /// Add a bridge token pair.
    pub fn add_token_pair(&mut self, pair: BridgeTokenPair) {
        self.token_pairs.push(pair);
    }

    /// Initiate a bridge transfer from PIChain to an external chain.
    /// This locks tokens on PIChain and creates a pending transfer.
    pub fn initiate_transfer(
        &mut self,
        sender: Address,
        dest_chain: BridgeChain,
        recipient: Vec<u8>,
        amount: u128,
    ) -> Result<Hash, BridgeError> {
        if amount == 0 {
            return Err(BridgeError::ZeroAmount);
        }

        // Reject new transfers when pending count is at capacity (Fix NET-226).
        // This prevents unbounded memory growth from sustained transfer spam.
        if self.pending_count() >= self.max_pending_transfers {
            return Err(BridgeError::TooManyPendingTransfers {
                current: self.pending_count(),
                max: self.max_pending_transfers,
            });
        }

        // Find the token pair
        let pair = self.token_pairs.iter_mut()
            .find(|p| p.chain == dest_chain && p.enabled)
            .ok_or(BridgeError::UnsupportedChain(dest_chain.clone()))?;

        if amount < pair.min_amount {
            return Err(BridgeError::BelowMinimum {
                amount,
                minimum: pair.min_amount,
            });
        }

        let nonce = self.next_nonce;
        self.next_nonce = self.next_nonce.saturating_add(1);

        // Compute transfer ID including dest_chain to prevent cross-chain replay.
        // Without dest_chain, attestation signatures for a transfer to Ethereum
        // could be replayed for an identical transfer to Arbitrum.
        let mut id_input = Vec::new();
        id_input.extend_from_slice(&sender.0);
        id_input.extend_from_slice(&recipient);
        id_input.extend_from_slice(&amount.to_le_bytes());
        id_input.extend_from_slice(&nonce.to_le_bytes());
        id_input.extend_from_slice(&dest_chain.chain_id().to_le_bytes());
        let transfer_id = pichain_crypto::hash(&id_input);

        let transfer = BridgeTransfer {
            id: transfer_id,
            source_chain: BridgeChain::Other(314159), // PIChain
            dest_chain,
            sender: sender.0.to_vec(),
            recipient,
            token: pair.pi_token.0.to_vec(),
            amount,
            source_block: self.current_height,
            source_tx_hash: vec![],
            status: TransferStatus::Pending,
            timestamp_ms: 0,
            nonce,
        };

        pair.total_locked = pair.total_locked.saturating_add(amount);
        self.total_value_locked = self.total_value_locked.saturating_add(amount);
        self.transfers.insert(transfer_id, transfer);
        self.attestations.insert(transfer_id, Vec::new());

        Ok(transfer_id)
    }

    /// Build the canonical attestation message that relayers must sign.
    /// Format: "pichain-bridge-attest:" || transfer_id || amount_le || nonce_le || dest_chain_id_le
    /// The dest_chain is included to prevent cross-chain replay attacks.
    pub fn attestation_message(transfer: &BridgeTransfer) -> Vec<u8> {
        let mut msg = Vec::with_capacity(32 + 16 + 8 + 8 + 22);
        msg.extend_from_slice(b"pichain-bridge-attest:");
        msg.extend_from_slice(transfer.id.as_bytes());
        msg.extend_from_slice(&transfer.amount.to_le_bytes());
        msg.extend_from_slice(&transfer.nonce.to_le_bytes());
        msg.extend_from_slice(&transfer.dest_chain.chain_id().to_le_bytes());
        msg
    }

    /// Submit an attestation for an incoming bridge transfer.
    /// The signature is verified against the relayer's registered public key.
    pub fn submit_attestation(
        &mut self,
        transfer_id: Hash,
        relayer: Address,
        signature: Signature,
    ) -> Result<bool, BridgeError> {
        let relay = self.relayers.get(&relayer)
            .ok_or(BridgeError::RelayerNotRegistered(relayer))?;

        if !relay.active {
            return Err(BridgeError::RelayerInactive(relayer));
        }

        let public_key = relay.public_key;

        let transfer = self.transfers.get(&transfer_id)
            .ok_or(BridgeError::TransferNotFound(transfer_id))?;

        if transfer.status != TransferStatus::Pending {
            return Err(BridgeError::TransferNotPending(transfer_id));
        }

        // Verify the Ed25519 signature over the canonical attestation message
        let msg = Self::attestation_message(transfer);
        public_key.verify(&msg, &signature).map_err(|_| {
            BridgeError::InvalidAttestationSignature(relayer, transfer_id)
        })?;

        // Compute dynamic quorum BEFORE mutable borrows on attestations.
        // Use the larger of: static required_attestations, or dynamic 2/3+1 of active relayers.
        // This prevents a situation where required_attestations = 3 but there are 10 relayers,
        // meaning only 3 of 10 relayers need to collude.
        let active_count = self.active_relayer_count();
        let dynamic_quorum = (active_count * 2 / 3) + 1;
        // Cap quorum at active relayer count to prevent deadlock when relayers go inactive.
        // Without this cap, if required_attestations=3 but only 2 relayers are active,
        // no transfers can ever be confirmed.
        // SECURITY: Hard floor of 1 — never allow zero-attestation confirmations.
        // If no relayers are active, transfers can never be confirmed (correct behavior).
        if active_count == 0 {
            return Err(BridgeError::NoActiveRelayers);
        }
        let effective_quorum = self.required_attestations.max(dynamic_quorum).min(active_count).max(1);

        let attestations = self.attestations.entry(transfer_id).or_default();

        // Check for duplicate attestation
        if attestations.iter().any(|a| a.relayer == relayer) {
            return Err(BridgeError::DuplicateAttestation(relayer, transfer_id));
        }

        // Update relayer stats (re-borrow mutably after signature verification)
        if let Some(relay) = self.relayers.get_mut(&relayer) {
            relay.attestation_count += 1;
        }

        attestations.push(Attestation {
            transfer_id,
            relayer,
            signature,
            submitted_at: self.current_height,
        });

        // Check if we've reached quorum
        let confirmed = attestations.len() >= effective_quorum;
        if confirmed {
            if let Some(transfer) = self.transfers.get_mut(&transfer_id) {
                transfer.status = TransferStatus::Confirmed;
            }
        }

        Ok(confirmed)
    }

    /// Execute a confirmed bridge transfer (mint/release tokens).
    /// Applies bridge fee and decrements TVL.
    /// Requires minimum block age since initiation to allow source chain finality.
    pub fn execute_transfer(&mut self, transfer_id: Hash) -> Result<&BridgeTransfer, BridgeError> {
        let transfer = self.transfers.get_mut(&transfer_id)
            .ok_or(BridgeError::TransferNotFound(transfer_id))?;

        if transfer.status != TransferStatus::Confirmed {
            return Err(BridgeError::TransferNotConfirmed(transfer_id));
        }

        // Enforce minimum block age to ensure source chain finality
        if self.current_height < transfer.source_block.saturating_add(MIN_BRIDGE_CONFIRMATION_BLOCKS) {
            return Err(BridgeError::TransferNotConfirmed(transfer_id));
        }

        let amount = transfer.amount;
        transfer.status = TransferStatus::Completed;

        // Decrement TVL and apply fee
        self.total_value_locked = self.total_value_locked.saturating_sub(amount);
        if let Some(pair) = self.token_pairs.iter_mut()
            .find(|p| p.chain == self.transfers[&transfer_id].dest_chain)
        {
            pair.total_locked = pair.total_locked.saturating_sub(amount);
            // Apply bridge fee (in basis points)
            let fee = amount * pair.fee_bps as u128 / 10_000;
            let net_amount = amount.saturating_sub(fee);
            pair.total_minted = pair.total_minted.saturating_add(net_amount);
        }

        self.transfers.get(&transfer_id)
            .ok_or(BridgeError::TransferNotFound(transfer_id))
    }

    /// Expire pending transfers that have been waiting too long.
    /// Returns the number of transfers expired.
    pub fn expire_stale_transfers(&mut self, max_age_blocks: u64) -> usize {
        let current = self.current_height;
        let expired_ids: Vec<Hash> = self.transfers.iter()
            .filter(|(_, t)| {
                t.status == TransferStatus::Pending
                    && current.saturating_sub(t.source_block) > max_age_blocks
            })
            .map(|(id, _)| *id)
            .collect();

        for id in &expired_ids {
            if let Some(transfer) = self.transfers.get_mut(id) {
                // Unlock the locked funds
                let amount = transfer.amount;
                self.total_value_locked = self.total_value_locked.saturating_sub(amount);
                if let Some(pair) = self.token_pairs.iter_mut()
                    .find(|p| p.chain == transfer.dest_chain)
                {
                    pair.total_locked = pair.total_locked.saturating_sub(amount);
                }
                transfer.status = TransferStatus::Failed("expired".to_string());
            }
        }

        expired_ids.len()
    }

    /// Get a transfer by ID.
    pub fn get_transfer(&self, id: &Hash) -> Option<&BridgeTransfer> {
        self.transfers.get(id)
    }

    /// Get attestations for a transfer.
    pub fn get_attestations(&self, id: &Hash) -> &[Attestation] {
        self.attestations.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get total value locked.
    pub fn total_value_locked(&self) -> u128 {
        self.total_value_locked
    }

    /// Get relayer count.
    pub fn relayer_count(&self) -> usize {
        self.relayers.len()
    }

    /// Get active relayer count.
    pub fn active_relayer_count(&self) -> usize {
        self.relayers.values().filter(|r| r.active).count()
    }

    /// Get pending transfer count.
    pub fn pending_count(&self) -> usize {
        self.transfers.values().filter(|t| t.status == TransferStatus::Pending).count()
    }

    /// Get required attestations for quorum.
    pub fn required_attestations(&self) -> usize {
        self.required_attestations
    }
}

impl Default for BridgeManager {
    fn default() -> Self {
        Self::new(3) // Default: 3 relayer attestations needed
    }
}

/// Bridge-related errors.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("relayer already registered: {0}")]
    RelayerAlreadyRegistered(Address),
    #[error("relayer not registered: {0}")]
    RelayerNotRegistered(Address),
    #[error("relayer inactive: {0}")]
    RelayerInactive(Address),
    #[error("unsupported chain: {0:?}")]
    UnsupportedChain(BridgeChain),
    #[error("transfer not found: {0}")]
    TransferNotFound(Hash),
    #[error("transfer not pending: {0}")]
    TransferNotPending(Hash),
    #[error("transfer not confirmed: {0}")]
    TransferNotConfirmed(Hash),
    #[error("duplicate attestation from {0} for {1}")]
    DuplicateAttestation(Address, Hash),
    #[error("zero amount")]
    ZeroAmount,
    #[error("below minimum: {amount}, minimum: {minimum}")]
    BelowMinimum { amount: u128, minimum: u128 },
    #[error("invalid attestation signature from {0} for transfer {1}")]
    InvalidAttestationSignature(Address, Hash),
    #[error("public key does not match address: {0}")]
    PublicKeyMismatch(Address),
    #[error("no active relayers — bridge transfers cannot be confirmed")]
    NoActiveRelayers,
    #[error("too many pending transfers: {current}/{max}")]
    TooManyPendingTransfers { current: usize, max: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::Keypair;

    /// Helper: generate a relayer keypair and return (address, public_key, keypair).
    fn make_relayer(seed: u8) -> (Address, PublicKey, Keypair) {
        let kp = Keypair::from_secret_bytes(&[seed; 32]);
        let addr = kp.address();
        (addr, kp.public, kp)
    }

    /// Sign an attestation message for a transfer using a relayer's keypair.
    fn sign_attestation(kp: &Keypair, transfer: &BridgeTransfer) -> Signature {
        let msg = BridgeManager::attestation_message(transfer);
        kp.sign(&msg)
    }

    fn setup_bridge() -> (BridgeManager, Vec<Keypair>) {
        let mut mgr = BridgeManager::new(3); // 3 attestations minimum (enforced floor)

        let (a1, pk1, kp1) = make_relayer(1);
        let (a2, pk2, kp2) = make_relayer(2);
        let (a3, pk3, kp3) = make_relayer(3);
        mgr.register_relayer(a1, pk1, 100_000).unwrap();
        mgr.register_relayer(a2, pk2, 100_000).unwrap();
        mgr.register_relayer(a3, pk3, 100_000).unwrap();

        // Add ETH bridge token pair
        mgr.add_token_pair(BridgeTokenPair {
            pi_token: Address::ZERO,
            external_token: vec![0u8; 20], // ETH null = native ETH
            chain: BridgeChain::Ethereum,
            decimal_factor: 1_000_000_000, // 9 to 18 decimal conversion
            enabled: true,
            total_locked: 0,
            total_minted: 0,
            min_amount: 1_000_000_000, // 1 PI minimum
            fee_bps: 30, // 0.3% bridge fee
        });

        (mgr, vec![kp1, kp2, kp3])
    }

    #[test]
    fn register_relayers() {
        let (mgr, _) = setup_bridge();
        assert_eq!(mgr.relayer_count(), 3);
        assert_eq!(mgr.active_relayer_count(), 3);
    }

    #[test]
    fn register_relayer_rejects_mismatched_pubkey() {
        let mut mgr = BridgeManager::new(3);
        let (_, _, kp) = make_relayer(1);
        // Try to register with a wrong address
        let result = mgr.register_relayer(Address([99u8; 20]), kp.public, 100_000);
        assert!(result.is_err());
    }

    #[test]
    fn initiate_bridge_transfer() {
        let (mut mgr, _) = setup_bridge();
        let sender = Address([10u8; 20]);
        let recipient = vec![20u8; 20];

        let id = mgr.initiate_transfer(
            sender,
            BridgeChain::Ethereum,
            recipient,
            5_000_000_000, // 5 PI
        ).unwrap();

        let transfer = mgr.get_transfer(&id).unwrap();
        assert_eq!(transfer.status, TransferStatus::Pending);
        assert_eq!(transfer.amount, 5_000_000_000);
        assert_eq!(mgr.total_value_locked(), 5_000_000_000);
    }

    #[test]
    fn reject_zero_amount() {
        let (mut mgr, _) = setup_bridge();
        assert!(mgr.initiate_transfer(
            Address([1u8; 20]), BridgeChain::Ethereum, vec![1u8; 20], 0,
        ).is_err());
    }

    #[test]
    fn reject_below_minimum() {
        let (mut mgr, _) = setup_bridge();
        assert!(mgr.initiate_transfer(
            Address([1u8; 20]), BridgeChain::Ethereum, vec![1u8; 20], 100, // Below 1 PI minimum
        ).is_err());
    }

    #[test]
    fn attestation_flow_with_verified_signatures() {
        let (mut mgr, kps) = setup_bridge();
        let sender = Address([10u8; 20]);
        let id = mgr.initiate_transfer(
            sender, BridgeChain::Ethereum, vec![20u8; 20], 5_000_000_000,
        ).unwrap();

        let transfer = mgr.get_transfer(&id).unwrap().clone();

        // First attestation — properly signed, not yet confirmed
        let sig1 = sign_attestation(&kps[0], &transfer);
        let confirmed = mgr.submit_attestation(id, kps[0].address(), sig1).unwrap();
        assert!(!confirmed);
        assert_eq!(mgr.get_transfer(&id).unwrap().status, TransferStatus::Pending);

        // Second attestation — still not confirmed (quorum = max(3, 2/3*3+1) = 3)
        let sig2 = sign_attestation(&kps[1], &transfer);
        let confirmed = mgr.submit_attestation(id, kps[1].address(), sig2).unwrap();
        assert!(!confirmed);
        assert_eq!(mgr.get_transfer(&id).unwrap().status, TransferStatus::Pending);

        // Third attestation — reaches quorum (3 required with 3 active relayers)
        let sig3 = sign_attestation(&kps[2], &transfer);
        let confirmed = mgr.submit_attestation(id, kps[2].address(), sig3).unwrap();
        assert!(confirmed);
        assert_eq!(mgr.get_transfer(&id).unwrap().status, TransferStatus::Confirmed);
    }

    #[test]
    fn reject_forged_attestation_signature() {
        let (mut mgr, kps) = setup_bridge();
        let id = mgr.initiate_transfer(
            Address([10u8; 20]), BridgeChain::Ethereum, vec![20u8; 20], 5_000_000_000,
        ).unwrap();

        // Try to submit attestation with wrong signature (signed by kp2 but claiming to be kp1)
        let transfer = mgr.get_transfer(&id).unwrap().clone();
        let wrong_sig = sign_attestation(&kps[1], &transfer); // signed by relayer 2
        let result = mgr.submit_attestation(id, kps[0].address(), wrong_sig); // but claiming relayer 1
        assert!(result.is_err());
    }

    #[test]
    fn reject_duplicate_attestation() {
        let (mut mgr, kps) = setup_bridge();
        let id = mgr.initiate_transfer(
            Address([10u8; 20]), BridgeChain::Ethereum, vec![20u8; 20], 5_000_000_000,
        ).unwrap();

        let transfer = mgr.get_transfer(&id).unwrap().clone();
        let sig = sign_attestation(&kps[0], &transfer);
        mgr.submit_attestation(id, kps[0].address(), sig).unwrap();

        let sig2 = sign_attestation(&kps[0], &transfer);
        assert!(mgr.submit_attestation(id, kps[0].address(), sig2).is_err());
    }

    #[test]
    fn execute_confirmed_transfer_decrements_tvl() {
        let (mut mgr, kps) = setup_bridge();
        let id = mgr.initiate_transfer(
            Address([10u8; 20]), BridgeChain::Ethereum, vec![20u8; 20], 5_000_000_000,
        ).unwrap();
        assert_eq!(mgr.total_value_locked(), 5_000_000_000);

        let transfer = mgr.get_transfer(&id).unwrap().clone();
        let sig1 = sign_attestation(&kps[0], &transfer);
        let sig2 = sign_attestation(&kps[1], &transfer);
        let sig3 = sign_attestation(&kps[2], &transfer);
        mgr.submit_attestation(id, kps[0].address(), sig1).unwrap();
        mgr.submit_attestation(id, kps[1].address(), sig2).unwrap();
        mgr.submit_attestation(id, kps[2].address(), sig3).unwrap();

        // Advance past minimum confirmation blocks
        mgr.set_height(MIN_BRIDGE_CONFIRMATION_BLOCKS + 1);

        let t = mgr.execute_transfer(id).unwrap();
        assert_eq!(t.status, TransferStatus::Completed);
        // TVL should be 0 after the transfer completes
        assert_eq!(mgr.total_value_locked(), 0);
    }

    #[test]
    fn reject_execute_pending_transfer() {
        let (mut mgr, _) = setup_bridge();
        let id = mgr.initiate_transfer(
            Address([10u8; 20]), BridgeChain::Ethereum, vec![20u8; 20], 5_000_000_000,
        ).unwrap();

        assert!(mgr.execute_transfer(id).is_err());
    }

    #[test]
    fn unsupported_chain_rejected() {
        let (mut mgr, _) = setup_bridge();
        assert!(mgr.initiate_transfer(
            Address([1u8; 20]), BridgeChain::Solana, vec![1u8; 32], 5_000_000_000,
        ).is_err());
    }

    #[test]
    fn unregistered_relayer_rejected() {
        let (mut mgr, _) = setup_bridge();
        let id = mgr.initiate_transfer(
            Address([10u8; 20]), BridgeChain::Ethereum, vec![20u8; 20], 5_000_000_000,
        ).unwrap();

        let fake_sig = Signature::from_bytes(&[0u8; 64]);
        assert!(mgr.submit_attestation(id, Address([99u8; 20]), fake_sig).is_err());
    }

    #[test]
    fn chain_ids() {
        assert_eq!(BridgeChain::Ethereum.chain_id(), 1);
        assert_eq!(BridgeChain::BnbChain.chain_id(), 56);
        assert_eq!(BridgeChain::Arbitrum.chain_id(), 42161);
    }

    #[test]
    fn expire_stale_transfers() {
        let (mut mgr, _) = setup_bridge();
        mgr.set_height(100);
        let id = mgr.initiate_transfer(
            Address([10u8; 20]), BridgeChain::Ethereum, vec![20u8; 20], 5_000_000_000,
        ).unwrap();
        assert_eq!(mgr.total_value_locked(), 5_000_000_000);

        // Advance height far beyond the transfer's source block
        mgr.set_height(200);
        let expired = mgr.expire_stale_transfers(50); // max 50 blocks age
        assert_eq!(expired, 1);
        assert_eq!(mgr.total_value_locked(), 0);
        assert!(matches!(
            mgr.get_transfer(&id).unwrap().status,
            TransferStatus::Failed(_)
        ));
    }

    /// NET-226: Bridge must reject new transfers when at pending capacity.
    #[test]
    fn bridge_rejects_when_at_capacity() {
        let (mut mgr, _) = setup_bridge();

        // Set a very low capacity for testing
        mgr.max_pending_transfers = 3;

        let sender = Address([10u8; 20]);

        // Fill up to the limit (3 pending transfers)
        for i in 0u8..3 {
            let recipient = vec![20 + i; 20];
            mgr.initiate_transfer(sender, BridgeChain::Ethereum, recipient, 5_000_000_000)
                .expect("should accept transfer within capacity");
        }
        assert_eq!(mgr.pending_count(), 3);

        // The 4th transfer must be rejected
        let result = mgr.initiate_transfer(
            sender, BridgeChain::Ethereum, vec![99u8; 20], 5_000_000_000,
        );
        assert!(result.is_err(), "must reject when at capacity");
        match result.unwrap_err() {
            BridgeError::TooManyPendingTransfers { current, max } => {
                assert_eq!(current, 3);
                assert_eq!(max, 3);
            }
            other => panic!("expected TooManyPendingTransfers, got: {other}"),
        }

        // Verify count didn't increase
        assert_eq!(mgr.pending_count(), 3);
    }
}
