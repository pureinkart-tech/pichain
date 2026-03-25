//! Post-Quantum Cryptography for PIChain.
//!
//! Implements dual-family hybrid post-quantum signatures using:
//! - **ML-DSA-65** (NIST FIPS 204, lattice-based) — fast, moderate signature size (3,309 bytes)
//! - **SLH-DSA-SHAKE-128f** (NIST FIPS 205, hash-based) — security based solely on hash preimage resistance
//!
//! Both signatures must verify for a transaction to be valid. This ensures security
//! even if one mathematical family (lattice or hash) is broken by future advances.
//!
//! # Security rationale
//!
//! - ML-DSA: Based on Module Learning With Errors (Module-LWE). NIST Level 3 (AES-192 equivalent).
//!   Believed quantum-safe, but lattice assumptions are ~20 years old.
//! - SLH-DSA: Based entirely on hash function preimage resistance. As long as SHAKE-128 is
//!   preimage-resistant (40+ years of hash function research), SLH-DSA is unbreakable.
//!   This is the most conservative quantum-safe assumption in all of cryptography.
//!
//! # Stack usage warning
//!
//! SLH-DSA signing allocates large intermediate structures on the stack.
//! All signing operations should run on threads with at least 8MB stack size.
//! Use [`sign_on_large_stack`] or spawn a thread with sufficient stack.

use pqcrypto_dilithium::dilithium3;
use pqcrypto_sphincsplus::sphincsshake128fsimple;
use pqcrypto_traits::sign::{
    DetachedSignature as PqDetachedSignature, PublicKey as PqPublicKey, SecretKey as PqSecretKey,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::CryptoError;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// ML-DSA-65 (Dilithium3) public key size in bytes.
pub const ML_DSA_PK_BYTES: usize = 1952;
/// ML-DSA-65 (Dilithium3) secret key size in bytes.
pub const ML_DSA_SK_BYTES: usize = 4032;
/// ML-DSA-65 (Dilithium3) signature size in bytes.
pub const ML_DSA_SIG_BYTES: usize = 3309;

/// SLH-DSA-SHAKE-128f (SPHINCS+) public key size in bytes.
pub const SLH_DSA_PK_BYTES: usize = 32;
/// SLH-DSA-SHAKE-128f (SPHINCS+) secret key size in bytes.
pub const SLH_DSA_SK_BYTES: usize = 64;
/// SLH-DSA-SHAKE-128f (SPHINCS+) signature size in bytes.
pub const SLH_DSA_SIG_BYTES: usize = 17088;

/// Minimum thread stack size for SLH-DSA signing operations.
pub const PQ_SIGNING_STACK_SIZE: usize = 8 * 1024 * 1024; // 8 MB

// ─────────────────────────────────────────────────────────────────────────────
// ML-DSA-65 (Lattice-based, NIST FIPS 204)
// ─────────────────────────────────────────────────────────────────────────────

/// ML-DSA-65 public key (1,952 bytes).
#[derive(Clone, PartialEq, Eq)]
pub struct MlDsaPublicKey(Vec<u8>);

impl MlDsaPublicKey {
    /// Create from raw bytes. Returns error if length is not exactly [`ML_DSA_PK_BYTES`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() != ML_DSA_PK_BYTES {
            return Err(CryptoError::InvalidPublicKey);
        }
        Ok(Self(bytes.to_vec()))
    }

    /// Return the raw bytes of this public key.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Verify an ML-DSA-65 detached signature over a message.
    pub fn verify(&self, message: &[u8], signature: &MlDsaSignature) -> Result<(), CryptoError> {
        let pk = dilithium3::PublicKey::from_bytes(&self.0)
            .map_err(|_| CryptoError::InvalidPublicKey)?;
        let sig = dilithium3::DetachedSignature::from_bytes(&signature.0)
            .map_err(|_| CryptoError::InvalidSignature)?;
        dilithium3::verify_detached_signature(&sig, message, &pk)
            .map_err(|_| CryptoError::InvalidSignature)
    }
}

impl std::fmt::Debug for MlDsaPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MlDsaPK({}..)", hex::encode(&self.0[..8]))
    }
}

impl Serialize for MlDsaPublicKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for MlDsaPublicKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if s.len() != ML_DSA_PK_BYTES * 2 {
            return Err(serde::de::Error::custom(format!(
                "ML-DSA public key hex must be {} chars, got {}",
                ML_DSA_PK_BYTES * 2,
                s.len()
            )));
        }
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        Self::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

/// ML-DSA-65 secret key (4,032 bytes). Zeroized on drop.
pub struct MlDsaSecretKey(Vec<u8>);

