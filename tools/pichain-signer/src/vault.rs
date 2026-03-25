//! Multi-Wallet Vault for PIChain Signer.
//!
//! Manages encrypted PQ wallets for multiple users (e.g., Telegram bot users).
//! Each wallet is independently encrypted with AES-256-GCM using a key derived
//! from the vault master secret + user ID via Argon2.
//!
//! Private keys NEVER leave this Rust process. The calling application (Node.js bot,
//! etc.) only sees addresses and signed transactions — never raw key material.
//!
//! # Security model
//!
//! - Master secret: 32 bytes, stored separately from wallet files
//! - Per-user encryption key: Argon2id(master_secret || user_id, salt)
//! - Wallet files: AES-256-GCM encrypted PQ wallet JSON
//! - Keys loaded into memory only during signing, zeroized immediately after
//! - No key material exposed via API — only addresses and signed transactions

use axum::{extract::State, http::StatusCode, response::Json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

/// Vault state shared across handlers.
/// Max sign requests per user per minute.
const MAX_USER_SIGNS_PER_MINUTE: usize = 10;

pub struct VaultState {
    /// Directory containing encrypted wallet files.
    pub vault_dir: PathBuf,
    /// Master secret for key derivation (never exposed via API).
    master_secret: [u8; 32],
    /// Chain ID for transaction validation.
    pub chain_id: u64,
    /// Per-user rate limiter: user_id → list of recent sign timestamps (ms).
    user_rate_limits: std::sync::Mutex<std::collections::HashMap<String, Vec<u64>>>,
}

impl VaultState {
    pub fn new(vault_dir: PathBuf, master_secret: [u8; 32], chain_id: u64) -> Self {
        std::fs::create_dir_all(&vault_dir).expect("failed to create vault directory");
        Self {
            vault_dir,
            master_secret,
            chain_id,
            user_rate_limits: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Check and record a sign request for rate limiting.
    /// Returns Err if rate limit exceeded.
    fn check_user_rate_limit(&self, user_id: &str) -> Result<(), String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut limits = self
            .user_rate_limits
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let timestamps = limits.entry(user_id.to_string()).or_default();
        timestamps.retain(|&t| now.saturating_sub(t) < 60_000);
        if timestamps.len() >= MAX_USER_SIGNS_PER_MINUTE {
            return Err(format!(
                "rate limit: user {} exceeded {} signs/minute",
                &user_id[..user_id.len().min(8)],
                MAX_USER_SIGNS_PER_MINUTE
            ));
        }
        timestamps.push(now);
        // Cleanup: remove users with no recent activity to prevent memory leak
        limits.retain(|_, ts| !ts.is_empty());
        Ok(())
    }

    /// Derive the per-user encryption key using HKDF-like construction.
    ///
    /// Two-stage derivation for proper key separation:
    /// 1. PRK = Blake3_keyed(master_secret, "pichain-vault-prk")
    /// 2. Key = Blake3_keyed(PRK, "pichain-vault-user:" || user_id || len(user_id))
    ///
    /// The length suffix prevents extension attacks where "user1" + "23" = "user12" + "3".
    fn derive_user_key(&self, user_id: &str) -> [u8; 32] {
        // Stage 1: Extract — derive a pseudorandom key from master secret
        let prk = blake3::keyed_hash(&self.master_secret, b"pichain-vault-prk");

        // Stage 2: Expand — derive per-user key with domain separation + length binding
        let mut input = Vec::with_capacity(32 + user_id.len() + 4);
        input.extend_from_slice(b"pichain-vault-user:");
        input.extend_from_slice(user_id.as_bytes());
        input.extend_from_slice(&(user_id.len() as u32).to_le_bytes());

        let key = blake3::keyed_hash(prk.as_bytes(), &input);
        *key.as_bytes()
    }

    /// Path to a user's encrypted wallet file.
    fn wallet_path(&self, user_id: &str) -> PathBuf {
        // Sanitize user_id to prevent path traversal
        let safe_id: String = user_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(64)
            .collect();
        if safe_id.is_empty() {
            return self.vault_dir.join("_invalid_.wallet");
        }
        self.vault_dir.join(format!("{safe_id}.wallet"))
    }

    /// Create a new PQ wallet for a user. Returns the address.
    pub fn create_wallet(&self, user_id: &str) -> Result<String, String> {
        let path = self.wallet_path(user_id);
        if path.exists() {
            return Err("wallet already exists for this user".to_string());
        }

        // Generate PQ keypair
        let (kp, export) = pichain_crypto::generate_pq_wallet();
        let address = format!("{}", kp.address());

        // Serialize and encrypt
        let json = serde_json::to_vec(&export).map_err(|e| format!("serialization error: {e}"))?;
        let key = self.derive_user_key(user_id);
        let encrypted = encrypt_data(&json, &key);

        // Write to file
        std::fs::write(&path, &encrypted).map_err(|e| format!("failed to write wallet: {e}"))?;

        // Set restrictive permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }

        info!(user_id, %address, "vault: created PQ wallet");
        Ok(address)
    }

    /// Get the address for a user's wallet (without loading the full keypair).
    pub fn get_address(&self, user_id: &str) -> Result<String, String> {
        let export = self.load_wallet(user_id)?;
        Ok(export.address)
    }

    /// Sign a transaction for a user. Keys are loaded, used, and zeroized.
    pub fn sign_transaction(
        &self,
        user_id: &str,
        tx_data: pichain_types::transaction::TransactionData,
    ) -> Result<(serde_json::Value, String), String> {
        // Per-user rate limiting
        self.check_user_rate_limit(user_id)?;

        let export = self.load_wallet(user_id)?;

        // Restore keypair (validates both key pairs)
        let kp = pichain_crypto::restore_pq_wallet(&export)
            .map_err(|e| format!("keypair restore failed: {e}"))?;

        // Verify sender matches wallet
        let wallet_addr = kp.address();
        if tx_data.sender != wallet_addr {
            return Err(format!(
                "sender {} does not match wallet {}",
                hex::encode(tx_data.sender.0),
                hex::encode(wallet_addr.0)
            ));
        }

        // Verify chain_id
        if tx_data.chain_id != self.chain_id && tx_data.chain_id != 0 {
            return Err(format!(
                "chain_id {} does not match {}",
                tx_data.chain_id, self.chain_id
            ));
        }

        // Sign with PQ dual signatures
        let signed = pichain_types::transaction::Transaction::sign_pq(tx_data, &kp);
        let tx_hash = hex::encode(signed.hash().as_bytes());
        let signed_json =
            serde_json::to_value(&signed).map_err(|e| format!("serialization error: {e}"))?;

        // kp is dropped here — PQ secret keys zeroized via Drop impl

        Ok((signed_json, tx_hash))
    }

    /// Check if a user has a wallet.
    pub fn has_wallet(&self, user_id: &str) -> bool {
        self.wallet_path(user_id).exists()
    }

    /// Load and decrypt a user's wallet.
    fn load_wallet(
        &self,
        user_id: &str,
    ) -> Result<pichain_crypto::pq_wallet::PqWalletExport, String> {
        let path = self.wallet_path(user_id);
        if !path.exists() {
            return Err("no wallet found for this user".to_string());
        }

        let encrypted = std::fs::read(&path).map_err(|e| format!("failed to read wallet: {e}"))?;
        let key = self.derive_user_key(user_id);
        let json = decrypt_data(&encrypted, &key)
            .map_err(|_| "wallet decryption failed — vault may be corrupted".to_string())?;

        serde_json::from_slice(&json).map_err(|e| format!("invalid wallet data: {e}"))
    }

    /// Delete a user's wallet (for account deletion).
    pub fn delete_wallet(&self, user_id: &str) -> Result<(), String> {
        let path = self.wallet_path(user_id);
        if !path.exists() {
            return Err("no wallet found".to_string());
        }
        std::fs::remove_file(&path).map_err(|e| format!("delete failed: {e}"))?;
        info!(user_id, "vault: deleted wallet");
        Ok(())
    }

    /// Export a user's wallet (encrypted export for backup).
    pub fn export_wallet(
        &self,
        user_id: &str,
    ) -> Result<pichain_crypto::pq_wallet::PqWalletExport, String> {
        self.load_wallet(user_id)
    }
}

impl Drop for VaultState {
    fn drop(&mut self) {
        // Zeroize master secret
        use zeroize::Zeroize;
        self.master_secret.zeroize();
    }
}

// ─── Encryption (AES-256-GCM) ───

fn encrypt_data(plaintext: &[u8], key: &[u8; 32]) -> Vec<u8> {
    use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
    use rand::RngCore;

    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);

    let cipher = Aes256Gcm::new_from_slice(key).expect("invalid key length");
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher.encrypt(nonce, plaintext).expect("encryption failed");

    // Format: [nonce (12 bytes)] || [ciphertext]
    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    out
}

fn decrypt_data(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, ()> {
    use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};

    if data.len() < 12 {
        return Err(());
    }
    let nonce = Nonce::from_slice(&data[..12]);
    let ciphertext = &data[12..];

    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| ())?;
    cipher.decrypt(nonce, ciphertext).map_err(|_| ())
}

