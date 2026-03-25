//! Post-Quantum Consensus Signatures for PIChain.
//!
//! Replaces BLS12-381 aggregate signatures with ML-DSA-65 per-validator signatures
//! for quantum-safe consensus. BLS12-381 is broken by Shor's algorithm.
//!
//! # Trade-offs
//!
//! - BLS: 48-byte aggregate signature for 500+ validators (bandwidth efficient)
//! - ML-DSA: 3,309-byte signature per validator (requires more bandwidth)
//!
//! With max 100 validators: 100 × 3,309 = ~330KB per consensus round.
//! At 314ms block time with modern networks, this is well within capacity.
//!
//! # Upgrade path
//!
//! When lattice-based aggregate signatures are standardized (active NIST research),
//! switch to those for bandwidth savings. The `PqConsensusVersion` enum enables
//! this without hard forks.

use pichain_crypto::keys::Address;
use pichain_crypto::pq::{MlDsaPublicKey, MlDsaSecretKey, MlDsaSignature};
use pichain_crypto::CryptoError;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Domain separator for PQ consensus signing.
/// Prevents cross-domain replay (a consensus sig cannot be used as a transaction sig).
const PQ_CONSENSUS_DOMAIN: &[u8] = b"pichain-pq-consensus-v1:";

/// Domain separator for PQ chain ID binding.
/// Certificates signed on devnet (31415) are invalid on mainnet (314159).
const PQ_CHAIN_ID_DOMAIN: &[u8] = b":chain_id=";

/// PQ consensus version for certificates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum PqConsensusVersion {
    /// ML-DSA-65 per-validator signatures (post-quantum).
    MlDsaV1 = 1,
}

impl Default for PqConsensusVersion {
    fn default() -> Self {
        Self::MlDsaV1
    }
}

/// Map from validator address to their ML-DSA public key.
pub type PqValidatorKeyMap = HashMap<Address, MlDsaPublicKey>;

/// A PQ consensus signature from a single validator.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PqConsensusSig {
    /// Signing validator's address.
    pub validator: Address,
    /// ML-DSA-65 signature over the consensus message.
    pub signature: MlDsaSignature,
}

/// Collection of PQ consensus signatures for a certificate/vote.
///
/// Replaces BLS aggregate signatures. Each validator signs independently.
/// Quorum is reached when 2f+1 valid signatures are collected.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PqConsensusSignatures {
    /// Individual validator signatures.
    pub signatures: Vec<PqConsensusSig>,
    /// Set of validators who have already signed (O(1) duplicate check).
    #[serde(skip)]
    seen_validators: HashSet<Address>,
}

impl PqConsensusSignatures {
    /// Create an empty signature collection.
    pub fn new() -> Self {
        Self {
            signatures: Vec::new(),
            seen_validators: HashSet::new(),
        }
    }

    /// Add a validator's signature.
    /// Returns true if added, false if this validator already signed.
    pub fn add(&mut self, validator: Address, signature: MlDsaSignature) -> bool {
        // O(1) duplicate check via HashSet
        if !self.seen_validators.insert(validator) {
            return false; // Already signed
        }
        self.signatures.push(PqConsensusSig {
            validator,
            signature,
        });
        true
    }

    /// Number of signatures collected.
    pub fn count(&self) -> usize {
        self.signatures.len()
    }

    /// Check if we have reached quorum (2f+1 out of total validators).
    pub fn has_quorum(&self, total_validators: usize) -> bool {
        let f = (total_validators.saturating_sub(1)) / 3;
        let quorum = 2 * f + 1;
        self.count() >= quorum
    }

    /// Verify all signatures against the given message and validator key map.
    ///
    /// Returns the number of valid signatures and the list of invalid validators.
    pub fn verify_all(
        &self,
        message: &[u8],
        key_map: &PqValidatorKeyMap,
    ) -> Result<usize, Vec<Address>> {
        let mut valid_count = 0;
        let mut invalid = Vec::new();

        for sig in &self.signatures {
            match key_map.get(&sig.validator) {
                Some(pk) => {
                    if pk.verify(message, &sig.signature).is_ok() {
                        valid_count += 1;
                    } else {
                        invalid.push(sig.validator);
                    }
                }
                None => {
                    invalid.push(sig.validator);
                }
            }
        }

        if invalid.is_empty() {
            Ok(valid_count)
        } else {
            Err(invalid)
        }
    }

