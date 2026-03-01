# PIChain Architecture

PIChain is a high-performance Layer 1 blockchain built in Rust with a Proof of Useful Work
consensus mechanism. Miners compute hexadecimal digits of PI using the Bailey-Borwein-Plouffe
(BBP) formula, contributing permanent mathematical artifacts to the network while earning
rewards proportional to their computational contributions.

Total supply: 3,141,592,653 PI (3.14 billion). Target block time: 314ms.

---

## High-Level Architecture

```
                          +--------------------------+
                          |       pichain-node       |
                          |  (binary: `pichain`)     |
                          +------+-------+-----------+
                                 |       |
                +----------------+       +----------------+
                |                                         |
   +------------v-----------+              +--------------v-----------+
   |    pichain-consensus   |              |     pichain-network      |
   | DAG + Bullshark + Fast |              | libp2p + GossipSub +     |
   |   Path + Staking       |              | Turbine + Bridge + Sync  |
   +------------+------------+              +--------------+-----------+
                |                                         |
   +------------v-----------+              +--------------v-----------+
   |   pichain-execution    |              |      pichain-rpc         |
   | Block-STM + Pipeline + |              | Axum REST API + WS +    |
   | Mempool + Sub-Executors|              | Prometheus Metrics       |
   +------------+------------+              +-------------------------+
                |
   +------------v-----------+
   |    pichain-storage      |
   | RocksDB + JMT + State  |
   +------------+------------+
                |
   +------------v-----------+     +-------------------------+
   |    pichain-mining       |     |    pichain-crypto       |
   | BBP + PoW + Rewards +   |     | Ed25519 + BLS + Blake3  |
   | Difficulty Adjustment   |     | + Poseidon              |
   +-------------------------+     +-------------------------+
                                   +-------------------------+
                                   |    pichain-types        |
                                   | Block, Tx, Account,     |
                                   | Object, Token, NFT, DEX |
                                   +-------------------------+
```

---

## Crate Structure

PIChain is organized as a Cargo workspace with 9 core crates, 2 tool binaries, and additional
tools for bridging and GUI mining.

### Core Crates (`crates/`)

| Crate | Description |
|-------|-------------|
| `pichain-crypto` | Ed25519 (user transactions), BLS12-381 (consensus attestations), Blake3 (hashing), Poseidon (ZK-friendly state trie) |
| `pichain-types` | Fundamental data structures: Transaction, Block, Account, Object, Token, NFT, DEX, Launchpad, Genesis |
| `pichain-storage` | RocksDB wrapper with 10 column families + Jellyfish Merkle Tree (JMT) for state proofs |
| `pichain-consensus` | Narwhal DAG data availability + Bullshark ordering + Avalanche fast-path + staking/slashing |
| `pichain-execution` | Block-STM parallel executor, 3-stage pipeline, mempool, sub-executors (token, DEX, NFT, launchpad, EVM, WASM) |
| `pichain-mining` | BBP PI digit computation, difficulty adjustment, proof verification, reward calculation |
| `pichain-network` | libp2p with GossipSub, Kademlia DHT, Turbine erasure-coded block propagation, bridge, state sync |
| `pichain-rpc` | Axum 0.7 REST API, WebSocket subscriptions, Prometheus metrics, embedded explorer UI |
| `pichain-node` | Main binary (`pichain`) that ties all components together |

### Tool Binaries (`tools/`)

| Binary | Description |
|--------|-------------|
| `pichain-cli` | CLI tools for developers (keygen, queries, debugging) |
| `pichain-miner` | Standalone miner with hardware profiles (laptop/desktop/server/max) |
| `pichain-miner-gui` | GUI miner application |
| `pichain-bridge` | Cross-chain bridge relayer |

---

## Consensus: PI-DAG

PIChain uses a hybrid consensus protocol combining three components:

### 1. Narwhal DAG (Data Availability)

All validators contribute simultaneously by proposing **headers** containing transaction
batches. Headers reference certificates from the previous round, forming a Directed Acyclic
Graph (DAG). This architecture provides data availability without a single leader bottleneck.

- Validators produce headers referencing 2f+1 certificates from the prior round
- Headers become certificates once they accumulate 2f+1 signatures
- Every validator contributes throughput, not just the leader

### 2. Bullshark Ordering (Total Order)

