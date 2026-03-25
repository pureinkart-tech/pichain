//! Post-Quantum Wallet for PIChain.
//!
//! Provides wallet generation, serialization, and import/export for
//! dual post-quantum keypairs (ML-DSA-65 + SLH-DSA-SHAKE-128f).
//!
//! # Architecture
//!
//! Unlike BIP-32 HD wallets that derive child keys from a master seed using
//! EC point multiplication (quantum-vulnerable), PIChain PQ wallets generate
//! keypairs directly using OS randomness and store the full key material.
//!
//! The wallet export format includes all key bytes needed for restoration.
//! The export is encrypted at rest using the user's passphrase (TODO: future work).
//!
//! # Security
//!
//! - Key generation: pqcrypto uses OS CSPRNG (getrandom) — quantum-safe entropy source
//! - Secret keys: zeroized on drop via the `zeroize` crate
//! - Address derivation: quantum-safe hash combiner (Blake3 XOR SHA3-256)

use crate::pq::PqKeypair;
use crate::CryptoError;

use serde::{Deserialize, Serialize};

/// Generate a new PQ wallet and return the keypair + export structure.
///
/// The export should be saved to a file (e.g., `wallet.json`) for backup.
///
/// # Stack usage
///
/// SLH-DSA key generation requires large stack. This function handles it internally.
pub fn generate_pq_wallet() -> (PqKeypair, PqWalletExport) {
    let kp = PqKeypair::generate();
    let export = PqWalletExport {
        version: 1,
        crypto_version: 1,
        address: format!("{}", kp.address()),
        ml_dsa_secret_key: hex::encode(kp.ml_dsa_sk.as_bytes()),
        ml_dsa_public_key: hex::encode(kp.ml_dsa_pk.as_bytes()),
        slh_dsa_secret_key: hex::encode(kp.slh_dsa_sk.as_bytes()),
        slh_dsa_public_key: hex::encode(kp.slh_dsa_pk.as_bytes()),
    };
    (kp, export)
}

/// Restore a PQ keypair from an exported wallet file.
pub fn restore_pq_wallet(export: &PqWalletExport) -> Result<PqKeypair, CryptoError> {
    if export.version != 1 || export.crypto_version != 1 {
        return Err(CryptoError::Serialization(format!(
            "unsupported wallet version: v{} crypto_v{}",
            export.version, export.crypto_version
        )));
    }

    // SECURITY: Validate hex string lengths BEFORE decoding to prevent
    // acceptance of truncated key material.
    use crate::pq::{ML_DSA_PK_BYTES, ML_DSA_SK_BYTES, SLH_DSA_PK_BYTES, SLH_DSA_SK_BYTES};
    let expected_lengths = [
        (
            "ml_dsa_secret_key",
            &export.ml_dsa_secret_key,
            ML_DSA_SK_BYTES * 2,
        ),
        (
            "ml_dsa_public_key",
            &export.ml_dsa_public_key,
            ML_DSA_PK_BYTES * 2,
        ),
        (
            "slh_dsa_secret_key",
            &export.slh_dsa_secret_key,
            SLH_DSA_SK_BYTES * 2,
        ),
        (
            "slh_dsa_public_key",
            &export.slh_dsa_public_key,
            SLH_DSA_PK_BYTES * 2,
        ),
    ];
    for (name, hex_str, expected_hex_len) in &expected_lengths {
        if hex_str.len() != *expected_hex_len {
            return Err(CryptoError::Serialization(format!(
                "{name} has incorrect hex length: {} (expected {expected_hex_len})",
                hex_str.len()
            )));
        }
    }

    let ml_sk_bytes = hex::decode(&export.ml_dsa_secret_key)
        .map_err(|e| CryptoError::Serialization(format!("ml_dsa_secret_key: {e}")))?;
    let ml_pk_bytes = hex::decode(&export.ml_dsa_public_key)
        .map_err(|e| CryptoError::Serialization(format!("ml_dsa_public_key: {e}")))?;
    let slh_sk_bytes = hex::decode(&export.slh_dsa_secret_key)
        .map_err(|e| CryptoError::Serialization(format!("slh_dsa_secret_key: {e}")))?;
    let slh_pk_bytes = hex::decode(&export.slh_dsa_public_key)
        .map_err(|e| CryptoError::Serialization(format!("slh_dsa_public_key: {e}")))?;

    PqKeypair::from_bytes(&ml_sk_bytes, &ml_pk_bytes, &slh_sk_bytes, &slh_pk_bytes)
}

