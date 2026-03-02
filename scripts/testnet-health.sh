#!/usr/bin/env bash
# ============================================================================
# PIChain Testnet Health Monitor
# ============================================================================
#
# Continuous health monitoring for the public testnet soak test.
# Checks consensus liveness, node health, resource usage, and chain
# progression. Designed to run alongside public-testnet.sh for 7-14 days.
#
# Usage:
#   ./scripts/testnet-health.sh                  — Run once
#   ./scripts/testnet-health.sh --continuous      — Loop every 60s
#   ./scripts/testnet-health.sh --interval 30     — Loop every 30s
#   ./scripts/testnet-health.sh --alert-cmd "..."  — Run command on failure
#
# ============================================================================

set -euo pipefail

TESTNET_DIR="${TESTNET_DIR:-/tmp/pichain-public-testnet}"
BASE_RPC_PORT="${BASE_RPC_PORT:-28314}"
PID_DIR="$TESTNET_DIR/pids"
METRICS_DIR="$TESTNET_DIR/metrics"
HEALTH_LOG="$METRICS_DIR/health.log"
SNAPSHOT_DIR="$METRICS_DIR/snapshots"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

CONTINUOUS=false
INTERVAL=60
ALERT_CMD=""
MAX_HEIGHT_DRIFT=5
MAX_STALL_CHECKS=3

# Track stalls across iterations
LAST_MAX_HEIGHT=0
STALL_COUNT=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --continuous) CONTINUOUS=true; shift ;;
        --interval) INTERVAL="$2"; shift 2 ;;
        --alert-cmd) ALERT_CMD="$2"; shift 2 ;;
        *) echo "Unknown flag: $1"; exit 1 ;;
    esac
done

mkdir -p "$METRICS_DIR" "$SNAPSHOT_DIR"

# ---- Alert Helper ----
alert() {
    local msg="$1"
    echo -e "${RED}[ALERT]${NC} $(date '+%Y-%m-%d %H:%M:%S') $msg"
    if [[ -n "$ALERT_CMD" ]]; then
        eval "$ALERT_CMD" "'$msg'" 2>/dev/null || true
    fi
}

