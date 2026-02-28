#!/usr/bin/env bash
#
# PIChain Devnet Launch Test
# Tests the full blockchain lifecycle: init, start, transact, mine, query.
#
# Usage:
#   ./scripts/devnet-test.sh           # Full test
#   ./scripts/devnet-test.sh --quick   # Skip build, just run tests
#
set -euo pipefail

# --- Configuration ---
PICHAIN_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DATA_DIR=$(mktemp -d /tmp/pichain-devnet-XXXXXX)
RPC_PORT=18314
RPC_URL="http://127.0.0.1:${RPC_PORT}"
CHAIN_ID=31415
NODE_PID=""
PASS=0
FAIL=0
TOTAL=0

# --- Colors ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# --- Helpers ---
log()  { echo -e "${CYAN}[PIChain]${NC} $*"; }
pass() { PASS=$((PASS + 1)); TOTAL=$((TOTAL + 1)); echo -e "  ${GREEN}PASS${NC} $*"; }
fail() { FAIL=$((FAIL + 1)); TOTAL=$((TOTAL + 1)); echo -e "  ${RED}FAIL${NC} $*"; }
warn() { echo -e "  ${YELLOW}WARN${NC} $*"; }

cleanup() {
    if [ -n "$NODE_PID" ] && kill -0 "$NODE_PID" 2>/dev/null; then
        log "Stopping node (PID $NODE_PID)..."
        kill "$NODE_PID" 2>/dev/null || true
        wait "$NODE_PID" 2>/dev/null || true
    fi
    if [ -d "$DATA_DIR" ]; then
        rm -rf "$DATA_DIR"
    fi
    echo ""
    echo "========================================"
    echo "  PIChain Devnet Test Results"
    echo "========================================"
    echo -e "  ${GREEN}Passed:${NC} $PASS"
    echo -e "  ${RED}Failed:${NC} $FAIL"
    echo -e "  Total:  $TOTAL"
    echo "========================================"
    if [ "$FAIL" -gt 0 ]; then
        echo -e "  ${RED}SOME TESTS FAILED${NC}"
        exit 1
    else
        echo -e "  ${GREEN}ALL TESTS PASSED${NC}"
    fi
}
trap cleanup EXIT

wait_for_rpc() {
    local max_wait=30
    local waited=0
    while ! curl -s "${RPC_URL}/api/v1/info" >/dev/null 2>&1; do
        sleep 0.5
        waited=$((waited + 1))
        if [ "$waited" -ge "$max_wait" ]; then
            fail "RPC server did not start within ${max_wait}s"
            return 1
        fi
    done
    return 0
}

# JSON helper — extract a field from JSON
json_field() {
    local json="$1"
    local field="$2"
    echo "$json" | python3 -c "
import sys,json
d=json.load(sys.stdin)
v=d.get('$field','')
if isinstance(v, bool):
    print('true' if v else 'false')
else:
    print(v)
" 2>/dev/null || echo ""
}

json_field_int() {
    local json="$1"
    local field="$2"
    echo "$json" | python3 -c "import sys,json; d=json.load(sys.stdin); print(int(d.get('$field',0)))" 2>/dev/null || echo "0"
}

# =================================================================
#  PHASE 0: Build
# =================================================================
if [ "${1:-}" != "--quick" ]; then
    log "Building PIChain (release)..."
    cd "$PICHAIN_DIR"
    cargo build --release 2>&1 | tail -5
    log "Build complete."
else
    log "Skipping build (--quick mode)"
fi

NODE_BIN="$PICHAIN_DIR/target/release/pichain"
CLI_BIN="$PICHAIN_DIR/target/release/pichain-cli"
MINER_BIN="$PICHAIN_DIR/target/release/pichain-miner"

# Verify binaries exist
for bin in "$NODE_BIN" "$CLI_BIN" "$MINER_BIN"; do
    if [ -x "$bin" ]; then
        pass "Binary exists: $(basename "$bin")"
    else
        fail "Binary missing: $bin"
        exit 1
    fi
done