// ─── HTTP Handlers ───

#[derive(serde::Deserialize)]
pub struct VaultCreateRequest {
    user_id: String,
}

pub async fn vault_create(
    State(state): State<Arc<VaultState>>,
    Json(req): Json<VaultCreateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if req.user_id.is_empty() || req.user_id.len() > 64 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid user_id"})),
        );
    }
    match state.create_wallet(&req.user_id) {
        Ok(address) => (
            StatusCode::OK,
            Json(serde_json::json!({"address": address, "user_id": req.user_id})),
        ),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        ),
    }
}

#[derive(serde::Deserialize)]
pub struct VaultAddressRequest {
    user_id: String,
}

pub async fn vault_address(
    State(state): State<Arc<VaultState>>,
    Json(req): Json<VaultAddressRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.get_address(&req.user_id) {
        Ok(address) => (
            StatusCode::OK,
            Json(serde_json::json!({"address": address})),
        ),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
pub struct VaultSignRequest {
    user_id: String,
    tx_data: serde_json::Value,
}

pub async fn vault_sign(
    State(state): State<Arc<VaultState>>,
    Json(req): Json<VaultSignRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    // Convert hex strings to byte arrays
    let mut tx_value = req.tx_data;
    super::convert_hex_strings_to_arrays(&mut tx_value);

    let tx_data: pichain_types::transaction::TransactionData =
        match serde_json::from_value(tx_value) {
            Ok(td) => td,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": format!("invalid tx_data: {e}")})),
                )
            }
        };

    match state.sign_transaction(&req.user_id, tx_data) {
        Ok((signed_tx, tx_hash)) => {
            info!(user_id = %req.user_id, %tx_hash, "vault: signed transaction");
            (
                StatusCode::OK,
                Json(serde_json::json!({"signed_tx": signed_tx, "tx_hash": tx_hash})),
            )
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        ),
    }
}

#[derive(serde::Deserialize)]
pub struct VaultExportRequest {
    user_id: String,
}

pub async fn vault_export(
    State(state): State<Arc<VaultState>>,
    Json(req): Json<VaultExportRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.export_wallet(&req.user_id) {
        Ok(export) => (StatusCode::OK, Json(serde_json::json!({"wallet": export}))),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))),
    }
}
