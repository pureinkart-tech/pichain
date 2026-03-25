// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod miner;
mod wallet;

use miner::MiningConfig;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{Manager, State};
use tokio::sync::Mutex;

// ---------- App state ----------

/// Cached PQ key material for the current session.
/// All four key components are needed to reconstruct the PqKeypair.
struct CachedPqKeys {
    ml_dsa_sk: Vec<u8>,
    ml_dsa_pk: Vec<u8>,
    slh_dsa_sk: Vec<u8>,
    slh_dsa_pk: Vec<u8>,
}

struct AppState {
    running: Arc<AtomicBool>,
    mining_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    wallet_path: Mutex<Option<String>>,
    /// Cached PQ key material (replaces legacy Ed25519 cached_secret).
    cached_pq_keys: Mutex<Option<CachedPqKeys>>,
    /// Legacy Ed25519 secret — kept only for loading old encrypted wallets during migration.
    cached_secret: Mutex<Option<[u8; 32]>>,
    wallet_encrypted: Mutex<bool>,
    http_client: reqwest::Client,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            mining_task: Mutex::new(None),
            wallet_path: Mutex::new(None),
            cached_pq_keys: Mutex::new(None),
            cached_secret: Mutex::new(None),
            wallet_encrypted: Mutex::new(false),
            http_client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
        }
    }
}

// ---------- Tauri commands ----------

#[tauri::command]
async fn get_wallet_path() -> Result<String, String> {
    Ok(wallet::default_wallet_path().display().to_string())
}

#[tauri::command]
async fn check_wallet_format(path: String) -> Result<wallet::WalletFormatInfo, String> {
    wallet::check_wallet_format(&path)
}

#[tauri::command]
async fn create_wallet(
    save_path: String,
    password: String,
) -> Result<wallet::CreateWalletResult, String> {
    // All new wallets are post-quantum
    wallet::create_pq_wallet(&save_path, &password)
}

#[tauri::command]
async fn import_wallet(
    _secret_key: String,
    _save_path: String,
    _password: String,
) -> Result<wallet::WalletInfo, String> {
    // Ed25519 key import no longer supported — wallets must be post-quantum.
    // Users should create a new PQ wallet instead of importing Ed25519 keys.
    Err("Importing Ed25519 keys is no longer supported. Create a new post-quantum wallet instead.".to_string())
}

#[tauri::command]
async fn load_wallet(
    path: String,
    password: Option<String>,
    state: State<'_, AppState>,
) -> Result<wallet::WalletLoadResult, String> {
    // Load as PQ wallet (legacy Ed25519 wallets are no longer supported)
    let (export, result) = wallet::load_pq_wallet(&path, password.as_deref())?;

    // Cache PQ key material for mining
    *state.cached_pq_keys.lock().await = Some(CachedPqKeys {
        ml_dsa_sk: hex::decode(&export.ml_dsa_secret_key)
            .map_err(|e| format!("Invalid ML-DSA SK: {e}"))?,
        ml_dsa_pk: hex::decode(&export.ml_dsa_public_key)
            .map_err(|e| format!("Invalid ML-DSA PK: {e}"))?,
        slh_dsa_sk: hex::decode(&export.slh_dsa_secret_key)
            .map_err(|e| format!("Invalid SLH-DSA SK: {e}"))?,
        slh_dsa_pk: hex::decode(&export.slh_dsa_public_key)
            .map_err(|e| format!("Invalid SLH-DSA PK: {e}"))?,
    });
    *state.wallet_path.lock().await = Some(path);
    *state.wallet_encrypted.lock().await = result.encrypted;
    Ok(result)
}

#[tauri::command]
async fn check_wallet_exists(path: String) -> Result<bool, String> {
    Ok(std::path::Path::new(&path).exists())
}

#[tauri::command]
async fn migrate_wallet(
    path: String,
    password: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    wallet::migrate_wallet(&path, &password)?;
    *state.wallet_encrypted.lock().await = true;
    Ok(())
}

