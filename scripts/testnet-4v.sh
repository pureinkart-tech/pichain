#!/usr/bin/env bash
# PIChain 4-Validator Local Testnet
#
# Starts 4 validator nodes on localhost, verifies consensus and tx propagation.
# Usage: ./scripts/testnet-4v.sh [--quick]  (--quick skips build)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
BINARY="$ROOT_DIR/target/release/pichain"
CHAIN_ID=31415
BASE_RPC_PORT=18314
BASE_P2P_PORT=19314
DATA_ROOT="/tmp/pichain-testnet-4v"
PIDS=()
PASS=0
FAIL=0

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC} $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $*"; }
fail()  { echo -e "${RED}[FAIL]${NC} $*"; }
check() {
    local desc="$1"; shift
    if "$@" >/dev/null 2>&1; then
        info "PASS: $desc"
        ((PASS++))
    else
        fail "FAIL: $desc"
        ((FAIL++))
    fi
}

cleanup() {
    info "Stopping all nodes..."
    for pid in "${PIDS[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    info "Cleanup complete"
}
trap cleanup EXIT

# --- Build ---
if [[ "${1:-}" != "--quick" ]]; then
    info "Building PIChain (release mode)..."
    (cd "$ROOT_DIR" && cargo build --release -p pichain-node 2>&1 | tail -5)
fi

if [[ ! -x "$BINARY" ]]; then
    fail "Binary not found at $BINARY"
    exit 1
fi

# --- Setup data directories ---
info "Setting up data directories at $DATA_ROOT"
rm -rf "$DATA_ROOT"
mkdir -p "$DATA_ROOT"

for i in 1 2 3 4; do
    mkdir -p "$DATA_ROOT/node$i"
done

# --- Generate validator keys ---
info "Generating validator keys..."
VALIDATORS=()
BLS_KEYS=()
BLS_POPS=()

for i in 1 2 3 4; do
    KEY_OUTPUT=$("$BINARY" keygen 2>&1)
    # Extract address, BLS public key, and PoP from keygen output
    ADDR=$(echo "$KEY_OUTPUT" | grep -oP 'Address:\s+\K[0-9a-f]+' || echo "$KEY_OUTPUT" | grep -i 'address' | grep -oP '[0-9a-f]{40}')
    BLS_PK=$(echo "$KEY_OUTPUT" | grep -i 'bls.*public' | grep -oP '[0-9a-f]{96}')
    BLS_POP_VAL=$(echo "$KEY_OUTPUT" | grep -i 'bls.*pop\|proof.of.possession' | grep -oP '[0-9a-f]{192}')

    # Save the keypair file
    ED_SECRET=$(echo "$KEY_OUTPUT" | grep -i 'ed25519.*secret\|secret.*key' | grep -oP '[0-9a-f]{64}')
    BLS_SECRET=$(echo "$KEY_OUTPUT" | grep -i 'bls.*secret' | grep -oP '[0-9a-f]{64}')

    if [[ -n "$ED_SECRET" && -n "$BLS_SECRET" ]]; then
        cat > "$DATA_ROOT/node$i/validator.key" << KEYEOF
{"ed25519_secret":"$ED_SECRET","bls_secret":"$BLS_SECRET"}
KEYEOF
        chmod 600 "$DATA_ROOT/node$i/validator.key"
    fi

    VALIDATORS+=("$ADDR")
    BLS_KEYS+=("$BLS_PK")
    BLS_POPS+=("$BLS_POP_VAL")
    info "Node $i: address=${ADDR:0:16}..."
done

# --- Generate config files ---
info "Generating node configs..."
for i in 1 2 3 4; do
    RPC_PORT=$((BASE_RPC_PORT + i - 1))
    P2P_PORT=$((BASE_P2P_PORT + i - 1))

    # Build genesis_validators entries (all nodes except self)
    GV_ENTRIES=""
    for j in 1 2 3 4; do
        if [[ $j -ne $i ]]; then
            GV_ENTRIES+="