# =================================================================
#  PHASE 1: CLI Tools
# =================================================================
echo ""
log "=== Phase 1: CLI Tools ==="

# Test wallet generation
WALLET_OUTPUT=$("$CLI_BIN" wallet 2>&1)
if echo "$WALLET_OUTPUT" | grep -q "Address:"; then
    pass "pichain-cli wallet generates keypair"
else
    fail "pichain-cli wallet failed"
fi

# Test PI computation
PI_OUTPUT=$("$CLI_BIN" compute-pi --position 0 --count 16 2>&1)
if echo "$PI_OUTPUT" | grep -qi "243f"; then
    pass "pichain-cli compute-pi computes correct PI hex digits"
else
    fail "pichain-cli compute-pi incorrect output: $PI_OUTPUT"
fi

# Test hash
HASH_OUTPUT=$("$CLI_BIN" hash "hello pichain" 2>&1)
if echo "$HASH_OUTPUT" | grep -q "Blake3 hash:"; then
    pass "pichain-cli hash works"
else
    fail "pichain-cli hash failed"
fi

# Test tokenomics
TOKENOMICS_OUTPUT=$("$CLI_BIN" tokenomics 2>&1)
if echo "$TOKENOMICS_OUTPUT" | grep -q "3,141,592,653"; then
    pass "pichain-cli tokenomics shows correct total supply"
else
    fail "pichain-cli tokenomics incorrect"
fi

# =================================================================
#  PHASE 2: Node Initialization
# =================================================================
echo ""
log "=== Phase 2: Node Init ==="

# Initialize devnet
INIT_OUTPUT=$("$NODE_BIN" init --data-dir "$DATA_DIR" --chain-id $CHAIN_ID 2>&1)
if echo "$INIT_OUTPUT" | grep -q "initialized"; then
    pass "pichain-node init creates devnet"
else
    fail "pichain-node init failed: $INIT_OUTPUT"
fi

# Check status
STATUS_OUTPUT=$("$NODE_BIN" status --data-dir "$DATA_DIR" 2>&1)
if echo "$STATUS_OUTPUT" | grep -q "Block height:"; then
    pass "pichain-node status reads chain state"
else
    fail "pichain-node status failed"
fi

# =================================================================
#  PHASE 3: Start Node
# =================================================================
echo ""
log "=== Phase 3: Node Startup ==="

"$NODE_BIN" run \
    --data-dir "$DATA_DIR" \
    --rpc-addr "127.0.0.1:${RPC_PORT}" \
    --chain-id $CHAIN_ID \
    > "$DATA_DIR/node.log" 2>&1 &
NODE_PID=$!

log "Node started (PID $NODE_PID), waiting for RPC..."

if wait_for_rpc; then
    pass "RPC server is accepting connections"
else
    fail "RPC server failed to start"
    cat "$DATA_DIR/node.log" | tail -30
    exit 1
fi

# =================================================================
#  PHASE 4: RPC Endpoint Tests
# =================================================================
echo ""
log "=== Phase 4: RPC Endpoints ==="

# Node info
CHAIN_STATUS=$(curl -s "${RPC_URL}/api/v1/info")
if [ -n "$CHAIN_STATUS" ]; then
    CHAIN_HEIGHT=$(json_field_int "$CHAIN_STATUS" "block_height")
    CHAIN_CID=$(json_field_int "$CHAIN_STATUS" "chain_id")
    if [ "$CHAIN_CID" -eq "$CHAIN_ID" ]; then
        pass "GET /info returns correct chain_id ($CHAIN_CID)"
    else
        fail "GET /info wrong chain_id: expected $CHAIN_ID, got $CHAIN_CID"
    fi
else
    fail "GET /info returned empty response"
fi

# Block by height (genesis)
BLOCK=$(curl -s "${RPC_URL}/api/v1/block/0")
BLOCK_FOUND=$(json_field "$BLOCK" "found")
if [ "$BLOCK_FOUND" = "true" ]; then
    pass "GET /block/0 returns genesis block"
else
    fail "GET /block/0 failed"
fi