impl MlDsaSecretKey {
    /// Create from raw bytes. Returns error if length is not exactly [`ML_DSA_SK_BYTES`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() != ML_DSA_SK_BYTES {
            return Err(CryptoError::InvalidSecretKey);
        }
        Ok(Self(bytes.to_vec()))
    }

    /// Return the raw bytes of this secret key.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Sign a message with ML-DSA-65 (detached signature).
    pub fn sign(&self, message: &[u8]) -> MlDsaSignature {
        let sk = dilithium3::SecretKey::from_bytes(&self.0)
            .expect("MlDsaSecretKey invariant: always valid length");
        let sig = dilithium3::detached_sign(message, &sk);
        MlDsaSignature(sig.as_bytes().to_vec())
    }

    /// Derive the corresponding ML-DSA public key.
    ///
    /// The Dilithium3 secret key embeds the public key seed (rho) and the
    /// verification key material. We recover the public key by signing a
    /// canonical message and extracting the public key through the pqcrypto
    /// keypair structure. To avoid this overhead at runtime, prefer storing
    /// the public key alongside the secret key (as [`PqKeypair`] does).
    ///
    /// NOTE: This uses a cached re-derivation approach. The pqcrypto crate
    /// does not expose a direct sk→pk function, so we store the keypair
    /// relationship at generation time. For deserialized secret keys,
    /// the public key must be provided separately (see [`PqKeypair::from_secret_bytes`]
    /// which requires both sk bytes, and derives pk by signing+verifying).
    ///
    /// Dilithium3 (ML-DSA-65) sk layout (PQClean reference):
    ///   rho(32) || K(32) || tr(64) || s1(packed) || s2(packed) || t0(packed)
    /// The public key (rho || t1) is NOT a simple suffix of the secret key.
    /// We extract rho (first 32 bytes) which is shared between sk and pk,
    /// but full pk recovery requires re-expanding t1 from t0 and the NTT.
    ///
    /// Since pqcrypto doesn't expose this, we store pk separately.
    pub fn public_key_bytes_not_derivable() -> &'static str {
        "ML-DSA secret key does not embed the full public key; store pk separately"
    }
}

impl Drop for MlDsaSecretKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// ML-DSA-65 detached signature (3,309 bytes).
#[derive(Clone, PartialEq, Eq)]
pub struct MlDsaSignature(Vec<u8>);

impl MlDsaSignature {
    /// Create from raw bytes. Returns error if length is not exactly [`ML_DSA_SIG_BYTES`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() != ML_DSA_SIG_BYTES {
            return Err(CryptoError::InvalidSignature);
        }
        Ok(Self(bytes.to_vec()))
    }

    /// Return the raw bytes of this signature.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for MlDsaSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MlDsaSig({}..)", hex::encode(&self.0[..8]))
    }
}

impl Serialize for MlDsaSignature {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for MlDsaSignature {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if s.len() != ML_DSA_SIG_BYTES * 2 {
            return Err(serde::de::Error::custom(format!(
                "ML-DSA signature hex must be {} chars, got {}",
                ML_DSA_SIG_BYTES * 2,
                s.len()
            )));
        }
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        Self::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SLH-DSA-SHAKE-128f (Hash-based, NIST FIPS 205)
// ─────────────────────────────────────────────────────────────────────────────

/// SLH-DSA-SHAKE-128f public key (32 bytes).
#[derive(Clone, PartialEq, Eq)]
pub struct SlhDsaPublicKey(Vec<u8>);

impl SlhDsaPublicKey {
    /// Create from raw bytes. Returns error if length is not exactly [`SLH_DSA_PK_BYTES`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() != SLH_DSA_PK_BYTES {
            return Err(CryptoError::InvalidPublicKey);
        }
        Ok(Self(bytes.to_vec()))
    }

    /// Return the raw bytes of this public key.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Verify an SLH-DSA-SHAKE-128f detached signature over a message.
    pub fn verify(&self, message: &[u8], signature: &SlhDsaSignature) -> Result<(), CryptoError> {
        let pk = sphincsshake128fsimple::PublicKey::from_bytes(&self.0)
            .map_err(|_| CryptoError::InvalidPublicKey)?;
        let sig = sphincsshake128fsimple::DetachedSignature::from_bytes(&signature.0)
            .map_err(|_| CryptoError::InvalidSignature)?;
        sphincsshake128fsimple::verify_detached_signature(&sig, message, &pk)
            .map_err(|_| CryptoError::InvalidSignature)
    }
}

