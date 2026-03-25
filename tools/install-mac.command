#!/bin/bash
# PIChain Miner — macOS Installer
# Double-click this file to install the PIChain Miner app.

set -e
clear

echo ""
echo "  ╔══════════════════════════════════════════════╗"
echo "  ║       PIChain Miner — macOS Installer        ║"
echo "  ╚══════════════════════════════════════════════╝"
echo ""

ARCH=$(uname -m)
APP_NAME="PIChain Miner.app"
APP_DEST="/Applications/$APP_NAME"

# Detect architecture
if [ "$ARCH" = "arm64" ]; then
    DMG_NAME="pichain-miner-gui-macos-arm64.dmg"
    echo "  Detected: Apple Silicon (M1/M2/M3/M4)"
else
    DMG_NAME="pichain-miner-gui-macos-x86_64.dmg"
    echo "  Detected: Intel Mac"
fi

DMG_URL="https://github.com/pureinkart-tech/pichain/releases/latest/download/$DMG_NAME"
DMG_PATH="/tmp/$DMG_NAME"

# Download DMG
echo ""
echo "  Downloading PIChain Miner..."
curl -L -o "$DMG_PATH" "$DMG_URL"
echo "  Download complete."

# Mount DMG
echo "  Installing..."
MOUNT_DIR=$(hdiutil attach "$DMG_PATH" -nobrowse -quiet | grep "/Volumes" | awk '{print substr($0, index($0, "/Volumes"))}')

if [ -z "$MOUNT_DIR" ]; then
    echo "  Error: Failed to mount DMG. Please try downloading manually."
    exit 1
fi

# Find the .app inside the mounted DMG
APP_SRC=$(find "$MOUNT_DIR" -maxdepth 1 -name "*.app" -type d | head -1)
if [ -z "$APP_SRC" ]; then
    echo "  Error: No .app found in DMG."
    hdiutil detach "$MOUNT_DIR" -quiet 2>/dev/null || true
    exit 1
fi

# Remove old version if present
if [ -d "$APP_DEST" ]; then
    echo "  Removing previous version..."
    rm -rf "$APP_DEST"
fi

# Copy to Applications
echo "  Copying to Applications..."
cp -R "$APP_SRC" "$APP_DEST"

# Unmount DMG
hdiutil detach "$MOUNT_DIR" -quiet 2>/dev/null || true
rm -f "$DMG_PATH"

# Remove quarantine attribute (this is what causes "move to trash")
echo "  Removing macOS security blocks..."
xattr -cr "$APP_DEST" 2>/dev/null || sudo xattr -cr "$APP_DEST" 2>/dev/null || true

# Ad-hoc code sign so macOS treats it as a known app
codesign --sign - --force --deep "$APP_DEST" 2>/dev/null || true

echo ""
echo "  ✅ PIChain Miner installed successfully!"
echo ""
echo "  The app is in your Applications folder."
echo "  You can find it in Launchpad or Spotlight (Cmd+Space → PIChain)."
echo ""

read -p "  Open PIChain Miner now? (y/n) " choice
if [ "$choice" = "y" ] || [ "$choice" = "Y" ]; then
    open "$APP_DEST"
fi
