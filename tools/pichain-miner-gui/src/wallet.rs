use pichain_crypto::Keypair;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
pub struct WalletFile {
    pub secret_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct WalletInfo {
    pub address: String,
    pub path: String,
}

pub fn default_wallet_dir() -> PathBuf {
    dirs_or_home().join(".pichain")
}

pub fn default_wallet_path() -> PathBuf {
    default_wallet_dir().join("wallet.json")
}

fn dirs_or_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn create_wallet(save_path: &str) -> Result<WalletInfo, String> {
    let path = PathBuf::from(save_path);

    if path.exists() {
        return Err(format!(
            "Wallet already exists at '{}'. Remove it first or choose a different path.",
            path.display()
        ));
    }

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    let kp = Keypair::generate();
    let address = hex::encode(kp.address().0);

    let wallet = WalletFile {
        secret_key: hex::encode(kp.secret.to_bytes()),
        address: Some(address.clone()),
    };

    let json =
        serde_json::to_string_pretty(&wallet).map_err(|e| format!("Serialization error: {e}"))?;
    std::fs::write(&path, &json).map_err(|e| format!("Failed to write wallet: {e}"))?;

    // Set restrictive permissions on unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(WalletInfo {
        address,
        path: path.display().to_string(),
    })
}

pub fn import_wallet(secret_key_hex: &str, save_path: &str) -> Result<WalletInfo, String> {
    let path = PathBuf::from(save_path);

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

    let wallet = WalletFile {
        secret_key: secret_key_hex.trim().to_string(),
        address: Some(address.clone()),
    };

    let json =
        serde_json::to_string_pretty(&wallet).map_err(|e| format!("Serialization error: {e}"))?;
    std::fs::write(&path, &json).map_err(|e| format!("Failed to write wallet: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(WalletInfo {
        address,
        path: path.display().to_string(),
    })
}

pub fn load_wallet(path: &str) -> Result<(Keypair, WalletInfo), String> {
    let path = PathBuf::from(path);
    let contents =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read wallet: {e}"))?;
    let wallet: WalletFile =
        serde_json::from_str(&contents).map_err(|e| format!("Invalid wallet JSON: {e}"))?;
    let secret_bytes =
        hex::decode(&wallet.secret_key).map_err(|e| format!("Invalid hex key: {e}"))?;

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
        WalletInfo {
            address,
            path: path.display().to_string(),
        },
    ))
}
