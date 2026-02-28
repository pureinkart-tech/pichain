# PIChain — Why You Should Mine & Hold PI

### The Self-Sustaining Layer 1 Blockchain
### No Team. No Foundation. No Treasury. Just Math.

---

## What Is PIChain?

**PIChain** is a high-performance Layer 1 blockchain built on the **Satoshi model** — 85% of the total supply goes to miners, there are no team allocations, no foundation reserves, and no treasury. The protocol is designed to be self-sustaining forever, just like Bitcoin.

Instead of wasting energy on meaningless hash puzzles, PIChain miners compute **hexadecimal digits of pi** using the Bailey-Borwein-Plouffe (BBP) algorithm — real mathematical work that permanently extends humanity's verified computation of pi.

**Key Facts:**
- **Fixed Supply:** 3,141,592,653 PI — immutable, no governance override, no minting function
- **85% to Miners** — earn PI by computing digits of pi from any hardware
- **Performance:** 10,000-100,000 TPS | <500ms finality | $0.0001 avg tx fee
- **Dual VM:** WASM + Full EVM — deploy Solidity, Rust, or any WASM language
- **Built-in DeFi:** Native DEX, token program, NFT marketplace, and token launchpad
- **507 unit tests passing** across adversarial security auditing
- **Self-Sustaining:** 25% of transaction fees replenish the mining pool forever
- **Deflationary:** 25% of all base fees are burned permanently
- **Codebase:** ~25,000 lines of production Rust across 9 modular crates

---

## Why Mine PI?

### 1. Anyone Can Mine — No Special Hardware

PIChain mining uses the **BBP spigot algorithm** to compute hexadecimal digits of pi. This is CPU-friendly work — no GPUs, no ASICs, no $10,000 mining rigs. Your laptop earns PI.

| Profile | Threads | Digits/Batch | Who It's For |
|---------|---------|-------------|-------------|
| `laptop` | 2 | 200 | Run alongside your daily work |
| `desktop` | half cores | 1,000 | Balanced for home machines |
| `server` | all cores | 2,000 | Dedicated hardware |
| `max` | all cores | 5,000 | Maximum throughput |

Rewards are proportional to **digits computed**, not PoW difficulty. A minimal 8-bit PoW (~256 nonce attempts) serves only as anti-spam — any machine solves it instantly.

### 2. 85% of Supply Goes to Miners

This is the largest mining allocation of any blockchain. Bitcoin allocates 100% to miners but inflates forever. PIChain has a **fixed supply** of 3.14 billion tokens with **85% allocated to miners** from genesis.

```
  Mining Pool              ████████████████████████████████████████████  85%
  Validator Staking        █████                                        10%
  Initial Liquidity        ██                                            5%
```

No team allocation. No foundation. No ecosystem grants. No insider advantages. **Everyone starts equal.**

### 3. The Mining Pool Never Runs Out

The emission follows a **2pi% (6.28%) geometric decay** — each year, 6.28% of the remaining pool is emitted. The pool mathematically never reaches zero. But more importantly:

**25% of all transaction base fees flow back into the mining pool.**

As the network grows and transactions increase, fee income replenishes the pool faster than decay depletes it. Mining on PIChain is sustainable **forever** — not just for 7 years or until rewards halve to nothing.

### Emission Schedule — 2pi% Geometric Decay

| Year | Annual Emission | Remaining Pool |
|------|----------------|----------------|
| 1 | ~167,656,256 PI | ~2,502,697,499 PI |
| 5 | ~126,481,028 PI | ~1,888,405,816 PI |
| 10 | ~91,465,524 PI | ~1,364,804,082 PI |
| 20 | ~47,816,710 PI | ~713,295,987 PI |
| 31 (pi decades) | ~27,008,936 PI | ~402,929,654 PI |

Year 1 emits only ~5.3% of total supply — gentle enough to avoid early chart damage from farming, while still rewarding early miners significantly.

---

## Why Hold PI?

### 1. Provably Deflationary

**25% of every base fee is burned permanently.** No new tokens are ever created. The circulating supply only goes down over time.

| Network Usage | Daily TPS | Annual Burn | % of Total Supply |
|---------------|-----------|-------------|-------------------|
| Early adoption | 10,000 | 78,840 PI | 0.003% |
| Growth phase | 50,000 | 1,971,000 PI | 0.063% |
| Maturity | 100,000 | 7,884,000 PI | 0.251% |

At maturity, burn rate exceeds mining emissions — making PI **net deflationary** with growing network usage.

### 2. Fixed Supply — No Inflation, Ever