#[tauri::command]
async fn get_balance(
    state: State<'_, AppState>,
    rpc_url: String,
    address: String,
) -> Result<serde_json::Value, String> {
    let resp = state
        .http_client
        .get(format!("{}/api/v1/account/{}", rpc_url, address))
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Server error: HTTP {}", resp.status()));
    }
    let acct: miner::AccountResponse =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;
    Ok(serde_json::json!({
        "balance": acct.balance,
        "nonce": acct.nonce,
        "found": acct.found,
        "locked_balance": acct.locked_balance.unwrap_or(0),
    }))
}

#[tauri::command]
async fn get_mining_status(
    state: State<'_, AppState>,
    rpc_url: String,
) -> Result<serde_json::Value, String> {
    let resp = state
        .http_client
        .get(format!("{}/api/v1/mining/status", rpc_url))
        .send()
        .await
        .map_err(|e| format!("Cannot reach node: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Server error: HTTP {}", resp.status()));
    }
    let status: miner::MiningStatus =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;
    Ok(serde_json::json!({
        "frontier_position": status.frontier_position,
        "total_digits_verified": status.total_digits_verified,
        "next_position": status.next_position,
        "reward_per_digit": status.reward_per_digit,
        "difficulty_bits": status.difficulty_bits,
        "min_batch_size": status.min_batch_size,
        "max_allowed_position": status.max_allowed_position,
        "frontier_bonus_at_next": status.frontier_bonus_at_next,
        "staking_reward_pool": status.staking_reward_pool,
    }))
}

#[tauri::command]
async fn start_mining(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    rpc_url: String,
    profile: String,
    chain_id: u64,
) -> Result<String, String> {
    if state.running.load(Ordering::Acquire) {
        return Err("Mining is already running".to_string());
    }

    // Reconstruct PQ keypair from cached key material.
    let pq_keys = state
        .cached_pq_keys
        .lock()
        .await;
    let pq_keys_ref = pq_keys
        .as_ref()
        .ok_or_else(|| "No PQ wallet loaded. Please load or create a wallet first.".to_string())?;
    let pq_keypair = pichain_crypto::PqKeypair::from_bytes(
        &pq_keys_ref.ml_dsa_sk,
        &pq_keys_ref.ml_dsa_pk,
        &pq_keys_ref.slh_dsa_sk,
        &pq_keys_ref.slh_dsa_pk,
    ).map_err(|e| format!("Failed to reconstruct PQ keypair: {e}"))?;
    drop(pq_keys); // release lock before spawning

    let config = MiningConfig::from_profile(rpc_url, chain_id, &profile);
    let running = state.running.clone();
    running.store(true, Ordering::Release);

    let handle = tokio::spawn(async move {
        miner::mining_loop(app, config, pq_keypair, running).await;
    });

    *state.mining_task.lock().await = Some(handle);

    Ok("Mining started".to_string())
}

#[tauri::command]
async fn stop_mining(state: State<'_, AppState>) -> Result<String, String> {
    state.running.store(false, Ordering::Release);

    let task = state.mining_task.lock().await.take();
    if let Some(handle) = task {
        // Wait up to 5 seconds for graceful stop
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    Ok("Mining stopped".to_string())
}

#[tauri::command]
async fn is_mining(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.running.load(Ordering::Acquire))
}

#[tauri::command]
async fn export_wallet_key(
    password: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    // PQ wallets: export the address (not raw keys — PQ keys are too large for clipboard)
    let _is_encrypted = *state.wallet_encrypted.lock().await;
    let pq_keys = state.cached_pq_keys.lock().await;
    let keys = pq_keys.as_ref().ok_or_else(|| "No wallet loaded".to_string())?;

    // Return the wallet address derived from cached PQ keys
    let kp = pichain_crypto::PqKeypair::from_bytes(
        &keys.ml_dsa_sk, &keys.ml_dsa_pk, &keys.slh_dsa_sk, &keys.slh_dsa_pk,
    ).map_err(|e| format!("Key error: {e}"))?;
    let _ = password; // PQ export doesn't need re-auth (address is public)
    Ok(format!("{}", kp.address()))
}

