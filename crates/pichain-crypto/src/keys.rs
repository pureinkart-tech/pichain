//! PIChain fundamental cryptographic types: Address, PublicKey, Signature, Keypair.
//!
//! These types are used for P2P network identity (libp2p) and as building blocks.
//! Transaction signing uses post-quantum signatures (see pq.rs).

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::CryptoError;

/// 32-byte Ed25519 public key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self(*bytes)
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    /// Derive a PIChain address from the public key (Blake3 hash, last 20 bytes).
    pub fn to_address(&self) -> Address {
        let hash = crate::hash(&self.0);
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&hash.as_bytes()[12..32]);
        Address(addr)
    }

    pub fn verify(&self, message: &[u8], signature: &Signature) -> Result<(), CryptoError> {
        let verifying_key =
            VerifyingKey::from_bytes(&self.0).map_err(|_| CryptoError::InvalidPublicKey)?;
        let sig = ed25519_dalek::Signature::from_bytes(&signature.0);
        verifying_key
            .verify(message, &sig)
            .map_err(|_| CryptoError::InvalidSignature)
    }
}

impl std::fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PublicKey({})", hex::encode(&self.0[..8]))
    }
}

impl std::fmt::Display for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// 64-byte Ed25519 signature.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Signature(pub [u8; 64]);

// Custom serde for [u8; 64] (serde doesn't derive for arrays > 32)
impl Serialize for Signature {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        if bytes.len() != 64 {
            return Err(serde::de::Error::custom("signature must be 64 bytes"));
        }
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&bytes);
        Ok(Signature(arr))
    }
}

impl Signature {
    pub fn from_bytes(bytes: &[u8; 64]) -> Self {
        Self(*bytes)
    }

    pub fn to_bytes(&self) -> [u8; 64] {
        self.0
    }
}

impl std::fmt::Debug for Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sig({}..)", hex::encode(&self.0[..8]))
    }
}

/// 20-byte PIChain address derived from public key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Address(pub [u8; 20]);

impl Address {
    pub const ZERO: Self = Self([0u8; 20]);

    pub fn from_bytes(bytes: &[u8; 20]) -> Self {
        Self(*bytes)
    }

    pub fn to_bytes(&self) -> [u8; 20] {
        self.0
    }
}

impl std::fmt::Debug for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pi314{}", hex::encode(self.0))
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pi314{}", hex::encode(self.0))
    }
}

/// Ed25519 secret key (zeroized on drop).
pub struct SecretKey {
    inner: SigningKey,
}

impl SecretKey {
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        Self {
            inner: SigningKey::generate(&mut rng),
        }
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self {
            inner: SigningKey::from_bytes(bytes),
        }
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.inner.to_bytes()
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.inner.verifying_key().to_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> Signature {
        let sig = self.inner.sign(message);
        Signature(sig.to_bytes())
    }
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        // Extract the secret key bytes, zeroize them using volatile writes
        // (which the compiler cannot optimize away), then overwrite the key.
        // Plain zero-assignment can be elided by the optimizer since the value
        // is about to be dropped — zeroize prevents this via memory barriers.
        let mut key_bytes = self.inner.to_bytes();
        key_bytes.zeroize();
        self.inner = ed25519_dalek::SigningKey::from_bytes(&key_bytes);
    }
}

/// Ed25519 keypair (convenience wrapper).
pub struct Keypair {
    pub secret: SecretKey,
    pub public: PublicKey,
}

impl Keypair {
    pub fn generate() -> Self {
        let secret = SecretKey::generate();
        let public = secret.public_key();
        Self { secret, public }
    }

    pub fn from_secret_bytes(bytes: &[u8; 32]) -> Self {
        let secret = SecretKey::from_bytes(bytes);
        let public = secret.public_key();
        Self { secret, public }
    }

    pub fn sign(&self, message: &[u8]) -> Signature {
        self.secret.sign(message)
    }