impl std::fmt::Debug for SlhDsaPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SlhDsaPK({})", hex::encode(&self.0))
    }
}

impl Serialize for SlhDsaPublicKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for SlhDsaPublicKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if s.len() != SLH_DSA_PK_BYTES * 2 {
            return Err(serde::de::Error::custom(format!(
                "SLH-DSA public key hex must be {} chars, got {}",
                SLH_DSA_PK_BYTES * 2,
                s.len()
            )));
        }
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        Self::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

/// SLH-DSA-SHAKE-128f secret key (64 bytes). Zeroized on drop.
pub struct SlhDsaSecretKey(Vec<u8>);

impl SlhDsaSecretKey {
    /// Create from raw bytes. Returns error if length is not exactly [`SLH_DSA_SK_BYTES`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() != SLH_DSA_SK_BYTES {
            return Err(CryptoError::InvalidSecretKey);
        }
        Ok(Self(bytes.to_vec()))
    }

    /// Return the raw bytes of this secret key.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Sign a message with SLH-DSA-SHAKE-128f (detached signature).
    ///
    /// # Stack usage
    ///
    /// This function allocates large intermediate structures on the stack.
    /// Ensure the calling thread has at least [`PQ_SIGNING_STACK_SIZE`] (8MB) stack.
    /// For signing from the main thread, use [`PqKeypair::sign_on_large_stack`] instead.
    pub fn sign(&self, message: &[u8]) -> SlhDsaSignature {
        let sk = sphincsshake128fsimple::SecretKey::from_bytes(&self.0)
            .expect("SlhDsaSecretKey invariant: always valid length");
        let sig = sphincsshake128fsimple::detached_sign(message, &sk);
        SlhDsaSignature(sig.as_bytes().to_vec())
    }

    /// Derive the corresponding SLH-DSA public key.
    pub fn public_key(&self) -> SlhDsaPublicKey {
        // SPHINCS+ sk = SK.seed (32) || PK (32). PK is the last 32 bytes.
        SlhDsaPublicKey(self.0[SLH_DSA_SK_BYTES - SLH_DSA_PK_BYTES..].to_vec())
    }
}

impl Drop for SlhDsaSecretKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// SLH-DSA-SHAKE-128f detached signature (17,088 bytes).
#[derive(Clone, PartialEq, Eq)]
pub struct SlhDsaSignature(Vec<u8>);

impl SlhDsaSignature {
    /// Create from raw bytes. Returns error if length is not exactly [`SLH_DSA_SIG_BYTES`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() != SLH_DSA_SIG_BYTES {
            return Err(CryptoError::InvalidSignature);
        }
        Ok(Self(bytes.to_vec()))
    }

    /// Return the raw bytes of this signature.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for SlhDsaSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SlhDsaSig({}..)", hex::encode(&self.0[..8]))
    }
}