#[tauri::command]
async fn reset_wallet(state: State<'_, AppState>) -> Result<String, String> {
    if state.running.load(Ordering::Acquire) {
        return Err("Stop mining before resetting wallet".to_string());
    }
    let mut path = state.wallet_path.lock().await;
    if let Some(p) = path.take() {
        let _ = std::fs::remove_file(&p);
    }
    *state.cached_pq_keys.lock().await = None;
    *state.cached_secret.lock().await = None;
    *state.wallet_encrypted.lock().await = false;
    Ok("Wallet reset".to_string())
}

#[tauri::command]
async fn activate_wallet(
    state: State<'_, AppState>,
    rpc_url: String,
    address: String,
) -> Result<serde_json::Value, String> {
    // Step 1: Request PoW challenge
    let challenge_body = serde_json::json!({ "address": &address });
    let challenge_resp = state
        .http_client
        .post(format!("{}/api/v1/wallet/challenge", rpc_url))
        .json(&challenge_body)
        .send()
        .await
        .map_err(|e| format!("Challenge request failed: {e}"))?;
    let challenge_data: serde_json::Value = challenge_resp
        .json()
        .await
        .map_err(|e| format!("Challenge parse error: {e}"))?;

    if !challenge_data["success"].as_bool().unwrap_or(false) {
        return Ok(challenge_data);
    }

    let challenge_hex = challenge_data["challenge"]
        .as_str()
        .ok_or("missing challenge field")?
        .to_string();
    let diff_bits = challenge_data["difficulty_bits"].as_u64().unwrap_or(20) as u32;

    // Step 2: Solve PoW in a blocking task
    let nonce = tokio::task::spawn_blocking(move || {
        let challenge_bytes = hex::decode(&challenge_hex).unwrap_or_default();
        solve_activation_pow(&challenge_bytes, diff_bits)
    })
    .await
    .map_err(|e| format!("PoW solve error: {e}"))?;

    // Step 3: Submit solution
    let activate_body = serde_json::json!({
        "address": &address,
        "challenge": challenge_data["challenge"],
        "nonce": nonce,
    });
    let resp = state
        .http_client
        .post(format!("{}/api/v1/wallet/activate", rpc_url))
        .json(&activate_body)
        .send()
        .await
        .map_err(|e| format!("Activate request failed: {e}"))?;
    let result: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Parse error: {e}"))?;
    Ok(result)
}

/// Solve PoW: find nonce where SHA-256(challenge || nonce_le) has `diff_bits` leading zero bits.
/// Must use SHA-256 to match the server-side verification (browsers use crypto.subtle).
fn solve_activation_pow(challenge: &[u8], diff_bits: u32) -> u64 {
    use sha2::{Sha256, Digest};

    let full_bytes = (diff_bits / 8) as usize;
    let rem_bits = diff_bits % 8;
    let mask = if rem_bits > 0 {
        0xFF << (8 - rem_bits)
    } else {
        0u8
    };

    for nonce in 0u64.. {
        let mut hasher = Sha256::new();
        hasher.update(challenge);
        hasher.update(nonce.to_le_bytes());
        let result = hasher.finalize();
        let h: &[u8] = result.as_ref();
        let mut ok = true;
        for byte in h.iter().take(full_bytes) {
            if *byte != 0 {
                ok = false;
                break;
            }
        }
        if ok && rem_bits > 0 && (h[full_bytes] & mask) != 0 {
            ok = false;
        }
        if ok {
            return nonce;
        }
    }
    0 // unreachable
}

