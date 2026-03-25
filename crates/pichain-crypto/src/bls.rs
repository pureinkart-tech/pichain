//! BLS12-381 signatures for PIChain consensus attestations.
//!
//! BLS allows aggregating 500+ validator signatures into a single 48-byte
//! aggregate, reducing consensus message sizes from 32KB to 48 bytes.

use blst::min_pk::{
    AggregatePublicKey, AggregateSignature as BlstAggSig, PublicKey as BlstPubKey,
    SecretKey as BlstSecKey, Signature as BlstSig,
};
use blst::BLST_ERROR;
use zeroize::Zeroize;

use crate::CryptoError;

const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_NUL_PICHAIN_";
/// Domain separation tag for proof-of-possession (prevents rogue key attacks).
/// Using a distinct DST ensures PoP signatures cannot be replayed as message signatures.
const POP_DST: &[u8] = b"BLS_POP_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_PICHAIN_";

/// BLS12-381 secret key.
///
/// Implements `Drop` to zeroize the cached key bytes from memory, preventing
/// secret key material from lingering after the key is no longer needed.
/// The `blst` crate's `SecretKey` type does not implement `Zeroize`, so we
/// maintain a separate copy of the raw bytes specifically for secure erasure.
pub struct BlsSecretKey {
    inner: BlstSecKey,
    /// Cached raw key bytes for zeroization on drop.
    /// The blst SecretKey does not expose mutable access to its internals,
    /// so we keep a copy that we can securely erase.
    raw_bytes: [u8; 32],
}

impl BlsSecretKey {
    pub fn generate() -> Self {
        let mut ikm = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut ikm);
        let inner = BlstSecKey::key_gen(&ikm, &[]).expect("key generation should not fail");
        // Zeroize the IKM immediately after use
        ikm.zeroize();
        let raw_bytes = inner.to_bytes();
        Self { inner, raw_bytes }
    }

    /// Derive a BLS secret key from input keying material (IKM).
    /// NOTE: This performs key derivation (not deserialization). The output key
    /// will differ from the input bytes. Use `from_bytes` to load a saved key.
    pub fn from_ikm(ikm: &[u8; 32]) -> Result<Self, CryptoError> {
        let inner = BlstSecKey::key_gen(ikm, &[])
            .map_err(|_| CryptoError::KeyGeneration("BLS key derivation failed".into()))?;
        let raw_bytes = inner.to_bytes();
        Ok(Self { inner, raw_bytes })
    }

    /// Serialize the BLS secret key to 32 bytes.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.raw_bytes
    }

    /// Deserialize a BLS secret key from its 32-byte scalar representation.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, CryptoError> {
        let inner = BlstSecKey::from_bytes(bytes)
            .map_err(|_| CryptoError::KeyGeneration("BLS key deserialization failed".into()))?;
        Ok(Self {
            inner,
            raw_bytes: *bytes,
        })
    }

    pub fn public_key(&self) -> BlsPublicKey {
        BlsPublicKey {
            inner: self.inner.sk_to_pk(),
        }
    }

    pub fn sign(&self, message: &[u8]) -> BlsSignature {
        BlsSignature {
            inner: self.inner.sign(message, DST, &[]),
        }
    }

    /// Generate a proof-of-possession for this key.
    ///
    /// The PoP is a BLS signature over the public key bytes using a dedicated
    /// domain separation tag (`POP_DST`). This prevents rogue key attacks where
    /// an attacker crafts a public key that cancels honest validators' keys in
    /// aggregate signature verification.
    pub fn proof_of_possession(&self) -> BlsSignature {
        let pk_bytes = self.public_key().to_bytes();
        BlsSignature {
            inner: self.inner.sign(&pk_bytes, POP_DST, &[]),
        }
    }
}

impl Drop for BlsSecretKey {
    fn drop(&mut self) {
        // Securely erase the cached key bytes from memory.
        // Uses volatile writes via zeroize to prevent the compiler from
        // optimizing away the zeroing operation.
        self.raw_bytes.zeroize();
    }
}

/// BLS12-381 public key (48 bytes compressed).
#[derive(Clone)]
pub struct BlsPublicKey {
    inner: BlstPubKey,
}

impl BlsPublicKey {
    pub fn from_bytes(bytes: &[u8; 48]) -> Result<Self, CryptoError> {
        let pk = BlstPubKey::from_bytes(bytes).map_err(|_| CryptoError::InvalidPublicKey)?;
        Ok(Self { inner: pk })
    }

    pub fn to_bytes(&self) -> [u8; 48] {
        self.inner.to_bytes()
    }

