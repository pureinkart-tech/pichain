#!/bin/bash
# PIChain Miner — macOS Installer
# Double-click this file to set up the miner automatically.

set -e
clear

echo ""
echo "  ╔══════════════════════════════════════════════╗"
echo "  ║         PIChain Miner — macOS Setup          ║"
echo "  ╚══════════════════════════════════════════════╝"
echo ""

cd "$(dirname "$0")"
ARCH=$(uname -m)

# Detect architecture
if [ "$ARCH" = "arm64" ]; then
    MINER="pichain-miner-macos-arm64"
    SIGNER="pichain-signer-macos-arm64"
    echo "  Detected: Apple Silicon (M1/M2/M3/M4)"
else
    MINER="pichain-miner-macos-x86_64"
    SIGNER="pichain-signer-macos-x86_64"
    echo "  Detected: Intel Mac"
fi

# Download if not present
if [ ! -f "$MINER" ]; then
    echo "  Downloading miner..."
    curl -L -o "$MINER" "https://github.com/pureinkart-tech/pichain/releases/latest/download/$MINER"
fi
if [ ! -f "$SIGNER" ]; then
    echo "  Downloading signer..."
    curl -L -o "$SIGNER" "https://github.com/pureinkart-tech/pichain/releases/latest/download/$SIGNER"
fi

# Make executable + remove quarantine + sign
echo "  Setting up permissions..."
chmod +x "$MINER" "$SIGNER"
xattr -cr "$MINER" 2>/dev/null || sudo xattr -cr "$MINER" 2>/dev/null || true
xattr -cr "$SIGNER" 2>/dev/null || sudo xattr -cr "$SIGNER" 2>/dev/null || true
codesign --sign - --force "$MINER" 2>/dev/null || true
codesign --sign - --force "$SIGNER" 2>/dev/null || true

# Generate wallet if needed
if [ ! -f "wallet.json" ]; then
    echo ""
    echo "  Creating your quantum-safe wallet..."
    ./"$MINER" --keypair wallet.json --generate-keypair
fi

echo ""
echo "  ✅ Setup complete!"
echo ""
echo "  Your wallet: wallet.json"
echo "  Address: $(grep -o '"address":"[^"]*"' wallet.json 2>/dev/null | head -1 | cut -d'"' -f4)"
echo ""
echo "  To start mining, run:"
echo "    ./$MINER --keypair wallet.json"
echo ""
echo "  To use the DEX/staking from your browser, run:"
echo "    ./$SIGNER --wallet wallet.json"
echo "    Then open https://pichain.net"
echo ""
read -p "  Start mining now? (y/n) " choice
if [ "$choice" = "y" ] || [ "$choice" = "Y" ]; then
    echo ""
    ./"$MINER" --keypair wallet.json --rpc-url https://pichain.net --profile desktop
fi
