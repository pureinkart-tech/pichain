# Getting Started with PIChain

This guide walks you through building PIChain from source, running a local devnet node,
mining PI tokens, and interacting with the chain via the CLI and REST API.

---

## Prerequisites

### System Requirements

- **OS**: Linux (Ubuntu 22.04+ recommended), macOS
- **CPU**: x86_64 or aarch64, 4+ cores recommended
- **RAM**: 8 GB minimum, 16 GB recommended
- **Disk**: 20 GB SSD for devnet, 100 GB+ for mainnet
- **Rust**: 1.75 or newer

### Install Rust

If you do not have Rust installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup update
```

### Install System Dependencies

Ubuntu / Debian:

```bash
sudo apt update
sudo apt install -y build-essential clang libssl-dev pkg-config libclang-dev
```

Fedora / RHEL:

```bash
sudo dnf install -y gcc clang openssl-devel pkg-config
```

macOS (with Homebrew):

```bash
brew install openssl pkg-config
```

RocksDB is compiled from source via the `rocksdb` crate, so no separate RocksDB
installation is required.

---

## Build from Source

Clone the repository and build in release mode:

```bash
git clone https://github.com/pureinkart-tech/pichain.git
cd pichain
cargo build --release
```

This produces three binaries in `./target/release/`:

| Binary | Description |
|--------|-------------|
| `pichain` | Node binary (validator + RPC server) |
| `pichain-cli` | Developer CLI tools |
| `pichain-miner` | Standalone miner |

Verify the build:

```bash
./target/release/pichain --version
./target/release/pichain-cli --help
./target/release/pichain-miner --help
```

---

## Run a Devnet Node

Start a local single-node devnet:

```bash
./target/release/pichain run \
    --data-dir ./devnet-data \
    --rpc-addr 0.0.0.0:8314 \
    --chain-id 31415
```

The node will:
- Initialize a fresh chain with the devnet genesis configuration
- Generate validator keys automatically (stored in `./devnet-data/validator.key`)
- Start the RPC server on port 8314
- Start the P2P listener on port 9314
- Begin producing blocks at the target interval of 314ms

### Using a Configuration File

Instead of CLI flags, you can use a TOML configuration file:

```bash
./target/release/pichain run --config deploy/devnet.toml
```

Example configuration (`deploy/devnet.toml`):

```toml
data_dir = "./pichain-data"
rpc_addr = "127.0.0.1:8314"
p2p_addr = "/ip4/0.0.0.0/tcp/9314"
chain_id = 31415
max_mempool_size = 100000
log_level = "debug"
```

### Check Node Status

Once the node is running, verify it is healthy:

```bash
# Health check
curl http://localhost:8314/health

# Node info (block height, peers, mempool size)
curl http://localhost:8314/api/v1/info

# Mining status
curl http://localhost:8314/api/v1/mining/status
```

---

## Run the Miner

### Generate a Miner Wallet

```bash
./target/release/pichain-miner --keypair ./miner-wallet.json --generate-keypair
```

This creates a new Ed25519 keypair and writes it to `miner-wallet.json`. Keep this
file safe -- it contains your private key.

### Start Mining

Connect the miner to your running node:

```bash
./target/release/pichain-miner \
    --keypair ./miner-wallet.json \
    --rpc-url http://127.0.0.1:8314 \
    --profile desktop
```

Available profiles:

| Profile | Threads | Digits/Batch | Concurrent Batches |
|---------|---------|-------------|-------------------|
| `laptop` | 2 | 200 | 1 |
| `desktop` | half cores | 1,000 | 2 |
| `server` | all cores | 2,000 | 4 |
| `max` | all cores | 5,000 | 8 |

You can override individual settings:

```bash
./target/release/pichain-miner \
    --keypair ./miner-wallet.json \
    --rpc-url http://127.0.0.1:8314 \
    --threads 4 \
    --digits-per-batch 1500 \
    --concurrent-batches 3