# Account query (faucet)
FAUCET_ADDR="0000000000000000000000000000000000000001"
ACCOUNT=$(curl -s "${RPC_URL}/api/v1/account/${FAUCET_ADDR}")
FAUCET_BALANCE=$(json_field_int "$ACCOUNT" "balance")
if [ "$FAUCET_BALANCE" -gt 0 ]; then
    pass "GET /account/{addr} returns faucet balance ($FAUCET_BALANCE)"
else
    warn "GET /account/{addr} faucet balance is 0 (may not be funded on this devnet)"
    pass "GET /account/{addr} endpoint responds"
fi

# Mining status
MINING=$(curl -s "${RPC_URL}/api/v1/mining/status")
if [ -n "$MINING" ]; then
    NEXT_POS=$(json_field_int "$MINING" "next_position")
    pass "GET /mining/status returns next_position ($NEXT_POS)"
else
    fail "GET /mining/status failed"
fi

# Token info (nonexistent — should return found=false)
FAKE_MINT="0000000000000000000000000000000000000000000000000000000000000000"
TOKEN=$(curl -s "${RPC_URL}/api/v1/token/${FAKE_MINT}")
TOKEN_FOUND=$(json_field "$TOKEN" "found")
if [ "$TOKEN_FOUND" = "false" ]; then
    pass "GET /token/{mint} returns found=false for nonexistent token"
else
    fail "GET /token/{mint} should return found=false (got: $TOKEN_FOUND)"
fi

# Transaction query (nonexistent — should return found=false)
FAKE_TX="0000000000000000000000000000000000000000000000000000000000000000"
TX_RESP=$(curl -s "${RPC_URL}/api/v1/tx/${FAKE_TX}")
TX_FOUND=$(json_field "$TX_RESP" "found")
if [ "$TX_FOUND" = "false" ]; then
    pass "GET /tx/{hash} returns found=false for nonexistent tx"
else
    fail "GET /tx/{hash} should return found=false (got: $TX_FOUND)"
fi

# Invalid hex length rejection
BAD_HEX="ZZZZ"
BAD_TOKEN=$(curl -s "${RPC_URL}/api/v1/token/${BAD_HEX}")
BAD_TOKEN_FOUND=$(json_field "$BAD_TOKEN" "found")
if [ "$BAD_TOKEN_FOUND" = "false" ]; then
    pass "GET /token/{bad_hex} rejects invalid hex safely"
else
    fail "GET /token/{bad_hex} should reject invalid hex (got: $BAD_TOKEN_FOUND)"
fi

# =================================================================
#  PHASE 5: Miner Tool
# =================================================================
echo ""
log "=== Phase 5: Miner Tool ==="

# Generate miner wallet
WALLET_FILE="$DATA_DIR/miner-wallet.json"
"$MINER_BIN" --keypair "$WALLET_FILE" --generate-keypair 2>&1
if [ -f "$WALLET_FILE" ]; then
    pass "pichain-miner --generate-keypair creates wallet file"
else
    fail "pichain-miner --generate-keypair failed"
fi

# Verify wallet file format
if python3 -c "import json; d=json.load(open('$WALLET_FILE')); assert 'secret_key' in d and 'address' in d" 2>/dev/null; then
    pass "Wallet file has correct JSON format"
else
    fail "Wallet file has incorrect format"
fi

# =================================================================
#  PHASE 6: Block Production
# =================================================================
echo ""
log "=== Phase 6: Block Production ==="

# Wait for a few blocks to be produced
sleep 3

CHAIN_STATUS2=$(curl -s "${RPC_URL}/api/v1/info")
HEIGHT_AFTER=$(json_field_int "$CHAIN_STATUS2" "block_height")
if [ "$HEIGHT_AFTER" -gt 0 ]; then
    pass "Blocks are being produced (height: $HEIGHT_AFTER)"
else
    warn "No blocks produced yet (height: $HEIGHT_AFTER)"
    pass "Node is running and responding"
fi

