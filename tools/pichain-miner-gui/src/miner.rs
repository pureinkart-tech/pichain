use pichain_crypto::Keypair;
use pichain_mining::bbp::BbpComputer;
use pichain_types::transaction::{Transaction, TransactionData, TransactionKind};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

// ---------- RPC response types ----------

#[derive(Deserialize, Debug)]
pub struct MiningStatus {
    pub frontier_position: u64,
    pub total_digits_verified: u64,
    pub next_position: u64,
    /// Maximum contiguous digits mineable at next_position before hitting
    /// an existing range. Miners should cap batch size to this value.
    #[serde(default = "default_max_batch")]
    pub max_batch_at_position: u64,
    #[serde(default)]
    pub reward_per_digit: u64,
    #[serde(default)]
    pub difficulty_bits: u32,
    #[serde(default)]
    pub difficulty_target_hex: String,
    #[serde(default)]
    pub anchor_block_hash: String,
}

fn default_max_batch() -> u64 {
    u64::MAX
}

#[derive(Deserialize, Debug)]
pub struct AccountResponse {
    pub balance: u64,
    pub nonce: u64,
    pub found: bool,
}

#[derive(Deserialize, Debug)]
pub struct SubmitResponse {
    pub tx_hash: String,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct FaucetResponse {
    pub success: bool,
    pub amount: u64,
    pub error: Option<String>,
}

// ---------- Event payloads ----------

#[derive(Serialize, Clone)]
pub struct MiningStats {
    pub digits_computed: u64,
    pub proofs_submitted: u64,
    pub proofs_confirmed: u64,
    pub balance: u64,
    pub uptime_secs: u64,
    pub digits_per_sec: f64,
    pub state: String, // idle, computing, submitting, waiting
    pub current_position: u64,
    pub reward_per_digit: u64,
}

#[derive(Serialize, Clone)]
pub struct LogEntry {
    pub timestamp: String,
    pub message: String,
    pub level: String, // info, success, error, warn
}

// ---------- Mining config ----------

#[derive(Clone)]
pub struct MiningConfig {
    pub rpc_url: String,
    pub chain_id: u64,
    pub profile: String,
    pub threads: usize,
    pub digits_per_batch: u32,
    pub concurrent_batches: u32,
}

impl MiningConfig {
    pub fn from_profile(rpc_url: String, chain_id: u64, profile: &str) -> Self {
        let total_cores = num_cpus::get();
        let (threads, digits, batches) = match profile {
            "laptop" => (2.min(total_cores), 200u32, 1u32),
            "desktop" => ((total_cores / 2).max(2), 1000, 2),
            "server" => (total_cores, 2000, 4),
            "max" => (total_cores, 5000, 8),
            _ => {
                if total_cores <= 4 {
                    (total_cores, 500, 1)
                } else if total_cores <= 16 {
                    (total_cores, 1000, 2)
                } else {
                    (total_cores, 2000, 4)
                }
            }
        };

        MiningConfig {
            rpc_url,
            chain_id,
            profile: profile.to_string(),
            threads,
            digits_per_batch: digits,
            concurrent_batches: batches,
        }
    }
}

fn now_ts() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() % 86400;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn emit_log(app: &AppHandle, message: &str, level: &str) {
    let _ = app.emit(
        "mining-log",
        LogEntry {
            timestamp: now_ts(),
            message: message.to_string(),
            level: level.to_string(),
        },
    );
}

/// Main mining loop — runs on a background thread, emits events to the frontend.
pub async fn mining_loop(
    app: AppHandle,
    config: MiningConfig,
    keypair: Keypair,
    running: Arc<AtomicBool>,
) {
    let address = keypair.address();
    let address_hex = hex::encode(address.0);

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    // Configure rayon
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(config.threads)
        .build_global();

    let mut local_nonce: Option<u64> = None;
    let mut total_digits: u64 = 0;
    let mut proofs_submitted: u64 = 0;
    let mut proofs_confirmed: u64 = 0;
    let mut loop_count: u64 = 0;
    let session_start = std::time::Instant::now();

    emit_log(
        &app,
        &format!(
            "Mining started — {} threads, {} digits/batch, profile: {}",
            config.threads, config.digits_per_batch, config.profile
        ),
        "info",
    );

    loop {
        if !running.load(Ordering::Relaxed) {
            emit_log(&app, "Mining stopped by user", "info");
            break;
        }

        // Emit current state
        let uptime = session_start.elapsed().as_secs();
        let dps = if uptime > 0 {
            total_digits as f64 / uptime as f64
        } else {
            0.0
        };

        // 1. Fetch mining status
        let _ = app.emit(
            "mining-stats",
            MiningStats {
                digits_computed: total_digits,
                proofs_submitted,
                proofs_confirmed,
                balance: 0,
                uptime_secs: uptime,
                digits_per_sec: dps,
                state: "fetching".to_string(),
                current_position: 0,
                reward_per_digit: 0,
            },
        );

        let mining_status = match client
            .get(format!("{}/api/v1/mining/status", config.rpc_url))
            .send()
            .await
        {
            Ok(resp) => match resp.json::<MiningStatus>().await {
                Ok(s) => s,
                Err(e) => {
                    emit_log(
                        &app,
                        &format!("Failed to parse mining status: {e}"),
                        "error",
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
            },
            Err(e) => {
                emit_log(
                    &app,
                    &format!("Cannot reach node: {e} — retrying in 5s"),
                    "error",
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        let position = mining_status.next_position;

        // Cap batch size to the available gap to avoid overlap with existing ranges
        let max_gap = mining_status.max_batch_at_position;
        let effective_batch_size = if max_gap < config.digits_per_batch as u64 && max_gap >= 10 {
            emit_log(
                &app,
                &format!(
                    "Capping batch to {} digits (gap size)",
                    max_gap
                ),
                "info",
            );
            max_gap as u32
        } else if max_gap < 10 {
            emit_log(&app, "Gap too small (< 10 digits), waiting...", "warn");
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            continue;
        } else {
            config.digits_per_batch
        };

        // Parse difficulty
        let difficulty_target: [u8; 32] = if !mining_status.difficulty_target_hex.is_empty() {
            let bytes = hex::decode(&mining_status.difficulty_target_hex).unwrap_or_default();
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                arr
            } else {
                pichain_mining::difficulty::INITIAL_DIFFICULTY
            }
        } else {
            pichain_mining::difficulty::INITIAL_DIFFICULTY
        };

        let anchor_block_hash: [u8; 32] = if !mining_status.anchor_block_hash.is_empty() {
            let bytes = hex::decode(&mining_status.anchor_block_hash).unwrap_or_default();
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                arr
            } else {
                [0u8; 32]
            }
        } else {
            [0u8; 32]
        };

        // 2. Get account nonce + balance
        let (nonce, balance) = if let Some(n) = local_nonce {
            (n, 0u64)
        } else {
            match client
                .get(format!("{}/api/v1/account/{}", config.rpc_url, address_hex))
                .send()
                .await
            {
                Ok(resp) => match resp.json::<AccountResponse>().await {
                    Ok(acct) if acct.found => {
                        let gas = 200_000u64 + effective_batch_size as u64 * 100;
                        let cost = gas * 1_100;
                        if acct.balance < cost {
                            emit_log(
                                &app,
                                &format!(
                                    "Low balance: {} PI — need {} for gas",
                                    acct.balance as f64 / 1e9,
                                    cost as f64 / 1e9
                                ),
                                "warn",
                            );
                            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                            continue;
                        }
                        (acct.nonce, acct.balance)
                    }
                    _ => (0, 0),
                },
                Err(_) => (0, 0),
            }
        };

        emit_log(
            &app,
            &format!(
                "Round {} — position {}, {} digits",
                loop_count + 1,
                position,
                effective_batch_size
            ),
            "info",
        );

        // 3. Compute digits
        let _ = app.emit(
            "mining-stats",
            MiningStats {
                digits_computed: total_digits,
                proofs_submitted,
                proofs_confirmed,
                balance,
                uptime_secs: session_start.elapsed().as_secs(),
                digits_per_sec: dps,
                state: "computing".to_string(),
                current_position: position,
                reward_per_digit: mining_status.reward_per_digit,
            },
        );

        // Reduce concurrent batches to 1 when gap is constrained
        let effective_batch_count = if effective_batch_size < config.digits_per_batch {
            1
        } else {
            config.concurrent_batches
        };

        let compute_start = std::time::Instant::now();

        let batches: Vec<(u64, u32, Vec<u8>)> = if effective_batch_count == 1 {
            let digits = BbpComputer::compute_hex_digits_parallel(position, effective_batch_size);
            vec![(position, effective_batch_size, digits)]
        } else {
            use rayon::prelude::*;
            (0..effective_batch_count)
                .into_par_iter()
                .map(|i| {
                    let pos = position.saturating_add((i as u64).saturating_mul(effective_batch_size as u64));
                    let digits = BbpComputer::compute_hex_digits_parallel(pos, effective_batch_size);
                    (pos, effective_batch_size, digits)
                })
                .collect()
        };

        let compute_time = compute_start.elapsed();
        let total_batch_digits = effective_batch_count as u64 * effective_batch_size as u64;

        emit_log(
            &app,
            &format!(
                "Computed {} digits in {:.1}s",
                total_batch_digits,
                compute_time.as_secs_f64()
            ),
            "info",
        );

        // 4. Find PoW and submit
        let _ = app.emit(
            "mining-stats",
            MiningStats {
                digits_computed: total_digits,
                proofs_submitted,
                proofs_confirmed,
                balance,
                uptime_secs: session_start.elapsed().as_secs(),
                digits_per_sec: dps,
                state: "submitting".to_string(),
                current_position: position,
                reward_per_digit: mining_status.reward_per_digit,
            },
        );

        let mut current_nonce = nonce;
        for (batch_pos, batch_digit_count, digits) in batches {
            if !running.load(Ordering::Relaxed) {
                break;
            }

            let max_nonce_attempts = if mining_status.difficulty_bits < 60 {
                8u64.saturating_mul(
                    1u64.checked_shl(mining_status.difficulty_bits)
                        .unwrap_or(u64::MAX),
                )
                .max(10_000_000)
            } else {
                u64::MAX
            };

            let pow_nonce = pichain_mining::find_nonce_parallel(
                &digits,
                &anchor_block_hash,
                &difficulty_target,
                max_nonce_attempts,
                &address.0,
            );

            let pn = match pow_nonce {
                Some(pn) => pn,
                None => {
                    emit_log(&app, "Failed to find PoW nonce, skipping batch", "error");
                    continue;
                }
            };

            let tx_data = TransactionData {
                sender: address,
                nonce: current_nonce,
                kind: TransactionKind::MiningProof {
                    start_position: batch_pos,
                    digit_count: batch_digit_count,
                    digits,
                    proof: vec![],
                    pow_nonce: pn,
                    anchor_block_hash: anchor_block_hash.to_vec(),
                },
                gas_limit: 200_000u64.saturating_add((batch_digit_count as u64).saturating_mul(100)),
                max_base_fee: 1_000,
                max_priority_fee: 100,
                chain_id: config.chain_id,
            };

            let signed = Transaction::sign(tx_data, &keypair);
            let tx_hex = match serde_json::to_vec(&signed) {
                Ok(v) => hex::encode(v),
                Err(e) => {
                    emit_log(&app, &format!("Serialization error: {e}"), "error");
                    continue;
                }
            };

            let submit_body = serde_json::json!({ "signed_tx_hex": tx_hex });
            proofs_submitted += 1;

            match client
                .post(format!("{}/api/v1/tx/submit", config.rpc_url))
                .json(&submit_body)
                .send()
                .await
            {
                Ok(resp) => match resp.json::<SubmitResponse>().await {
                    Ok(result) => {
                        if result.status == "pending" {
                            proofs_confirmed += 1;
                            total_digits += batch_digit_count as u64;
                            let reward =
                                (mining_status.reward_per_digit as f64 * batch_digit_count as f64) / 1e9;
                            emit_log(
                                &app,
                                &format!(
                                    "Proof submitted! tx:{} +{:.4} PI",
                                    &result.tx_hash[..12],
                                    reward
                                ),
                                "success",
                            );
                            current_nonce = current_nonce.saturating_add(1);
                        } else {
                            emit_log(
                                &app,
                                &format!(
                                    "Proof rejected: {}",
                                    result.error.as_deref().unwrap_or(&result.status)
                                ),
                                "error",
                            );
                            local_nonce = None;
                            break;
                        }
                    }
                    Err(e) => {
                        emit_log(&app, &format!("Bad response: {e}"), "error");
                        local_nonce = None;
                        break;
                    }
                },
                Err(e) => {
                    emit_log(&app, &format!("Submit failed: {e}"), "error");
                    local_nonce = None;
                    break;
                }
            }
        }

        if local_nonce.is_some() || current_nonce > nonce {
            local_nonce = Some(current_nonce);
        }

        loop_count += 1;

        // Update stats after round
        let uptime = session_start.elapsed().as_secs();
        let dps = if uptime > 0 {
            total_digits as f64 / uptime as f64
        } else {
            0.0
        };
        let _ = app.emit(
            "mining-stats",
            MiningStats {
                digits_computed: total_digits,
                proofs_submitted,
                proofs_confirmed,
                balance,
                uptime_secs: uptime,
                digits_per_sec: dps,
                state: "waiting".to_string(),
                current_position: position,
                reward_per_digit: mining_status.reward_per_digit,
            },
        );

        // Brief pause between rounds
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }

    // Final stats
    let uptime = session_start.elapsed().as_secs();
    let dps = if uptime > 0 {
        total_digits as f64 / uptime as f64
    } else {
        0.0
    };
    let _ = app.emit(
        "mining-stats",
        MiningStats {
            digits_computed: total_digits,
            proofs_submitted,
            proofs_confirmed,
            balance: 0,
            uptime_secs: uptime,
            digits_per_sec: dps,
            state: "idle".to_string(),
            current_position: 0,
            reward_per_digit: 0,
        },
    );
}