[[genesis_validators]]
address = \"${VALIDATORS[$((j-1))]:-0000000000000000000000000000000000000000}\"
stake_pi = 100000
bls_public_key = \"${BLS_KEYS[$((j-1))]:-000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000}\"
bls_pop = \"${BLS_POPS[$((j-1))]:-000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000}\"
"
        fi
    done

    # Build bootstrap peers (connect to node1 if not self)
    BOOTSTRAP=""
    if [[ $i -ne 1 ]]; then
        BOOTSTRAP="bootstrap_peers = [\"/ip4/127.0.0.1/tcp/$BASE_P2P_PORT\"]"
    else
        BOOTSTRAP="bootstrap_peers = []"
    fi

    cat > "$DATA_ROOT/node$i.toml" << TOMLEOF
data_dir = "$DATA_ROOT/node$i"
rpc_addr = "127.0.0.1:$RPC_PORT"
p2p_addr = "/ip4/127.0.0.1/tcp/$P2P_PORT"
chain_id = $CHAIN_ID
max_mempool_size = 10000
validator_stake_pi = 100000
log_level = "info"
$BOOTSTRAP
$GV_ENTRIES
TOMLEOF

    info "Config for node$i: RPC=$RPC_PORT, P2P=$P2P_PORT"
done

# --- Start nodes ---
info "Starting 4 validator nodes..."
for i in 1 2 3 4; do
    "$BINARY" run --config "$DATA_ROOT/node$i.toml" \
        > "$DATA_ROOT/node$i.log" 2>&1 &
    PIDS+=($!)
    info "Node $i started (PID=${PIDS[-1]})"
    sleep 1
done

# --- Wait for nodes to sync ---
info "Waiting for nodes to start and sync (30s)..."
sleep 30

# --- Verification Tests ---
echo ""
info "=== Running Verification Tests ==="
echo ""

# Test 1: All nodes responding
for i in 1 2 3 4; do
    PORT=$((BASE_RPC_PORT + i - 1))
    check "Node $i RPC responding" curl -sf "http://127.0.0.1:$PORT/health"
done

# Test 2: All nodes at same height (within 2 blocks)
HEIGHTS=()
for i in 1 2 3 4; do
    PORT=$((BASE_RPC_PORT + i - 1))
    H=$(curl -sf "http://127.0.0.1:$PORT/api/v1/status" 2>/dev/null | grep -oP '"height"\s*:\s*\K\d+' || echo "0")
    HEIGHTS+=("$H")
    info "Node $i height: $H"
done

# Check consensus: heights should be within 2 of each other
MAX_H=0
MIN_H=999999999
for h in "${HEIGHTS[@]}"; do
    [[ $h -gt $MAX_H ]] && MAX_H=$h
    [[ $h -lt $MIN_H ]] && MIN_H=$h
done
DIFF=$((MAX_H - MIN_H))
check "Consensus: heights within 2 blocks (diff=$DIFF)" test "$DIFF" -le 2
check "Consensus: chain is progressing (height > 0)" test "$MAX_H" -gt 0

# Test 3: Submit a transaction to node1 and check propagation
info "Submitting test transaction to node1..."
FAUCET_RESULT=$(curl -sf -X POST "http://127.0.0.1:$BASE_RPC_PORT/api/v1/faucet" \
    -H "Content-Type: application/json" \
    -d '{"address":"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"}' 2>/dev/null || echo '{"error":"no_faucet"}')
info "Faucet response: ${FAUCET_RESULT:0:100}"

# Wait for tx to propagate
sleep 5

# Check balance on all nodes (tx should propagate)
for i in 1 2 3 4; do
    PORT=$((BASE_RPC_PORT + i - 1))
    BAL=$(curl -sf "http://127.0.0.1:$PORT/api/v1/account/deadbeefdeadbeefdeadbeefdeadbeefdeadbeef" 2>/dev/null | grep -oP '"balance"\s*:\s*\K\d+' || echo "0")
    if [[ "$BAL" != "0" ]]; then
        check "Node $i sees faucet tx (balance=$BAL)" true
    else
        check "Node $i sees faucet tx" test "$BAL" != "0"
    fi
done

# --- Summary ---
echo ""
info "=== Test Summary ==="
info "Passed: $PASS"
if [[ $FAIL -gt 0 ]]; then
    fail "Failed: $FAIL"
else
    info "Failed: 0"
fi
echo ""
info "Logs available at: $DATA_ROOT/node*.log"

exit $FAIL