# ---- Single Health Check ----
run_health_check() {
    local timestamp
    timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    local status="ALL_HEALTHY"
    local issues=()

    # Discover nodes
    local node_count=0
    local nodes_alive=0
    local -a heights=()
    local -a node_names=()

    for pidfile in "$PID_DIR"/node*.pid 2>/dev/null; do
        [[ -f "$pidfile" ]] || continue
        ((node_count++))
        local pid name idx port
        pid=$(cat "$pidfile")
        name=$(basename "$pidfile" .pid)
        idx=$(echo "$name" | grep -oP '\d+')
        port=$((BASE_RPC_PORT + idx - 1))
        node_names+=("$name")

        # --- Check 1: Process alive ---
        if ! kill -0 "$pid" 2>/dev/null; then
            issues+=("$name: process dead (PID=$pid)")
            status="UNHEALTHY"
            heights+=("0")
            continue
        fi
        ((nodes_alive++))

        # --- Check 2: RPC responding ---
        local health_resp
        health_resp=$(curl -sf --max-time 5 "http://127.0.0.1:$port/health" 2>/dev/null || echo "TIMEOUT")
        if [[ "$health_resp" == "TIMEOUT" ]]; then
            issues+=("$name: RPC not responding (port $port)")
            status="DEGRADED"
            heights+=("0")
            continue
        fi

        # --- Check 3: Chain height ---
        local status_json height peer_count
        status_json=$(curl -sf --max-time 5 "http://127.0.0.1:$port/api/v1/status" 2>/dev/null || echo "{}")
        height=$(echo "$status_json" | grep -oP '"height"\s*:\s*\K\d+' || echo "0")
        peer_count=$(echo "$status_json" | grep -oP '"peer_count"\s*:\s*\K\d+' || echo "0")
        heights+=("$height")

        # --- Check 4: Peer connectivity ---
        if [[ "$peer_count" == "0" && $node_count -gt 1 ]]; then
            issues+=("$name: 0 peers (isolated)")
            [[ "$status" != "UNHEALTHY" ]] && status="DEGRADED"
        fi

        # --- Check 5: Memory usage (RSS in KB) ---
        local rss_kb
        rss_kb=$(ps -o rss= -p "$pid" 2>/dev/null || echo "0")
        local rss_mb=$((rss_kb / 1024))
        if [[ $rss_mb -gt 4096 ]]; then
            issues+=("$name: high memory ${rss_mb}MB")
            [[ "$status" != "UNHEALTHY" ]] && status="DEGRADED"
        fi

        # --- Check 6: Open file descriptors ---
        local fd_count
        fd_count=$(ls /proc/"$pid"/fd 2>/dev/null | wc -l || echo "0")
        if [[ $fd_count -gt 10000 ]]; then
            issues+=("$name: high FDs ($fd_count)")
            [[ "$status" != "UNHEALTHY" ]] && status="DEGRADED"
        fi
    done

    # --- Check 7: Consensus - height agreement ---
    if [[ ${#heights[@]} -gt 1 ]]; then
        local max_h=0 min_h=999999999
        for h in "${heights[@]}"; do
            [[ $h -gt $max_h ]] && max_h=$h
            [[ $h -lt $min_h && $h -gt 0 ]] && min_h=$h
        done
        local drift=$((max_h - min_h))
        if [[ $drift -gt $MAX_HEIGHT_DRIFT ]]; then
            issues+=("consensus: height drift ${drift} blocks (max=$max_h, min=$min_h)")
            [[ "$status" != "UNHEALTHY" ]] && status="DEGRADED"
        fi

        # --- Check 8: Chain stall detection ---
        if [[ $max_h -le $LAST_MAX_HEIGHT && $max_h -gt 0 ]]; then
            ((STALL_COUNT++))
            if [[ $STALL_COUNT -ge $MAX_STALL_CHECKS ]]; then
                issues+=("consensus: chain stalled at height $max_h for $STALL_COUNT checks")
                status="UNHEALTHY"
            fi
        else
            STALL_COUNT=0
        fi
        LAST_MAX_HEIGHT=$max_h
    fi

    # --- Check 9: Miner health ---
    for pidfile in "$PID_DIR"/miner*.pid 2>/dev/null; do
        [[ -f "$pidfile" ]] || continue
        local pid name
        pid=$(cat "$pidfile")
        name=$(basename "$pidfile" .pid)
        if ! kill -0 "$pid" 2>/dev/null; then
            issues+=("$name: process dead (PID=$pid)")
            [[ "$status" != "UNHEALTHY" ]] && status="DEGRADED"
        fi
    done

    # --- Check 10: Disk space ---
    local disk_avail_kb
    disk_avail_kb=$(df -k "$TESTNET_DIR" 2>/dev/null | tail -1 | awk '{print $4}' || echo "999999999")
    local disk_avail_gb=$((disk_avail_kb / 1024 / 1024))
    if [[ $disk_avail_gb -lt 5 ]]; then
        issues+=("disk: only ${disk_avail_gb}GB free")
        [[ "$status" != "UNHEALTHY" ]] && status="DEGRADED"
    fi
    if [[ $disk_avail_gb -lt 1 ]]; then
        status="UNHEALTHY"
    fi

    # --- Output ---
    local height_str
    height_str=$(IFS=,; echo "${heights[*]}")

    if [[ "$status" == "ALL_HEALTHY" ]]; then
        echo -e "${GREEN}[HEALTHY]${NC} $timestamp | nodes=$nodes_alive/$node_count | heights=[$height_str]"
    elif [[ "$status" == "DEGRADED" ]]; then
        echo -e "${YELLOW}[DEGRADED]${NC} $timestamp | nodes=$nodes_alive/$node_count | heights=[$height_str]"
        for issue in "${issues[@]}"; do
            echo -e "  ${YELLOW}→${NC} $issue"
        done
    else
        echo -e "${RED}[UNHEALTHY]${NC} $timestamp | nodes=$nodes_alive/$node_count | heights=[$height_str]"
        for issue in "${issues[@]}"; do
            echo -e "  ${RED}→${NC} $issue"
        done
        alert "Testnet unhealthy: ${issues[*]}"
    fi

    # --- Log to file ---
    local issues_str=""
    if [[ ${#issues[@]} -gt 0 ]]; then
        issues_str=$(IFS="; "; echo "${issues[*]}")
    fi
    echo "$timestamp | $status | nodes=$nodes_alive/$node_count | heights=[$height_str] | $issues_str" >> "$HEALTH_LOG"

    # --- Periodic snapshot (every 10 minutes = every 10 checks at 60s) ---
    local check_num
    check_num=$(wc -l < "$HEALTH_LOG" 2>/dev/null || echo "0")
    if (( check_num % 10 == 0 )); then
        save_snapshot "$timestamp"
    fi

    return 0
}

# ---- Snapshot: detailed point-in-time state ----
save_snapshot() {
    local timestamp="$1"
    local snap_file="$SNAPSHOT_DIR/$(date +%Y%m%d-%H%M%S).json"

    local nodes_json="["
    local first=true
    for pidfile in "$PID_DIR"/node*.pid 2>/dev/null; do
        [[ -f "$pidfile" ]] || continue
        local pid name idx port
        pid=$(cat "$pidfile")
        name=$(basename "$pidfile" .pid)
        idx=$(echo "$name" | grep -oP '\d+')
        port=$((BASE_RPC_PORT + idx - 1))

        local alive="false"
        kill -0 "$pid" 2>/dev/null && alive="true"

        local height="0" peers="0" rss_mb="0"
        if [[ "$alive" == "true" ]]; then
            local sj
            sj=$(curl -sf --max-time 3 "http://127.0.0.1:$port/api/v1/status" 2>/dev/null || echo "{}")
            height=$(echo "$sj" | grep -oP '"height"\s*:\s*\K\d+' || echo "0")
            peers=$(echo "$sj" | grep -oP '"peer_count"\s*:\s*\K\d+' || echo "0")
            local rss_kb
            rss_kb=$(ps -o rss= -p "$pid" 2>/dev/null || echo "0")
            rss_mb=$((rss_kb / 1024))
        fi

        [[ "$first" == "true" ]] && first=false || nodes_json+=","
        nodes_json+="{\"name\":\"$name\",\"pid\":$pid,\"alive\":$alive,\"height\":$height,\"peers\":$peers,\"rss_mb\":$rss_mb}"
    done
    nodes_json+="]"

    local disk_used
    disk_used=$(du -sm "$TESTNET_DIR" 2>/dev/null | awk '{print $1}' || echo "0")

    cat > "$snap_file" <<EOF
{
  "timestamp": "$timestamp",
  "nodes": $nodes_json,
  "disk_used_mb": $disk_used
}
EOF
}

# ---- Main ----
if [[ ! -d "$PID_DIR" ]]; then
    echo -e "${RED}No testnet found at $TESTNET_DIR${NC}"
    echo "Start one with: ./scripts/public-testnet.sh launch"
    exit 1
fi

echo -e "${CYAN}PIChain Testnet Health Monitor${NC}"
echo "  Testnet dir: $TESTNET_DIR"
echo "  Health log:  $HEALTH_LOG"
if [[ "$CONTINUOUS" == "true" ]]; then
    echo "  Mode:        continuous (every ${INTERVAL}s)"
    echo "  Press Ctrl+C to stop"
fi
echo ""

if [[ "$CONTINUOUS" == "true" ]]; then
    while true; do
        run_health_check
        sleep "$INTERVAL"
    done
else
    run_health_check
fi