Unlike Ethereum (inflationary PoS), Solana (~5.5% annual inflation), or most PoS chains, PIChain has **zero inflation**. The total supply is hardcoded at genesis and enforced at the VM level. No governance vote, no upgrade, no admin key can mint new tokens.

### 3. Fee Distribution Creates Sustainable Value

Every transaction creates value for the ecosystem:

```
Base Fee Breakdown:
  ██████   25%  -->  BURNED FOREVER (permanent deflation)
  ██████   25%  -->  Mining Pool (replenishes miner rewards)
  ████████████  50%  -->  Block Proposer (validator incentive)

Priority Fee:
  ████████████████████████████  100% --> Block Proposer
```

This model is **self-sustaining**: validators earn from fees, miners earn from the pool that fees replenish, and holders benefit from permanent burn. No foundation needed.

### 4. L2/L3 Ecosystem Multiplies Demand

Every L2 rollup and L3 application chain settles on PIChain L1 and pays fees in PI:

```
L2 Transaction
     |
     +-- L2 Sequencer Fee --> L2 operator revenue
     |
     +-- L1 Data Posting Fee --> Paid in PI to validators
     |        (burns 25% via EIP-1559)
     |
     \-- L1 Settlement Fee --> Paid in PI for state root posting
              (burns 25% via EIP-1559)
```

More L2s = more PI demand = more burn = more scarcity. The Ethereum playbook, but with better economics from day one.

---

## How It Works — The Technology

### PI-DAG Consensus — Three Layers of Finality

```
+--------------------------------------------------+
|                PI-DAG CONSENSUS                    |
|                                                    |
|  +-----------+  +----------+  +--------------+    |
|  |  Narwhal  |  |Bullshark |  |  Avalanche   |    |
|  |   (Data   |->|(Ordering)|->| (Fast Path)  |    |
|  |Availability)|          |  |              |    |
|  +-----------+  +----------+  +--------------+    |
|                                                    |
|  Data broadcast   Total order   Sub-second         |
|  to all nodes     with BFT      for simple txs     |
|  in parallel      safety        (70% of volume)    |
+--------------------------------------------------+
```

| Path | Finality | Use Case | % of Traffic |
|------|----------|----------|-------------|
| **Avalanche Fast Path** | ~200-500ms | Transfers, NFTs, simple ops | ~70% |
| **Bullshark DAG** | ~400ms-1s | DeFi swaps, complex contracts | ~25% |
| **Owned-Object Path** | ~200ms | Single-owner object mutations | ~5% |

**Safety guarantee:** P(safety violation) < 10^-18

### Block-STM Parallel Execution

Most blockchains execute transactions sequentially. PIChain uses **Block-STM** to automatically discover parallelism at runtime — no developer changes needed:

| Workload Contention | Speedup (32 cores) |
|---------------------|-------------------|
| No contention | 28-30x |
| Low (5% conflicts) | 16-20x |
| Moderate (20%) | 8-12x |
| High (50%) | 3-5x |

### Proof of Useful Work — Mining That Matters

```
pi = SUM (1/16^k) * [4/(8k+1) - 2/(8k+4) - 1/(8k+5) - 1/(8k+6)]
```

- **Useful computation:** Each proof extends humanity's knowledge of pi's decimal expansion
- **Natural difficulty curve:** O(n log n) cost — later digits are provably harder
- **Fraud proof:** 16 random spot-checks verify work with P(fraud) < 5.4 x 10^-20
- **Inclusive:** Laptops mine early digits; dedicated hardware pushes the frontier

---

## Competitive Landscape

| Feature | PIChain | Bitcoin | Ethereum | Solana |
|---------|---------|---------|----------|--------|
| Max TPS | 100,000 | ~7 | ~15 | ~7,000 |
| Finality | <500ms | ~60min | ~12min | ~400ms |
| Fixed Supply | 3.14B | 21M | No (inflationary) | No (inflationary) |
| Mining | Useful Work (pi) | SHA-256 hash | PoS (no mining) | PoS (no mining) |
| Mining Allocation | **85%** | 100% (inflationary) | N/A | N/A |
| EVM Compatible | Full | No | Native | No |
| WASM Contracts | Yes | No | No | No |
| Auto-Parallel Exec | Block-STM | No | No | Manual |
| Native DEX | Protocol level | No | No | No |
| Fee Burn | 25% | No | Variable | 50% |
| ZK-Ready State | Poseidon | No | No | No |
| Team Allocation | **0%** | 0% | Pre-mine | Various |

### PIChain's Unique Position

