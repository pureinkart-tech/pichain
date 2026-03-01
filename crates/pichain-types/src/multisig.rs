//! Multi-signature wallet support for PIChain.
//!
//! Allows M-of-N threshold signing: a transaction from a multisig address
//! is only valid when signed by at least `threshold` of the `signers`.

use pichain_crypto::ed25519::{Address, PublicKey, Signature};
use serde::{Deserialize, Serialize};

/// Multi-signature wallet configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MultisigWallet {
    /// The derived multisig address (deterministic from sorted signers + threshold).
    pub address: Address,
    /// Required number of signatures.
    pub threshold: u8,
    /// Authorized signer addresses.
    pub signers: Vec<Address>,
    /// Transaction nonce for replay protection.
    pub nonce: u64,
    /// Creation timestamp.
    pub created_at_ms: u64,
}

/// A pending multisig transaction awaiting signatures.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MultisigProposal {
    /// Unique proposal ID (hash of multisig_address + nonce + inner_tx).
    pub id: [u8; 32],
    /// The multisig wallet address.
    pub multisig_address: Address,
    /// The inner transaction to execute when threshold is met.
    pub inner_tx_data: Vec<u8>,
    /// Collected signatures so far.
    pub signatures: Vec<MultisigSignature>,
    /// Whether this proposal has been executed.
    pub executed: bool,
    /// Proposal nonce (must match multisig wallet nonce).
    pub nonce: u64,
}

/// A signature on a multisig proposal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MultisigSignature {
    /// Signer address.
    pub signer: Address,
    /// Ed25519 signature over the proposal ID.
    pub signature: Signature,
    /// Public key for verification.
    pub public_key: PublicKey,
}

impl MultisigWallet {
    /// Derive a deterministic multisig address from sorted signers + threshold.
    pub fn derive_address(signers: &[Address], threshold: u8) -> Address {
        let mut data = Vec::new();
        data.extend_from_slice(b"pichain-multisig-v1");
        data.push(threshold);
        // Sort signers for deterministic derivation
        let mut sorted = signers.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        for signer in &sorted {
            data.extend_from_slice(&signer.0);
        }
        let hash = pichain_crypto::hash(&data);
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&hash.as_bytes()[..20]);
        Address(addr)
    }

    /// Check if an address is an authorized signer.
    pub fn is_signer(&self, address: &Address) -> bool {
        self.signers.contains(address)
    }

    /// Create a new multisig wallet.
    pub fn new(signers: Vec<Address>, threshold: u8, created_at_ms: u64) -> Result<Self, String> {
        if threshold == 0 {
            return Err("threshold must be > 0".to_string());
        }
        if threshold as usize > signers.len() {
            return Err(format!(
                "threshold {} exceeds signer count {}",
                threshold,
                signers.len()
            ));
        }
        if signers.len() > 10 {
            return Err("maximum 10 signers allowed".to_string());
        }
        let address = Self::derive_address(&signers, threshold);
        Ok(Self {
            address,
            threshold,
            signers,
            nonce: 0,
            created_at_ms,
        })
    }
}

impl MultisigProposal {
    /// Check if this proposal has enough signatures to execute.
    pub fn has_threshold(&self, wallet: &MultisigWallet) -> bool {
        self.signatures.len() >= wallet.threshold as usize
    }

    /// Verify all collected signatures.
    pub fn verify_signatures(&self) -> bool {
        self.signatures.iter().all(|sig| {
            sig.public_key.verify(&self.id, &sig.signature).is_ok()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::Keypair;

    #[test]
    fn derive_address_deterministic() {
        let kp1 = Keypair::generate();
        let kp2 = Keypair::generate();
        let signers = vec![kp1.address(), kp2.address()];

        let addr1 = MultisigWallet::derive_address(&signers, 2);
        let addr2 = MultisigWallet::derive_address(&signers, 2);
        assert_eq!(addr1, addr2);

        // Different threshold = different address
        let addr3 = MultisigWallet::derive_address(&signers, 1);
        assert_ne!(addr1, addr3);
    }

    #[test]
    fn create_wallet() {
        let kp1 = Keypair::generate();
        let kp2 = Keypair::generate();
        let kp3 = Keypair::generate();

        let wallet = MultisigWallet::new(
            vec![kp1.address(), kp2.address(), kp3.address()],
            2,
            1000,
        ).unwrap();

        assert_eq!(wallet.threshold, 2);
        assert_eq!(wallet.signers.len(), 3);
        assert!(wallet.is_signer(&kp1.address()));
        assert!(!wallet.is_signer(&Address::ZERO));
    }

    #[test]
    fn threshold_validation() {
        let kp1 = Keypair::generate();
        assert!(MultisigWallet::new(vec![kp1.address()], 0, 0).is_err());
        assert!(MultisigWallet::new(vec![kp1.address()], 2, 0).is_err());
        assert!(MultisigWallet::new(vec![kp1.address()], 1, 0).is_ok());
    }
}