    /// Total size of all signatures in bytes.
    pub fn total_bytes(&self) -> usize {
        self.signatures.len() * pichain_crypto::pq::ML_DSA_SIG_BYTES
    }

    /// Get the addresses of all signers.
    pub fn signers(&self) -> Vec<Address> {
        self.signatures.iter().map(|s| s.validator).collect()
    }
}

/// Build the consensus message that validators sign.
///
/// Includes domain separation and chain ID to prevent:
/// - Cross-domain replay (consensus sig used as transaction sig)
/// - Cross-chain replay (devnet sig used on mainnet)
pub fn consensus_message(round: u64, data: &[u8], chain_id: u64) -> Vec<u8> {
    let mut msg = Vec::with_capacity(
        PQ_CONSENSUS_DOMAIN.len() + 8 + 4 + data.len() + PQ_CHAIN_ID_DOMAIN.len() + 8,
    );
    msg.extend_from_slice(PQ_CONSENSUS_DOMAIN);
    msg.extend_from_slice(&round.to_le_bytes());
    // Length-prefix the data to prevent boundary-shift attacks where an attacker
    // crafts (round, data, chain_id) tuples that produce the same concatenation.
    msg.extend_from_slice(&(data.len() as u32).to_le_bytes());
    msg.extend_from_slice(data);
    msg.extend_from_slice(PQ_CHAIN_ID_DOMAIN);
    msg.extend_from_slice(&chain_id.to_le_bytes());
    msg
}

/// Sign a consensus message with a validator's ML-DSA secret key.
pub fn sign_consensus(
    round: u64,
    data: &[u8],
    chain_id: u64,
    sk: &MlDsaSecretKey,
    validator_addr: Address,
) -> PqConsensusSig {
    let msg = consensus_message(round, data, chain_id);
    let signature = sk.sign(&msg);
    PqConsensusSig {
        validator: validator_addr,
        signature,
    }
}

