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
    #[serde(default)]
    #[allow(dead_code)]
    pub unique_miners: u64,
    /// Minimum digits per proof at current frontier.
    #[serde(default)]
    pub min_batch_size: u32,
    /// Maximum position allowed (frontier + max_frontier_distance).
    #[serde(default)]
    pub max_allowed_position: u64,
    /// Current frontier bonus multiplier at next_position.
    #[serde(default)]
    pub frontier_bonus_at_next: String,
    /// Accumulated staking reward pool from unmined emission recycling.
    #[serde(default)]
    pub staking_reward_pool: u64,
}

/// Slot assignment response from the RPC.
#[derive(Deserialize, Debug)]
pub struct SlotResponse {
    #[allow(dead_code)]
    pub address: String,
    pub slot_index: u32,
    pub recommended_position: u64,
    pub active_miners: usize,
    #[allow(dead_code)]
    pub slot_range_size: u64,
}

fn default_max_batch() -> u64 {
    u64::MAX
}

#[derive(Deserialize, Debug)]
pub struct AccountResponse {
    pub balance: u64,
    pub nonce: u64,
    pub found: bool,
    #[serde(default)]
    pub locked_balance: Option<u64>,
}

#[derive(Deserialize, Debug)]
pub struct SubmitResponse {
    pub tx_hash: String,
    pub status: String,
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
            "low" => (2.min(total_cores), 200u32, 1u32),
            "normal" => ((total_cores / 2).max(2), 500, 1),
            "max" => (total_cores, 5000, 8),
            _ => {
                // Auto-detect optimal settings based on available cores
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
    chrono::Local::now().format("%H:%M:%S").to_string()
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

    // Build a per-session thread pool so profile changes take effect on restart
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.threads)
        .build()
        .unwrap_or_else(|_| rayon::ThreadPoolBuilder::new().build().unwrap());

