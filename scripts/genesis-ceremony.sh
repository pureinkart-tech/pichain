#!/usr/bin/env bash
# ============================================================================
# PIChain Genesis Ceremony Script
# ============================================================================
#
# This script orchestrates the mainnet genesis ceremony. It:
#   1. Generates validator keys for all participants
#   2. Creates the genesis configuration with real addresses
#   3. Computes and displays the genesis hash for all validators to verify
#   4. Each validator signs the genesis to prove agreement
#   5. Initializes the data directory from the finalized genesis
#
# Usage:
#   ./genesis-ceremony.sh generate-keys <count>    — Generate validator keypairs
#   ./genesis-ceremony.sh create-genesis            — Create genesis from key files
#   ./genesis-ceremony.sh verify                    — Verify genesis file
#   ./genesis-ceremony.sh sign <validator-key>      — Sign genesis with validator key
#   ./genesis-ceremony.sh init <data-dir>           — Initialize data directory
#   ./genesis-ceremony.sh full <count>              — Run full ceremony (all steps)
#
# ============================================================================

set -euo pipefail

PICHAIN="${PICHAIN:-./target/release/pichain}"
GENESIS_FILE="${GENESIS_FILE:-./genesis.json}"
CEREMONY_DIR="${CEREMONY_DIR:-./ceremony}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

log()  { echo -e "${GREEN}[CEREMONY]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARNING]${NC} $*"; }
err()  { echo -e "${RED}[ERROR]${NC} $*"; exit 1; }

# ---- Step 1: Generate validator keys ----
generate_keys() {
    local count="${1:-4}"
    log "Generating $count validator keypairs..."
    mkdir -p "$CEREMONY_DIR/keys"

    for i in $(seq 1 "$count"); do
        local key_dir="$CEREMONY_DIR/keys/validator-$i"
        mkdir -p "$key_dir"

        if [ -f "$key_dir/validator.key" ]; then
            log "  Validator $i: key already exists, skipping"
            continue
        fi

        # Generate keys using pichain keygen
        local output
        output=$($PICHAIN keygen 2>&1)

        # Extract key info from keygen output
        local ed_secret ed_address bls_pk bls_pop bls_secret
        ed_secret=$(echo "$output" | grep -i "ed25519 secret" | awk '{print $NF}')
        ed_address=$(echo "$output" | grep -i "address" | awk '{print $NF}')
        bls_pk=$(echo "$output" | grep -i "bls public" | awk '{print $NF}')
        bls_pop=$(echo "$output" | grep -i "bls pop\|proof.of.possession" | awk '{print $NF}')
        bls_secret=$(echo "$output" | grep -i "bls secret" | awk '{print $NF}')

        # Save validator key file
        cat > "$key_dir/validator.key" <<EOF
{
  "ed25519_secret": "$ed_secret",
  "bls_secret": "$bls_secret"
}
EOF
        chmod 600 "$key_dir/validator.key"

        # Save public info (shareable)
        cat > "$key_dir/validator-info.json" <<EOF
{
  "validator_index": $i,
  "address": "$ed_address",
  "bls_public_key": "$bls_pk",
  "bls_pop": "$bls_pop",
  "stake_pi": 100000
}
EOF

        log "  Validator $i: address=$ed_address"
    done

    log "Keys generated in $CEREMONY_DIR/keys/"
    log ""
    log "SECURITY: Distribute validator.key files to each validator operator SECURELY."
    log "          Only share validator-info.json publicly."
}

# ---- Step 2: Create genesis from validator info files ----
create_genesis() {
    log "Creating genesis configuration..."

    # First, generate template
    $PICHAIN genesis-ceremony --genesis-file "$GENESIS_FILE" --generate-template

    # Collect validator info
    local validator_files
    validator_files=$(find "$CEREMONY_DIR/keys" -name "validator-info.json" 2>/dev/null | sort)

    if [ -z "$validator_files" ]; then
        err "No validator-info.json files found in $CEREMONY_DIR/keys/"
    fi

    local count
    count=$(echo "$validator_files" | wc -l)
    log "Found $count validator info files"

    if [ "$count" -lt 4 ]; then
        warn "Mainnet requires minimum 4 validators for BFT safety (3f+1). Got $count."
    fi

    # Set ceremony timestamp (current UTC time)
    local timestamp_ms
    timestamp_ms=$(date +%s%3N 2>/dev/null || python3 -c "import time; print(int(time.time()*1000))")

    log "Ceremony timestamp: $timestamp_ms"

    # Build genesis JSON with real addresses using Python for JSON manipulation
    python3 - "$GENESIS_FILE" "$timestamp_ms" $validator_files <<'PYEOF'
import json, sys

genesis_file = sys.argv[1]
timestamp_ms = int(sys.argv[2])
validator_files = sys.argv[3:]

with open(genesis_file) as f:
    genesis = json.load(f)

# Set timestamp
genesis["timestamp_ms"] = timestamp_ms

# Add validators
genesis["validators"] = []
for vf in validator_files:
    with open(vf) as f:
        info = json.load(f)
    genesis["validators"].append({
        "address": info["address"],
        "stake": info["stake_pi"] * 1_000_000_000,  # Convert PI to base units
        "name": f"Validator {info['validator_index']}"
    })

# Use first validator's address for staking reserve,
# second for liquidity (or same if fewer)
addrs = [v["address"] for v in genesis["validators"]]
for alloc in genesis["allocations"]:
    if "Staking" in alloc["label"] and not alloc.get("virtual_pool", False):
        if alloc["address"] == "0000000000000000000000000000000000000000":
            alloc["address"] = addrs[0] if addrs else alloc["address"]
    if "Liquidity" in alloc["label"] and not alloc.get("virtual_pool", False):
        if alloc["address"] == "0000000000000000000000000000000000000000":
            alloc["address"] = addrs[1 % len(addrs)] if addrs else alloc["address"]

with open(genesis_file, "w") as f:
    json.dump(genesis, f, indent=2)

print(f"Genesis updated with {len(genesis['validators'])} validators")
PYEOF

    log "Genesis file written to: $GENESIS_FILE"
}

