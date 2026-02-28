# PIChain

A high-performance Layer 1 blockchain with Proof of Useful Work mining based on computing digits of Pi.

## Overview

PIChain is a novel L1 blockchain protocol built around a fixed, non-mintable supply of **3,141,592,653 PI tokens**. Instead of wasting energy on arbitrary hash puzzles, miners compute hexadecimal digits of Pi using the **Bailey-Borwein-Plouffe (BBP)** algorithm — real mathematical work that extends humanity's verified computation of Pi.

Designed as a **Satoshi model** chain: 85% of the total supply goes to miners, transaction fees flow back to the mining pool forever, and no team/foundation/treasury allocations exist. The protocol is self-sustaining without governance.

### Key Features

- **Fixed Supply** — 3,141,592,653 PI tokens. No inflation, no minting. 25% of base fees are burned permanently.
- **Self-Sustaining Mining** — 85% of supply goes to miners via 2π% geometric decay. 25% of transaction fees replenish the mining pool forever.
- **Proof of Useful Work** — Mining computes Pi digits using the BBP spigot algorithm. Anyone can mine — from a laptop to a data center.
- **DAG + Bullshark Consensus** — Narwhal-inspired DAG for data availability with Bullshark zero-overhead ordering. Sub-second finality.
- **Avalanche Fast Path** — Sub-500ms finality for simple transactions via sub-sampled repeated voting.
- **Block-STM Parallel Execution** — Optimistic parallel transaction scheduling for 8-16x throughput on commodity hardware.
- **Dual VM** — Primary WASM (Wasmtime JIT) alongside full EVM compatibility (revm).
- **Turbine Block Propagation** — Reed-Solomon erasure-coded block propagation for efficient bandwidth utilization.

## Public Testnet

PIChain testnet is live. Start mining in minutes — no special hardware required.

| Resource | URL |
|----------|-----|
| Homepage | https://pichain.net |
| Block Explorer | https://pichain.net/explorer |
| Mining Dashboard | https://pichain.net/mining |
| Faucet | https://pichain.net/faucet |
| RPC Endpoint | https://pichain.net |
| Chain ID | 31415 |

### Quick Start — Mine on Testnet

```bash
# 1. Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 2. Build the miner
git clone https://github.com/pureinkart-tech/pichain.git
cd pichain
cargo build --release -p pichain-miner

# 3. Generate a wallet
./target/release/pichain-miner --keypair wallet.json --generate-keypair

# 4. Start mining (the faucet auto-funds your wallet)
./target/release/pichain-miner \
  --keypair wallet.json \
  --rpc-url https://pichain.net \
  --profile desktop
```

Mining profiles: `laptop` (2 threads), `desktop` (half cores), `server` (all cores), `max` (all cores + large batches). Fine-tune with `--threads`, `--max-cpu`, `--digits-per-batch`.

## Architecture

```
pichain/
├── crates/
│   ├── pichain-crypto/       # Ed25519, BLS, BLAKE3, Poseidon hash
│   ├── pichain-types/        # Core types: blocks, transactions, accounts, tokens, NFTs
│   ├── pichain-storage/      # RocksDB + Jellyfish Merkle Tree state storage
│   ├── pichain-consensus/    # DAG construction, Bullshark ordering, Avalanche fast-path
│   ├── pichain-execution/    # Block-STM executor, mempool, DEX, NFT, token, launchpad
│   ├── pichain-mining/       # BBP algorithm, proof verification, reward calculation
│   ├── pichain-network/      # libp2p gossip, sync, Turbine propagation
│   ├── pichain-rpc/          # Axum HTTP/WebSocket API server
│   └── pichain-node/         # Node binary, CLI, state management
├── tools/
│   ├── pichain-cli/          # Command-line client
│   └── pichain-miner/        # Mining client
├── explorer/                 # Web UI (block explorer, mining dashboard)
├── scripts/                  # Devnet test suite, key generation
└── deploy/                   # Systemd services, config templates, Prometheus
```

### Crate Overview