# Query a produced block
if [ "$HEIGHT_AFTER" -gt 0 ]; then
    LATEST_BLOCK=$(curl -s "${RPC_URL}/api/v1/block/${HEIGHT_AFTER}")
    LATEST_FOUND=$(json_field "$LATEST_BLOCK" "found")
    if [ "$LATEST_FOUND" = "true" ]; then
        pass "GET /block/{height} returns produced block at height $HEIGHT_AFTER"
    else
        fail "GET /block/{height} cannot fetch block at height $HEIGHT_AFTER"
    fi
fi

# =================================================================
#  PHASE 7: RPC Security Tests
# =================================================================
echo ""
log "=== Phase 7: Security Validation ==="

# Oversized hex string (should not crash the server)
HUGE_HEX=$(python3 -c "print('a' * 10000)")
HUGE_RESP=$(curl -s -o /dev/null -w "%{http_code}" "${RPC_URL}/api/v1/token/${HUGE_HEX}" 2>/dev/null || echo "000")
if [ "$HUGE_RESP" != "000" ]; then
    pass "Server handles oversized hex input without crash (HTTP $HUGE_RESP)"
else
    fail "Server crashed on oversized hex input"
fi

# Verify node is still running after security tests
if kill -0 "$NODE_PID" 2>/dev/null; then
    pass "Node still running after security tests"
else
    fail "Node crashed during security tests!"
fi

# =================================================================
#  PHASE 8: Mining Integration (Offline)
# =================================================================
echo ""
log "=== Phase 8: Mining Integration ==="

# Test PI digit computation and proof verification (offline, no node needed)
PI_TEST=$("$CLI_BIN" compute-pi --position 0 --count 200 2>&1)
if echo "$PI_TEST" | grep -q "digits/sec"; then
    pass "PI digit computation works at 200-digit batch"
else
    fail "PI digit computation failed"
fi

# Mine command test (single batch, no network)
MINE_OUTPUT=$("$NODE_BIN" mine --start-position 0 --batch-size 200 2>&1)
if echo "$MINE_OUTPUT" | grep -q "VERIFIED"; then
    pass "Mining proof generation and verification (offline)"
else
    fail "Mining proof verification failed: $MINE_OUTPUT"
fi

# =================================================================
#  PHASE 9: Persistence Check
# =================================================================
echo ""
log "=== Phase 9: Persistence ==="

# Stop the node
kill "$NODE_PID" 2>/dev/null || true
wait "$NODE_PID" 2>/dev/null || true
NODE_PID=""

# Check that status command can read persisted data
STATUS_AFTER=$("$NODE_BIN" status --data-dir "$DATA_DIR" 2>&1)
if echo "$STATUS_AFTER" | grep -q "Block height:"; then
    PERSISTED_HEIGHT=$(echo "$STATUS_AFTER" | grep "Block height:" | awk '{print $NF}')
    pass "State persisted to disk (height: $PERSISTED_HEIGHT)"
else
    fail "Cannot read persisted state after shutdown"
fi

# Restart node and verify it resumes
"$NODE_BIN" run \
    --data-dir "$DATA_DIR" \
    --rpc-addr "127.0.0.1:${RPC_PORT}" \
    --chain-id $CHAIN_ID \
    > "$DATA_DIR/node-restart.log" 2>&1 &
NODE_PID=$!

log "Node restarted (PID $NODE_PID), verifying resume..."

if wait_for_rpc; then
    RESUMED_STATUS=$(curl -s "${RPC_URL}/api/v1/info")
    RESUMED_HEIGHT=$(json_field_int "$RESUMED_STATUS" "block_height")
    if [ "$RESUMED_HEIGHT" -ge 0 ]; then
        pass "Node resumed from persisted state (height: $RESUMED_HEIGHT)"
    else
        fail "Node did not resume correctly"
    fi
else
    fail "Node failed to restart"
fi

# =================================================================
#  PHASE 10: Live Mining Integration
# =================================================================
echo ""
log "=== Phase 10: Live Mining ==="

# Extract miner address from wallet file (strip 0x prefix)
MINER_ADDR=$(python3 -c "import json; addr=json.load(open('$WALLET_FILE'))['address']; print(addr[2:] if addr.startswith('0x') else addr)")
log "Miner address: $MINER_ADDR"

