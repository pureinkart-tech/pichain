#!/usr/bin/env bash
#
# PIChain Multi-Node Integration Test
# Tests that 3 validators can connect, produce blocks, and sync.
#
# Usage:
#   ./scripts/multi-node-test.sh           # Full test (build + run)
#   ./scripts/multi-node-test.sh --quick   # Skip build
#
# Prerequisites:
#   - Docker and docker compose installed
#   - Devnet keys generated: ./scripts/generate-devnet-keys.sh
#
set -euo pipefail

PICHAIN_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PICHAIN_DIR"

# --- Colors ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

PASS=0
FAIL=0
TOTAL=0

# --- Helpers ---
check() {
    local name="$1"
    local result="$2"
    TOTAL=$((TOTAL + 1))
    if [ "$result" = "true" ]; then
        echo -e "  ${GREEN}PASS${NC} ${name}"
        PASS=$((PASS + 1))
    else
        echo -e "  ${RED}FAIL${NC} ${name}"
        FAIL=$((FAIL + 1))
    fi
}

rpc_call() {
    local port="$1"
    local method="$2"
    local params="${3:-null}"
    curl -s -m 5 "http://localhost:${port}" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"method\":\"${method}\",\"params\":${params},\"id\":1}" 2>/dev/null || echo '{"error":"connection_refused"}'
}

get_height() {
    local port="$1"
    local resp
    resp=$(rpc_call "$port" "pi_blockHeight")
    echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('result',{}).get('height',0))" 2>/dev/null || echo "0"
}

get_peer_count() {
    local port="$1"
    local resp
    resp=$(rpc_call "$port" "pi_nodeInfo")
    echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('result',{}).get('peer_count',0))" 2>/dev/null || echo "0"
}

# --- Main ---
echo -e "${CYAN}=== PIChain Multi-Node Integration Test ===${NC}"
echo ""

# Step 0: Check prerequisites
if ! command -v docker &>/dev/null; then
    echo -e "${RED}Docker not found. Install Docker first.${NC}"
    exit 1
fi

# Step 1: Build (unless --quick)
if [ "${1:-}" != "--quick" ]; then
    echo -e "${CYAN}Step 1: Building PIChain...${NC}"
    cargo build --release 2>&1 | tail -3
    echo ""
fi

# Step 2: Generate devnet keys (if not already done)
if [ ! -f "${PICHAIN_DIR}/devnet-config/node1.toml" ]; then
    echo -e "${CYAN}Step 2: Generating devnet keys...${NC}"
    ./scripts/generate-devnet-keys.sh
    echo ""
else
    echo -e "${CYAN}Step 2: Devnet config already exists, skipping key generation${NC}"
fi

# Step 3: Start the devnet
echo -e "${CYAN}Step 3: Starting 3-node devnet...${NC}"
docker compose down -v 2>/dev/null || true
docker compose up -d --build 2>&1 | tail -5
echo ""

# Step 4: Wait for nodes to start
echo -e "${CYAN}Step 4: Waiting for nodes to start (30s)...${NC}"
sleep 30

# Step 5: Check all nodes are responsive
echo -e "${CYAN}Step 5: Checking node responsiveness...${NC}"
for port in 8314 8315 8316; do
    resp=$(rpc_call "$port" "pi_blockHeight")
    is_responsive=$(echo "$resp" | python3 -c "import sys,json; d=json.load(sys.stdin); print('true' if 'result' in d else 'false')" 2>/dev/null || echo "false")
    check "Node on port ${port} is responsive" "$is_responsive"
done
echo ""

# Step 6: Check peer connections
echo -e "${CYAN}Step 6: Checking peer connections...${NC}"
for port in 8314 8315 8316; do
    peers=$(get_peer_count "$port")
    has_peers=$([ "$peers" -ge 1 ] 2>/dev/null && echo "true" || echo "false")
    check "Node on port ${port} has peers (count: ${peers})" "$has_peers"
done
echo ""

# Step 7: Check block production
echo -e "${CYAN}Step 7: Checking block production (waiting 20s)...${NC}"
sleep 20

h1=$(get_height 8314)
h2=$(get_height 8315)
h3=$(get_height 8316)
echo "  Heights: node1=${h1}, node2=${h2}, node3=${h3}"

check "Node 1 has produced blocks (height=${h1})" "$([ "$h1" -gt 0 ] 2>/dev/null && echo true || echo false)"
check "Node 2 has blocks (height=${h2})" "$([ "$h2" -gt 0 ] 2>/dev/null && echo true || echo false)"
check "Node 3 has blocks (height=${h3})" "$([ "$h3" -gt 0 ] 2>/dev/null && echo true || echo false)"
echo ""

# Step 8: Check height consensus (all within 2 blocks)
echo -e "${CYAN}Step 8: Checking height consensus...${NC}"
max_h=$h1
[ "$h2" -gt "$max_h" ] 2>/dev/null && max_h=$h2
[ "$h3" -gt "$max_h" ] 2>/dev/null && max_h=$h3

diff1=$((max_h - h1))
diff2=$((max_h - h2))
diff3=$((max_h - h3))

check "Node 1 within 2 blocks of tip (diff=${diff1})" "$([ "$diff1" -le 2 ] 2>/dev/null && echo true || echo false)"
check "Node 2 within 2 blocks of tip (diff=${diff2})" "$([ "$diff2" -le 2 ] 2>/dev/null && echo true || echo false)"
check "Node 3 within 2 blocks of tip (diff=${diff3})" "$([ "$diff3" -le 2 ] 2>/dev/null && echo true || echo false)"
echo ""

# Step 9: Test sync after restart
echo -e "${CYAN}Step 9: Testing sync after restart...${NC}"
echo "  Stopping node 3..."
docker compose stop node3 2>/dev/null

echo "  Waiting 15s for blocks to accumulate..."
sleep 15

h1_before=$(get_height 8314)
echo "  Node 1 height before restart: ${h1_before}"

echo "  Restarting node 3..."
docker compose start node3 2>/dev/null

echo "  Waiting 30s for sync..."
sleep 30

h3_after=$(get_height 8316)
h1_after=$(get_height 8314)
sync_diff=$((h1_after - h3_after))

echo "  After sync: node1=${h1_after}, node3=${h3_after}, diff=${sync_diff}"
check "Node 3 synced after restart (within 3 blocks, diff=${sync_diff})" \
    "$([ "$h3_after" -gt 0 ] && [ "$sync_diff" -le 3 ] 2>/dev/null && echo true || echo false)"
echo ""

# --- Cleanup ---
echo -e "${CYAN}Cleaning up...${NC}"
docker compose down -v 2>/dev/null || true

# --- Summary ---
echo ""
echo -e "${CYAN}=== Results ===${NC}"
echo -e "  Total:  ${TOTAL}"
echo -e "  Passed: ${GREEN}${PASS}${NC}"
echo -e "  Failed: ${RED}${FAIL}${NC}"
echo ""

if [ "$FAIL" -eq 0 ]; then
    echo -e "${GREEN}ALL TESTS PASSED${NC}"
    exit 0
else
    echo -e "${RED}${FAIL} TEST(S) FAILED${NC}"
    exit 1
fi