    let mut local_nonce: Option<u64> = None;
    let mut local_position: Option<u64> = None;
    let mut total_digits: u64 = 0;
    let mut proofs_submitted: u64 = 0;
    let mut proofs_confirmed: u64 = 0;
    let mut loop_count: u64 = 0;
    let mut last_known_balance: u64 = 0;
    let mut consecutive_errors: u32 = 0;
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
        if !running.load(Ordering::Acquire) {
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
                balance: last_known_balance,
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
            Ok(resp) => {
                if !resp.status().is_success() {
                    consecutive_errors += 1;
                    let delay = 5u64.min(2u64.saturating_pow(consecutive_errors)).max(2);
                    emit_log(
                        &app,
                        &format!("Node returned HTTP {} — retrying in {}s", resp.status(), delay),
                        "error",
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                    continue;
                }
                match resp.json::<MiningStatus>().await {
                    Ok(s) => s,
                    Err(e) => {
                        consecutive_errors += 1;
                        let delay = 5u64.min(2u64.saturating_pow(consecutive_errors)).max(2);
                        emit_log(
                            &app,
                            &format!("Failed to parse mining status: {e} — retrying in {}s", delay),
                            "error",
                        );
                        tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                        continue;
                    }
                }
            }
            Err(e) => {
                consecutive_errors += 1;
                let delay = 5u64.min(2u64.saturating_pow(consecutive_errors)).max(2);
                emit_log(
                    &app,
                    &format!("Cannot reach node: {e} — retrying in {}s", delay),
                    "error",
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                continue;
            }
        };

        // Enforce server-reported minimum batch size (frontier-scaled)
        let effective_min_batch = if mining_status.min_batch_size > 0 {
            config.digits_per_batch.max(mining_status.min_batch_size)
        } else {
            config.digits_per_batch
        };

        // Calculate position with collision avoidance:
        // - If we have a local_position from last round, use it (prevents self-collision)
        // - Otherwise, query slot endpoint for server-assigned position
        let (position, effective_batch_size) = if let Some(local_pos) = local_position {
            let pos = local_pos.max(mining_status.next_position);
            (pos, effective_min_batch)
        } else {
            // Query slot endpoint for server-assigned position
            let slot_position = match client
                .get(format!("{}/api/v1/mining/slot/{}", config.rpc_url, hex::encode(address.0)))
                .send()
                .await
            {
                Ok(resp) => match resp.json::<SlotResponse>().await {
                    Ok(slot) => {
                        emit_log(
                            &app,
                            &format!(
                                "Assigned slot {} (position {}, {} active miners)",
                                slot.slot_index, slot.recommended_position, slot.active_miners
                            ),
                            "info",
                        );
                        Some(slot.recommended_position)
                    }
                    Err(_) => None,
                },
                Err(_) => None,
            };

            let pos = if let Some(slot_pos) = slot_position {
                slot_pos
            } else {
                // Fallback: old-style offset from address
                let stride = effective_min_batch as u64 * config.concurrent_batches as u64;
                let offset = if stride > 0 {
                    let addr_seed = u32::from_le_bytes([
                        address.0[0], address.0[1], address.0[2], address.0[3],
                    ]);
                    let slot = (addr_seed as u64) % 8;
                    slot.saturating_mul(stride)
                } else {
                    0
                };
                mining_status.next_position.saturating_add(offset)
            };

            // Enforce max_allowed_position from frontier distance limit
            let pos = if mining_status.max_allowed_position > 0 {
                pos.min(mining_status.max_allowed_position)
            } else {
                pos
            };

            // Cap batch size to available gap
            let max_gap = mining_status.max_batch_at_position;
            let min_required = mining_status.min_batch_size.max(10) as u64;
            let effective = if max_gap < min_required {
                // Gap too small for minimum proof size
                if pos != mining_status.next_position && mining_status.max_batch_at_position >= min_required {
                    emit_log(&app, &format!("Jumping to server position {} (local gap too small)", mining_status.next_position), "info");
                    local_position = Some(mining_status.next_position);
                    continue;
                }
                emit_log(&app, &format!("Gap too small ({} < {} min), waiting...", max_gap, min_required), "warn");
                local_position = None;
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            } else if max_gap < effective_min_batch as u64 {
                emit_log(&app, &format!("Capping batch to {} digits (gap size)", max_gap), "info");
                max_gap as u32
            } else {
                effective_min_batch
            };
            (pos, effective)
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
        // Re-query balance periodically (every 10 rounds) even when nonce is cached
        let (nonce, balance) = if let Some(n) = local_nonce {
            // Periodic balance check (non-blocking, doesn't reset nonce)
            let bal = if loop_count % 10 == 0 {
                match client.get(format!("{}/api/v1/account/{}", config.rpc_url, address_hex)).send().await {
                    Ok(resp) => resp.json::<AccountResponse>().await.ok().map(|a| a.balance).unwrap_or(0),
                    Err(_) => 0,
                }
            } else { 0 };
            (n, bal)
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
        if balance > 0 { last_known_balance = balance; }

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
            let digits = pool.install(|| BbpComputer::compute_hex_digits_parallel(position, effective_batch_size));
            vec![(position, effective_batch_size, digits)]
        } else {
            use rayon::prelude::*;
            pool.install(|| {
                (0..effective_batch_count)
                    .into_par_iter()
                    .map(|i| {
                        let pos = position.saturating_add((i as u64).saturating_mul(effective_batch_size as u64));
                        let digits = BbpComputer::compute_hex_digits_parallel(pos, effective_batch_size);
                        (pos, effective_batch_size, digits)
                    })
                    .collect()
            })
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
            if !running.load(Ordering::Acquire) {
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
                            let tx_short = &result.tx_hash[..result.tx_hash.len().min(12)];
                            emit_log(
                                &app,
                                &format!(
                                    "Proof submitted! tx:{} +{:.4} PI",
                                    tx_short,
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
                            local_position = None;
                            break;
                        }
                    }
                    Err(e) => {
                        emit_log(&app, &format!("Bad response: {e}"), "error");
                        local_nonce = None;
                        local_position = None;
                        break;
                    }
                },
                Err(e) => {
                    emit_log(&app, &format!("Submit failed: {e}"), "error");
                    local_nonce = None;
                    local_position = None;
                    break;
                }
            }
        }

        if local_nonce.is_some() || current_nonce > nonce {
            local_nonce = Some(current_nonce);
        }

        // Advance local position past submitted batches to prevent self-collision
        if current_nonce > nonce {
            let total_digits_this_round = effective_batch_count as u64 * effective_batch_size as u64;
            local_position = Some(position.saturating_add(total_digits_this_round));
        }

        consecutive_errors = 0; // Reset backoff on successful round
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

    // Session summary
    let uptime = session_start.elapsed().as_secs();
    let dps = if uptime > 0 {
        total_digits as f64 / uptime as f64
    } else {
        0.0
    };
    let h = uptime / 3600;
    let m = (uptime % 3600) / 60;
    let s = uptime % 60;
    let uptime_str = if h > 0 {
        format!("{}h {}m {}s", h, m, s)
    } else if m > 0 {
        format!("{}m {}s", m, s)
    } else {
        format!("{}s", s)
    };
    emit_log(
        &app,
        &format!(
            "Session summary: {} digits, {}/{} proofs confirmed, {:.1} digits/sec, uptime {}",
            total_digits, proofs_confirmed, proofs_submitted, dps, uptime_str
        ),
        "info",
    );

    // Final stats emit
    let _ = app.emit(
        "mining-stats",
        MiningStats {
            digits_computed: total_digits,
            proofs_submitted,
            proofs_confirmed,
            balance: last_known_balance,
            uptime_secs: uptime,
            digits_per_sec: dps,
            state: "idle".to_string(),
            current_position: 0,
            reward_per_digit: 0,
        },
    );
}
