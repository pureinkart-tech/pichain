#!/usr/bin/env bash
# ============================================================================
# PIChain Public Testnet Launcher
# ============================================================================
#
# Launches a multi-validator public testnet for pre-mainnet soak testing.
# Designed to run for 7-14 days to verify stability, consensus, and
# performance under sustained load.
#
# Usage:
#   ./scripts/public-testnet.sh launch [--validators N] [--with-miners N]
#   ./scripts/public-testnet.sh stop
#   ./scripts/public-testnet.sh status
#   ./scripts/public-testnet.sh stress [--duration SECS] [--tps N]
#   ./scripts/public-testnet.sh report
#
# Environment:
#   TESTNET_DIR    — Data root (default: /tmp/pichain-public-testnet)
#   PICHAIN        — Binary path (default: ./target/release/pichain)
#   PICHAIN_MINER  — Miner binary (default: ./target/release/pichain-miner)
#
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
PICHAIN="${PICHAIN:-$ROOT_DIR/target/release/pichain}"
PICHAIN_MINER="${PICHAIN_MINER:-$ROOT_DIR/target/release/pichain-miner}"
TESTNET_DIR="${TESTNET_DIR:-/tmp/pichain-public-testnet}"
CHAIN_ID=31415926
BASE_RPC_PORT=28314
BASE_P2P_PORT=29314
PID_DIR="$TESTNET_DIR/pids"
LOG_DIR="$TESTNET_DIR/logs"
METRICS_DIR="$TESTNET_DIR/metrics"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

log()   { echo -e "${GREEN}[TESTNET]${NC} $(date '+%H:%M:%S') $*"; }
warn()  { echo -e "${YELLOW}[WARNING]${NC} $(date '+%H:%M:%S') $*"; }
err()   { echo -e "${RED}[ERROR]${NC} $(date '+%H:%M:%S') $*"; }
info()  { echo -e "${CYAN}[INFO]${NC} $(date '+%H:%M:%S') $*"; }

# ---- Launch ----
cmd_launch() {
    local num_validators=4
    local num_miners=1

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --validators) num_validators="$2"; shift 2 ;;
            --with-miners) num_miners="$2"; shift 2 ;;
            *) err "Unknown flag: $1"; exit 1 ;;
        esac
    done

    if [[ $num_validators -lt 4 ]]; then
        err "BFT requires minimum 4 validators (3f+1). Got $num_validators."
        exit 1
    fi

    echo -e "${CYAN}"
    echo "╔══════════════════════════════════════════════════════════════╗"
    echo "║           PIChain Public Testnet Launch                     ║"
    echo "║   Validators: $num_validators  |  Miners: $num_miners  |  Chain ID: $CHAIN_ID      ║"
    echo "╚══════════════════════════════════════════════════════════════╝"
    echo -e "${NC}"

    # Check binary
    if [[ ! -x "$PICHAIN" ]]; then
        err "Binary not found: $PICHAIN"
        err "Run: cargo build --release -p pichain-node"
        exit 1
    fi

    # Create directories
    mkdir -p "$PID_DIR" "$LOG_DIR" "$METRICS_DIR"

    # Record launch metadata
    cat > "$TESTNET_DIR/launch-info.json" <<EOF
{
  "launched_at": "$(date -Iseconds)",
  "chain_id": $CHAIN_ID,
  "validators": $num_validators,
  "miners": $num_miners,
  "binary": "$PICHAIN",
  "binary_version": "$($PICHAIN --version 2>/dev/null || echo 'unknown')",
  "host": "$(hostname)",
  "target_soak_days": 14
}
EOF

    # Generate validator keys
    log "Generating $num_validators validator keypairs..."
    local -a ADDRS=()
    local -a BLS_PKS=()
    local -a BLS_POPS=()

    for i in $(seq 1 "$num_validators"); do
        local node_dir="$TESTNET_DIR/node$i"
        mkdir -p "$node_dir"

        if [[ -f "$node_dir/keygen.out" ]]; then
            log "  Validator $i: reusing existing keys"
        else
            "$PICHAIN" keygen > "$node_dir/keygen.out" 2>&1
        fi

        local keygen_out
        keygen_out=$(cat "$node_dir/keygen.out")

        local addr bls_pk bls_pop ed_secret bls_secret
        addr=$(echo "$keygen_out" | grep -oP 'Address:\s+\K[0-9a-f]+' || echo "$keygen_out" | grep -i 'address' | grep -oP '[0-9a-f]{40}')
        bls_pk=$(echo "$keygen_out" | grep -i 'bls.*public' | grep -oP '[0-9a-f]{96}')
        bls_pop=$(echo "$keygen_out" | grep -i 'bls.*pop\|proof.of.possession' | grep -oP '[0-9a-f]{192}')
        ed_secret=$(echo "$keygen_out" | grep -i 'ed25519.*secret\|secret.*key' | grep -oP '[0-9a-f]{64}')
        bls_secret=$(echo "$keygen_out" | grep -i 'bls.*secret' | grep -oP '[0-9a-f]{64}')

        # Save keypair file
        if [[ -n "$ed_secret" && -n "$bls_secret" ]]; then
            cat > "$node_dir/validator.key" <<KEYEOF
{"ed25519_secret":"$ed_secret","bls_secret":"$bls_secret"}
KEYEOF
            chmod 600 "$node_dir/validator.key"
        fi

        ADDRS+=("$addr")
        BLS_PKS+=("$bls_pk")
        BLS_POPS+=("$bls_pop")
        log "  Validator $i: ${addr:0:16}..."
    done

    # Generate config files
    log "Generating node configs..."
    for i in $(seq 1 "$num_validators"); do
        local rpc_port=$((BASE_RPC_PORT + i - 1))
        local p2p_port=$((BASE_P2P_PORT + i - 1))

        # Build genesis validator entries
        local gv_entries=""
        for j in $(seq 1 "$num_validators"); do
            if [[ $j -ne $i ]]; then
                gv_entries+="