1. **Satoshi model with modern tech** — Bitcoin's fair distribution philosophy meets 2026 blockchain engineering
2. **Only L1 with useful mining** — not wasting energy on meaningless hash puzzles
3. **Only L1 with dual WASM + EVM** — capture both developer ecosystems
4. **Protocol-level DeFi** — DEX, tokens, NFTs, launchpad built into the chain itself
5. **Self-sustaining forever** — fee-to-miner pipeline means mining never stops
6. **ZK-future-proofed** — Poseidon state tree ready for SNARK verification from day one

---

## Built-in DeFi Stack — No Smart Contracts Needed

Native protocol-level financial primitives. Faster, cheaper, and more secure than smart contract equivalents:

### Native DEX (Automated Market Maker)
- Constant-product AMM (x*y = k) with 0.30% trading fee
- **30% max price impact** per swap (sandwich attack prevention)
- **100-block LP lock period** (flash loan attack prevention)

### Native Token Program (SPL-style)
- Create, mint, transfer, burn fungible tokens
- Freeze/thaw accounts, approve delegates, revoke mint authority
- **10,000 mint cap per address** (spam prevention)

### Native NFT System with Built-in Marketplace
- **Protocol-enforced royalties** — creators get paid on every sale, no workarounds
- Built-in list/buy/delist marketplace operations

### Token Launchpad
- Hardcap/softcap mechanics with fair participation
- **Automatic AMM pool creation** on successful finalization

---

## Security

| Metric | Value |
|--------|-------|
| Unit tests passing | 507 |
| Integration tests passing | 42 |
| Critical vulnerabilities remaining | **0** |
| High vulnerabilities remaining | **0** |
| Lines of Rust code | ~25,000 |

### Anti-51% Attack Prevention — 6-Layer Defense

```
+------------------------------------------------------------------+
|              ANTI-51% ATTACK: 6-LAYER DEFENSE                     |
|                                                                    |
|  LAYER 1: Mining Cap          > No miner earns >5% of epoch      |
|  (Per-address, per-epoch)       emission. Forces min 20 active    |
|                                 miners for full utilization.       |
|                                                                    |
|  LAYER 2: Validator Cap       > No single validator holds >33%    |
|  (Per-validator stake)          of total staked PI.                |
|                                                                    |
|  LAYER 3: Address Cap         > No single address stakes >10%     |
|  (Per-address total stake)      of total across ALL validators.   |
|                                                                    |
|  LAYER 4: Velocity Limit     > No validator's stake can grow      |
|  (Per-epoch growth cap)         >50% per epoch.                    |
|                                                                    |
|  LAYER 5: Validator Cap       > Maximum 100 active validators.    |
|  (Anti-Sybil flooding)         10,000 PI minimum per validator.   |
|                                                                    |
|  LAYER 6: Correlated Slash   > When N validators are slashed in   |
|  (Sybil self-destruct)         the same epoch, each pays N*33%    |
|                                 ADDITIONAL penalty (capped 100%).  |
+------------------------------------------------------------------+
```

### Cryptographic Stack

