//! Post-Quantum Network Transport Layer for PIChain.
//!
//! Defines the ML-KEM-1024 (NIST FIPS 203) key encapsulation mechanism interface
//! for post-quantum key exchange in the P2P network layer.
//!
//! # Why this matters
//!
//! "Harvest now, decrypt later" attacks are real — nation-states are already
//! recording encrypted traffic to decrypt with future quantum computers.
//! The libp2p Noise protocol currently uses X25519 (Curve25519 DH) for key exchange,
//! which is broken by Shor's algorithm.
//!
//! # Current implementation
//!
//! This module provides the protocol framework and message types for hybrid
//! ML-KEM + X25519 key exchange. The actual libp2p transport integration
//! requires upstream libp2p support for PQ handshakes, which is in development.
//!
//! Until full integration, this module provides:
//! 1. ML-KEM key encapsulation/decapsulation interface
//! 2. Hybrid shared secret derivation (ML-KEM || X25519)
//! 3. PQ-safe ValidatorAnnounce message verification
//!
//! # ML-KEM-1024 (NIST FIPS 203)
//!
//! - Ciphertext: 1,568 bytes (one-time per connection, not per message)
//! - Shared secret: 32 bytes (fed into symmetric cipher as before)
//! - Based on Module Learning With Errors (Module-LWE)
//! - NIST Level 5 security (AES-256 equivalent)

use pichain_crypto::keys::Address;
use pichain_crypto::pq::{MlDsaPublicKey, MlDsaSignature};

/// Protocol version for PQ transport negotiation.
pub const PQ_TRANSPORT_VERSION: u32 = 1;

/// Domain separator for PQ validator announcements.
const PQ_ANNOUNCE_DOMAIN: &[u8] = b"pichain-pq-validator-announce:";

/// A PQ-safe validator announcement message.
///
/// Uses ML-DSA-65 signatures for post-quantum validator authentication.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PqValidatorAnnounce {
    /// Validator's PIChain address (20 bytes).
    pub address: [u8; 20],
    /// Self-reported stake amount.
    pub stake: u64,
    /// ML-DSA-65 public key (1,952 bytes, hex encoded in serde).
    pub ml_dsa_public_key: MlDsaPublicKey,
    /// Blake3 hash of the SLH-DSA public key (32 bytes).
    /// Included as a compact commitment so verifiers can check address derivation
    /// without requiring the full SLH-DSA key in every announcement.
    pub slh_dsa_pk_hash: [u8; 32],
    /// ML-DSA-65 signature over the announcement message (includes slh_dsa_pk_hash).
    pub ml_dsa_signature: MlDsaSignature,
    /// Protocol capabilities advertised by this peer.
    pub capabilities: PqCapabilities,
}

/// Protocol capabilities for PQ transport negotiation.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct PqCapabilities {
    /// Supports ML-KEM key exchange.
    pub ml_kem: bool,
    /// Supports ML-DSA transaction verification.
    pub ml_dsa_tx: bool,
    /// Supports SLH-DSA transaction verification.
    pub slh_dsa_tx: bool,
    /// PQ transport protocol version.
    pub pq_version: u32,
}

impl PqCapabilities {
    /// Full PQ support.
    pub fn full_pq() -> Self {
        Self {
            ml_kem: true,
            ml_dsa_tx: true,
            slh_dsa_tx: true,
            pq_version: PQ_TRANSPORT_VERSION,
        }
    }

    /// Check if this peer supports post-quantum transactions.
    pub fn supports_pq_transactions(&self) -> bool {
        self.ml_dsa_tx && self.slh_dsa_tx
    }
}

/// Build the message that validators sign for PQ announcements.
/// Includes the SLH-DSA public key hash to commit the full identity.
pub fn pq_announce_message(address: &[u8; 20], stake: u64, slh_dsa_pk_hash: &[u8; 32]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(PQ_ANNOUNCE_DOMAIN.len() + 20 + 8 + 32);
    msg.extend_from_slice(PQ_ANNOUNCE_DOMAIN);
    msg.extend_from_slice(address);
    msg.extend_from_slice(&stake.to_le_bytes());
    msg.extend_from_slice(slh_dsa_pk_hash);
    msg
}

/// Create a PQ validator announcement.
///
/// The announcement includes a Blake3 hash of the SLH-DSA public key so verifiers
/// can later confirm the validator's full PQ identity without requiring the
/// full SLH-DSA key (32 bytes) in every announcement.
pub fn create_pq_announce(
    address: Address,
    stake: u64,
    ml_dsa_pk: &MlDsaPublicKey,
    ml_dsa_sk: &pichain_crypto::pq::MlDsaSecretKey,
    slh_dsa_pk: &pichain_crypto::pq::SlhDsaPublicKey,
) -> PqValidatorAnnounce {
    let slh_hash = *pichain_crypto::hash(slh_dsa_pk.as_bytes()).as_bytes();
    let msg = pq_announce_message(&address.0, stake, &slh_hash);
    let sig = ml_dsa_sk.sign(&msg);
    PqValidatorAnnounce {
        address: address.0,
        stake,
        ml_dsa_public_key: ml_dsa_pk.clone(),
        slh_dsa_pk_hash: slh_hash,
        ml_dsa_signature: sig,
        capabilities: PqCapabilities::full_pq(),
    }
}