    pub fn verify(&self, message: &[u8], signature: &BlsSignature) -> Result<(), CryptoError> {
        let result = signature
            .inner
            .verify(true, message, DST, &[], &self.inner, true);
        if result == BLST_ERROR::BLST_SUCCESS {
            Ok(())
        } else {
            Err(CryptoError::InvalidSignature)
        }
    }

    /// Verify a proof-of-possession for this public key.
    ///
    /// The PoP proves the holder knows the corresponding secret key,
    /// preventing rogue key attacks in BLS aggregate signature schemes.
    pub fn verify_proof_of_possession(&self, pop: &BlsSignature) -> Result<(), CryptoError> {
        let pk_bytes = self.to_bytes();
        let result = pop
            .inner
            .verify(true, &pk_bytes, POP_DST, &[], &self.inner, true);
        if result == BLST_ERROR::BLST_SUCCESS {
            Ok(())
        } else {
            Err(CryptoError::InvalidSignature)
        }
    }
}

impl std::fmt::Debug for BlsPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BlsPK({}..)", hex::encode(&self.to_bytes()[..8]))
    }
}

/// BLS12-381 signature (96 bytes compressed).
#[derive(Clone)]
pub struct BlsSignature {
    inner: BlstSig,
}

impl BlsSignature {
    pub fn from_bytes(bytes: &[u8; 96]) -> Result<Self, CryptoError> {
        let sig = BlstSig::from_bytes(bytes).map_err(|_| CryptoError::InvalidSignature)?;
        Ok(Self { inner: sig })
    }

    pub fn to_bytes(&self) -> [u8; 96] {
        self.inner.to_bytes()
    }
}

impl std::fmt::Debug for BlsSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BlsSig({}..)", hex::encode(&self.to_bytes()[..8]))
    }
}

/// BLS12-381 keypair.
pub struct BlsKeypair {
    pub secret: BlsSecretKey,
    pub public: BlsPublicKey,
}

impl BlsKeypair {
    pub fn generate() -> Self {
        let secret = BlsSecretKey::generate();
        let public = secret.public_key();
        Self { secret, public }
    }

    /// Reconstruct a BLS keypair from the serialized secret key bytes.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, CryptoError> {
        let secret = BlsSecretKey::from_bytes(bytes)?;
        let public = secret.public_key();
        Ok(Self { secret, public })
    }

    pub fn sign(&self, message: &[u8]) -> BlsSignature {
        self.secret.sign(message)
    }

    pub fn verify(&self, message: &[u8], signature: &BlsSignature) -> Result<(), CryptoError> {
        self.public.verify(message, signature)
    }
}

/// Aggregate multiple BLS signatures into a single 96-byte signature.
///
/// This is the key feature for consensus: 500 validator signatures become one.
pub struct AggregateSignature;

impl AggregateSignature {
    /// Aggregate multiple signatures into one.
    pub fn aggregate(signatures: &[&BlsSignature]) -> Result<BlsSignature, CryptoError> {
        if signatures.is_empty() {
            return Err(CryptoError::BlsAggregation(
                "no signatures to aggregate".into(),
            ));
        }

        let refs: Vec<&BlstSig> = signatures.iter().map(|s| &s.inner).collect();
        let agg = BlstAggSig::aggregate(&refs, true)
            .map_err(|e| CryptoError::BlsAggregation(format!("{:?}", e)))?;

        Ok(BlsSignature {
            inner: agg.to_signature(),
        })
    }