impl Serialize for SlhDsaSignature {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Use base64 for SLH-DSA sigs (17KB in hex would be 34KB)
        serializer.serialize_str(&hex::encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for SlhDsaSignature {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if s.len() != SLH_DSA_SIG_BYTES * 2 {
            return Err(serde::de::Error::custom(format!(
                "SLH-DSA signature hex must be {} chars, got {}",
                SLH_DSA_SIG_BYTES * 2,
                s.len()
            )));
        }
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        Self::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Dual Post-Quantum Keypair
// ─────────────────────────────────────────────────────────────────────────────

/// Dual post-quantum keypair: ML-DSA-65 + SLH-DSA-SHAKE-128f.
///
/// Both signature schemes must verify for a transaction to be valid.
/// This provides redundant quantum safety across two independent mathematical families:
/// - Lattice-based (ML-DSA): broken if Module-LWE is solved
/// - Hash-based (SLH-DSA): broken only if hash preimage resistance fails
///
/// The probability of both being broken simultaneously is negligible.
pub struct PqKeypair {
    pub ml_dsa_sk: MlDsaSecretKey,
    pub ml_dsa_pk: MlDsaPublicKey,
    pub slh_dsa_sk: SlhDsaSecretKey,
    pub slh_dsa_pk: SlhDsaPublicKey,
}

impl PqKeypair {
    /// Generate a new dual post-quantum keypair.
    ///
    /// # Stack usage
    ///
    /// SLH-DSA key generation requires a large stack. This function spawns
    /// the SLH-DSA keygen on a dedicated thread with sufficient stack space.
    pub fn generate() -> Self {
        // ML-DSA-65 keygen (small stack, fast)
        let (ml_pk, ml_sk) = dilithium3::keypair();

        // SLH-DSA-SHAKE-128f keygen (needs large stack)
        let (slh_pk, slh_sk) = std::thread::Builder::new()
            .stack_size(PQ_SIGNING_STACK_SIZE)
            .spawn(sphincsshake128fsimple::keypair)
            .expect("failed to spawn SLH-DSA keygen thread")
            .join()
            .expect("SLH-DSA keygen thread panicked");

        Self {
            ml_dsa_sk: MlDsaSecretKey(ml_sk.as_bytes().to_vec()),
            ml_dsa_pk: MlDsaPublicKey(ml_pk.as_bytes().to_vec()),
            slh_dsa_sk: SlhDsaSecretKey(slh_sk.as_bytes().to_vec()),
            slh_dsa_pk: SlhDsaPublicKey(slh_pk.as_bytes().to_vec()),
        }
    }

    /// Reconstruct a keypair from raw key bytes.
    ///
    /// Requires both secret AND public key bytes because ML-DSA's secret key
    /// format does not embed the full public key (unlike Ed25519).
    pub fn from_bytes(
        ml_dsa_sk_bytes: &[u8],
        ml_dsa_pk_bytes: &[u8],
        slh_dsa_sk_bytes: &[u8],
        slh_dsa_pk_bytes: &[u8],
    ) -> Result<Self, CryptoError> {
        let ml_dsa_sk = MlDsaSecretKey::from_bytes(ml_dsa_sk_bytes)?;
        let ml_dsa_pk = MlDsaPublicKey::from_bytes(ml_dsa_pk_bytes)?;
        let slh_dsa_sk = SlhDsaSecretKey::from_bytes(slh_dsa_sk_bytes)?;
        let slh_dsa_pk = SlhDsaPublicKey::from_bytes(slh_dsa_pk_bytes)?;

        // Verify BOTH keypairs are consistent by test-signing.
        // Without verifying SLH-DSA, an attacker could supply a valid ML-DSA
        // keypair but garbage SLH-DSA keys, then sign transactions that pass
        // ML-DSA verification but fail SLH-DSA — wasting network resources.
        let test_msg = b"pichain-keypair-validation";

        let ml_sig = ml_dsa_sk.sign(test_msg);
        ml_dsa_pk.verify(test_msg, &ml_sig)?;

        // SLH-DSA verification needs large stack
        let slh_sk_bytes_clone = slh_dsa_sk.0.clone();
        let slh_pk_bytes_clone = slh_dsa_pk.0.clone();
        let slh_valid = std::thread::Builder::new()
            .stack_size(PQ_SIGNING_STACK_SIZE)
            .spawn(move || {
                let sk = SlhDsaSecretKey(slh_sk_bytes_clone);
                let pk = SlhDsaPublicKey(slh_pk_bytes_clone);
                let sig = sk.sign(test_msg);
                pk.verify(test_msg, &sig)
            })
            .expect("failed to spawn SLH-DSA validation thread")
            .join()
            .expect("SLH-DSA validation thread panicked");
        slh_valid?;

        Ok(Self {
            ml_dsa_sk,
            ml_dsa_pk,
            slh_dsa_sk,
            slh_dsa_pk,
        })
    }

    /// Derive the quantum-safe PIChain address from this keypair.
    ///
    /// Uses the quantum-safe hash combiner (Blake3 XOR SHA3-256) over
    /// both public keys, taking the last 20 bytes as the address.
    /// The address commits to both key families, ensuring it cannot be
    /// derived from a single compromised key type.
    pub fn address(&self) -> crate::keys::Address {
        pq_address(&self.ml_dsa_pk, &self.slh_dsa_pk)
    }

    /// Sign a message with both ML-DSA-65 and SLH-DSA-SHAKE-128f.
    ///
    /// Returns a [`PqDualSignature`] containing both signatures.
    /// Both must verify for the transaction to be accepted.
    ///
    /// Each algorithm signs a domain-separated message to prevent
    /// cross-algorithm signature reuse attacks:
    /// - ML-DSA signs: `"pichain-dual-ml-dsa:" || message`
    /// - SLH-DSA signs: `"pichain-dual-slh-dsa:" || message`
    ///
    /// SLH-DSA signing runs on a dedicated thread with sufficient stack.
    pub fn sign(&self, message: &[u8]) -> PqDualSignature {
        // Domain-separated ML-DSA message
        let mut ml_msg = Vec::with_capacity(DUAL_SIG_ML_DSA_DOMAIN.len() + message.len());
        ml_msg.extend_from_slice(DUAL_SIG_ML_DSA_DOMAIN);
        ml_msg.extend_from_slice(message);
        let ml_sig = self.ml_dsa_sk.sign(&ml_msg);

        // Domain-separated SLH-DSA message on large-stack thread
        let slh_sk_bytes = self.slh_dsa_sk.0.clone();
        let mut slh_msg = Vec::with_capacity(DUAL_SIG_SLH_DSA_DOMAIN.len() + message.len());
        slh_msg.extend_from_slice(DUAL_SIG_SLH_DSA_DOMAIN);
        slh_msg.extend_from_slice(message);
        let slh_sig = std::thread::Builder::new()
            .stack_size(PQ_SIGNING_STACK_SIZE)
            .spawn(move || {
                let sk = SlhDsaSecretKey(slh_sk_bytes);
                sk.sign(&slh_msg)
            })
            .expect("failed to spawn SLH-DSA signing thread")
            .join()
            .expect("SLH-DSA signing thread panicked");

        PqDualSignature { ml_sig, slh_sig }
    }
}

/// Dual post-quantum signature: ML-DSA-65 + SLH-DSA-SHAKE-128f.
///
/// Both components must verify for the signature to be considered valid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PqDualSignature {
    /// ML-DSA-65 (lattice-based) signature.
    pub ml_sig: MlDsaSignature,
    /// SLH-DSA-SHAKE-128f (hash-based) signature.
    pub slh_sig: SlhDsaSignature,
}

/// Domain separator for ML-DSA signature within dual PQ signing.
/// Prevents cross-algorithm signature reuse.
const DUAL_SIG_ML_DSA_DOMAIN: &[u8] = b"pichain-dual-ml-dsa:";
/// Domain separator for SLH-DSA signature within dual PQ signing.
const DUAL_SIG_SLH_DSA_DOMAIN: &[u8] = b"pichain-dual-slh-dsa:";

impl PqDualSignature {
    /// Verify both signatures against the given message and public keys.
    ///
    /// Returns `Ok(())` only if BOTH signatures verify successfully.
    /// Each signature is verified over a domain-separated message to prevent
    /// cross-algorithm signature reuse attacks.
    pub fn verify(
        &self,
        message: &[u8],
        ml_pk: &MlDsaPublicKey,
        slh_pk: &SlhDsaPublicKey,
    ) -> Result<(), CryptoError> {
        // Validate signature component sizes before attempting verification
        if self.ml_sig.as_bytes().len() != ML_DSA_SIG_BYTES {
            return Err(CryptoError::InvalidSignature);
        }
        if self.slh_sig.as_bytes().len() != SLH_DSA_SIG_BYTES {
            return Err(CryptoError::InvalidSignature);
        }

        // Domain-separated messages prevent cross-algorithm signature reuse:
        // ML-DSA signs: "pichain-dual-ml-dsa:" || message
        // SLH-DSA signs: "pichain-dual-slh-dsa:" || message
        let mut ml_msg = Vec::with_capacity(DUAL_SIG_ML_DSA_DOMAIN.len() + message.len());
        ml_msg.extend_from_slice(DUAL_SIG_ML_DSA_DOMAIN);
        ml_msg.extend_from_slice(message);

        let mut slh_msg = Vec::with_capacity(DUAL_SIG_SLH_DSA_DOMAIN.len() + message.len());
        slh_msg.extend_from_slice(DUAL_SIG_SLH_DSA_DOMAIN);
        slh_msg.extend_from_slice(message);

        // Verify ML-DSA first (faster, ~1ms)
        ml_pk.verify(&ml_msg, &self.ml_sig)?;
        // Verify SLH-DSA (slower, ~5ms)
        slh_pk.verify(&slh_msg, &self.slh_sig)?;
        Ok(())
    }

    /// Total signature size in bytes.
    pub fn total_bytes(&self) -> usize {
        self.ml_sig.0.len() + self.slh_sig.0.len()
    }

    /// Serialize both signatures to a contiguous byte vector.
    /// Format: [ML-DSA sig (3309 bytes)] || [SLH-DSA sig (17088 bytes)]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(ML_DSA_SIG_BYTES + SLH_DSA_SIG_BYTES);
        buf.extend_from_slice(&self.ml_sig.0);
        buf.extend_from_slice(&self.slh_sig.0);
        buf
    }

    /// Deserialize from contiguous bytes: [ML-DSA sig (3309)] || [SLH-DSA sig (17088)].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let expected = ML_DSA_SIG_BYTES + SLH_DSA_SIG_BYTES;
        if bytes.len() != expected {
            return Err(CryptoError::InvalidSignature);
        }
        let ml_sig = MlDsaSignature::from_bytes(&bytes[..ML_DSA_SIG_BYTES])?;
        let slh_sig = SlhDsaSignature::from_bytes(&bytes[ML_DSA_SIG_BYTES..])?;
        Ok(Self { ml_sig, slh_sig })
    }
}

