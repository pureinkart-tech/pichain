//! PIChain PQ Wallet — Post-Quantum only (ML-DSA-65 + SLH-DSA-SHAKE-128f).
//!
//! All wallets are encrypted with AES-256-GCM using Argon2-derived keys.
//! No legacy crypto paths exist.

use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------- File format ----------

/// On-disk wallet file format (version 3 = encrypted PQ).
#[derive(Serialize, Deserialize)]
pub struct WalletFile {
    pub version: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Legacy field — ignored, kept only so old wallet files can be deserialized
    /// without error (they'll fail at the PQ key validation step instead).
    #[serde(default, skip_serializing)]
    pub secret_key: Option<String>,
}

impl WalletFile {
    pub fn is_encrypted(&self) -> bool {
        self.version >= 3 && self.encrypted_key.is_some()
    }
}

// ---------- Return types ----------

#[derive(Serialize, Clone)]
pub struct WalletInfo {
    pub address: String,
    pub path: String,
}

#[derive(Serialize, Clone)]
pub struct CreateWalletResult {
    pub address: String,
    pub path: String,
}

#[derive(Serialize, Clone)]
pub struct WalletLoadResult {
    pub address: String,
    pub path: String,
    pub encrypted: bool,
}

#[derive(Serialize, Clone)]
pub struct WalletFormatInfo {
    pub exists: bool,
    pub encrypted: bool,
}

// ---------- Paths ----------

pub fn default_wallet_dir() -> PathBuf {
    dirs_or_home().join(".pichain")
}

pub fn default_wallet_path() -> PathBuf {
    default_wallet_dir().join("wallet.json")
}

fn dirs_or_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

// ---------- Encryption primitives ----------

fn derive_encryption_key(password: &str, salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .expect("argon2 derivation failed");
    key
}

fn encrypt_secret(secret_bytes: &[u8], password: &str) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);

    let key = derive_encryption_key(password, &salt);
    let cipher = Aes256Gcm::new_from_slice(&key).expect("invalid key length");
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, secret_bytes)
        .expect("encryption failed");

    (salt.to_vec(), nonce_bytes.to_vec(), ciphertext)
}

fn decrypt_secret(
    salt: &[u8],
    nonce_bytes: &[u8],
    ciphertext: &[u8],
    password: &str,
) -> Result<Vec<u8>, String> {
    let key = derive_encryption_key(password, salt);
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|_| "Invalid encryption key".to_string())?;
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "Wrong password or corrupted wallet file".to_string())
}

// ---------- Wallet operations (PQ only) ----------

pub fn check_wallet_format(path: &str) -> Result<WalletFormatInfo, String> {
    let path = PathBuf::from(path);
    if !path.exists() {
        return Ok(WalletFormatInfo {
            exists: false,
            encrypted: false,
        });
    }
    let contents =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read wallet: {e}"))?;
    let wallet: WalletFile =
        serde_json::from_str(&contents).map_err(|e| format!("Invalid wallet JSON: {e}"))?;
    Ok(WalletFormatInfo {
        exists: true,
        encrypted: wallet.is_encrypted(),
    })
}

/// Create a new post-quantum wallet (ML-DSA-65 + SLH-DSA-SHAKE-128f).
/// The PQ key material is encrypted with AES-256-GCM using Argon2-derived key.
pub fn create_pq_wallet(save_path: &str, password: &str) -> Result<CreateWalletResult, String> {
    let path = PathBuf::from(save_path);
    if path.exists() {
        return Err(format!("Wallet already exists at '{}'.", path.display()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    let (kp, export) = pichain_crypto::generate_pq_wallet();
    let address = format!("{}", kp.address());

    // Serialize the PQ export as JSON, then encrypt the entire JSON blob
    let export_json =
        serde_json::to_vec(&export).map_err(|e| format!("Serialization error: {e}"))?;
    let (salt, nonce, ciphertext) = encrypt_secret(&export_json, password);

    let wallet = WalletFile {
        version: 3,
        secret_key: None,
        encrypted_key: Some(hex::encode(&ciphertext)),
        salt: Some(hex::encode(&salt)),
        nonce: Some(hex::encode(&nonce)),
        address: Some(address.clone()),
    };

    let json =
        serde_json::to_string_pretty(&wallet).map_err(|e| format!("Serialization error: {e}"))?;
    std::fs::write(&path, &json).map_err(|e| format!("Failed to write wallet: {e}"))?;
    restrict_file_permissions(&path);

    Ok(CreateWalletResult {
        address,
        path: path.display().to_string(),
    })
}

/// Load a PQ wallet. Returns the PQ key material for caching.
pub fn load_pq_wallet(
    path: &str,
    password: Option<&str>,
) -> Result<(pichain_crypto::pq_wallet::PqWalletExport, WalletLoadResult), String> {
    let path = PathBuf::from(path);
    let contents =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read wallet: {e}"))?;
    let wallet: WalletFile =
        serde_json::from_str(&contents).map_err(|e| format!("Invalid wallet JSON: {e}"))?;

    if wallet.is_encrypted() {
        // Encrypted PQ wallet (v3)
        let password = password.ok_or("Password required for encrypted PQ wallet")?;
        let salt = hex::decode(wallet.salt.as_deref().ok_or("Missing salt")?)
            .map_err(|e| format!("Invalid salt: {e}"))?;
        let nonce = hex::decode(wallet.nonce.as_deref().ok_or("Missing nonce")?)
            .map_err(|e| format!("Invalid nonce: {e}"))?;
        let ct = hex::decode(
            wallet
                .encrypted_key
                .as_deref()
                .ok_or("Missing encrypted_key")?,
        )
        .map_err(|e| format!("Invalid ciphertext: {e}"))?;
        let decrypted = decrypt_secret(&salt, &nonce, &ct, password)?;
        let export: pichain_crypto::pq_wallet::PqWalletExport = serde_json::from_slice(&decrypted)
            .map_err(|e| format!("Invalid PQ wallet data: {e}"))?;
        let address = export.address.clone();
        Ok((
            export,
            WalletLoadResult {
                address,
                path: path.display().to_string(),
                encrypted: true,
            },
        ))
    } else {
        // Try loading as unencrypted PQ wallet export
        let export: pichain_crypto::pq_wallet::PqWalletExport = serde_json::from_str(&contents)
            .map_err(|_| "Invalid wallet format. Create a new PQ wallet.".to_string())?;
        let address = export.address.clone();
        Ok((
            export,
            WalletLoadResult {
                address,
                path: path.display().to_string(),
                encrypted: false,
            },
        ))
    }
}

/// Restrict file permissions so only the current user can read it.
fn restrict_file_permissions(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let (Some(path_str), Ok(username)) = (path.to_str(), std::env::var("USERNAME")) {
            let grant_arg = format!("{}:F", username);
            let _ = std::process::Command::new("icacls")
                .args([path_str, "/inheritance:r", "/grant:r", &grant_arg])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .output();
        }
    }
}