| Crate | Description |
|-------|-------------|
| `pichain-crypto` | Ed25519 signatures, BLS multi-signatures, BLAKE3 hashing, Poseidon ZK-friendly hash |
| `pichain-types` | Block, transaction, account, token, NFT, DEX, and launchpad data structures |
| `pichain-storage` | RocksDB persistence with Jellyfish Merkle Tree for authenticated state |
| `pichain-consensus` | DAG+Bullshark BFT consensus with Avalanche fast-path and staking |
| `pichain-execution` | Block-STM parallel executor, mempool, fee model (EIP-1559), all transaction executors |
| `pichain-mining` | BBP Pi digit computation, on-chain proof verification (16-point spot-check), emission schedule |
| `pichain-network` | libp2p networking: gossipsub, Kademlia DHT, block sync, Turbine erasure propagation |
| `pichain-rpc` | Axum-based HTTP/WS API with rate limiting, CORS, and embedded web explorer |
| `pichain-node` | Main node binary, CLI commands, state coordination |

## Building

### Prerequisites

- Rust 1.75+ (2021 edition)
- Clang/LLVM (for RocksDB compilation)
- pkg-config, libssl-dev

### Build

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run all tests (500+)
cargo test --workspace
```

This produces three binaries:
- `target/release/pichain` — The node
- `target/release/pichain-miner` — The miner
- `target/release/pichain-cli` — CLI client

## Running

### Start a Node

```bash
./target/release/pichain run \
  --data-dir ./data \
  --rpc-addr 0.0.0.0:8314 \
  --chain-id 31415
```

### Start Mining

```bash
# Generate a wallet
./target/release/pichain-cli keygen --output wallet.json

# Start mining with a hardware profile
./target/release/pichain-miner \
  --keypair wallet.json \
  --rpc-url http://127.0.0.1:8314 \
  --profile desktop