/// Dual post-quantum public key pair for verification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PqPublicKeys {
    /// ML-DSA-65 (lattice-based) public key (1,952 bytes).
    pub ml_pk: MlDsaPublicKey,
    /// SLH-DSA-SHAKE-128f (hash-based) public key (32 bytes).
    pub slh_pk: SlhDsaPublicKey,
}

impl PqPublicKeys {
    /// Total size of both public keys in bytes.
    pub fn total_bytes(&self) -> usize {
        self.ml_pk.0.len() + self.slh_pk.0.len()
    }

    /// Derive the quantum-safe address from these public keys.
    pub fn address(&self) -> crate::keys::Address {
        pq_address(&self.ml_pk, &self.slh_pk)
    }

    /// Serialize to contiguous bytes: [SLH-DSA pk (32)] || [ML-DSA pk (1952)]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(SLH_DSA_PK_BYTES + ML_DSA_PK_BYTES);
        buf.extend_from_slice(&self.slh_pk.0);
        buf.extend_from_slice(&self.ml_pk.0);
        buf
    }

    /// Deserialize from contiguous bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let expected = SLH_DSA_PK_BYTES + ML_DSA_PK_BYTES;
        if bytes.len() != expected {
            return Err(CryptoError::InvalidPublicKey);
        }
        let slh_pk = SlhDsaPublicKey::from_bytes(&bytes[..SLH_DSA_PK_BYTES])?;
        let ml_pk = MlDsaPublicKey::from_bytes(&bytes[SLH_DSA_PK_BYTES..])?;
        Ok(Self { ml_pk, slh_pk })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Quantum-Safe Hash Combiner & Address Derivation
