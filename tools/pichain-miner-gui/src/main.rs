// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod miner;
mod wallet;

use miner::MiningConfig;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::State;
use tokio::sync::Mutex;

// ---------- App state ----------

struct AppState {
    running: Arc<AtomicBool>,
    mining_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    wallet_path: Mutex<Option<String>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            mining_task: Mutex::new(None),
            wallet_path: Mutex::new(None),
        }
    }
}

// ---------- Tauri commands ----------

#[tauri::command]
async fn get_wallet_path() -> Result<String, String> {
    Ok(wallet::default_wallet_path().display().to_string())
}

#[tauri::command]
async fn create_wallet(save_path: String) -> Result<wallet::WalletInfo, String> {
    wallet::create_wallet(&save_path)
}

#[tauri::command]
async fn import_wallet(
    secret_key: String,
    save_path: String,
) -> Result<wallet::WalletInfo, String> {
    wallet::import_wallet(&secret_key, &save_path)
}

#[tauri::command]
async fn load_wallet(
    path: String,
    state: State<'_, AppState>,
) -> Result<wallet::WalletInfo, String> {
    let (_kp, info) = wallet::load_wallet(&path)?;
    *state.wallet_path.lock().await = Some(path);
    Ok(info)
}

#[tauri::command]
async fn check_wallet_exists(path: String) -> Result<bool, String> {
    Ok(std::path::Path::new(&path).exists())
}

#[tauri::command]
async fn get_balance(rpc_url: String, address: String) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/v1/account/{}", rpc_url, address))
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    let acct: miner::AccountResponse =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;
    Ok(serde_json::json!({
        "balance": acct.balance,
        "nonce": acct.nonce,
        "found": acct.found,
    }))
}

#[tauri::command]
async fn get_mining_status(rpc_url: String) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/v1/mining/status", rpc_url))
        .send()
        .await
        .map_err(|e| format!("Cannot reach node: {e}"))?;
    let status: miner::MiningStatus = resp.json().await.map_err(|e| format!("Parse error: {e}"))?;
    Ok(serde_json::json!({
        "frontier_position": status.frontier_position,
        "total_digits_verified": status.total_digits_verified,
        "next_position": status.next_position,
        "reward_per_digit": status.reward_per_digit,
        "difficulty_bits": status.difficulty_bits,
    }))
}

#[tauri::command]
async fn claim_faucet(rpc_url: String, address: String) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({ "address": address });
    let resp = client
        .post(format!("{}/api/v1/faucet", rpc_url))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    let result: miner::FaucetResponse =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;
    Ok(serde_json::json!({
        "success": result.success,
        "amount": result.amount,
        "error": result.error,
    }))
}

#[tauri::command]
async fn start_mining(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    rpc_url: String,
    wallet_path: String,
    profile: String,
    chain_id: u64,
) -> Result<String, String> {
    // Check not already running
    if state.running.load(Ordering::Relaxed) {
        return Err("Mining is already running".to_string());
    }

    // Load wallet
    let (keypair, _info) = wallet::load_wallet(&wallet_path)?;

    let config = MiningConfig::from_profile(rpc_url, chain_id, &profile);
    let running = state.running.clone();
    running.store(true, Ordering::Relaxed);

    let handle = tokio::spawn(async move {
        miner::mining_loop(app, config, keypair, running).await;
    });

    *state.mining_task.lock().await = Some(handle);

    Ok("Mining started".to_string())
}

#[tauri::command]
async fn stop_mining(state: State<'_, AppState>) -> Result<String, String> {
    state.running.store(false, Ordering::Relaxed);

    // Wait for mining task to finish
    let task = state.mining_task.lock().await.take();
    if let Some(handle) = task {
        let _ = handle.await;
    }

    Ok("Mining stopped".to_string())
}

#[tauri::command]
async fn is_mining(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.running.load(Ordering::Relaxed))
}

#[tauri::command]
async fn get_system_info() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "cpu_cores": num_cpus::get(),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    }))
}

// ---------- Entry point ----------

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_wallet_path,
            create_wallet,
            import_wallet,
            load_wallet,
            check_wallet_exists,
            get_balance,
            get_mining_status,
            claim_faucet,
            start_mining,
            stop_mining,
            is_mining,
            get_system_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PIChain Miner");
}