#[tauri::command]
async fn get_system_info() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "cpu_cores": num_cpus::get(),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    }))
}

// ---------- Update checker ----------

#[tauri::command]
async fn check_for_updates(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let current = env!("CARGO_PKG_VERSION");
    let resp = state
        .http_client
        .get("https://api.github.com/repos/pureinkart-tech/pichain/releases/latest")
        .header("User-Agent", "PIChain-Miner")
        .send()
        .await
        .map_err(|e| format!("{e}"))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub returned HTTP {}", resp.status()));
    }

    let release: serde_json::Value = resp.json().await.map_err(|e| format!("{e}"))?;
    let tag = release["tag_name"].as_str().unwrap_or("");
    let latest = tag.trim_start_matches('v');

    Ok(serde_json::json!({
        "has_update": is_newer_version(latest, current),
        "latest_version": latest,
        "current_version": current,
        "release_url": release["html_url"].as_str().unwrap_or(""),
    }))
}

fn is_newer_version(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> (u32, u32, u32) {
        let mut parts = s.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
        (
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        )
    };
    parse(latest) > parse(current)
}

#[tauri::command]
async fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("Only HTTPS URLs are allowed".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer")
            .arg(&url)
            .spawn()
            .map_err(|e| format!("Failed to open URL: {e}"))?;
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg(&url)
            .spawn()
            .map_err(|e| format!("Failed to open URL: {e}"))?;
    }

    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open")
            .arg(&url)
            .spawn()
            .map_err(|e| format!("Failed to open URL: {e}"))?;
    }

    Ok(())
}

// ---------- Autostart ----------

#[tauri::command]
async fn toggle_autostart(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| format!("Failed to enable autostart: {e}"))?;
    } else {
        manager.disable().map_err(|e| format!("Failed to disable autostart: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
async fn get_autostart_status(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch()
        .is_enabled()
        .map_err(|e| format!("Failed to check autostart: {e}"))
}

// ---------- Entry point ----------

fn main() {
    // Crash logging: capture panics to log file
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("PANIC: {}", info);
        default_hook(info);
    }));

    tauri::Builder::default()
        // --- Plugins ---
        .plugin(
            tauri_plugin_log::Builder::new()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("pichain-miner".into()),
                    },
                ))
                .max_file_size(5_000_000) // 5 MB
                .build(),
        )
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // Focus existing window when a second instance is launched
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        // --- Setup ---
        .setup(|app| {
            // Build system tray
            let show_item =
                tauri::menu::MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
            let separator = tauri::menu::PredefinedMenuItem::separator(app)?;
            let quit_item =
                tauri::menu::MenuItem::with_id(app, "quit", "Quit PIChain Miner", true, None::<&str>)?;
            let menu =
                tauri::menu::Menu::with_items(app, &[&show_item, &separator, &quit_item])?;

            let _tray = tauri::tray::TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("PIChain Miner")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        // Graceful shutdown: signal mining to stop
                        let state = app.state::<AppState>();
                        state.running.store(false, Ordering::Release);
                        // Give the mining loop a moment to finish current work
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(window) = tray.app_handle().get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            log::info!("PIChain Miner v{} started", env!("CARGO_PKG_VERSION"));

            Ok(())
        })
        // --- State ---
        .manage(AppState::default())
        // --- Commands ---
        .invoke_handler(tauri::generate_handler![
            get_wallet_path,
            check_wallet_format,
            create_wallet,
            import_wallet,
            load_wallet,
            check_wallet_exists,
            migrate_wallet,
            get_balance,
            get_mining_status,
            start_mining,
            stop_mining,
            is_mining,
            export_wallet_key,
            reset_wallet,
            activate_wallet,
            get_system_info,
            check_for_updates,
            open_url,
            toggle_autostart,
            get_autostart_status,
        ])
        // --- Window events ---
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Minimize to system tray instead of quitting
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running PIChain Miner");
}