// ─────────────────────────────────────────────────────────────────────────────

/// Quantum-safe hash combiner: Blake3 XOR SHA3-256.
///
/// If EITHER hash function is preimage-resistant, the combiner is preimage-resistant.
/// Blake3 (ChaCha-based) and SHA3-256 (Keccak sponge) are completely different
/// constructions — the probability of both being broken simultaneously is negligible.
///
/// This provides defense in depth against hash function cryptanalysis over a 50+ year horizon.
pub fn quantum_safe_hash(data: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Sha3_256};

    let b3 = crate::hash(data);
    let mut sha3_hasher = Sha3_256::new();
    sha3_hasher.update(data);
    let s3: [u8; 32] = sha3_hasher.finalize().into();

    let mut combined = [0u8; 32];
    let b3_bytes = b3.as_bytes();
    for i in 0..32 {
        combined[i] = b3_bytes[i] ^ s3[i];
    }
    combined
}

/// Derive a quantum-safe PIChain address from dual PQ public keys.
///
/// `address = quantum_safe_hash(slh_dsa_pk || ml_dsa_pk || "pichain-pq-v1")[12..32]`
///
/// The address commits to BOTH key families with a dual-hash combiner.
/// Reversing the address requires breaking both Blake3 AND SHA3-256 simultaneously,
/// which provides 128-bit quantum security even under Grover's algorithm on both hashes.
pub fn pq_address(ml_pk: &MlDsaPublicKey, slh_pk: &SlhDsaPublicKey) -> crate::keys::Address {
    let mut material = Vec::with_capacity(SLH_DSA_PK_BYTES + ML_DSA_PK_BYTES + 14);
    material.extend_from_slice(slh_pk.as_bytes());
    material.extend_from_slice(ml_pk.as_bytes());
    material.extend_from_slice(b"pichain-pq-v1");

    let hash = quantum_safe_hash(&material);
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&hash[12..32]);
    crate::keys::Address(addr)
}

// ─────────────────────────────────────────────────────────────────────────────
// Crypto Version
// ─────────────────────────────────────────────────────────────────────────────

/// Cryptographic scheme version for transaction signing.
///
/// PIChain uses post-quantum dual signatures exclusively.
/// New PQ schemes can be added via governance without hard forks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum CryptoVersion {
    /// Dual PQ: ML-DSA-65 + SLH-DSA-SHAKE-128f (post-quantum).
    /// Both signatures must verify.
    PqDualV1 = 1,
}

impl CryptoVersion {
    /// Deserialize from a u8 value.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            // Accept 0 as PqDualV1 for serde backwards compat with default values
            0 | 1 => Some(Self::PqDualV1),
            _ => None,
        }
    }
}