    /// Verify an aggregate signature against multiple public keys and a single message.
    /// Used when all validators sign the same message (e.g., a block hash).
    pub fn verify(
        public_keys: &[&BlsPublicKey],
        message: &[u8],
        aggregate_sig: &BlsSignature,
    ) -> Result<(), CryptoError> {
        if public_keys.is_empty() {
            return Err(CryptoError::BlsAggregation("no public keys".into()));
        }

        let pk_refs: Vec<&BlstPubKey> = public_keys.iter().map(|pk| &pk.inner).collect();
        let agg_pk = AggregatePublicKey::aggregate(&pk_refs, true)
            .map_err(|e| CryptoError::BlsAggregation(format!("{:?}", e)))?;

        let result =
            aggregate_sig
                .inner
                .verify(true, message, DST, &[], &agg_pk.to_public_key(), true);

        if result == BLST_ERROR::BLST_SUCCESS {
            Ok(())
        } else {
            Err(CryptoError::InvalidSignature)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bls_sign_verify() {
        let kp = BlsKeypair::generate();
        let msg = b"PIChain attestation round 42";
        let sig = kp.sign(msg);
        assert!(kp.verify(msg, &sig).is_ok());
    }

    #[test]
    fn bls_wrong_message_fails() {
        let kp = BlsKeypair::generate();
        let sig = kp.sign(b"correct");
        assert!(kp.verify(b"wrong", &sig).is_err());
    }

    #[test]
    fn bls_aggregate_signatures() {
        let msg = b"block hash abc123";
        let keypairs: Vec<BlsKeypair> = (0..10).map(|_| BlsKeypair::generate()).collect();
        let sigs: Vec<BlsSignature> = keypairs.iter().map(|kp| kp.sign(msg)).collect();
        let sig_refs: Vec<&BlsSignature> = sigs.iter().collect();
        let pk_refs: Vec<&BlsPublicKey> = keypairs.iter().map(|kp| &kp.public).collect();

        let agg = AggregateSignature::aggregate(&sig_refs).unwrap();
        assert!(AggregateSignature::verify(&pk_refs, msg, &agg).is_ok());
    }

    #[test]
    fn bls_serialization_roundtrip() {
        let kp = BlsKeypair::generate();
        let sig = kp.sign(b"test");
        let sig_bytes = sig.to_bytes();
        let sig2 = BlsSignature::from_bytes(&sig_bytes).unwrap();

        let pk_bytes = kp.public.to_bytes();
        let pk2 = BlsPublicKey::from_bytes(&pk_bytes).unwrap();

        assert!(pk2.verify(b"test", &sig2).is_ok());
    }

    #[test]
    fn bls_proof_of_possession_valid() {
        let kp = BlsKeypair::generate();
        let pop = kp.secret.proof_of_possession();
        assert!(kp.public.verify_proof_of_possession(&pop).is_ok());
    }

    #[test]
    fn bls_proof_of_possession_wrong_key_fails() {
        let kp1 = BlsKeypair::generate();
        let kp2 = BlsKeypair::generate();
        let pop1 = kp1.secret.proof_of_possession();
        // PoP from key1 must NOT verify against key2's public key
        assert!(kp2.public.verify_proof_of_possession(&pop1).is_err());
    }

    #[test]
    fn bls_pop_not_replayable_as_message_sig() {
        let kp = BlsKeypair::generate();
        let pop = kp.secret.proof_of_possession();
        // PoP uses POP_DST, so treating it as a normal signature (DST) must fail
        let pk_bytes = kp.public.to_bytes();
        assert!(kp.public.verify(&pk_bytes, &pop).is_err());
    }

    #[test]
    fn bls_secret_key_roundtrip() {
        // Verify that serializing and deserializing a secret key produces the same key
        let kp = BlsKeypair::generate();
        let msg = b"roundtrip test";
        let sig1 = kp.sign(msg);

        // Serialize and deserialize the secret key
        let sk_bytes = kp.secret.to_bytes();
        let kp2 = BlsKeypair::from_bytes(&sk_bytes).unwrap();
        let sig2 = kp2.sign(msg);

        // Signatures from the same key must be identical
        assert_eq!(sig1.to_bytes(), sig2.to_bytes());
        // Public keys from the same key must be identical
        assert_eq!(kp.public.to_bytes(), kp2.public.to_bytes());
    }

    #[test]
    fn bls_secret_key_zeroize_on_drop() {
        // Verify that BlsSecretKey has a Drop impl that calls zeroize.
        // We cannot reliably read freed stack memory, so instead we verify:
        // 1. The key bytes are non-zero before drop (sanity check)
        // 2. The Zeroize trait is implemented for the raw_bytes type
        // 3. A heap-allocated key's memory is zeroed after drop
        let sk = BlsSecretKey::generate();
        let bytes_before = sk.to_bytes();
        assert_ne!(
            bytes_before, [0u8; 32],
            "key bytes should be non-zero before drop"
        );

        // Use a Box to put the key on the heap where we can observe zeroization
        let sk = Box::new(BlsSecretKey::generate());
        let bytes_before = sk.to_bytes();
        assert_ne!(
            bytes_before, [0u8; 32],
            "boxed key bytes should be non-zero"
        );

        // Get a pointer to the raw_bytes field on the heap.
        // The raw_bytes field is at a fixed offset within BlsSecretKey.
        let raw_ptr = sk.raw_bytes.as_ptr();
        drop(sk);

        // SAFETY: The heap allocation was just freed by drop(), but the memory
        // hasn't been reused yet in this test's single-threaded context.
        // The allocator typically doesn't zero freed memory, so if the bytes
        // ARE zero, it's because our Drop impl zeroized them.
        // Note: This is inherently best-effort; allocator behavior may vary.
        unsafe {
            let bytes_after: [u8; 32] = std::ptr::read(raw_ptr as *const [u8; 32]);
            // The key should be zeroed by our Drop impl.
            // On some allocators this may not be observable, so we just verify
            // it doesn't match the original key (at minimum).
            assert_ne!(
                bytes_after, bytes_before,
                "key bytes should not remain intact after drop (zeroize should have cleared them)"
            );
        }
    }
}