[[genesis_validators]]
address = \"${ADDRS[$((j-1))]:-0000000000000000000000000000000000000000}\"
stake_pi = 100000
bls_public_key = \"${BLS_PKS[$((j-1))]:-000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000}\"
bls_pop = \"${BLS_POPS[$((j-1))]:-000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000}\"
"
            fi
        done

        # Bootstrap: connect to node1 if not self
        local bootstrap=""
        if [[ $i -ne 1 ]]; then
            bootstrap="bootstrap_peers = [\"/ip4/127.0.0.1/tcp/$BASE_P2P_PORT\"]"
        else
            bootstrap="bootstrap_peers = []"
        fi

        cat > "$TESTNET_DIR/node$i.toml" <<TOMLEOF
data_dir = "$TESTNET_DIR/node$i"
rpc_addr = "127.0.0.1:$rpc_port"
p2p_addr = "/ip4/127.0.0.1/tcp/$p2p_port"
chain_id = $CHAIN_ID
max_mempool_size = 100000
validator_stake_pi = 100000
log_level = "info"
$bootstrap
$gv_entries
TOMLEOF
    done

    # Start validator nodes
    log "Starting $num_validators validator nodes..."
    for i in $(seq 1 "$num_validators"); do
        local rpc_port=$((BASE_RPC_PORT + i - 1))
        "$PICHAIN" run --config "$TESTNET_DIR/node$i.toml" \
            >> "$LOG_DIR/node$i.log" 2>&1 &
        local pid=$!
        echo "$pid" > "$PID_DIR/node$i.pid"
        log "  Node $i started: PID=$pid RPC=127.0.0.1:$rpc_port"
        sleep 2
    done

    # Start miners
    if [[ $num_miners -gt 0 && -x "$PICHAIN_MINER" ]]; then
        log "Starting $num_miners miners..."
        for m in $(seq 1 "$num_miners"); do
            local target_node=$(( (m - 1) % num_validators + 1 ))
            local target_port=$((BASE_RPC_PORT + target_node - 1))
            local miner_dir="$TESTNET_DIR/miner$m"
            mkdir -p "$miner_dir"

            "$PICHAIN_MINER" \
                --rpc-url "http://127.0.0.1:$target_port" \
                --keypair "$miner_dir/wallet.json" \
                --chain-id "$CHAIN_ID" \
                --generate-keypair \
                --profile desktop \
                >> "$LOG_DIR/miner$m.log" 2>&1 &
            local pid=$!
            echo "$pid" > "$PID_DIR/miner$m.pid"
            log "  Miner $m started: PID=$pid → node$target_node"
        done
    fi

    # Wait for startup
    log "Waiting for nodes to initialize (15s)..."
    sleep 15

    # Initial health check
    local healthy=0
    for i in $(seq 1 "$num_validators"); do
        local port=$((BASE_RPC_PORT + i - 1))
        if curl -sf "http://127.0.0.1:$port/health" >/dev/null 2>&1; then
            ((healthy++))
        fi
    done

    echo ""
    if [[ $healthy -eq $num_validators ]]; then
        log "${BOLD}All $num_validators validators healthy!${NC}"
    else
        warn "Only $healthy/$num_validators validators responding. Check logs: $LOG_DIR/"
    fi

    echo ""
    log "Testnet is running."
    log "  RPC endpoints: 127.0.0.1:$BASE_RPC_PORT through 127.0.0.1:$((BASE_RPC_PORT + num_validators - 1))"
    log "  Explorer: http://127.0.0.1:$BASE_RPC_PORT/explorer"
    log "  Logs: $LOG_DIR/"
    log "  PIDs: $PID_DIR/"
    log ""
    log "Next steps:"
    log "  1. Monitor: ./scripts/testnet-health.sh --continuous"
    log "  2. Stress:  ./scripts/public-testnet.sh stress --duration 300 --tps 50"
    log "  3. Report:  ./scripts/public-testnet.sh report"
    log "  4. Stop:    ./scripts/public-testnet.sh stop"
}

