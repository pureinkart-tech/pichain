use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
use pichain_crypto::Keypair;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const WALLET_VERSION: u8 = 2;

// ---------- File format ----------

#[derive(Serialize, Deserialize)]
pub struct WalletFile {
    #[serde(default = "default_version")]
    pub version: u8,
    // V1 (legacy plaintext)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_key: Option<String>,
    // V2 (encrypted)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    // Common
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

fn default_version() -> u8 {
    1
}

impl WalletFile {
    pub fn is_encrypted(&self) -> bool {
        self.version >= 2 && self.encrypted_key.is_some()
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
    pub secret_key: String,
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

// ---------- Wallet operations ----------

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

pub fn create_wallet(save_path: &str, password: &str) -> Result<CreateWalletResult, String> {
    let path = PathBuf::from(save_path);

    if path.exists() {
        return Err(format!(
            "Wallet already exists at '{}'. Remove it first or choose a different path.",
            path.display()
        ));
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    let kp = Keypair::generate();
    let address = hex::encode(kp.address().0);
    let secret_bytes = kp.secret.to_bytes();
    let secret_key_hex = hex::encode(secret_bytes);

    let (salt, nonce, ciphertext) = encrypt_secret(&secret_bytes, password);

    let wallet = WalletFile {
        version: WALLET_VERSION,
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
        secret_key: secret_key_hex,
    })
}

pub fn import_wallet(
    secret_key_hex: &str,
    save_path: &str,
    password: &str,
) -> Result<WalletInfo, String> {
    let path = PathBuf::from(save_path);

    if path.exists() {
        return Err(format!(
            "Wallet already exists at '{}'. Remove it first or choose a different path.",
            path.display()
        ));
    }

    let secret_bytes =
        hex::decode(secret_key_hex.trim()).map_err(|e| format!("Invalid hex: {e}"))?;
    if secret_bytes.len() != 32 {
        return Err(format!(
            "Secret key must be 32 bytes (got {})",
            secret_bytes.len()
        ));
    }

    let mut arr = [0u8; 32];
    arr.copy_from_slice(&secret_bytes);
    let kp = Keypair::from_secret_bytes(&arr);
    let address = hex::encode(kp.address().0);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    let (salt, nonce, ciphertext) = encrypt_secret(&secret_bytes, password);

    let wallet = WalletFile {
        version: WALLET_VERSION,
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

    Ok(WalletInfo {
        address,
        path: path.display().to_string(),
    })
}

/// Load a wallet. For encrypted wallets, password is required.
/// Returns (keypair, info, is_encrypted).
pub fn load_wallet(
    path: &str,
    password: Option<&str>,
) -> Result<(Keypair, WalletLoadResult, bool), String> {
    let path = PathBuf::from(path);
    let contents =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read wallet: {e}"))?;
    let wallet: WalletFile =
        serde_json::from_str(&contents).map_err(|e| format!("Invalid wallet JSON: {e}"))?;

    let (secret_bytes, is_encrypted) = if wallet.is_encrypted() {
        let password = password.ok_or("Password required")?;
        let salt = hex::decode(wallet.salt.as_deref().ok_or("Missing salt field")?)
            .map_err(|e| format!("Invalid salt hex: {e}"))?;
        let nonce = hex::decode(wallet.nonce.as_deref().ok_or("Missing nonce field")?)
            .map_err(|e| format!("Invalid nonce hex: {e}"))?;
        let ct = hex::decode(
            wallet
                .encrypted_key
                .as_deref()
                .ok_or("Missing encrypted_key field")?,
        )
        .map_err(|e| format!("Invalid ciphertext hex: {e}"))?;
        let decrypted = decrypt_secret(&salt, &nonce, &ct, password)?;
        (decrypted, true)
    } else {
        // Legacy plaintext wallet
        let sk_hex = wallet
            .secret_key
            .as_deref()
            .ok_or("Missing secret_key field")?;
        let bytes = hex::decode(sk_hex).map_err(|e| format!("Invalid hex key: {e}"))?;
        (bytes, false)
    };

    if secret_bytes.len() != 32 {
        return Err(format!(
            "Secret key must be 32 bytes (got {})",
            secret_bytes.len()
        ));
    }

    let mut arr = [0u8; 32];
    arr.copy_from_slice(&secret_bytes);
    let kp = Keypair::from_secret_bytes(&arr);
    let address = hex::encode(kp.address().0);

    Ok((
        kp,
        WalletLoadResult {
            address,
            path: path.display().to_string(),
            encrypted: is_encrypted,
        },
        is_encrypted,
    ))
}

/// Migrate a plaintext wallet to encrypted format.
pub fn migrate_wallet(path: &str, password: &str) -> Result<(), String> {
    let path_buf = PathBuf::from(path);
    let contents =
        std::fs::read_to_string(&path_buf).map_err(|e| format!("Failed to read wallet: {e}"))?;
    let wallet: WalletFile =
        serde_json::from_str(&contents).map_err(|e| format!("Invalid wallet JSON: {e}"))?;

    if wallet.is_encrypted() {
        return Err("Wallet is already encrypted".to_string());
    }

    let sk_hex = wallet
        .secret_key
        .as_deref()
        .ok_or("Missing secret_key field")?;
    let secret_bytes = hex::decode(sk_hex).map_err(|e| format!("Invalid hex key: {e}"))?;

    let (salt, nonce, ciphertext) = encrypt_secret(&secret_bytes, password);

    let new_wallet = WalletFile {
        version: WALLET_VERSION,
        secret_key: None,
        encrypted_key: Some(hex::encode(&ciphertext)),
        salt: Some(hex::encode(&salt)),
        nonce: Some(hex::encode(&nonce)),
        address: wallet.address,
    };

    let json = serde_json::to_string_pretty(&new_wallet)
        .map_err(|e| format!("Serialization error: {e}"))?;
    std::fs::write(&path_buf, &json).map_err(|e| format!("Failed to write wallet: {e}"))?;
    restrict_file_permissions(&path_buf);

    Ok(())
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
