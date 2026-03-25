#!/bin/bash
# PIChain Mainnet Readiness Test
# Runs ALL pre-launch checks automatically on a fresh chain.
# Usage: bash scripts/mainnet-readiness-test.sh

set -e
cd "$(dirname "$0")/.."

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'
PASS=0
FAIL=0

pass() { echo -e "  ${GREEN}PASS${NC} $1"; ((PASS++)); }
fail() { echo -e "  ${RED}FAIL${NC} $1"; ((FAIL++)); }
info() { echo -e "  ${YELLOW}....${NC} $1"; }

PORT=18314
RPC="http://127.0.0.1:$PORT"
DATA_DIR="/tmp/pichain-mainnet-test-$$"
WALLET1="$DATA_DIR/wallet1.json"
WALLET2="$DATA_DIR/wallet2.json"
NODE_PID=""
MINER_PID=""

cleanup() {
    [ -n "$MINER_PID" ] && kill $MINER_PID 2>/dev/null || true
    [ -n "$NODE_PID" ] && kill $NODE_PID 2>/dev/null || true
    sleep 2
    rm -rf "$DATA_DIR"
}
trap cleanup EXIT

echo ""
echo "========================================"
echo "  PIChain Mainnet Readiness Test"
echo "========================================"
echo ""

# ── 0. Build ──────────────────────────────────────────────────────────
echo "=== BUILD ==="
info "Building release binaries..."
cargo build --release -p pichain-node -p pichain-miner -p pichain-cli 2>&1 | tail -1
if [ ${PIPESTATUS[0]} -eq 0 ]; then pass "Release build"; else fail "Release build"; exit 1; fi

WARNINGS=$(cargo build --workspace 2>&1 | grep "warning:" | grep -v "generated\|Compiling\|Finished" | wc -l)
if [ "$WARNINGS" -eq 0 ]; then pass "Zero warnings"; else fail "$WARNINGS warnings"; fi

echo ""
echo "=== TESTS ==="
info "Running test suite..."
TEST_COUNT=$(cargo test --workspace 2>&1 | grep "test result: ok" | awk '{sum+=$4} END {print sum}')
FAILURES=$(cargo test --workspace 2>&1 | grep -c "FAILED" || true)
if [ "$FAILURES" -eq 0 ]; then pass "$TEST_COUNT tests passing"; else fail "$FAILURES test failures"; exit 1; fi

# ── 1. Fresh Genesis ─────────────────────────────────────────────────
echo ""
echo "=== 1. FRESH GENESIS ==="
mkdir -p "$DATA_DIR"
info "Starting node from genesis..."
./target/release/pichain run \
    --data-dir "$DATA_DIR" \
    --rpc-addr "127.0.0.1:$PORT" \
    --p2p-addr "/ip4/127.0.0.1/tcp/19314" \
    --chain-id 31415 \
    > "$DATA_DIR/node.log" 2>&1 &
NODE_PID=$!

# Wait for node to start
for i in $(seq 1 30); do
    if curl -s "$RPC/health" > /dev/null 2>&1; then break; fi
    sleep 1
done

HEALTH=$(curl -s "$RPC/health" 2>/dev/null)
if echo "$HEALTH" | grep -q '"status":"healthy"'; then
    pass "Node started from genesis"
else
    fail "Node failed to start"
    cat "$DATA_DIR/node.log" | tail -20
    exit 1
fi

HEIGHT=$(echo "$HEALTH" | python3 -c "import sys,json; print(json.load(sys.stdin)['block_height'])" 2>/dev/null)
if [ "$HEIGHT" -ge 0 ]; then pass "Genesis block height: $HEIGHT"; else fail "Bad genesis height"; fi

CHAIN_ID=$(echo "$HEALTH" | python3 -c "import sys,json; print(json.load(sys.stdin)['chain_id'])" 2>/dev/null)
if [ "$CHAIN_ID" = "31415" ]; then pass "Chain ID: 31415"; else fail "Wrong chain ID: $CHAIN_ID"; fi

# ── 2. Generate Wallets ──────────────────────────────────────────────
echo ""
echo "=== 2. WALLET GENERATION ==="
./target/release/pichain-miner --keypair "$WALLET1" --generate-keypair > /dev/null 2>&1
if [ -f "$WALLET1" ]; then pass "Wallet 1 created"; else fail "Wallet 1 creation failed"; fi

./target/release/pichain-cli wallet --output "$WALLET2" > /dev/null 2>&1
if [ -f "$WALLET2" ]; then pass "Wallet 2 created (CLI)"; else fail "Wallet 2 creation failed"; fi

# Extract addresses
ADDR1=$(python3 -c "import json; w=json.load(open('$WALLET1')); print(w.get('address',''))" 2>/dev/null)
ADDR2=$(python3 -c "import json; w=json.load(open('$WALLET2')); print(w.get('address',''))" 2>/dev/null)
ADDR1_HEX=$(echo "$ADDR1" | sed 's/^Pi314//')