```

**Miner profiles:**

| Profile | Threads | Digits/Batch | Description |
|---------|---------|-------------|-------------|
| `laptop` | 2 | 200 | Light CPU usage, runs alongside other work |
| `desktop` | half cores | 1,000 | Balanced for daily-driver machines |
| `server` | all cores | 2,000 | Maximum throughput for dedicated hardware |
| `max` | all cores | 5,000 | Aggressive — 8 concurrent batches |

Fine-tune with `--threads`, `--digits-per-batch`, `--concurrent-batches`, `--max-cpu`.

### Monitor

```bash
bash monitor.sh  # Live terminal dashboard
```

## Mining

PIChain mining computes hexadecimal digits of Pi using the **Bailey-Borwein-Plouffe (BBP)** formula:

```
Pi = SUM(k=0 to inf) [1/16^k * (4/(8k+1) - 2/(8k+4) - 1/(8k+5) - 1/(8k+6))]
```

The BBP algorithm computes the n-th hex digit of Pi **directly** without computing all preceding digits, making it naturally parallelizable. Miners:

1. Query the current frontier (highest verified digit position)
2. Compute a batch of hex digits starting at the frontier
3. Submit a proof transaction with the computed digits
4. Earn PI tokens proportional to the number of digits computed

**Verification** uses a 16-point random spot-check scheme with a fraud acceptance probability below 5.4 x 10^-20. The on-chain verifier recomputes digits at 16 randomly-selected positions within the submitted range and compares them against the proof.

**Proof of Work** is minimal — an 8-bit PoW (~256 nonce attempts) serves as anti-spam only. Any machine can solve it instantly. Rewards are proportional to **digits computed**, not PoW difficulty.

### Emission Schedule — 2π% Geometric Decay

The mining pool contains **~2,670,353,755 PI** (85% of total supply), distributed via **2π% (6.28%) geometric decay** — each year, 6.28% of the remaining pool is emitted. The pool never fully drains, and transaction fee income replenishes it.

| Year | Emission | Remaining Pool |
|------|----------|---------------|
| 1 | ~167,656,256 PI | ~2,502,697,499 PI |
| 5 | ~126,481,028 PI | ~1,888,405,816 PI |
| 10 | ~91,465,524 PI | ~1,364,804,082 PI |
| 20 | ~47,816,710 PI | ~713,295,987 PI |
| 31 (π decades) | ~27,008,936 PI | ~402,929,654 PI |

Additionally, **25% of all transaction base fees** flow back into the mining pool, extending its lifetime indefinitely.

## Tokenomics — Satoshi Model

**Total Supply: 3,141,592,653 PI** (immutable, set at genesis)

No team, no foundation, no treasury. The protocol is self-sustaining.

| Allocation | Amount | % |
|------------|--------|---|
| Mining Pool | ~2,670,353,755 | 85% |
| Validator Staking | ~314,159,265 | 10% |
| Initial Liquidity | ~157,079,633 | 5% |

### Fee Distribution

| Destination | % of Base Fee | Purpose |
|------------|--------------|---------|
| Burned | 25% | Deflationary pressure |
| Mining Pool | 25% | Perpetual mining incentive |
| Block Proposer | 50% | Validator/staker reward |
| *Priority Fee* | *100% to proposer* | *Transaction inclusion incentive* |

**Deflationary**: 25% of all base fees are permanently burned. 25% flows to the mining pool, making mining sustainable forever.

## Configuration

See [`deploy/pichain.toml`](deploy/pichain.toml) for a full configuration template.

| Parameter | Default | Description |
|-----------|---------|-------------|
| `data_dir` | `/var/lib/pichain` | Blockchain data directory |
| `rpc_addr` | `0.0.0.0:8314` | RPC server listen address |
| `p2p_addr` | `/ip4/0.0.0.0/tcp/9314` | P2P listen address |
| `chain_id` | `31415` | Chain ID (314159 = mainnet) |
| `max_mempool_size` | `100000` | Maximum pending transactions |
| `log_level` | `info` | Logging verbosity |

## API

The RPC server exposes REST and WebSocket endpoints on port 8314:

```
GET  /api/v1/info                    # Node info, block height, supply
GET  /api/v1/block/:height           # Block by height
GET  /api/v1/account/:address        # Account balance and nonce
GET  /api/v1/tx/:hash                # Transaction by hash
POST /api/v1/tx                      # Submit transaction
GET  /api/v1/mining/status           # Mining frontier, rewards, difficulty
GET  /api/v1/mempool                 # Mempool contents
POST /api/v1/faucet                  # Devnet faucet
```

## Deployment

### Docker

```bash
docker-compose up -d
```

### Systemd

```bash
# Install services
sudo cp deploy/pichain.service /etc/systemd/system/
sudo cp deploy/pichain-miner.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now pichain
sudo systemctl enable --now pichain-miner
```

### Validator Setup

```bash
bash deploy/setup-validator.sh
```

## Testing

```bash
# Unit tests (500+)
cargo test --workspace

# Devnet integration tests (42 tests, 10 phases)
bash scripts/devnet-test.sh

# Quick mode (skip build)
bash scripts/devnet-test.sh --quick

# Multi-node test
bash scripts/multi-node-test.sh
```

## Technical Specifications

| Specification | Value |
|--------------|-------|
| Block Time | ~314ms |
| Finality | 400ms - 1s (Bullshark), <500ms (Avalanche fast-path) |
| Consensus | DAG + Bullshark (aBFT) |
| Execution | Block-STM parallel (8-16x single-thread) |
| State Storage | RocksDB + Jellyfish Merkle Tree |
| Networking | libp2p (QUIC, TCP, Noise, Yamux) |
| Smart Contracts | WASM (Wasmtime) + EVM (revm) |
| Signature Scheme | Ed25519 + BLS |
| Hash Function | BLAKE3 (primary), Poseidon (ZK circuits) |
| Mining Algorithm | BBP Pi digit computation |
| Fee Model | EIP-1559: 25% burn, 25% miners, 50% stakers |

## License

Apache-2.0

## Links

- **Website**: [https://pichain.net](https://pichain.net)
- **Explorer**: [https://pichain.net/explorer](https://pichain.net/explorer)
- **Mining Dashboard**: [https://pichain.net/mining](https://pichain.net/mining)
- **Whitepaper**: [WHITEPAPER.md](WHITEPAPER.md)