Bullshark provides zero-overhead total ordering on the DAG with 2-round finality:

1. Block proposed -- create DAG header + certificate (round N)
2. Next round -- certificate references previous round (round N+1)
3. On even rounds, check if 2f+1 next-round certificates reference the leader
4. If quorum reached, commit the leader and all causally prior blocks

View change timeout: 5 seconds per round with exponential backoff (capped at 8x) to
prevent a single Byzantine leader from stalling the chain.

### 3. Avalanche Fast-Path (Sub-Second Finality)

Simple transactions (transfers, single-owner operations) can be finalized in ~200-500ms:

- Query k=40 random validators for transaction validity
- If alpha=30+ agree, the transaction is finalized
- Probability of error < 10^-18
- Falls back to full DAG consensus for complex transactions

### Finality States

```rust
enum FinalityStatus {
    Proposed,       // Block proposed, not yet committed
    Committed,      // Committed through Bullshark (2-round finality)
    FastFinalized,  // Finalized via Avalanche fast-path (<500ms)
}
```

---

## Staking and Slashing

| Parameter | Value |
|-----------|-------|
| Minimum validator stake | 10,000 PI |
| Minimum delegation | 1 PI |
| Unbonding period | 62 epochs (~7 days) |
| Max validators | 3,141 |
| Epoch length | 31,415 blocks |
| Max commission | 20% |
| Max validator stake share | 33.33% |
| Max address stake share | 10% |
| Max stake growth per epoch | 50% (anti-flash-staking) |

### Slashing Penalties

| Offense | Penalty | Jail Duration |
|---------|---------|---------------|
| Double-signing | 33% of stake | 88 epochs (~10 days) |
| Extended downtime | 1% of stake | 88 epochs |
| Invalid block proposal | 5% of stake | 88 epochs |

Evidence must be submitted within 42 epochs (less than the unbonding period).

---

## State Management: RocksDB + JMT

### Column Families

PIChain uses RocksDB with 10 dedicated column families for data separation:

| Column Family | Purpose |
|---------------|---------|
| `CF_STATE` | Account balances, nonces, and state data |
| `CF_BLOCKS` | Block headers and data indexed by height |
| `CF_TRANSACTIONS` | Transaction data indexed by hash |
| `CF_RECEIPTS` | Transaction execution receipts |
| `CF_METADATA` | Database version, chain config, and schema metadata |
| `CF_OBJECTS` | Sui-inspired objects with ownership tracking |
| `CF_JMT_NODES` | Jellyfish Merkle Tree internal and leaf nodes |
| `CF_TX_HISTORY` | Per-address transaction history for queries |
| `CF_EVENT_INDEX` | Event logs indexed for efficient lookup |
| `CF_EVM_STATE` | EVM contract storage (revm integration) |

### Jellyfish Merkle Tree (JMT)

The JMT provides authenticated state proofs with the following properties:

- Binary trie structure using Poseidon hashing (ZK-friendly)
- Inclusion and exclusion proofs for any account or object
- Domain-separated keys: account namespace and object namespace
- Atomic state commitment with ordered JMT updates
- O(1) node lookup via hash-based secondary index

---

## Execution Engine

### Block-STM Parallel Execution

Inspired by Aptos, PIChain executes transactions optimistically in parallel:

1. Execute all transactions concurrently across threads
2. Track read and write sets via a multi-version hash map (`MVHashMap`)
3. Detect read/write conflicts automatically
4. Re-execute conflicting transactions sequentially
5. Achieves ~8-16x speedup under moderate contention

### 3-Stage Transaction Pipeline

A Solana-style pipelined architecture overlaps work across consecutive blocks:

```
Block N:   [Fetch+SigVerify] --> [Banking] --> [Ledger Write]
Block N+1:                       [Fetch+SigVerify] --> [Banking] --> [Ledger Write]
```

**Stage 1 -- Fetch + SigVerify**: Pull transactions from the mempool and batch-verify
Ed25519 signatures using `ed25519-dalek` batch verification.

**Stage 2 -- Banking**: Execute transactions via the Sealevel-style parallel scheduler.
The `TransactionExecutor` dispatches to specialized sub-executors:

- `TokenExecutor` -- SPL-style token mints, transfers, burns, freezing
- `DexExecutor` -- AMM pools with MEV-resistant price impact (block-start snapshots)
- `NftExecutor` -- Collections, minting, marketplace operations
- `LaunchpadExecutor` -- Token launch with bonding curve graduation
- `EvmExecutor` -- EVM smart contracts via `revm`
- `WasmVM` -- WASM smart contracts via `wasmtime`

**Stage 3 -- Ledger Write**: Persist the produced block and state changes to RocksDB.

### Fee Model (EIP-1559)

Fees follow an EIP-1559 dynamic base fee model with the following distribution:

| Allocation | Share |
|-----------|-------|
| Burn (deflationary) | 25% |
| Mining pool | 25% |
| Block proposer / stakers | 50% |

The 25% fee-to-mining recycling ensures the mining pool remains viable indefinitely,
even after the geometric decay emission schedule approaches zero.

---

## Mining: Proof of Useful Work

### BBP Formula

The Bailey-Borwein-Plouffe (BBP) algorithm computes hexadecimal digits of PI at any
position without computing all preceding digits. This property makes mining
embarrassingly parallel: each digit position is independent.

### Mining Flow

1. Miner queries the node for the current mining frontier (highest verified position)
2. Miner computes a batch of hex digits at positions beyond the frontier
3. Miner finds a nonce where `blake3(digits || nonce || anchor_block_hash) < difficulty_target`
4. Miner submits a `SubmitMiningProof` transaction to the node
5. The `MiningProcessor` in the executor verifies the digits and PoW
6. On success, the miner receives a reward proportional to the number of digits computed

### Difficulty and PoW

- Minimum PoW: 8 bits (fixed) -- approximately 256 nonce attempts on average
- Purpose: anti-spam only, not competitive hashpower
- Difficulty adjusts every 10 proofs with a max 2x change per adjustment
- Target: one proof every 10 seconds
- Window: 144 proofs (~24 minutes)

### Reward Schedule

- 85% of total supply (2,670,353,755 PI) allocated to mining
- 2pi% (6.28%) geometric decay per year
- Year 1 emission: ~167.6M PI (~5.3% of total supply)
- Year 10 emission: ~91.5M PI
- Pool never fully drains (geometric series)
- 25% of transaction fees recycled back to mining pool

---

## Networking

### libp2p Stack

PIChain uses libp2p 0.54 with the following protocols:

| Protocol | Purpose |
|----------|---------|
| GossipSub 1.1 | Transaction and block propagation |
| Kademlia DHT | Peer discovery and routing |
| Noise | Encrypted transport |
| TCP + QUIC | Dual transport support |
| Yamux | Connection multiplexing |
| AutoNAT | NAT traversal detection |
| Relay | Relay for NAT-blocked nodes |
| Identify | Peer identification |

Default ports: RPC on 8314, P2P on 9314.

### Turbine Block Propagation

Blocks are propagated using erasure-coded shreds for Byzantine fault tolerance:

- 32 data shreds + 32 recovery shreds = 64 total (Reed-Solomon coding)
- Any 32 of 64 shreds can reconstruct the full block
- Logarithmic fanout tree with fanout factor of 8
- Tolerates Byzantine nodes withholding shreds

### State Sync

New nodes can sync state via a multi-layered protocol:

- Chain tip discovery via GossipSub
- Block range requests for catching up
- Equivocation evidence propagation for deterministic slashing

### Network Messages

```rust
enum NetworkMessage {
    Transaction(SignedTransaction),
    BlockShred { block_height, shred_index, total_shreds, data },
    DagCertificate { round, author, data, signature },
    ValidatorAnnounce { public_key, address, stake, signature },
}
```

---

## Bridge: Cross-Chain Asset Transfers

### Supported Chains

Ethereum, BNB Chain, Arbitrum, Base, Solana, Bitcoin, and generic chains by ID.

### Bridge Flow

1. User locks tokens on source chain
2. Bridge relayers observe the lock event
3. Relayers submit signed attestations to PIChain
4. Once quorum is reached, wrapped tokens are minted on the destination chain
5. Reverse: burn wrapped tokens to release originals on the source chain

### Safety Mechanisms

- **Relayer quorum**: Multiple relayers must attest before minting
- **Confirmation delay**: Minimum 5,732 blocks (~30 minutes) between initiation and execution
- **Circuit breaker**: Automatically pauses bridge when hourly volume exceeds threshold
- **Relayer staking**: Relayers bond PI as collateral; incorrect attestations trigger slashing
- **Admin pause**: Manual emergency override for bridge operations