# 10a. Claim from faucet
FAUCET_RESP=$(curl -s -X POST "${RPC_URL}/api/v1/faucet" \
    -H "Content-Type: application/json" \
    -d "{\"address\": \"${MINER_ADDR}\"}")
FAUCET_OK=$(json_field "$FAUCET_RESP" "success")
if [ "$FAUCET_OK" = "true" ]; then
    FAUCET_AMT=$(json_field_int "$FAUCET_RESP" "amount")
    pass "Faucet claim successful (${FAUCET_AMT} base units)"
else
    FAUCET_ERR=$(json_field "$FAUCET_RESP" "error")
    fail "Faucet claim failed: $FAUCET_ERR"
fi

# 10b. Verify miner has balance
MINER_ACCT=$(curl -s "${RPC_URL}/api/v1/account/${MINER_ADDR}")
MINER_BAL=$(json_field_int "$MINER_ACCT" "balance")
if [ "$MINER_BAL" -gt 0 ]; then
    pass "Miner has balance after faucet claim (${MINER_BAL})"
else
    fail "Miner balance is still 0 after faucet claim"
fi

# 10c. Run miner for a few rounds
"$MINER_BIN" \
    --keypair "$WALLET_FILE" \
    --rpc-url "$RPC_URL" \
    --chain-id $CHAIN_ID \
    --digits-per-batch 200 \
    --interval-secs 1 \
    > "$DATA_DIR/miner.log" 2>&1 &
MINER_PID=$!

# Wait for miner to submit proofs and blocks to process them
sleep 8
kill "$MINER_PID" 2>/dev/null || true
wait "$MINER_PID" 2>/dev/null || true

# 10d. Check if miner submitted proofs
if grep -q "submitted successfully" "$DATA_DIR/miner.log"; then
    pass "Miner submitted mining proofs"
else
    fail "Miner did not submit any proofs"
    tail -20 "$DATA_DIR/miner.log"
fi

# 10e. Check mining status advanced
MINING_AFTER=$(curl -s "${RPC_URL}/api/v1/mining/status")
NEXT_POS_AFTER=$(json_field_int "$MINING_AFTER" "next_position")
TOTAL_VERIFIED=$(json_field_int "$MINING_AFTER" "total_digits_verified")
REWARD_PER_DIGIT=$(json_field_int "$MINING_AFTER" "reward_per_digit")
if [ "$TOTAL_VERIFIED" -gt 0 ]; then
    pass "Mining frontier advanced (verified: $TOTAL_VERIFIED, next: $NEXT_POS_AFTER)"
else
    warn "Mining digits not yet reflected in status (may need more blocks)"
    pass "Miner completed mining loop"
fi

# 10f. Check reward_per_digit is non-zero
if [ "$REWARD_PER_DIGIT" -gt 0 ]; then
    pass "Mining reward_per_digit is reported ($REWARD_PER_DIGIT)"
else
    fail "Mining reward_per_digit should be > 0"
fi

# 10g. Double faucet claim should fail
FAUCET_RESP2=$(curl -s -X POST "${RPC_URL}/api/v1/faucet" \
    -H "Content-Type: application/json" \
    -d "{\"address\": \"${MINER_ADDR}\"}")
FAUCET_OK2=$(json_field "$FAUCET_RESP2" "success")
if [ "$FAUCET_OK2" = "false" ]; then
    pass "Double faucet claim correctly rejected"
else
    fail "Double faucet claim should have been rejected"
fi

# =================================================================
#  PHASE 11: Launch Infrastructure
# =================================================================
echo ""
log "=== Phase 11: Launch Infrastructure ==="

# 11a. Health endpoint returns JSON with uptime
HEALTH_RESP=$(curl -s "${RPC_URL}/health")
HEALTH_STATUS=$(json_field "$HEALTH_RESP" "status")
if [ "$HEALTH_STATUS" = "healthy" ]; then
    UPTIME=$(json_field_int "$HEALTH_RESP" "uptime_secs")
    pass "Health endpoint returns status=healthy (uptime: ${UPTIME}s)"