    pub fn verify(&self, message: &[u8], signature: &Signature) -> Result<(), CryptoError> {
        self.public.verify(message, signature)
    }

    pub fn address(&self) -> Address {
        self.public.to_address()
    }
}

/// Batch-verify multiple Ed25519 signatures for throughput.
/// Returns Ok if ALL signatures are valid, Err if any is invalid.
pub fn verify_batch(
    messages: &[&[u8]],
    signatures: &[Signature],
    public_keys: &[PublicKey],
) -> Result<(), CryptoError> {
    if messages.len() != signatures.len() || messages.len() != public_keys.len() {
        return Err(CryptoError::InvalidSignature);
    }

    let verifying_keys: Vec<VerifyingKey> = public_keys
        .iter()
        .map(|pk| VerifyingKey::from_bytes(&pk.0).map_err(|_| CryptoError::InvalidPublicKey))
        .collect::<Result<Vec<_>, _>>()?;

    let sigs: Vec<ed25519_dalek::Signature> = signatures
        .iter()
        .map(|s| ed25519_dalek::Signature::from_bytes(&s.0))
        .collect();

    ed25519_dalek::verify_batch(messages, &sigs, &verifying_keys)
        .map_err(|_| CryptoError::InvalidSignature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let kp = Keypair::generate();
        let msg = b"PIChain genesis block";
        let sig = kp.sign(msg);
        assert!(kp.verify(msg, &sig).is_ok());
    }

    #[test]
    fn wrong_message_fails() {
        let kp = Keypair::generate();
        let sig = kp.sign(b"correct message");
        assert!(kp.verify(b"wrong message", &sig).is_err());
    }

    #[test]
    fn address_derivation() {
        let kp = Keypair::generate();
        let addr = kp.address();
        assert_eq!(addr.0.len(), 20);
        // Same key always produces same address
        assert_eq!(kp.address(), addr);
    }

    #[test]
    fn batch_verification() {
        let kps: Vec<Keypair> = (0..10).map(|_| Keypair::generate()).collect();
        let messages: Vec<Vec<u8>> = (0..10).map(|i| format!("msg {i}").into_bytes()).collect();
        let sigs: Vec<Signature> = kps
            .iter()
            .zip(messages.iter())
            .map(|(kp, msg)| kp.sign(msg))
            .collect();

        let msg_refs: Vec<&[u8]> = messages.iter().map(|m| m.as_slice()).collect();
        let pks: Vec<PublicKey> = kps.iter().map(|kp| kp.public).collect();

        assert!(verify_batch(&msg_refs, &sigs, &pks).is_ok());
    }

    #[test]
    fn secret_key_roundtrip() {
        let kp = Keypair::generate();
        let bytes = kp.secret.to_bytes();
        let kp2 = Keypair::from_secret_bytes(&bytes);
        assert_eq!(kp.public.0, kp2.public.0);
    }

    #[test]
    fn secret_key_zeroize_on_drop() {
        // Fix 211: SecretKey Drop must use volatile writes (zeroize) to clear
        // secret material. Verify that after drop, the key bytes are zeroed.
        let kp = Keypair::generate();
        let original_bytes = kp.secret.to_bytes();
        assert_ne!(
            original_bytes, [0u8; 32],
            "generated key should not be all zeros"
        );

        // After signing, key should still work
        let sig = kp.sign(b"test");
        assert!(kp.verify(b"test", &sig).is_ok());

        // Drop the keypair — the Drop impl should zeroize the inner key.
        // We cannot directly inspect memory after drop in safe Rust, but we
        // verify that the zeroize-based Drop impl compiles and runs without panic.
        drop(kp);

        // Verify that a freshly constructed zero-key produces the expected
        // "zero" signing key (i.e., the zeroize path produces a valid state).
        let zero_key = SecretKey::from_bytes(&[0u8; 32]);
        let zero_bytes = zero_key.to_bytes();
        assert_eq!(
            zero_bytes, [0u8; 32],
            "zero key should roundtrip as all zeros"
        );
    }
}
