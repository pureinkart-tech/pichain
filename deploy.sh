#!/bin/bash
# PIChain Deploy Script — Build, stop, restart node + miner
# Usage: ./deploy.sh [--skip-build] [--node-only] [--miner-only]
set -e

PICHAIN_DIR="$(cd "$(dirname "$0")" && pwd)"
DATA_DIR="$PICHAIN_DIR/live-data"
NODE_BIN="$PICHAIN_DIR/target/release/pichain"
MINER_BIN="$PICHAIN_DIR/target/release/pichain-miner"
NODE_LOG="/tmp/pichain-node.log"
MINER_LOG="/tmp/pichain-miner.log"
RPC_ADDR="0.0.0.0:8314"
P2P_ADDR="/ip4/0.0.0.0/tcp/9314"
CHAIN_ID="31415"
MINER_WALLET="$DATA_DIR/miner-wallet.json"
MINER_PROFILE="auto"

SKIP_BUILD=false
NODE_ONLY=false
MINER_ONLY=false

for arg in "$@"; do
  case $arg in
    --skip-build) SKIP_BUILD=true ;;
    --node-only) NODE_ONLY=true ;;
    --miner-only) MINER_ONLY=true ;;
    --help|-h)
      echo "PIChain Deploy Script"
      echo "  ./deploy.sh              Build + restart node + miner"
      echo "  ./deploy.sh --skip-build Stop/restart without rebuilding"
      echo "  ./deploy.sh --node-only  Only restart the node"
      echo "  ./deploy.sh --miner-only Only restart the miner"
      exit 0
      ;;
  esac
done

cd "$PICHAIN_DIR"

# ---- Build ----
if [ "$SKIP_BUILD" = false ]; then
  echo ">>> Building release binary..."
  cargo build --release 2>&1 | tail -5
  echo ">>> Build complete."
else
  echo ">>> Skipping build (--skip-build)"
fi

# ---- Stop existing processes ----
echo ">>> Stopping existing processes..."

if [ "$MINER_ONLY" = false ]; then
  # Stop miner first (it depends on node)
  MINER_PIDS=$(pgrep -f "pichain-miner" 2>/dev/null || true)
  if [ -n "$MINER_PIDS" ]; then
    echo "    Stopping miner(s): $MINER_PIDS"
    kill $MINER_PIDS 2>/dev/null || true
    sleep 2
  fi

  # Stop node gracefully (SIGTERM, wait for clean shutdown)
  NODE_PIDS=$(pgrep -f "pichain run" 2>/dev/null || true)
  if [ -n "$NODE_PIDS" ]; then
    echo "    Stopping node(s) gracefully: $NODE_PIDS"
    kill $NODE_PIDS 2>/dev/null || true
    # Wait up to 10s for graceful shutdown
    for i in $(seq 1 10); do
      if ! kill -0 $NODE_PIDS 2>/dev/null; then break; fi
      sleep 1
    done
    # Force kill if still running
    kill -9 $NODE_PIDS 2>/dev/null || true
    sleep 1
  fi

  # Make sure port is free
  fuser -k 8314/tcp 2>/dev/null || true
  sleep 1
fi

if [ "$NODE_ONLY" = false ] && [ "$MINER_ONLY" = true ]; then
  MINER_PIDS=$(pgrep -f "pichain-miner" 2>/dev/null || true)
  if [ -n "$MINER_PIDS" ]; then
    echo "    Stopping miner(s): $MINER_PIDS"
    kill $MINER_PIDS 2>/dev/null || true
    sleep 1
  fi
fi

# ---- Clean stale lock + repair DB if needed ----
rm -f "$DATA_DIR/LOCK"
if [ "$MINER_ONLY" = false ]; then
  echo ">>> Repairing RocksDB (preventive)..."
  cd "$PICHAIN_DIR"
  # Quick repair — silently fixes any corruption from kill signals
  mkdir -p crates/pichain-storage/examples
  cat > crates/pichain-storage/examples/repair_db.rs << 'REPAIR_EOF'
use rocksdb::{Options, DB};
fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "./live-data".to_string());
    let mut opts = Options::default();
    opts.create_if_missing(false);
    if let Err(e) = DB::repair(&opts, &path) {
        opts.set_paranoid_checks(false);
        let _ = DB::repair(&opts, &path);
    }
}
REPAIR_EOF
  cargo run --example repair_db -p pichain-storage -- "$DATA_DIR" 2>/dev/null && echo "    DB OK" || echo "    DB repair skipped"
  rm -rf crates/pichain-storage/examples
fi

# ---- Start Node ----
if [ "$MINER_ONLY" = false ]; then
  echo ">>> Starting node..."
  nohup "$NODE_BIN" run \
    --data-dir "$DATA_DIR" \
    --rpc-addr "$RPC_ADDR" \
    --p2p-addr "$P2P_ADDR" \
    --chain-id "$CHAIN_ID" \
    > "$NODE_LOG" 2>&1 &
  NODE_PID=$!
  echo "    Node PID: $NODE_PID"

  # Wait for RPC to be ready
  echo "    Waiting for RPC..."
  for i in $(seq 1 30); do
    if curl -s http://127.0.0.1:8314/ > /dev/null 2>&1; then
      echo "    RPC ready after ${i}s"
      break
    fi
    if [ $i -eq 30 ]; then
      echo "    WARNING: RPC not responding after 30s, check $NODE_LOG"
      tail -5 "$NODE_LOG"
    fi
    sleep 1
  done
fi

# ---- Start Miner ----
if [ "$NODE_ONLY" = false ]; then
  echo ">>> Starting miner..."
  nohup "$MINER_BIN" \
    --keypair "$MINER_WALLET" \
    --rpc-url "http://127.0.0.1:8314" \
    --profile "$MINER_PROFILE" \
    > "$MINER_LOG" 2>&1 &
  MINER_PID=$!
  echo "    Miner PID: $MINER_PID"
  sleep 3
  if grep -q "Mining round" "$MINER_LOG" 2>/dev/null; then
    echo "    Miner is mining!"
  else
    echo "    Miner starting up..."
    tail -2 "$MINER_LOG"
  fi
fi

echo ""
echo "=== Deploy Complete ==="
echo "  Node log:  $NODE_LOG"
echo "  Miner log: $MINER_LOG"
echo "  Website:   http://127.0.0.1:8314"
echo "  Status:    curl http://127.0.0.1:8314/api/v1/mining/status"
echo ""