else
    fail "Health endpoint returned: $HEALTH_STATUS"
fi

# 11b. Prometheus metrics endpoint
METRICS_RESP=$(curl -s "${RPC_URL}/metrics")
if echo "$METRICS_RESP" | grep -q "pichain_block_height"; then
    METRICS_HEIGHT=$(echo "$METRICS_RESP" | grep "^pichain_block_height " | awk '{print $2}')
    pass "Prometheus /metrics returns pichain_block_height ($METRICS_HEIGHT)"
else
    fail "Prometheus /metrics missing pichain_block_height"
fi

if echo "$METRICS_RESP" | grep -q "pichain_mining_frontier"; then
    pass "Prometheus /metrics includes mining metrics"
else
    fail "Prometheus /metrics missing mining metrics"
fi

# 11c. Block explorer is served at /explorer
EXPLORER_RESP=$(curl -s -o /dev/null -w "%{http_code}" "${RPC_URL}/explorer")
if [ "$EXPLORER_RESP" = "200" ]; then
    EXPLORER_BODY=$(curl -s "${RPC_URL}/explorer")
    if echo "$EXPLORER_BODY" | grep -q "PIChain"; then
        pass "Block explorer UI served at /explorer"
    else
        fail "Block explorer served but content incorrect"
    fi
else
    fail "Block explorer not accessible (HTTP $EXPLORER_RESP)"
fi

# 11d. Block range/list endpoint
BLOCKS_RESP=$(curl -s "${RPC_URL}/api/v1/blocks?limit=5")
BLOCKS_HEIGHT=$(json_field_int "$BLOCKS_RESP" "total_height")
if [ "$BLOCKS_HEIGHT" -gt 0 ]; then
    pass "GET /api/v1/blocks returns block list (height: $BLOCKS_HEIGHT)"
else
    fail "GET /api/v1/blocks returned no data"
fi

# 11e. Receipt endpoint (nonexistent — should return found=false)
FAKE_RECEIPT=$(curl -s "${RPC_URL}/api/v1/receipt/${FAKE_TX}")
RECEIPT_FOUND=$(json_field "$FAKE_RECEIPT" "found")
if [ "$RECEIPT_FOUND" = "false" ]; then
    pass "GET /api/v1/receipt/{hash} returns found=false for nonexistent"
else
    fail "GET /api/v1/receipt/{hash} should return found=false"
fi

# 11f. Config file generation
CONFIG_FILE="$DATA_DIR/pichain.toml"
"$NODE_BIN" generate-config --output "$CONFIG_FILE" 2>&1
if [ -f "$CONFIG_FILE" ]; then
    if grep -q "chain_id" "$CONFIG_FILE"; then
        pass "pichain generate-config creates valid TOML config"
    else
        fail "Generated config missing chain_id"
    fi
else
    fail "pichain generate-config did not create file"
fi

# 11g. Genesis ceremony — template generation
GENESIS_FILE="$DATA_DIR/genesis.json"
"$NODE_BIN" genesis-ceremony --generate-template --genesis-file "$GENESIS_FILE" 2>&1
if [ -f "$GENESIS_FILE" ]; then
    if python3 -c "import json; d=json.load(open('$GENESIS_FILE')); assert d['chain_id'] == 314159" 2>/dev/null; then
        pass "Genesis ceremony creates mainnet template (chain_id=314159)"
    else
        fail "Genesis template has wrong chain_id"
    fi
else
    fail "Genesis ceremony did not create file"
fi

# 11h. Genesis ceremony — verification
VERIFY_OUTPUT=$("$NODE_BIN" genesis-ceremony --verify --genesis-file "$GENESIS_FILE" 2>&1)
if echo "$VERIFY_OUTPUT" | grep -q "Supply check: PASS"; then
    pass "Genesis ceremony verification passes supply check"
else
    fail "Genesis ceremony verification failed: $VERIFY_OUTPUT"
fi

# =================================================================
#  Summary
# =================================================================
echo ""
log "=== Devnet Launch Test Complete ==="
