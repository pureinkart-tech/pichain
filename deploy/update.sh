#!/bin/bash
set -e

# PIChain live server update script
# Usage: bash deploy/update.sh [--skip-tests] [--skip-build]

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

SKIP_TESTS=false
SKIP_BUILD=false
for arg in "$@"; do
    case $arg in
        --skip-tests) SKIP_TESTS=true ;;
        --skip-build) SKIP_BUILD=true ;;
    esac
done

echo "=== PIChain Update ==="
echo "Project: $PROJECT_DIR"
echo ""

# Step 1: Build
if [ "$SKIP_BUILD" = false ]; then
    echo "[1/4] Building release binaries..."
    cargo build --release 2>&1 | tail -5
    echo "  Build complete."
else
    echo "[1/4] Skipping build (--skip-build)"
fi

# Step 2: Tests
if [ "$SKIP_TESTS" = false ]; then
    echo "[2/4] Running tests..."
    if cargo test --workspace --quiet 2>&1 | tail -3; then
        echo "  Tests passed."
    else
        echo "  ERROR: Tests failed. Aborting update."
        exit 1
    fi
else
    echo "[2/4] Skipping tests (--skip-tests)"
fi

# Step 3: Update service files if changed
echo "[3/4] Syncing service files..."
NEED_RELOAD=false
if ! diff -q "$SCRIPT_DIR/pichain-node.service" /etc/systemd/system/pichain-node.service > /dev/null 2>&1; then
    sudo cp "$SCRIPT_DIR/pichain-node.service" /etc/systemd/system/pichain-node.service
    NEED_RELOAD=true
    echo "  Updated pichain-node.service"
fi
if ! diff -q "$SCRIPT_DIR/pichain-miner-systemd.service" /etc/systemd/system/pichain-miner.service > /dev/null 2>&1; then
    sudo cp "$SCRIPT_DIR/pichain-miner-systemd.service" /etc/systemd/system/pichain-miner.service
    NEED_RELOAD=true
    echo "  Updated pichain-miner.service"
fi
if [ "$NEED_RELOAD" = true ]; then
    sudo systemctl daemon-reload
    echo "  Reloaded systemd."
else
    echo "  Service files unchanged."
fi

# Step 4: Restart services
echo "[4/4] Restarting services..."
sudo systemctl restart pichain-node
echo "  Node restarting..."
sleep 5

# Verify node
if curl -s http://localhost:8314/api/v1/info > /dev/null 2>&1; then
    HEIGHT=$(curl -s http://localhost:8314/api/v1/info | python3 -c "import json,sys; print(json.load(sys.stdin)['block_height'])" 2>/dev/null)
    echo "  Node UP (block $HEIGHT)"
else
    echo "  WARNING: Node not responding yet (may still be loading)"
fi

sudo systemctl restart pichain-miner
sleep 2
echo "  Miner restarted."

echo ""
echo "=== Update Complete ==="
echo "  Node:  $(systemctl is-active pichain-node)"
echo "  Miner: $(systemctl is-active pichain-miner)"
echo ""
echo "  Logs: journalctl -u pichain-node -f"