/// Verify a single consensus signature.
pub fn verify_consensus_sig(
    round: u64,
    data: &[u8],
    chain_id: u64,
    sig: &PqConsensusSig,
    pk: &MlDsaPublicKey,
) -> Result<(), CryptoError> {
    let msg = consensus_message(round, data, chain_id);
    pk.verify(&msg, &sig.signature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::pq::PqKeypair;

    fn make_validator() -> (Address, MlDsaSecretKey, MlDsaPublicKey) {
        let kp = PqKeypair::generate();
        let addr = kp.address();
        // Clone key bytes since PqKeypair owns them
        let sk = pichain_crypto::pq::MlDsaSecretKey::from_bytes(kp.ml_dsa_sk.as_bytes()).unwrap();
        let pk = pichain_crypto::pq::MlDsaPublicKey::from_bytes(kp.ml_dsa_pk.as_bytes()).unwrap();
        (addr, sk, pk)
    }

    #[test]
    fn sign_and_verify_consensus() {
        let (addr, sk, pk) = make_validator();
        let round = 42;
        let data = b"block hash data";
        let chain_id = 31415;

        let sig = sign_consensus(round, data, chain_id, &sk, addr);
        assert!(verify_consensus_sig(round, data, chain_id, &sig, &pk).is_ok());
    }

    #[test]
    fn wrong_round_fails_verification() {
        let (addr, sk, pk) = make_validator();
        let sig = sign_consensus(1, b"data", 31415, &sk, addr);
        assert!(verify_consensus_sig(2, b"data", 31415, &sig, &pk).is_err());
    }

    #[test]
    fn wrong_chain_id_fails_verification() {
        let (addr, sk, pk) = make_validator();
        let sig = sign_consensus(1, b"data", 31415, &sk, addr);
        // Devnet sig invalid on mainnet
        assert!(verify_consensus_sig(1, b"data", 314159, &sig, &pk).is_err());
    }

    #[test]
    fn wrong_data_fails_verification() {
        let (addr, sk, pk) = make_validator();
        let sig = sign_consensus(1, b"correct", 31415, &sk, addr);
        assert!(verify_consensus_sig(1, b"tampered", 31415, &sig, &pk).is_err());
    }

    #[test]
    fn quorum_calculation() {
        let sigs = PqConsensusSignatures::new();
        // 1 validator: need 1 (2*0+1)
        assert!(!sigs.has_quorum(1));

        // 4 validators: f=1, need 3
        let mut sigs = PqConsensusSignatures::new();
        for i in 0..3 {
            let (addr, sk, _) = make_validator();
            let sig = sign_consensus(1, b"data", 31415, &sk, addr);
            sigs.add(addr, sig.signature);
        }
        assert!(sigs.has_quorum(4)); // 4 validators: f=1, need 3. 3>=3 ✓
        assert!(sigs.has_quorum(5)); // 5 validators: f=1, need 3. 3>=3 ✓
        assert!(!sigs.has_quorum(7)); // 7 validators: f=2, need 5. 3<5 ✗
    }

    #[test]
    fn verify_all_signatures() {
        let validators: Vec<_> = (0..4).map(|_| make_validator()).collect();
        let mut key_map = PqValidatorKeyMap::new();
        let mut sigs = PqConsensusSignatures::new();

        let round = 1;
        let data = b"consensus data";
        let chain_id = 31415;

        for (addr, sk, pk) in &validators {
            key_map.insert(*addr, pk.clone());
            let sig = sign_consensus(round, data, chain_id, sk, *addr);
            sigs.add(*addr, sig.signature);
        }

        let msg = consensus_message(round, data, chain_id);
        let result = sigs.verify_all(&msg, &key_map);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 4);
    }

    #[test]
    fn verify_rejects_unknown_validator() {
        let (addr1, sk1, pk1) = make_validator();
        let (addr2, sk2, _pk2) = make_validator(); // pk2 not in key_map

        let mut key_map = PqValidatorKeyMap::new();
        key_map.insert(addr1, pk1);
        // Intentionally omit addr2 from key_map

        let mut sigs = PqConsensusSignatures::new();
        let msg_bytes = b"data";
        let sig1 = sign_consensus(1, msg_bytes, 31415, &sk1, addr1);
        let sig2 = sign_consensus(1, msg_bytes, 31415, &sk2, addr2);
        sigs.add(addr1, sig1.signature);
        sigs.add(addr2, sig2.signature);

        let msg = consensus_message(1, msg_bytes, 31415);
        let result = sigs.verify_all(&msg, &key_map);
        assert!(result.is_err());
        let invalid = result.unwrap_err();
        assert_eq!(invalid.len(), 1);
        assert_eq!(invalid[0], addr2);
    }

    #[test]
    fn duplicate_signer_rejected() {
        let (addr, sk, _) = make_validator();
        let mut sigs = PqConsensusSignatures::new();

        let sig1 = sign_consensus(1, b"data", 31415, &sk, addr);
        let sig2 = sign_consensus(1, b"data", 31415, &sk, addr);
        assert!(sigs.add(addr, sig1.signature)); // first add succeeds
        assert!(!sigs.add(addr, sig2.signature)); // duplicate rejected

        assert_eq!(sigs.count(), 1);
    }

    #[test]
    fn consensus_message_domain_separation() {
        let msg1 = consensus_message(1, b"data", 31415);
        let msg2 = consensus_message(1, b"data", 314159);
        assert_ne!(
            msg1, msg2,
            "different chain IDs must produce different messages"
        );

        let msg3 = consensus_message(1, b"data", 31415);
        let msg4 = consensus_message(2, b"data", 31415);
        assert_ne!(
            msg3, msg4,
            "different rounds must produce different messages"
        );
    }

    #[test]
    fn total_bytes_calculation() {
        let mut sigs = PqConsensusSignatures::new();
        assert_eq!(sigs.total_bytes(), 0);

        let (addr, sk, _) = make_validator();
        let sig = sign_consensus(1, b"data", 31415, &sk, addr);
        sigs.add(addr, sig.signature);

        assert_eq!(sigs.total_bytes(), pichain_crypto::pq::ML_DSA_SIG_BYTES);
    }
}