# ---- Step 3: Verify genesis ----
verify_genesis() {
    log "Verifying genesis configuration..."
    echo ""
    $PICHAIN genesis-ceremony --genesis-file "$GENESIS_FILE" --verify
}

# ---- Step 4: Sign genesis ----
sign_genesis() {
    local key_file="${1:-}"
    if [ -z "$key_file" ]; then
        err "Usage: $0 sign <path/to/validator.key>"
    fi
    log "Signing genesis with key: $key_file"
    echo ""
    $PICHAIN genesis-ceremony --genesis-file "$GENESIS_FILE" --sign --key-file "$key_file"
}

# ---- Step 5: Initialize data directory ----
init_from_genesis() {
    local data_dir="${1:-./mainnet-data}"
    log "Initializing data directory: $data_dir"
    mkdir -p "$data_dir"
    $PICHAIN genesis-ceremony --genesis-file "$GENESIS_FILE" --data-dir "$data_dir"
}

# ---- Full ceremony ----
full_ceremony() {
    local count="${1:-4}"

    echo -e "${CYAN}"
    echo "╔══════════════════════════════════════════════════════════╗"
    echo "║            PIChain Mainnet Genesis Ceremony             ║"
    echo "╚══════════════════════════════════════════════════════════╝"
    echo -e "${NC}"

    log "Step 1/5: Generate $count validator keypairs"
    generate_keys "$count"
    echo ""

    log "Step 2/5: Create genesis configuration"
    create_genesis
    echo ""

    log "Step 3/5: Verify genesis"
    verify_genesis
    echo ""

    log "Step 4/5: Sign genesis (all validators)"
    for key_dir in "$CEREMONY_DIR"/keys/validator-*/; do
        if [ -f "$key_dir/validator.key" ]; then
            sign_genesis "$key_dir/validator.key"
            echo ""
        fi
    done

    log "Step 5/5: Initialize data directories"
    for i in $(seq 1 "$count"); do
        init_from_genesis "$CEREMONY_DIR/data/validator-$i"
        echo ""
    done

    echo -e "${GREEN}"
    echo "╔══════════════════════════════════════════════════════════╗"
    echo "║              Genesis Ceremony Complete!                 ║"
    echo "╚══════════════════════════════════════════════════════════╝"
    echo -e "${NC}"
    log "Genesis file: $GENESIS_FILE"
    log "Validator keys: $CEREMONY_DIR/keys/"
    log "Data directories: $CEREMONY_DIR/data/"
    log ""
    log "Next: Start each validator with:"
    log "  pichain run --config mainnet.toml --data-dir $CEREMONY_DIR/data/validator-N"
}

# ---- Main ----
case "${1:-help}" in
    generate-keys) generate_keys "${2:-4}" ;;
    create-genesis) create_genesis ;;
    verify)         verify_genesis ;;
    sign)           sign_genesis "${2:-}" ;;
    init)           init_from_genesis "${2:-./mainnet-data}" ;;
    full)           full_ceremony "${2:-4}" ;;
    *)
        echo "PIChain Genesis Ceremony"
        echo ""
        echo "Usage: $0 <command> [args]"
        echo ""
        echo "Commands:"
        echo "  generate-keys <count>  Generate validator keypairs (default: 4)"
        echo "  create-genesis         Create genesis config from validator keys"
        echo "  verify                 Verify genesis file"
        echo "  sign <key-file>        Sign genesis with validator key"
        echo "  init <data-dir>        Initialize data directory from genesis"
        echo "  full <count>           Run complete ceremony (all steps)"
        echo ""
        echo "Environment:"
        echo "  PICHAIN=./target/release/pichain    Binary path"
        echo "  GENESIS_FILE=./genesis.json         Genesis config path"
        echo "  CEREMONY_DIR=./ceremony             Working directory"
        ;;
esac