---

## Cryptographic Primitives

| Scheme | Library | Purpose |
|--------|---------|---------|
| Ed25519 | `ed25519-dalek` | User transaction signatures, batch verification |
| BLS12-381 | `blst` | Consensus attestations, aggregate signatures (48-byte pubkeys, 96-byte sigs) |
| Blake3 | `blake3` | General-purpose hashing (10-15 GB/s throughput) |
| Poseidon | Custom (Blake3 stand-in) | ZK-friendly JMT state trie hashing |

### Key Types

- Ed25519 secret keys are zeroized on drop (`zeroize` crate)
- Addresses are 20 bytes, derived from the Blake3 hash of the Ed25519 public key
- BLS proof-of-possession required in multi-validator mode to prevent rogue-key attacks

---

## RPC API

The RPC server runs on Axum 0.7 with rate limiting via `governor`:

### Key Endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /api/v1/info` | Node status (height, peers, base fee, mempool) |
| `GET /health` | Health check |
| `GET /metrics` | Prometheus metrics (localhost only) |
| `POST /api/v1/transaction/submit` | Submit a signed transaction |
| `GET /api/v1/account/:address` | Query account balance and nonce |
| `GET /api/v1/block/:height` | Get block by height |
| `GET /api/v1/mining/status` | Mining frontier, difficulty, reward per digit |
| `WS /ws` | WebSocket subscriptions for blocks and transactions |
| `GET /` | Embedded block explorer UI |

### Prometheus Metrics

The `/metrics` endpoint exposes (restricted to localhost):

```
pichain_block_height          Current block height
pichain_peer_count            Connected peers
pichain_base_fee              Current base fee
pichain_total_burned          Total PI burned
pichain_mining_frontier       Mining frontier position
pichain_mining_digits_verified Total digits verified
pichain_mining_unique_miners  Unique miner addresses
pichain_uptime_seconds        Node uptime
pichain_requests_total        Total RPC requests
pichain_requests_errors       RPC errors
pichain_ws_subscribers        Active WebSocket subscribers
pichain_consensus_round       Current consensus round
pichain_bullshark_commits     Total Bullshark commits
pichain_certificates_produced Certificates produced
pichain_validator_count       Active validators
pichain_total_stake           Total staked PI
```

---

## Smart Contracts

### EVM Compatibility

PIChain supports EVM smart contracts via `revm` v19:

- Dedicated `CF_EVM_STATE` column family for contract storage
- Standard EVM opcodes and Solidity compatibility
- Transaction type: `EvmCall` / `EvmDeploy`

### WASM Virtual Machine

Native WASM smart contracts via `wasmtime` v29:

- Host functions for state reads/writes with 1MB state change limit per call
- Block height and timestamp access
- Contract registry with ABI metadata

---

## Security Features

- **Arithmetic safety**: `checked_add`/`checked_mul`/`checked_sub` for all balance operations
- **Hex input validation**: Length checks before `hex::decode` to prevent DoS
- **Batch signature verification**: At mempool ingress and in the pipeline
- **TOCTOU protection**: Faucet re-checks state under write lock
- **Rate limiting**: Per-IP request throttling on RPC endpoints
- **Security hardening**: systemd service runs as dedicated `pichain` user with `ProtectSystem=strict`
- **Domain separation**: BLS signatures include chain_id to prevent cross-chain replay
- **Anti-concentration**: Stake caps and velocity limits prevent validator centralization

---

## Configuration

Configuration is loaded from TOML files or CLI flags:

```toml
# deploy/pichain.toml
data_dir = "/var/lib/pichain"
rpc_addr = "0.0.0.0:8314"
p2p_addr = "/ip4/0.0.0.0/tcp/9314"
chain_id = 31415          # 314159 for mainnet
max_mempool_size = 100000
log_level = "info"
bootstrap_peers = []
```

### Chain IDs

| Chain ID | Network |
|----------|---------|
| 314159 | Mainnet |
| 31415 | Devnet |

---

## Build and Release

```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = "symbols"
overflow-checks = true   # Critical for financial software
```

The release profile enables overflow checks even in optimized builds, ensuring
arithmetic panics rather than silently wrapping in production.
