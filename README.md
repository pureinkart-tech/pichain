# PIChain

A high-performance Layer 1 blockchain with Proof of Useful Work mining based on computing digits of Pi.

## Overview

PIChain is a novel L1 blockchain protocol built around a fixed, non-mintable supply of **3,141,592,653 PI tokens**. Instead of wasting energy on arbitrary hash puzzles, miners compute hexadecimal digits of Pi using the **Bailey-Borwein-Plouffe (BBP)** algorithm — real mathematical work that extends humanity's verified computation of Pi.

### Key Features

- **Fixed Supply** — 3,141,592,653 PI tokens. No inflation, no minting. 25% of base fees are burned permanently.
- **Proof of Useful Work** — Mining computes Pi digits using the BBP spigot algorithm. Anyone can mine — from a laptop to a data center.
- **DAG + Bullshark Consensus** — Narwhal-inspired DAG for data availability with Bullshark zero-overhead ordering. Sub-second finality.
- **Avalanche Fast Path** — Sub-500ms finality for simple transactions via sub-sampled repeated voting.
- **Block-STM Parallel Execution** — Optimistic parallel transaction scheduling for 8-16x throughput on commodity hardware.
- **Dual VM** — Primary WASM (Wasmtime JIT) alongside full EVM compatibility (revm).
- **Turbine Block Propagation** — Reed-Solomon erasure-coded block propagation for efficient bandwidth utilization.

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

### Emission Schedule

The mining pool contains **1,256,637,061 PI** (40% of total supply), distributed over 7 years:

| Year | Emission | % of Pool |
|------|----------|-----------|
| 1 | 314,159,265 PI | 25% |
| 2 | 251,327,412 PI | 20% |
| 3 | 188,495,559 PI | 15% |
| 4 | 150,796,447 PI | 12% |
| 5 | 125,663,706 PI | 10% |
| 6 | 113,097,336 PI | 9% |
| 7 | 113,097,336 PI | 9% |

## Tokenomics

**Total Supply: 3,141,592,653 PI** (immutable, set at genesis)

| Allocation | Amount | % |
|------------|--------|---|
| Mining Pool | 1,256,637,061 | 40% |
| Community & Ecosystem | 628,318,531 | 20% |
| Team & Development | 471,238,898 | 15% |
| Validators & Staking | 314,159,265 | 10% |
| Treasury | 314,159,265 | 10% |
| Liquidity | 157,079,633 | 5% |

**Deflationary**: 25% of all base fees are permanently burned, ensuring circulating supply monotonically decreases over time.

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
| Fee Model | EIP-1559 with 25% burn |

## License

Apache-2.0

## Links

- **Website**: [https://pichain.net](https://pichain.net)
- **Explorer**: [https://pichain.net/explorer](https://pichain.net/explorer)
- **Mining Dashboard**: [https://pichain.net/mining](https://pichain.net/mining)
- **Whitepaper**: [WHITEPAPER.md](WHITEPAPER.md)
