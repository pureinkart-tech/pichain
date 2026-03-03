#!/bin/bash
set -e

# PIChain systemd service installer
# Run with: sudo bash deploy/setup-systemd.sh

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== PIChain Systemd Service Setup ==="
echo ""

# Stop any manually-running instances first
echo "Stopping any running pichain processes..."
pkill -9 -f 'pichain-miner' 2>/dev/null || true
pkill -9 -f 'pichain run' 2>/dev/null || true
sleep 2

# Install service files
echo "Installing service files..."
cp "$SCRIPT_DIR/pichain-node.service" /etc/systemd/system/pichain-node.service
cp "$SCRIPT_DIR/pichain-miner-systemd.service" /etc/systemd/system/pichain-miner.service

# Reload systemd
systemctl daemon-reload

# Enable services (start on boot)
systemctl enable pichain-node.service
systemctl enable pichain-miner.service

# Start services
echo "Starting pichain-node..."
systemctl start pichain-node.service
sleep 5

# Verify node is up
if curl -s http://localhost:8314/api/v1/info > /dev/null 2>&1; then
    echo "  Node is UP"
else
    echo "  WARNING: Node not responding yet (may still be starting)"
fi

echo "Starting pichain-miner..."
systemctl start pichain-miner.service
sleep 2

echo ""
echo "=== Status ==="
systemctl --no-pager status pichain-node.service | head -5
echo ""
systemctl --no-pager status pichain-miner.service | head -5

echo ""
echo "=== Useful Commands ==="
echo "  View node logs:    journalctl -u pichain-node -f"
echo "  View miner logs:   journalctl -u pichain-miner -f"
echo "  Restart node:      sudo systemctl restart pichain-node"
echo "  Restart miner:     sudo systemctl restart pichain-miner"
echo "  Stop everything:   sudo systemctl stop pichain-miner pichain-node"
echo "  Check status:      systemctl status pichain-node pichain-miner"
echo ""
echo "Done! Services will auto-restart on crash and start on boot."