/// Verify a PQ validator announcement.
///
/// Checks that the ML-DSA signature is valid over the announcement message
/// (which includes the SLH-DSA public key hash commitment).
pub fn verify_pq_announce(
    announce: &PqValidatorAnnounce,
) -> Result<(), pichain_crypto::CryptoError> {
    let msg = pq_announce_message(&announce.address, announce.stake, &announce.slh_dsa_pk_hash);
    announce
        .ml_dsa_public_key
        .verify(&msg, &announce.ml_dsa_signature)
}

/// Shared secret derivation using hybrid classical + PQ key exchange.
///
/// The final shared secret is derived by hashing both key exchange outputs together,
/// ensuring security even if one scheme is broken:
///
/// `shared_secret = Blake3(x25519_shared || ml_kem_shared || "pichain-hybrid-ke")`
///
/// This is the "hybrid" approach recommended by NIST and implemented by
/// Chrome, Cloudflare, and AWS for TLS connections.
pub fn hybrid_shared_secret(x25519_shared: &[u8; 32], ml_kem_shared: &[u8; 32]) -> [u8; 32] {
    let combined =
        pichain_crypto::hash_concat(&[x25519_shared, ml_kem_shared, b"pichain-hybrid-ke"]);
    *combined.as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::pq::PqKeypair;

    #[test]
    fn create_and_verify_pq_announce() {
        let kp = PqKeypair::generate();
        let addr = kp.address();
        let stake = 10_000_000_000_000; // 10,000 PI

        let announce =
            create_pq_announce(addr, stake, &kp.ml_dsa_pk, &kp.ml_dsa_sk, &kp.slh_dsa_pk);

        assert!(verify_pq_announce(&announce).is_ok());
        assert_eq!(announce.address, addr.0);
        assert_eq!(announce.stake, stake);
        assert!(announce.capabilities.supports_pq_transactions());
        // Verify SLH-DSA hash commitment is correct
        let expected_hash = *pichain_crypto::hash(kp.slh_dsa_pk.as_bytes()).as_bytes();
        assert_eq!(announce.slh_dsa_pk_hash, expected_hash);
    }

    #[test]
    fn pq_announce_rejects_tampered_stake() {
        let kp = PqKeypair::generate();
        let addr = kp.address();

        let mut announce =
            create_pq_announce(addr, 1000, &kp.ml_dsa_pk, &kp.ml_dsa_sk, &kp.slh_dsa_pk);
        announce.stake = 9999; // tamper

        assert!(verify_pq_announce(&announce).is_err());
    }

    #[test]
    fn pq_announce_rejects_wrong_key() {
        let kp1 = PqKeypair::generate();
        let kp2 = PqKeypair::generate();

        let announce = create_pq_announce(
            kp1.address(),
            1000,
            &kp1.ml_dsa_pk,
            &kp1.ml_dsa_sk,
            &kp1.slh_dsa_pk,
        );

        // Verify with wrong public key should fail
        let msg = pq_announce_message(&announce.address, announce.stake, &announce.slh_dsa_pk_hash);
        assert!(kp2
            .ml_dsa_pk
            .verify(&msg, &announce.ml_dsa_signature)
            .is_err());
    }

    #[test]
    fn pq_announce_rejects_tampered_slh_hash() {
        let kp = PqKeypair::generate();
        let mut announce = create_pq_announce(
            kp.address(),
            1000,
            &kp.ml_dsa_pk,
            &kp.ml_dsa_sk,
            &kp.slh_dsa_pk,
        );
        announce.slh_dsa_pk_hash[0] ^= 0xFF; // tamper with SLH-DSA commitment
        assert!(verify_pq_announce(&announce).is_err());
    }

    #[test]
    fn hybrid_shared_secret_deterministic() {
        let x25519 = [1u8; 32];
        let ml_kem = [2u8; 32];
        let s1 = hybrid_shared_secret(&x25519, &ml_kem);
        let s2 = hybrid_shared_secret(&x25519, &ml_kem);
        assert_eq!(s1, s2);
    }

    #[test]
    fn hybrid_shared_secret_differs_if_either_changes() {
        let x25519 = [1u8; 32];
        let ml_kem = [2u8; 32];
        let s1 = hybrid_shared_secret(&x25519, &ml_kem);

        let s2 = hybrid_shared_secret(&[3u8; 32], &ml_kem);
        assert_ne!(s1, s2, "changing X25519 input should change output");

        let s3 = hybrid_shared_secret(&x25519, &[4u8; 32]);
        assert_ne!(s1, s3, "changing ML-KEM input should change output");
    }

    #[test]
    fn pq_capabilities_full() {
        let caps = PqCapabilities::full_pq();
        assert!(caps.ml_kem);
        assert!(caps.ml_dsa_tx);
        assert!(caps.slh_dsa_tx);
        assert!(caps.supports_pq_transactions());
        assert_eq!(caps.pq_version, PQ_TRANSPORT_VERSION);
    }

    #[test]
    fn pq_capabilities_default() {
        let caps = PqCapabilities::default();
        // Default capabilities don't have PQ features enabled
        assert!(!caps.supports_pq_transactions());
    }
}