# ---- Stop ----
cmd_stop() {
    log "Stopping all testnet processes..."

    if [[ ! -d "$PID_DIR" ]]; then
        warn "No PID directory found at $PID_DIR"
        return
    fi

    local stopped=0
    for pidfile in "$PID_DIR"/*.pid; do
        [[ -f "$pidfile" ]] || continue
        local pid
        pid=$(cat "$pidfile")
        local name
        name=$(basename "$pidfile" .pid)
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            log "  Stopped $name (PID=$pid)"
            ((stopped++))
        else
            log "  $name already stopped"
        fi
        rm -f "$pidfile"
    done

    log "Stopped $stopped processes."
    log "Data preserved at: $TESTNET_DIR"
    log "To clean up: rm -rf $TESTNET_DIR"
}

# ---- Status ----
cmd_status() {
    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║           PIChain Testnet Status                            ║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    if [[ -f "$TESTNET_DIR/launch-info.json" ]]; then
        local launched_at
        launched_at=$(grep -oP '"launched_at":\s*"\K[^"]+' "$TESTNET_DIR/launch-info.json" 2>/dev/null || echo "unknown")
        local uptime_secs=0
        if command -v python3 &>/dev/null; then
            uptime_secs=$(python3 -c "
from datetime import datetime, timezone
import sys
try:
    t = datetime.fromisoformat('$launched_at')
    if t.tzinfo is None:
        t = t.replace(tzinfo=timezone.utc)
    diff = datetime.now(timezone.utc) - t
    print(int(diff.total_seconds()))
except:
    print(0)
" 2>/dev/null || echo "0")
        fi
        local days=$((uptime_secs / 86400))
        local hours=$(( (uptime_secs % 86400) / 3600 ))
        local mins=$(( (uptime_secs % 3600) / 60 ))
        info "Launched: $launched_at"
        info "Uptime:   ${days}d ${hours}h ${mins}m"
    fi

    echo ""
    printf "  ${BOLD}%-8s %-10s %-10s %-12s %-12s${NC}\n" "Node" "Status" "PID" "Height" "Peers"
    printf "  %-8s %-10s %-10s %-12s %-12s\n" "--------" "----------" "----------" "------------" "------------"

    for pidfile in "$PID_DIR"/node*.pid 2>/dev/null; do
        [[ -f "$pidfile" ]] || continue
        local name pid status height peers
        name=$(basename "$pidfile" .pid)
        pid=$(cat "$pidfile")
        local idx
        idx=$(echo "$name" | grep -oP '\d+')
        local port=$((BASE_RPC_PORT + idx - 1))

        if kill -0 "$pid" 2>/dev/null; then
            status="${GREEN}running${NC}"
        else
            status="${RED}dead${NC}"
            printf "  %-8s %-10b %-10s %-12s %-12s\n" "$name" "$status" "$pid" "-" "-"
            continue
        fi

        # Fetch height + peers from status endpoint
        local status_json
        status_json=$(curl -sf "http://127.0.0.1:$port/api/v1/status" 2>/dev/null || echo "{}")
        height=$(echo "$status_json" | grep -oP '"height"\s*:\s*\K\d+' || echo "?")
        peers=$(echo "$status_json" | grep -oP '"peer_count"\s*:\s*\K\d+' || echo "?")

        printf "  %-8s %-10b %-10s %-12s %-12s\n" "$name" "$status" "$pid" "$height" "$peers"
    done

    # Miner status
    echo ""
    for pidfile in "$PID_DIR"/miner*.pid 2>/dev/null; do
        [[ -f "$pidfile" ]] || continue
        local name pid status
        name=$(basename "$pidfile" .pid)
        pid=$(cat "$pidfile")
        if kill -0 "$pid" 2>/dev/null; then
            status="${GREEN}running${NC}"
        else
            status="${RED}dead${NC}"
        fi
        printf "  %-8s %-10b %-10s\n" "$name" "$status" "$pid"
    done

    echo ""
}

# ---- Stress Test ----
cmd_stress() {
    local duration=60
    local tps=10

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --duration) duration="$2"; shift 2 ;;
            --tps) tps="$2"; shift 2 ;;
            *) err "Unknown flag: $1"; exit 1 ;;
        esac
    done

    log "Starting stress test: ${duration}s at ~${tps} TPS"
    log "Target: http://127.0.0.1:$BASE_RPC_PORT"

    # Check node is running
    if ! curl -sf "http://127.0.0.1:$BASE_RPC_PORT/health" >/dev/null 2>&1; then
        err "Node not responding on port $BASE_RPC_PORT. Is the testnet running?"
        exit 1
    fi

    # Get initial state
    local start_height
    start_height=$(curl -sf "http://127.0.0.1:$BASE_RPC_PORT/api/v1/status" | grep -oP '"height"\s*:\s*\K\d+' || echo "0")
    local start_time
    start_time=$(date +%s)

    local interval
    interval=$(python3 -c "print(round(1.0/$tps, 4))" 2>/dev/null || echo "0.1")
    local total_sent=0
    local total_ok=0
    local total_fail=0

    log "Interval: ${interval}s between transactions"
    echo ""

    # Stress: send faucet transactions to random addresses
    local end_time=$((start_time + duration))
    while [[ $(date +%s) -lt $end_time ]]; do
        local addr
        addr=$(python3 -c "import secrets; print(secrets.token_hex(20))" 2>/dev/null || head -c 20 /dev/urandom | xxd -p)

        local result
        result=$(curl -sf -X POST "http://127.0.0.1:$BASE_RPC_PORT/api/v1/faucet" \
            -H "Content-Type: application/json" \
            -d "{\"address\":\"$addr\"}" 2>/dev/null || echo "FAIL")
        ((total_sent++))

        if echo "$result" | grep -qi "error\|FAIL"; then
            ((total_fail++))
        else
            ((total_ok++))
        fi

        # Progress every 100 txs
        if (( total_sent % 100 == 0 )); then
            local elapsed=$(( $(date +%s) - start_time ))
            local actual_tps=0
            [[ $elapsed -gt 0 ]] && actual_tps=$((total_sent / elapsed))
            info "Sent: $total_sent (OK: $total_ok, Fail: $total_fail) — ${actual_tps} TPS actual"
        fi

        sleep "$interval"
    done

    # Final stats
    local end_height
    end_height=$(curl -sf "http://127.0.0.1:$BASE_RPC_PORT/api/v1/status" | grep -oP '"height"\s*:\s*\K\d+' || echo "0")
    local elapsed=$(( $(date +%s) - start_time ))
    local actual_tps=0
    [[ $elapsed -gt 0 ]] && actual_tps=$((total_sent / elapsed))
    local blocks_produced=$((end_height - start_height))

    echo ""
    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║           Stress Test Results                               ║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════╝${NC}"
    echo ""
    echo "  Duration:          ${elapsed}s"
    echo "  Transactions sent: $total_sent"
    echo "  Successful:        $total_ok"
    echo "  Failed:            $total_fail"
    echo "  Success rate:      $(python3 -c "print(f'{$total_ok/$total_sent*100:.1f}%')" 2>/dev/null || echo "?")"
    echo "  Actual TPS:        $actual_tps"
    echo "  Blocks produced:   $blocks_produced ($start_height → $end_height)"
    echo ""

    # Save results
    cat > "$METRICS_DIR/stress-$(date +%Y%m%d-%H%M%S).json" <<EOF
{
  "timestamp": "$(date -Iseconds)",
  "duration_secs": $elapsed,
  "target_tps": $tps,
  "actual_tps": $actual_tps,
  "total_sent": $total_sent,
  "total_ok": $total_ok,
  "total_fail": $total_fail,
  "start_height": $start_height,
  "end_height": $end_height,
  "blocks_produced": $blocks_produced
}
EOF
    log "Results saved to $METRICS_DIR/"
}

# ---- Report ----
cmd_report() {
    echo -e "${CYAN}"
    echo "╔══════════════════════════════════════════════════════════════╗"
    echo "║           PIChain Testnet Soak Report                      ║"
    echo "╚══════════════════════════════════════════════════════════════╝"
    echo -e "${NC}"

    # Launch info
    if [[ -f "$TESTNET_DIR/launch-info.json" ]]; then
        echo ""
        echo "  === Launch Info ==="
        cat "$TESTNET_DIR/launch-info.json" | sed 's/^/  /'
    fi

    echo ""
    echo "  === Node Status ==="
    cmd_status 2>/dev/null

    # Health check history
    echo "  === Health History ==="
    local health_log="$METRICS_DIR/health.log"
    if [[ -f "$health_log" ]]; then
        local total_checks
        total_checks=$(wc -l < "$health_log")
        local healthy_checks
        healthy_checks=$(grep -c "ALL_HEALTHY" "$health_log" || echo "0")
        local unhealthy_checks
        unhealthy_checks=$(grep -c "UNHEALTHY\|DEGRADED" "$health_log" || echo "0")

        echo "  Total checks:    $total_checks"
        echo "  Healthy:         $healthy_checks"
        echo "  Unhealthy:       $unhealthy_checks"
        if [[ $total_checks -gt 0 ]]; then
            echo "  Uptime:          $(python3 -c "print(f'{$healthy_checks/$total_checks*100:.2f}%')" 2>/dev/null || echo "?")"
        fi

        echo ""
        echo "  Last 10 entries:"
        tail -10 "$health_log" | sed 's/^/    /'
    else
        echo "  No health data yet. Run: ./scripts/testnet-health.sh --continuous"
    fi

    # Stress test results
    echo ""
    echo "  === Stress Test History ==="
    local found_stress=0
    for f in "$METRICS_DIR"/stress-*.json 2>/dev/null; do
        [[ -f "$f" ]] || continue
        found_stress=1
        local ts tps ok_rate
        ts=$(grep -oP '"timestamp":\s*"\K[^"]+' "$f")
        tps=$(grep -oP '"actual_tps":\s*\K\d+' "$f")
        ok_rate=$(python3 -c "
import json
with open('$f') as fp:
    d = json.load(fp)
if d['total_sent'] > 0:
    print(f\"{d['total_ok']/d['total_sent']*100:.1f}%\")
else:
    print('N/A')
" 2>/dev/null || echo "?")
        echo "    $ts — ${tps} TPS, ${ok_rate} success"
    done
    if [[ $found_stress -eq 0 ]]; then
        echo "  No stress tests yet. Run: ./scripts/public-testnet.sh stress"
    fi

    # Log analysis
    echo ""
    echo "  === Log Analysis ==="
    local total_errors=0
    local total_warns=0
    for logfile in "$LOG_DIR"/node*.log 2>/dev/null; do
        [[ -f "$logfile" ]] || continue
        local name errs warns
        name=$(basename "$logfile" .log)
        errs=$(grep -ci "error\|panic\|fatal" "$logfile" 2>/dev/null || echo "0")
        warns=$(grep -ci "warn" "$logfile" 2>/dev/null || echo "0")
        total_errors=$((total_errors + errs))
        total_warns=$((total_warns + warns))
        echo "    $name: $errs errors, $warns warnings"
    done
    echo ""
    echo "  Total: $total_errors errors, $total_warns warnings across all nodes"

    # Disk usage
    echo ""
    echo "  === Disk Usage ==="
    du -sh "$TESTNET_DIR" 2>/dev/null | sed 's/^/  /'
    for dir in "$TESTNET_DIR"/node*; do
        [[ -d "$dir" ]] || continue
        du -sh "$dir" 2>/dev/null | sed 's/^/    /'
    done

    echo ""
    echo -e "${GREEN}Report complete.${NC}"
}

# ---- Main ----
case "${1:-help}" in
    launch)  shift; cmd_launch "$@" ;;
    stop)    cmd_stop ;;
    status)  cmd_status ;;
    stress)  shift; cmd_stress "$@" ;;
    report)  cmd_report ;;
    *)
        echo "PIChain Public Testnet"
        echo ""
        echo "Usage: $0 <command> [options]"
        echo ""
        echo "Commands:"
        echo "  launch   Start the testnet"
        echo "    --validators N    Number of validators (default: 4, min: 4)"
        echo "    --with-miners N   Number of miners (default: 1)"
        echo ""
        echo "  stop     Stop all testnet processes"
        echo "  status   Show node status"
        echo "  stress   Run stress test"
        echo "    --duration SECS   Duration in seconds (default: 60)"
        echo "    --tps N           Target TPS (default: 10)"
        echo ""
        echo "  report   Generate soak test report"
        echo ""
        echo "Environment:"
        echo "  TESTNET_DIR=$TESTNET_DIR"
        echo "  PICHAIN=$PICHAIN"
        echo "  PICHAIN_MINER=$PICHAIN_MINER"
        ;;
esac