/// Verify that a wallet export is self-consistent (keys match, address correct).
pub fn verify_pq_wallet(export: &PqWalletExport) -> Result<(), CryptoError> {
    let kp = restore_pq_wallet(export)?;

    // Verify address matches
    let expected_addr = format!("{}", kp.address());
    if expected_addr != export.address {
        return Err(CryptoError::InvalidPublicKey);
    }

    // Verify signing works
    let test_msg = b"pichain-wallet-verify";
    let sig = kp.sign(test_msg);
    sig.verify(test_msg, &kp.ml_dsa_pk, &kp.slh_dsa_pk)?;

    Ok(())
}

/// JSON-serializable wallet export format.
///
/// Contains all key material needed to restore the wallet.
/// This should be stored securely (encrypted file, hardware wallet, etc.).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PqWalletExport {
    /// Wallet format version (currently 1).
    pub version: u32,
    /// Crypto version (1 = ML-DSA-65 + SLH-DSA-SHAKE-128f).
    pub crypto_version: u32,
    /// Primary address (hex, Pi314 prefix format).
    pub address: String,
    /// ML-DSA-65 secret key (hex, 4032 bytes = 8064 hex chars).
    pub ml_dsa_secret_key: String,
    /// ML-DSA-65 public key (hex, 1952 bytes = 3904 hex chars).
    pub ml_dsa_public_key: String,
    /// SLH-DSA-SHAKE-128f secret key (hex, 64 bytes = 128 hex chars).
    pub slh_dsa_secret_key: String,
    /// SLH-DSA-SHAKE-128f public key (hex, 32 bytes = 64 hex chars).
    pub slh_dsa_public_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_and_restore_wallet() {
        let (kp, export) = generate_pq_wallet();
        let restored = restore_pq_wallet(&export).unwrap();

        // Same address
        assert_eq!(kp.address(), restored.address());

        // Restored keypair can sign and verify
        let msg = b"restored wallet test";
        let sig = restored.sign(msg);
        assert!(sig
            .verify(msg, &restored.ml_dsa_pk, &restored.slh_dsa_pk)
            .is_ok());
    }

    #[test]
    fn wallet_json_roundtrip() {
        let (_, export) = generate_pq_wallet();
        let json = serde_json::to_string_pretty(&export).unwrap();
        let restored_export: PqWalletExport = serde_json::from_str(&json).unwrap();
        let restored = restore_pq_wallet(&restored_export).unwrap();

        // Verify address matches
        assert_eq!(export.address, format!("{}", restored.address()));
    }

    #[test]
    fn verify_wallet_valid() {
        let (_, export) = generate_pq_wallet();
        assert!(verify_pq_wallet(&export).is_ok());
    }

    #[test]
    fn verify_wallet_rejects_tampered_key() {
        let (_, mut export) = generate_pq_wallet();
        // Tamper with the ML-DSA public key
        let mut pk_bytes = hex::decode(&export.ml_dsa_public_key).unwrap();
        pk_bytes[0] ^= 0xFF;
        export.ml_dsa_public_key = hex::encode(&pk_bytes);

        // Restoration should fail (sk/pk mismatch detected by sign+verify)
        assert!(restore_pq_wallet(&export).is_err());
    }

    #[test]
    fn verify_wallet_rejects_wrong_version() {
        let (_, mut export) = generate_pq_wallet();
        export.version = 99;
        assert!(restore_pq_wallet(&export).is_err());
    }

    #[test]
    fn wallet_export_has_correct_sizes() {
        let (_, export) = generate_pq_wallet();
        assert_eq!(hex::decode(&export.ml_dsa_secret_key).unwrap().len(), 4032);
        assert_eq!(hex::decode(&export.ml_dsa_public_key).unwrap().len(), 1952);
        assert_eq!(hex::decode(&export.slh_dsa_secret_key).unwrap().len(), 64);
        assert_eq!(hex::decode(&export.slh_dsa_public_key).unwrap().len(), 32);
    }
}