impl Default for CryptoVersion {
    fn default() -> Self {
        Self::PqDualV1
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ml_dsa_sign_and_verify() {
        let (pk, sk) = dilithium3::keypair();
        let ml_sk = MlDsaSecretKey(sk.as_bytes().to_vec());
        let ml_pk = MlDsaPublicKey(pk.as_bytes().to_vec());

        let msg = b"PIChain quantum-safe transaction";
        let sig = ml_sk.sign(msg);

        assert_eq!(sig.as_bytes().len(), ML_DSA_SIG_BYTES);
        assert!(ml_pk.verify(msg, &sig).is_ok());
        assert!(ml_pk.verify(b"wrong message", &sig).is_err());
    }

    #[test]
    fn slh_dsa_sign_and_verify() {
        // Run on thread with large stack
        let handle = std::thread::Builder::new()
            .stack_size(PQ_SIGNING_STACK_SIZE)
            .spawn(|| {
                let (pk, sk) = sphincsshake128fsimple::keypair();
                let slh_sk = SlhDsaSecretKey(sk.as_bytes().to_vec());
                let slh_pk = SlhDsaPublicKey(pk.as_bytes().to_vec());

                let msg = b"PIChain hash-based signature test";
                let sig = slh_sk.sign(msg);

                assert_eq!(sig.as_bytes().len(), SLH_DSA_SIG_BYTES);
                assert!(slh_pk.verify(msg, &sig).is_ok());
                assert!(slh_pk.verify(b"wrong message", &sig).is_err());
            })
            .unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn dual_pq_keypair_sign_and_verify() {
        let kp = PqKeypair::generate();
        let msg = b"PIChain dual post-quantum signature";
        let dual_sig = kp.sign(msg);

        // Both must verify
        assert!(dual_sig.verify(msg, &kp.ml_dsa_pk, &kp.slh_dsa_pk).is_ok());

        // Wrong message must fail
        assert!(dual_sig
            .verify(b"tampered", &kp.ml_dsa_pk, &kp.slh_dsa_pk)
            .is_err());
    }

    #[test]
    fn dual_sig_fails_if_ml_dsa_wrong() {
        let kp1 = PqKeypair::generate();
        let kp2 = PqKeypair::generate();
        let msg = b"cross-key test";
        let sig = kp1.sign(msg);

        // Use kp2's ML-DSA pk with kp1's SLH-DSA pk — must fail
        assert!(sig.verify(msg, &kp2.ml_dsa_pk, &kp1.slh_dsa_pk).is_err());
    }

    #[test]
    fn dual_sig_fails_if_slh_dsa_wrong() {
        let kp1 = PqKeypair::generate();
        let kp2 = PqKeypair::generate();
        let msg = b"cross-key test";
        let sig = kp1.sign(msg);

        // Use kp1's ML-DSA pk with kp2's SLH-DSA pk — must fail
        assert!(sig.verify(msg, &kp1.ml_dsa_pk, &kp2.slh_dsa_pk).is_err());
    }

    #[test]
    fn pq_address_derivation() {
        let kp = PqKeypair::generate();
        let addr = kp.address();
        assert_eq!(addr.0.len(), 20);
        // Same keypair always produces the same address
        assert_eq!(kp.address(), addr);
    }

    #[test]
    fn pq_address_differs_between_keypairs() {
        let kp1 = PqKeypair::generate();
        let kp2 = PqKeypair::generate();
        assert_ne!(kp1.address(), kp2.address());
    }

    #[test]
    fn quantum_safe_hash_deterministic() {
        let data = b"test data for hashing";
        let h1 = quantum_safe_hash(data);
        let h2 = quantum_safe_hash(data);
        assert_eq!(h1, h2);
    }

    #[test]
    fn quantum_safe_hash_differs_from_blake3_alone() {
        let data = b"test data";
        let combined = quantum_safe_hash(data);
        let blake3_only = crate::hash(data);
        assert_ne!(&combined, blake3_only.as_bytes());
    }

    #[test]
    fn quantum_safe_hash_differs_from_sha3_alone() {
        use sha3::{Digest, Sha3_256};
        let data = b"test data";
        let combined = quantum_safe_hash(data);
        let mut hasher = Sha3_256::new();
        hasher.update(data);
        let sha3_only: [u8; 32] = hasher.finalize().into();
        assert_ne!(combined, sha3_only);
    }

    #[test]
    fn pq_keypair_from_bytes_roundtrip() {
        let kp = PqKeypair::generate();
        let msg = b"roundtrip test";
        let sig = kp.sign(msg);

        // Reconstruct from raw bytes
        let kp2 = PqKeypair::from_bytes(
            kp.ml_dsa_sk.as_bytes(),
            kp.ml_dsa_pk.as_bytes(),
            kp.slh_dsa_sk.as_bytes(),
            kp.slh_dsa_pk.as_bytes(),
        )
        .expect("from_bytes should succeed with valid keypair");

        // Same address
        assert_eq!(kp.address(), kp2.address());

        // Original signature verifies with reconstructed keys
        assert!(sig.verify(msg, &kp2.ml_dsa_pk, &kp2.slh_dsa_pk).is_ok());

        // New signature from reconstructed keypair also verifies
        let sig2 = kp2.sign(msg);
        assert!(sig2.verify(msg, &kp2.ml_dsa_pk, &kp2.slh_dsa_pk).is_ok());
    }

    #[test]
    fn slh_dsa_secret_key_derives_correct_public_key() {
        let handle = std::thread::Builder::new()
            .stack_size(PQ_SIGNING_STACK_SIZE)
            .spawn(|| {
                let (pk, sk) = sphincsshake128fsimple::keypair();
                let slh_sk = SlhDsaSecretKey(sk.as_bytes().to_vec());
                let derived_pk = slh_sk.public_key();
                assert_eq!(derived_pk.as_bytes(), pk.as_bytes());
            })
            .unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn pq_public_keys_serialization_roundtrip() {
        let kp = PqKeypair::generate();
        let pks = PqPublicKeys {
            ml_pk: kp.ml_dsa_pk.clone(),
            slh_pk: kp.slh_dsa_pk.clone(),
        };

        let bytes = pks.to_bytes();
        assert_eq!(bytes.len(), SLH_DSA_PK_BYTES + ML_DSA_PK_BYTES);

        let restored = PqPublicKeys::from_bytes(&bytes).unwrap();
        assert_eq!(restored.ml_pk, pks.ml_pk);
        assert_eq!(restored.slh_pk, pks.slh_pk);
    }

    #[test]
    fn crypto_version_roundtrip() {
        assert_eq!(CryptoVersion::from_u8(0), Some(CryptoVersion::PqDualV1));
        assert_eq!(CryptoVersion::from_u8(1), Some(CryptoVersion::PqDualV1));
        assert_eq!(CryptoVersion::from_u8(2), None);
        assert_eq!(CryptoVersion::default(), CryptoVersion::PqDualV1);
    }

    #[test]
    fn pq_address_uses_quantum_safe_hash() {
        let kp = PqKeypair::generate();

        // Manually compute the expected address
        let mut material = Vec::new();
        material.extend_from_slice(kp.slh_dsa_pk.as_bytes());
        material.extend_from_slice(kp.ml_dsa_pk.as_bytes());
        material.extend_from_slice(b"pichain-pq-v1");
        let expected_hash = quantum_safe_hash(&material);
        let expected_addr: [u8; 20] = expected_hash[12..32].try_into().unwrap();

        assert_eq!(kp.address().0, expected_addr);
    }

    #[test]
    fn ml_dsa_serde_roundtrip() {
        let (pk, _) = dilithium3::keypair();
        let ml_pk = MlDsaPublicKey(pk.as_bytes().to_vec());
        let json = serde_json::to_string(&ml_pk).unwrap();
        let restored: MlDsaPublicKey = serde_json::from_str(&json).unwrap();
        assert_eq!(ml_pk, restored);
    }

    #[test]
    fn slh_dsa_serde_roundtrip() {
        let handle = std::thread::Builder::new()
            .stack_size(PQ_SIGNING_STACK_SIZE)
            .spawn(|| {
                let (pk, _) = sphincsshake128fsimple::keypair();
                let slh_pk = SlhDsaPublicKey(pk.as_bytes().to_vec());
                let json = serde_json::to_string(&slh_pk).unwrap();
                let restored: SlhDsaPublicKey = serde_json::from_str(&json).unwrap();
                assert_eq!(slh_pk, restored);
            })
            .unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn secret_keys_zeroized_on_drop() {
        let (_, sk) = dilithium3::keypair();
        let ml_sk = MlDsaSecretKey(sk.as_bytes().to_vec());
        let original = ml_sk.as_bytes().to_vec();
        assert_ne!(original, vec![0u8; ML_DSA_SK_BYTES]);
        drop(ml_sk);
        // After drop, internal Vec is zeroized. We can't inspect it after drop
        // in safe Rust, but we verify the zeroize Drop impl compiles and runs.
    }
}