```

### Monitor Mining

Use the included monitoring script:

```bash
bash monitor.sh
```

This displays a live dashboard with block height, mining frontier, miner balance,
and process statistics.

---

## Explorer

PIChain includes a built-in block explorer. Open your browser and navigate to:

```
http://localhost:8314/
```

The explorer is served directly by the RPC server -- no separate process needed.
It provides pages for blocks, transactions, accounts, mining status, token trading,
and bridge operations.

---

## CLI Usage

The `pichain-cli` tool provides developer utilities:

```bash
# Show all available commands
./target/release/pichain-cli --help

# Generate a new keypair
./target/release/pichain-cli keygen

# Query an account
curl http://localhost:8314/api/v1/account/<40-char-hex-address>
```

Note: Addresses in the RPC API use raw 40-character hex format without the `0x` prefix.

---

## REST API Quick Reference

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/health` | Health check |
| GET | `/api/v1/info` | Node status |
| GET | `/api/v1/block/:height` | Get block by height |
| GET | `/api/v1/account/:address` | Account balance and nonce |
| POST | `/api/v1/transaction/submit` | Submit signed transaction |
| GET | `/api/v1/mining/status` | Mining frontier and difficulty |
| GET | `/metrics` | Prometheus metrics (localhost only) |
| WS | `/ws` | WebSocket subscriptions |

### Example: Query Account Balance

```bash
curl -s http://localhost:8314/api/v1/account/a2dd927f21d9d251536c6679c111679790225b69 | python3 -m json.tool
```

### Example: Get Latest Block

```bash
# First get the current height
HEIGHT=$(curl -s http://localhost:8314/api/v1/info | python3 -c "import json,sys; print(json.load(sys.stdin)['block_height'])")

# Then fetch that block
curl -s http://localhost:8314/api/v1/block/$HEIGHT | python3 -m json.tool
```

---

## Run Devnet Tests

PIChain includes a comprehensive devnet test suite with 42 integration tests across
10 phases:

```bash
# Full test run (builds first)
bash scripts/devnet-test.sh

# Quick test (skip build)
bash scripts/devnet-test.sh --quick
```

The test script uses port 18314 and chain_id 31415.

### Run Unit Tests

```bash
cargo test --workspace
```

This runs all unit tests across every crate.

---

## Project Layout

```
pichain/
  crates/
    pichain-crypto/       Ed25519, BLS, Blake3, Poseidon
    pichain-types/        Block, Transaction, Account, Token, NFT, DEX
    pichain-storage/      RocksDB + Jellyfish Merkle Tree
    pichain-consensus/    DAG + Bullshark + Avalanche fast-path
    pichain-execution/    Block-STM executor + 3-stage pipeline
    pichain-mining/       BBP digit computation + rewards
    pichain-network/      libp2p networking + bridge + state sync
    pichain-rpc/          REST API + WebSocket + metrics
    pichain-node/         Main node binary
  tools/
    pichain-cli/          Developer CLI
    pichain-miner/        Standalone miner
    pichain-miner-gui/    GUI miner
    pichain-bridge/       Bridge relayer
  deploy/
    devnet.toml           Devnet configuration
    mainnet.toml          Mainnet configuration
    pichain.toml          Template configuration
    pichain.service       Systemd unit for the node
    pichain-miner.service Systemd unit for the miner
    prometheus.yml        Prometheus scrape config
    setup-validator.sh    Automated validator setup
  explorer/               Embedded web UI HTML files
  scripts/
    devnet-test.sh        Integration test suite (42 tests)
    generate-devnet-keys.sh  Key generation helper
  monitor.sh              Live mining dashboard
```

---

## Next Steps

- **Mine PI tokens**: See [MINING_GUIDE.md](MINING_GUIDE.md) for detailed mining instructions
- **Run a validator**: See [VALIDATOR_GUIDE.md](VALIDATOR_GUIDE.md) for production setup
- **Understand the architecture**: See [ARCHITECTURE.md](ARCHITECTURE.md) for system design