if [ -n "$ADDR1_HEX" ] && [ ${#ADDR1_HEX} -eq 40 ]; then
    pass "Wallet 1 address: ${ADDR1:0:20}..."
else
    fail "Invalid wallet 1 address"
fi

# ── 3. Mining ────────────────────────────────────────────────────────
echo ""
echo "=== 3. MINING ==="
info "Starting miner (30 second test)..."
./target/release/pichain-miner \
    --keypair "$WALLET1" \
    --rpc-url "$RPC" \
    --profile desktop \
    --chain-id 31415 \
    > "$DATA_DIR/miner.log" 2>&1 &
MINER_PID=$!

sleep 30

# Check mining status
STATUS=$(curl -s "$RPC/api/v1/mining/status" 2>/dev/null)
FRONTIER=$(echo "$STATUS" | python3 -c "import sys,json; print(json.load(sys.stdin).get('frontier_position',0))" 2>/dev/null)
if [ "$FRONTIER" -gt 0 ]; then
    pass "Mining working — frontier at $FRONTIER digits"
else
    fail "Mining not advancing (frontier=$FRONTIER)"
fi

# Check miner balance
BALANCE=$(curl -s "$RPC/api/v1/account/$ADDR1_HEX" 2>/dev/null)
BAL_AMT=$(echo "$BALANCE" | python3 -c "import sys,json; print(json.load(sys.stdin).get('balance',0))" 2>/dev/null)
if [ "$BAL_AMT" -gt 0 ]; then
    BAL_PI=$(python3 -c "print(f'{$BAL_AMT / 1e9:.4f}')")
    pass "Miner earned $BAL_PI PI"
else
    fail "Miner balance is 0 after mining"
fi

# Check emission rate
RPD=$(echo "$STATUS" | python3 -c "import sys,json; print(json.load(sys.stdin).get('reward_per_digit',0))" 2>/dev/null)
if [ "$RPD" -gt 0 ]; then
    pass "Reward per digit: $RPD base units"
else
    fail "Reward per digit is 0"
fi

# ── 4. Send PI ───────────────────────────────────────────────────────
echo ""
echo "=== 4. SEND PI ==="
ADDR2_HEX=$(echo "$ADDR2" | sed 's/^Pi314//')

info "Sending 10 PI from wallet1 to wallet2..."
SEND_RESULT=$(./target/release/pichain-cli send \
    --wallet "$WALLET1" \
    --to "$ADDR2" \
    --amount 10 \
    --rpc-url "$RPC" \
    --chain-id 31415 2>&1)

if echo "$SEND_RESULT" | grep -q "SUCCESS"; then
    pass "Transfer submitted"
else
    fail "Transfer failed: $SEND_RESULT"
fi

# Wait for block inclusion
sleep 2

# Check recipient balance
RECV_BAL=$(curl -s "$RPC/api/v1/account/$ADDR2_HEX" 2>/dev/null)
RECV_AMT=$(echo "$RECV_BAL" | python3 -c "import sys,json; print(json.load(sys.stdin).get('balance',0))" 2>/dev/null)
EXPECTED=$((10 * 1000000000))
if [ "$RECV_AMT" -eq "$EXPECTED" ]; then
    pass "Recipient received exactly 10 PI"
elif [ "$RECV_AMT" -gt 0 ]; then
    pass "Recipient received $(python3 -c "print($RECV_AMT / 1e9)") PI"
else
    fail "Recipient balance is 0"
fi

# ── 5. Edge Cases ────────────────────────────────────────────────────
echo ""
echo "=== 5. EDGE CASES ==="

# Send more than balance
info "Trying to send more PI than available..."
OVERSEND=$(./target/release/pichain-cli send \
    --wallet "$WALLET2" \
    --to "$ADDR1" \
    --amount 999999999 \
    --rpc-url "$RPC" \
    --chain-id 31415 2>&1 || true)
if echo "$OVERSEND" | grep -qi "error\|reject\|fail\|insufficient"; then
    pass "Overspend correctly rejected"
else
    fail "Overspend was NOT rejected: $OVERSEND"
fi

# Send 0 PI
info "Trying to send 0 PI..."
ZERO_SEND=$(./target/release/pichain-cli send \
    --wallet "$WALLET1" \
    --to "$ADDR2" \
    --amount 0 \
    --rpc-url "$RPC" \
    --chain-id 31415 2>&1 || true)
if echo "$ZERO_SEND" | grep -qi "error\|reject\|must be"; then
    pass "Zero amount correctly rejected"
else
    fail "Zero amount was NOT rejected"
fi

# ── 6. Browser Pool Mining ───────────────────────────────────────────
echo ""
echo "=== 6. POOL MINING ENDPOINT ==="
POOL_RESP=$(curl -s -X POST "$RPC/api/v1/mining/pool-submit" \
    -H "Content-Type: application/json" \
    -d "{\"miner_address\":\"$ADDR1_HEX\",\"start_position\":0,\"digit_count\":0,\"digits\":[],\"pow_nonce\":0,\"anchor_block_hash\":\"0000000000000000000000000000000000000000000000000000000000000000\"}" 2>/dev/null)
if echo "$POOL_RESP" | grep -q "error\|empty"; then
    pass "Pool endpoint rejects empty proof"
else
    fail "Pool endpoint accepted empty proof: $POOL_RESP"
fi

# ── 7. Node Restart ─────────────────────────────────────────────────
echo ""
echo "=== 7. NODE RESTART ==="
PRE_HEIGHT=$(curl -s "$RPC/health" | python3 -c "import sys,json; print(json.load(sys.stdin)['block_height'])" 2>/dev/null)
info "Killing node at height $PRE_HEIGHT..."
kill $MINER_PID 2>/dev/null; MINER_PID=""
kill $NODE_PID 2>/dev/null; NODE_PID=""
sleep 3

info "Restarting node..."
./target/release/pichain run \
    --data-dir "$DATA_DIR" \
    --rpc-addr "127.0.0.1:$PORT" \
    --p2p-addr "/ip4/127.0.0.1/tcp/19314" \
    --chain-id 31415 \
    > "$DATA_DIR/node2.log" 2>&1 &
NODE_PID=$!

for i in $(seq 1 30); do
    if curl -s "$RPC/health" > /dev/null 2>&1; then break; fi
    sleep 1
done

POST_HEALTH=$(curl -s "$RPC/health" 2>/dev/null)
POST_HEIGHT=$(echo "$POST_HEALTH" | python3 -c "import sys,json; print(json.load(sys.stdin)['block_height'])" 2>/dev/null)

if [ "$POST_HEIGHT" -ge "$PRE_HEIGHT" ]; then
    pass "Node resumed at height $POST_HEIGHT (was $PRE_HEIGHT)"
else
    fail "Node lost data: was $PRE_HEIGHT, now $POST_HEIGHT"
fi

# Check balance survived restart
POST_BAL=$(curl -s "$RPC/api/v1/account/$ADDR1_HEX" 2>/dev/null)
POST_BAL_AMT=$(echo "$POST_BAL" | python3 -c "import sys,json; print(json.load(sys.stdin).get('balance',0))" 2>/dev/null)
if [ "$POST_BAL_AMT" -gt 0 ]; then
    pass "Balance survived restart: $(python3 -c "print(f'{$POST_BAL_AMT / 1e9:.4f}')") PI"
else
    fail "Balance lost after restart"
fi

# ── 8. API Endpoints ────────────────────────────────────────────────
echo ""
echo "=== 8. API ENDPOINTS ==="
for endpoint in "health" "api/v1/mining/status" "api/v1/block/latest" "api/v1/info"; do
    RESP=$(curl -s -o /dev/null -w "%{http_code}" "$RPC/$endpoint" 2>/dev/null)
    if [ "$RESP" = "200" ]; then pass "GET /$endpoint → 200"; else fail "GET /$endpoint → $RESP"; fi
done

# ── 9. Supply Check ─────────────────────────────────────────────────
echo ""
echo "=== 9. SUPPLY VERIFICATION ==="
MINING_STATUS=$(curl -s "$RPC/api/v1/mining/status" 2>/dev/null)
REMAINING=$(echo "$MINING_STATUS" | python3 -c "import sys,json; print(json.load(sys.stdin).get('remaining_pool',0))" 2>/dev/null)
POOL=$((2670353755 * 1000000000))
if [ "$REMAINING" -gt 0 ]; then
    REMAINING_PI=$(python3 -c "print(f'{$REMAINING / 1e9:,.0f}')")
    pass "Mining pool remaining: $REMAINING_PI PI"
else
    fail "Mining pool is 0"
fi

# Check emission year
YEAR=$(echo "$MINING_STATUS" | python3 -c "import sys,json; print(json.load(sys.stdin).get('emission_year',0))" 2>/dev/null)
if [ "$YEAR" -eq 1 ]; then pass "Emission year: 1 (correct for new chain)"; else fail "Emission year: $YEAR"; fi

# ── SUMMARY ──────────────────────────────────────────────────────────
echo ""
echo "========================================"
echo "  RESULTS: $PASS passed, $FAIL failed"
echo "========================================"
echo ""

if [ "$FAIL" -eq 0 ]; then
    echo -e "  ${GREEN}ALL CHECKS PASSED — MAINNET READY${NC}"
    echo ""
    exit 0
else
    echo -e "  ${RED}$FAIL FAILURES — DO NOT LAUNCH${NC}"
    echo ""
    exit 1
fi
