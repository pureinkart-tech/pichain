#!/usr/bin/env bash
# PIChain Validator Setup Script
# Usage: sudo bash setup-validator.sh [devnet|mainnet]
set -euo pipefail

NETWORK="${1:-devnet}"
CHAIN_ID=31415
DATA_DIR="/var/lib/pichain"
BIN_DIR="/usr/local/bin"
SERVICE_DIR="/etc/systemd/system"

if [ "$NETWORK" = "mainnet" ]; then
    CHAIN_ID=314159
fi

echo "============================================"
echo "  PIChain Validator Setup"
echo "  Network: $NETWORK (chain_id=$CHAIN_ID)"
echo "============================================"
echo

# 1. Create pichain user
if ! id -u pichain &>/dev/null; then
    echo "[1/7] Creating pichain user..."
    useradd -m -s /bin/bash pichain
else
    echo "[1/7] User pichain already exists"
fi

# 2. Create data directory
echo "[2/7] Creating data directory at $DATA_DIR..."
mkdir -p "$DATA_DIR"
chown pichain:pichain "$DATA_DIR"

# 3. Install binaries
echo "[3/7] Installing binaries to $BIN_DIR..."
if [ -f "./target/release/pichain" ]; then
    cp target/release/pichain "$BIN_DIR/"
    cp target/release/pichain-cli "$BIN_DIR/"
    cp target/release/pichain-miner "$BIN_DIR/"
    chmod +x "$BIN_DIR"/pichain*
    echo "  Installed from local build"
else
    echo "  ERROR: Run 'cargo build --release' first"
    exit 1
fi

# 4. Initialize chain data
echo "[4/7] Initializing chain data..."
if [ ! -d "$DATA_DIR/db" ]; then
    sudo -u pichain pichain init --data-dir "$DATA_DIR" --chain-id "$CHAIN_ID"
else
    echo "  Data directory already initialized, skipping"
fi

# 5. Generate validator keys
echo "[5/7] Generating validator keypair..."
KEYS_FILE="$DATA_DIR/validator-keys.txt"
if [ ! -f "$KEYS_FILE" ]; then
    sudo -u pichain pichain keygen > "$KEYS_FILE"
    chmod 600 "$KEYS_FILE"
    chown pichain:pichain "$KEYS_FILE"
    echo "  Keys saved to $KEYS_FILE"
    echo "  IMPORTANT: Back up this file securely!"
else
    echo "  Keys already exist at $KEYS_FILE"
fi

# 6. Generate miner wallet
echo "[6/7] Generating miner wallet..."
WALLET_FILE="$DATA_DIR/miner-wallet.json"
if [ ! -f "$WALLET_FILE" ]; then
    sudo -u pichain pichain-miner --keypair "$WALLET_FILE" --generate-keypair
    chmod 600 "$WALLET_FILE"
    chown pichain:pichain "$WALLET_FILE"
else
    echo "  Miner wallet already exists at $WALLET_FILE"
fi

# 7. Install systemd services
echo "[7/7] Installing systemd services..."
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Patch chain-id in service file
sed "s/--chain-id 31415/--chain-id $CHAIN_ID/" \
    "$SCRIPT_DIR/pichain.service" > "$SERVICE_DIR/pichain.service"

cp "$SCRIPT_DIR/pichain-miner.service" "$SERVICE_DIR/pichain-miner.service"
sed -i "s/--chain-id 31415/--chain-id $CHAIN_ID/" "$SERVICE_DIR/pichain-miner.service"

systemctl daemon-reload

echo
echo "============================================"
echo "  Setup Complete!"
echo "============================================"
echo
echo "  Start the validator:"
echo "    sudo systemctl start pichain"
echo "    sudo systemctl enable pichain"
echo
echo "  Start the miner:"
echo "    sudo systemctl start pichain-miner"
echo "    sudo systemctl enable pichain-miner"
echo
echo "  View logs:"
echo "    journalctl -u pichain -f"
echo "    journalctl -u pichain-miner -f"
echo
echo "  Check status:"
echo "    curl http://localhost:8314/api/v1/info"
echo "    curl http://localhost:8314/health"
echo "    curl http://localhost:8314/metrics"
echo
echo "  RPC endpoint: http://localhost:8314"
echo "  P2P endpoint: /ip4/0.0.0.0/tcp/9314"
echo "============================================"