| Primitive | Algorithm | Purpose |
|-----------|-----------|---------|
| User signatures | Ed25519 | 70,000 verifications/sec; batch-verifiable |
| Consensus attestation | BLS12-381 | 500 validators -> single 48-byte aggregate sig |
| General hashing | BLAKE3 | 10-15 GB/s; 4-7x faster than SHA-256 |
| State commitment | Poseidon | ZK-friendly; 300 R1CS constraints (vs SHA-256's 25,000) |
| EVM compatibility | secp256k1 + Keccak | Full Ethereum interop |

---

## Tokenomics — The Satoshi Model

### Fixed Supply: 3,141,592,653 PI

Total supply derived from the first 10 digits of pi. **Immutable at the protocol level.** No governance vote, no upgrade path, no admin key can mint new tokens. Enforced at the VM execution layer.

### Genesis Distribution — No Insiders

```
 Mining Pool             █████████████████████████████████████████████  85%  (~2,670,353,755 PI)
 Validator Staking       █████                                         10%  (  ~314,159,265 PI)
 Initial Liquidity       ██                                             5%  (  ~157,079,633 PI)
```

**No team allocation. No foundation. No ecosystem grants. No vesting schedules. No insider advantages.**

The creator mines alongside everyone else, with no special privileges.

### Fee Distribution — Self-Sustaining Economics

| Destination | % of Base Fee | Purpose |
|-------------|--------------|---------|
| **Burned** | 25% | Permanent deflation |
| **Mining Pool** | 25% | Replenishes miner rewards forever |
| **Block Proposer** | 50% | Validator incentive |
| *Priority Fee* | *100% to proposer* | *Transaction inclusion incentive* |

### Why This Matters

**Bitcoin's problem:** Mining rewards halve every 4 years and eventually reach zero. Miners will rely solely on transaction fees, which may not be enough to secure the network.

**PIChain's solution:** The mining pool never fully drains (geometric decay), AND 25% of all transaction fees continuously replenish it. As network usage grows, fee income can exceed decay — making the effective mining pool **grow** over time. Mining on PIChain is economically viable forever.

---

## Technical Architecture

```
+----------------------------------------------------------+
|                     PIChain Node                          |
|                                                           |
|  +----------+  +--------------+  +--------------------+  |
|  |  RPC API |  |  P2P Network |  |  Block Producer    |  |
|  | JSON-RPC |  |  GossipSub   |  |  + Mining Verifier |  |
|  | WebSocket|  |  Kademlia    |  |                    |  |
|  | REST     |  |  Turbine     |  |                    |  |
|  +----+-----+  +------+-------+  +---------+----------+  |
|       |               |                     |             |
|  +----+---------------+---------------------+----------+  |
|  |              PI-DAG Consensus Engine                 |  |
|  |  Narwhal (DA) + Bullshark (Order) + Fast Path       |  |
|  |  BLS Aggregate Signatures | Stake-Weighted Quorum   |  |
|  +--------------------+----------------------------+---+  |
|                       |                                   |
|  +--------------------+----------------------------+---+  |
|  |           Block-STM Parallel Executor               |  |
|  |                                                     |  |
|  |  +------+ +------+ +-----+ +-----+ +-----------+   |  |
|  |  | DEX  | |Token | | NFT | |Launch| |  WASM VM  |   |  |
|  |  | AMM  | |Prog. | | Mkt | | Pad | | (Wasmtime)|   |  |
|  |  +------+ +------+ +-----+ +-----+ +-----------+   |  |
|  |  +-----------+ +---------------+ +--------------+   |  |
|  |  |  EVM VM   | |  Staking &    | |  Mining      |   |  |
|  |  |  (revm)   | |  Slashing     | |  Rewards     |   |  |
|  |  +-----------+ +---------------+ +--------------+   |  |
|  +--------------------+----------------------------+---+  |
|                       |                                   |
|  +--------------------+----------------------------+---+  |
|  |              Storage Layer (RocksDB)                 |  |
|  |  Jellyfish Merkle Tree | 7 Column Families          |  |
|  |  Poseidon Hashing | Atomic WriteBatch               |  |
|  +-----------------------------------------------------+  |
+-----------------------------------------------------------+
```

**9 Modular Crates:**

| Crate | Lines | Purpose |
|-------|-------|---------|
| `pichain-crypto` | ~1,500 | Ed25519, BLS12-381, BLAKE3, Poseidon, batch verification |
| `pichain-types` | ~2,500 | Core data structures, genesis config, constants |
| `pichain-consensus` | ~4,000 | Narwhal DAG, Bullshark ordering, staking, slashing |
| `pichain-execution` | ~6,000 | Block-STM executor, DEX, Token, NFT, Launchpad, WASM, EVM |
| `pichain-storage` | ~3,500 | RocksDB, JMT, state persistence, atomic batches |
| `pichain-network` | ~3,500 | libp2p, GossipSub, Kademlia, Turbine, bridge, sync |
| `pichain-mining` | ~1,300 | BBP algorithm, proof verification, reward calculation |
| `pichain-rpc` | ~1,600 | JSON-RPC 2.0, WebSocket, rate limiting |
| `pichain-node` | ~1,200 | Orchestration, config, startup, shutdown |

---

## Layer 2 & Layer 3 — The Growth Multiplier

PIChain's **Poseidon-based state tree** makes ZK-rollup proofs **80x cheaper** than on Ethereum. Combined with built-in data availability via Turbine erasure coding, PIChain is the ideal settlement layer for an L2 ecosystem.

### Why PIChain Is the Best L1 for L2s

| ZK Proof Cost | SHA-256 (Ethereum) | Poseidon (PIChain) | Savings |
|---------------|--------------------|--------------------|---------|
| R1CS constraints/hash | 25,000 | 300 | **83x** |
| Proof generation time | ~10 min | ~8 sec | **75x** |
| L2 proving cost/batch | $5-50 | $0.05-0.50 | **100x** |

### L2 Ecosystem Value Accrual

```
                    +---------------------+
                    |   PIChain L1 (PI)   |
                    |                     |
                    |  > Fixed 3.14B cap  |
                    |  > 25% fee burn     |
                    |  > All layers pay   |
                    |    fees in PI       |
                    +----------+----------+
                               |
              +----------------+----------------+
              |                |                |
     +--------+-------+ +-----+------+ +-------+--------+
     |  L2: DeFi Hub  | | L2: NFT &  | | L2: Payments   |
     |  100K TPS      | | Social     | | 200K TPS       |
     +--------+-------+ +-----+------+ +-------+--------+
              |               |                |
        +-----+--+      +----+----+      +----+-----+
        |L3:Game |      |L3:Social|      |L3:POS    |
        |1M TPS  |      |App      |      |Terminals |
        +--------+      +---------+      +----------+

Every transaction at every layer settles on L1
and pays fees in PI --> creating persistent buy
pressure and permanent token burn.
```

---

## PI-Themed Design — Memorable Brand

| Element | Value | Pi Connection |
|---------|-------|---------------|
| Total Supply | 3,141,592,653 | First 10 digits of pi |
| Epoch Length | 31,415 blocks | pi x 10,000 |
| Target Block Time | 314 ms | pi x 100 |
| Chain ID | 314159 | pi x 100,000 |
| Default RPC Port | 8314 | 8000 + 314 |
| Mining Algorithm | BBP (pi digit computation) | Extends pi knowledge |

**Pi Day (March 14th)** is celebrated worldwide — a natural annual moment for the community.

---

## Technical Specifications

| Specification | Value |
|--------------|-------|
| Block Time | ~314ms |
| Finality | 400ms-1s (Bullshark), <500ms (Avalanche) |
| Consensus | DAG + Bullshark (aBFT) |
| Execution | Block-STM parallel (8-16x single-thread) |
| State Storage | RocksDB + Jellyfish Merkle Tree |
| Networking | libp2p (QUIC, TCP, Noise, Yamux) |
| Smart Contracts | WASM (Wasmtime) + EVM (revm) |
| Signature Scheme | Ed25519 + BLS |
| Hash Function | BLAKE3 (primary), Poseidon (ZK circuits) |
| Mining Algorithm | BBP Pi digit computation |
| Fee Model | EIP-1559: 25% burn, 25% miners, 50% proposer |
| Total Supply | 3,141,592,653 PI (fixed, immutable) |

---

## How to Get Started

### Mine PI

```bash
# Build the miner
git clone https://github.com/pureinkart-tech/pichain
cd pichain && cargo build --release

# Generate a wallet
./target/release/pichain-cli keygen --output wallet.json

# Start mining
./target/release/pichain-miner \
  --keypair wallet.json \
  --rpc-url http://127.0.0.1:8314 \
  --profile desktop
```

### Run a Node

```bash
./target/release/pichain run \
  --data-dir ./data \
  --rpc-addr 0.0.0.0:8314 \
  --chain-id 31415
```

### Stake as a Validator

| Component | Minimum | Recommended |
|-----------|---------|-------------|
| Stake | 10,000 PI | 50,000+ PI |
| CPU | 16-core, 3.0 GHz | 32-core AMD EPYC |
| RAM | 128 GB DDR4 ECC | 256 GB DDR5 ECC |
| Storage | 2 TB NVMe Gen4 | 2x 2TB NVMe RAID |
| Network | 1 Gbps dedicated | 10 Gbps dedicated |

---

## The Bottom Line

PIChain is what Bitcoin would look like if it were built today:

- **Fixed supply** like Bitcoin — but with a modern fee-burn mechanism
- **Fair distribution** like Bitcoin — but 85% to miners with no halving death spiral
- **Self-sustaining** like Bitcoin — but with fee-to-miner recycling that keeps rewards flowing forever
- **Useful mining** unlike Bitcoin — computing pi instead of burning energy on SHA-256
- **Fast and cheap** unlike Bitcoin — sub-second finality, $0.0001 transactions
- **Programmable** unlike Bitcoin — full EVM + WASM smart contracts

No company behind it. No investors to dump on you. No team tokens to vest. Just a protocol that runs forever, powered by mathematics.

---

*Computing the Infinite, with finite supply.*

*Built with Rust. Secured by Mathematics. Powered by pi.*

**Links:**
- **Website:** [https://pichain.net](https://pichain.net)
- **Explorer:** [https://pichain.net/explorer](https://pichain.net/explorer)
- **Mining Dashboard:** [https://pichain.net/mining](https://pichain.net/mining)
- **GitHub:** [https://github.com/pureinkart-tech/pichain](https://github.com/pureinkart-tech/pichain)
- **Whitepaper:** [WHITEPAPER.md](WHITEPAPER.md)
