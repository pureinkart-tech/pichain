# PIChain Security Documentation

> Last updated: 2026-03-01
> Maintainer: PIChain Core Team
> Status: Pre-audit — internal review complete, third-party audit planned

---

## Table of Contents

1. [Overview](#overview)
2. [Threat Model](#threat-model)
3. [Trust Boundaries](#trust-boundaries)
4. [Security Invariants](#security-invariants)
5. [Cryptographic Primitives](#cryptographic-primitives)
6. [Consensus Security](#consensus-security)
7. [Execution Security](#execution-security)
8. [Network Security](#network-security)
9. [Bridge Security](#bridge-security)
10. [RPC Security](#rpc-security)
11. [Mining Security](#mining-security)
12. [Storage Security](#storage-security)
13. [Known Limitations](#known-limitations)
14. [Fuzz Testing](#fuzz-testing)
15. [Responsible Disclosure](#responsible-disclosure)

---

## Overview

PIChain is a custom Layer 1 blockchain written in Rust. This document describes the
threat model, security invariants, trust boundaries, and known limitations of the
system. It is intended for auditors, security researchers, and contributors.

The codebase is organized into 9 core crates and 3 tool binaries:

| Crate | Purpose |
|-------|---------|
| `pichain-crypto` | Ed25519, BLS12-381, Blake3, Poseidon hashing |
| `pichain-types` | Transaction, Block, Account, Object, Token, NFT, DEX types |
| `pichain-storage` | RocksDB persistence, Jellyfish Merkle Tree (JMT), column families |
| `pichain-execution` | Mempool, Block-STM parallel executor, WASM VM, sub-executors |
| `pichain-consensus` | DAG+Bullshark with Avalanche fast-path, staking, validator set |
| `pichain-mining` | BBP pi-digit computation, PoW anti-spam, difficulty adjustment |
| `pichain-network` | libp2p (QUIC/TCP), GossipSub 1.1, Kademlia, Turbine, Bridge |
| `pichain-rpc` | Axum 0.7 REST + WebSocket server, rate limiting |
| `pichain-node` | Node orchestration, pipeline, shutdown |

| Binary | Purpose |
|--------|---------|
| `pichain` | Full node (validator/non-validator) |
| `pichain-cli` | Command-line wallet and admin tool |
| `pichain-miner` | Standalone miner connecting via RPC |

---

## Threat Model

### T1: Validator Collusion

**Threat:** A coalition of validators controlling >1/3 of stake could halt consensus
or >2/3 could finalize malicious blocks.

**Mitigations:**
- Bullshark BFT tolerates up to f < n/3 Byzantine validators.
- Anti-concentration staking: velocity limiting caps stake growth per epoch to prevent
  rapid accumulation by a single entity.
- Validator set capped at MAX_VALIDATORS (3,141) to maintain decentralization.
- Epoch-based rotation (EPOCH_LENGTH = 31,415 blocks) limits sustained control.

### T2: Bridge Exploits

**Threat:** Compromised bridge relayers could mint unbacked wrapped tokens, drain
custodial wallets, or submit fraudulent attestations.

**Mitigations:**
- 2/3+1 quorum required for attestation confirmation.
- Circuit breaker: automatic pause when hourly volume exceeds threshold
  (DEFAULT_MAX_HOURLY_VOLUME = 1e18 base units).
- Minimum confirmation blocks (MIN_BRIDGE_CONFIRMATION_BLOCKS = 5,732) before
  execution (~30 minutes at 314ms blocks).
- Per-chain confirmation requirements: Ethereum 12, Solana 32, Bitcoin 6.
- Relayer staking with slashing for incorrect attestations.
- Pending transfer cap (DEFAULT_MAX_PENDING_TRANSFERS = 10,000) prevents memory
  exhaustion.
- Bridge can be admin-paused in emergencies.

### T3: Mempool Denial of Service

**Threat:** An attacker floods the mempool with transactions to exhaust memory,
prevent legitimate transactions from being included, or stall block production.

**Mitigations:**
- Global pool size limit: 100,000 transactions.
- Per-sender limit: 1,000 pending transactions.
- Transaction TTL: 120 seconds.
- Maximum transaction size: 512KB (prevents oversized code/data payloads).
- Minimum base fee enforcement (min_base_fee = 1 base unit).
- Chain-ID enforcement rejects transactions targeting other networks.
- Low-fee eviction when the pool is full.
- Signature verification at mempool ingress rejects invalid transactions immediately.
- Mining proof size capped at 100KB.

### T4: RPC Abuse

**Threat:** External callers abuse public RPC endpoints to exhaust server resources,
perform reconnaissance, or inject malicious transactions.

**Mitigations:**
- Per-IP rate limiting with configurable windows and request caps.
- Maximum tracked IPs (50,000) prevents OOM from distributed attacks.
- Periodic cleanup of stale rate-limit buckets.
- IPv6-aware loopback detection (including IPv4-mapped `::ffff:127.0.0.1`).
- Admin-only endpoints restricted to loopback IPs.
- Body size limits via `DefaultBodyLimit`.
- Transaction size validation before mempool insertion.
- Thread-pool protection on expensive queries (DEX swap quotes, account lookups)
  to prevent blocking the async runtime.

### T5: Mining Manipulation

**Threat:** A miner submits fraudulent pi-digit proofs, pre-computes proofs to
stockpile rewards, or manipulates difficulty.

**Mitigations:**
- Anchor block hash in mining proofs ties computation to a recent block, preventing
  pre-computation and stockpiling.
- PoW nonce required: `blake3(digits || pow_nonce || anchor_block_hash) < target`,
  providing anti-spam even though difficulty is minimal (8-bit minimum).
- Difficulty adjustment only every 10 proofs, with maximum 2x adjustment per period.
- Digit computation verified on-chain against known BBP algorithm results.
- Mining proof transaction size limited to 100KB.

### T6: Replay Attacks

**Threat:** A transaction valid on one chain or at one point in time is resubmitted
on another chain or after state changes.

**Mitigations:**
- `chain_id` embedded in `TransactionData` and included in the signed message.
- Per-account nonces enforce strict monotonic ordering.
- Bridge transfers include a nonce field preventing cross-chain replay.
- Mempool chain-ID enforcement rejects mismatched transactions.

### T7: State Corruption

**Threat:** A crash or bug corrupts on-chain state, leading to divergence between
validators or loss of user funds.

**Mitigations:**
- Jellyfish Merkle Tree (JMT) provides authenticated state with Poseidon hashes.
- Atomic JMT writes (STOR-220 fix) ensure state root consistency.
- Block header includes triple Merkle roots: `tx_root`, `state_root`, `receipts_root`.
- Canonical binary encoding for block header hashing (not JSON) prevents
  non-deterministic serialization.
- `overflow-checks = true` in release profile catches arithmetic bugs.
- Executor snapshot/restore for sub-executor state consistency.

---

## Trust Boundaries

```
                                    UNTRUSTED
    +--------+      +-----+      +---------+      +-----------+
    |  User  | ---> | RPC | ---> | Mempool | ---> | SigVerify |
    +--------+      +-----+      +---------+      +-----------+
                      |                                 |
                      | rate-limit                      | batch verify
                      | body-limit                      | Ed25519
                      | IP-filter                       |
                                                        v
                                    SEMI-TRUSTED
                                  +-----------+
                                  |  Banking  |
                                  | (Executor)|
                                  +-----------+
                                        |
                                        | checked arithmetic
                                        | gas metering
                                        | nonce ordering
                                        | WASM sandboxing
                                        v
                                    TRUSTED
                            +---------------+
                            |   Consensus   |
                            | (Bullshark)   |
                            +---------------+
                                    |
                                    | BFT agreement
                                    | quorum signatures
                                    v
                            +---------------+
                            |   Storage     |
                            | (RocksDB+JMT) |
                            +---------------+
```

**Boundary 1: User to RPC** — All input is untrusted. Rate limiting, body size
limits, hex input validation, and IP filtering are applied before any processing.

**Boundary 2: RPC to Mempool** — Transactions are size-checked, chain-ID validated,
fee-checked, and signature-verified before entering the mempool.

**Boundary 3: Mempool to Executor** — The pipeline's SigVerify stage performs batch
Ed25519 verification using `ed25519-dalek` with `batch` feature. Only verified
transactions proceed to the Banking stage.

**Boundary 4: Executor to Consensus** — The executor produces blocks with computed
state roots. Consensus (Bullshark BFT) ensures validators agree on block ordering.
Block headers use canonical binary encoding for deterministic hashing.

**Boundary 5: Consensus to Storage** — Finalized blocks are written atomically to
RocksDB with JMT state updates. STOR-220 ensures atomic writes.

---

## Security Invariants

### SI-1: Safe Arithmetic

All arithmetic in executors, fee calculations, staking, mining rewards, and token
operations uses Rust's checked arithmetic to prevent overflow/underflow:

- `checked_add` / `checked_sub` / `checked_mul` for all balance and amount operations.
- 85+ call sites across 16 files (as of last audit).
- `u128` casting for intermediate calculations where products may exceed `u64::MAX`
  (e.g., EIP-1559 base fee computation).
- Release profile has `overflow-checks = true` as a defense-in-depth backstop.

**Files:** `executor.rs`, `dex_executor.rs`, `token_executor.rs`, `nft_executor.rs`,
`launchpad_executor.rs`, `fee.rs`, `node_state.rs`, `difficulty.rs`, `account.rs`,
`dex.rs`, `launchpad.rs`, `onchain.rs`, `registry.rs`, `wasm_vm.rs`, `evm.rs`,
`contract_store.rs`.

### SI-2: Hex Input Validation

All hex string inputs from RPC are length-validated before calling `hex::decode` to
prevent allocation-based DoS:

- Address inputs checked for exactly 40 hex characters (20 bytes).
- Transaction hash inputs checked for exactly 64 hex characters (32 bytes).
- The `strip_0x` helper normalizes `0x`, `0X`, `Pi314`, `pi314` prefixes.

**Files:** `server.rs`, `main.rs`, `ed25519.rs`, `launchpad.rs`.

### SI-3: Signature Verification

Signatures are verified at two points in the pipeline:

1. **Mempool ingress:** Each transaction's Ed25519 signature is verified against the
   sender's public key before pool admission.
2. **Pipeline Stage 1 (SigVerify):** Batch verification using `ed25519-dalek` batch
   feature with `rayon` parallelism before forwarding to the Banking stage.

Bridge attestation signatures are separately verified per-relayer before counting
toward quorum.

**Files:** `mempool.rs`, `pipeline.rs`, `bridge.rs`, `engine.rs`, `dag.rs`,
`block_producer.rs`, `sync.rs`, `ed25519.rs`, `bls.rs`, `lib.rs` (crypto).

### SI-4: TOCTOU Protection

The faucet endpoint re-checks balance and cooldown conditions under a write lock to
prevent time-of-check-to-time-of-use races where concurrent requests could drain the
faucet beyond intended limits.

**Files:** `node_state.rs`, `server.rs`.

### SI-5: Bridge Quorum and Circuit Breaker

- Transfers require 2/3+1 relayer attestations before execution.
- Circuit breaker automatically pauses the bridge when rolling hourly volume exceeds
  `DEFAULT_MAX_HOURLY_VOLUME` (1e18 base units, approximately 1 billion PI).
- Minimum confirmation blocks (5,732 blocks, ~30 minutes) prevent same-block exploits.
- Pending transfer cap (10,000) prevents memory exhaustion.
- Admin override allows emergency pause/resume.

**Files:** `bridge.rs`.

### SI-6: Rate Limiting

The RPC server implements tiered rate limiting:

| Tier | Operations | Rate |
|------|-----------|------|
| Read | `getBalance`, `getAccount`, `getBlock`, `getTransaction` | 100/s per IP |
| Write | `submitTransaction`, `faucet` | 20/s per IP |
| Expensive | `simulateTransaction`, DEX swap quotes | 10/s per IP |

Rate limiter tracks up to 50,000 unique IPs before rejecting unknown callers.
Periodic cleanup removes stale entries. Loopback IPs bypass rate limiting for admin
operations.

**Files:** `server.rs`.

### SI-7: NAT Detection

The network layer uses libp2p's AutoNAT protocol for NAT traversal detection. Nodes
behind NAT are detected and relay connections are established to maintain
connectivity.

**Files:** `swarm.rs`.

### SI-8: Replay Protection

- **Intra-chain:** Per-account nonces enforce strict monotonic ordering. The mempool
  tracks `next_nonce` per sender and rejects stale or duplicate nonces.
- **Cross-chain:** `chain_id` is embedded in `TransactionData` and included in the
  signed message hash, preventing cross-chain replay.
- **Bridge:** Each `BridgeTransfer` has a `nonce` field preventing duplicate minting.

**Files:** `transaction.rs`, `mempool.rs`, `bridge.rs`.

---

## Cryptographic Primitives

| Primitive | Library | Usage |
|-----------|---------|-------|
| Ed25519 | `ed25519-dalek 2.x` | Transaction signing, batch verification |
| BLS12-381 | `blst 0.3` | Consensus aggregate signatures |
| Blake3 | `blake3 1.x` | Block hashing, PoW, Merkle trees (tx_root, receipts_root) |
| Poseidon | Custom (`pichain-crypto`) | JMT state root (ZK-friendly) |
| SHA-256 | `sha2 0.10` | Compatibility hashing |

Key management uses `zeroize` (with derive feature) to scrub private keys from
memory on drop.

---

## Consensus Security

- **Protocol:** DAG+Bullshark BFT with Avalanche fast-path for optimistic finality.
- **Fault tolerance:** f < n/3 Byzantine validators.
- **Epoch rotation:** Every 31,415 blocks.
- **Staking anti-concentration:** Velocity limiting caps epoch-to-epoch stake growth,
  preventing rapid validator dominance.
- **Finality:** Bullshark provides deterministic finality; Avalanche fast-path
  provides probabilistic pre-confirmation.

---

## Execution Security

- **Parallel execution:** Block-STM (Sealevel-inspired) with read-write conflict
  detection. Transactions declare virtual address dependencies for scheduling.
- **Gas metering:** EIP-1559 dynamic base fee with MAX_BLOCK_GAS = 100,000,000.
- **Fee distribution:** 25% burned, 25% to mining pool, 50% to block proposer.
- **WASM sandboxing:** Wasmtime 29 with bounded memory (state changes capped at 1MB
  per contract call).
- **Sub-executor isolation:** DEX, Token, NFT, Launchpad executors use snapshot/restore
  to prevent partial state corruption on failure.
- **MEV resistance:** DEX executor uses block-start reserve snapshots for price impact
  calculation, reducing sandwich attack effectiveness.

---

## Network Security

- **Transport:** libp2p with Noise encryption over QUIC and TCP/Yamux.
- **Peer discovery:** Kademlia DHT with GossipSub 1.1 for message propagation.
- **Eclipse protection:** Peer count monitoring with alerts for low unique-peer counts.
- **Block propagation:** Turbine protocol with Reed-Solomon erasure coding.
- **State sync:** Multi-layered security against malicious peers and state poisoning.
- **P2P rate limiting:** Per-peer message rate enforcement.

---

## Bridge Security

The cross-chain bridge supports Ethereum, BNB Chain, Arbitrum, Base, Solana, and
Bitcoin via a lock/mint mechanism:

1. User locks tokens on the source chain.
2. Bridge relayers observe and submit attestations.
3. Once 2/3+1 quorum is reached, wrapped tokens mint on the destination chain.
4. Reverse: burn wrapped tokens to release originals.

**Security controls:**
- Relayer bonding with slashing for dishonest behavior.
- Circuit breaker (hourly volume cap).
- Minimum confirmation blocks per chain.
- Pending transfer cap.
- Admin emergency pause.
- Transfer nonces prevent replay.

---

## RPC Security

- Axum 0.7 framework with `tower-http` CORS and tracing middleware.
- Per-IP rate limiting with tiered thresholds.
- Admin endpoints restricted to loopback addresses (IPv6-aware).
- Body size limits prevent oversized request DoS.
- `strip_0x` normalizes hex prefixes before parsing.
- Thread-pool isolation for expensive queries prevents async runtime starvation.
- Reverse tabnapping protection on web UI links.

---

## Mining Security

PIChain's mining is based on BBP (Bailey-Borwein-Plouffe) pi-digit computation:

- **Rewards** are proportional to digits computed, NOT to PoW difficulty.
- **PoW** is anti-spam only: fixed 8-bit minimum (~256 nonce attempts).
- **Anchor block hash** ties proofs to recent blocks, preventing pre-computation.
- **Difficulty adjustment** occurs every 10 proofs with max 2x change.
- **Proof size** capped at 100KB per mining transaction.
- **Profile system:** laptop/desktop/server/max auto-configures thread counts.

---

## Storage Security

- **RocksDB** with multi-threaded column families for account, block, transaction,
  and state data.
- **Jellyfish Merkle Tree (JMT)** with Poseidon hashing for authenticated state.
- **Atomic writes:** STOR-220 fix ensures JMT state root updates and leaf writes
  are atomic within a single RocksDB WriteBatch.
- **Lazy cache loading:** Executor cache initializes from storage on restart when
  block height > 0, preventing stale-cache divergence.
- **Mempool nonce recovery:** On restart, SenderQueue loads next_nonce from on-chain
  state rather than defaulting to 0.

---

## Known Limitations

### L1: Single-Node Testnet

The current live deployment is a single-node testnet. Multi-node consensus has been
tested in devnet integration tests but not in prolonged adversarial conditions.

**Mitigation:** 42 devnet integration tests cover multi-phase scenarios. Full
multi-node testnet is planned before mainnet launch.

### L2: Bridge Relayer Centralization

Bridge relayers are currently a small trusted set. A compromise of 2/3+1 relayers
would allow fraudulent minting.

**Mitigation:** Circuit breaker caps hourly volume. Long-term: transition to
decentralized relayer set with economic incentives and slashing.

### L3: WASM VM Feature Completeness

The WASM smart contract VM (Wasmtime) is functional but the host function API is
still maturing. Some advanced features may have incomplete validation.

**Mitigation:** State change cap (1MB per call). Host function registration returns
Results for error handling. Further audit of the WASM interface is planned.

### L4: EVM Compatibility

The `pichain-evm` crate uses `revm 19` for EVM execution. This is a secondary
execution environment and has received less audit attention than the native executor.

**Mitigation:** EVM is currently opt-in. Dedicated EVM security audit planned.

### L5: No Formal Verification

The consensus protocol and critical state transitions have not been formally verified.

**Mitigation:** 538+ unit tests, 42 devnet integration tests, and fuzz targets
provide empirical coverage. Formal verification is on the roadmap.

---

## Fuzz Testing

Fuzz targets are located in `/fuzz/fuzz_targets/`:

| Target | Description |
|--------|-------------|
| `tx_deserialize` | Fuzz `SignedTransaction` deserialization from arbitrary bytes (JSON + raw) |
| `block_deserialize` | Fuzz `Block` deserialization from arbitrary bytes (JSON) |

Run with:
```bash
cargo +nightly fuzz run tx_deserialize -- -max_len=65536
cargo +nightly fuzz run block_deserialize -- -max_len=65536
```

Additional fuzz targets planned:
- Mempool insertion/eviction under adversarial inputs.
- WASM bytecode validation.
- Bridge attestation parsing.
- DEX swap calculation edge cases.

---

## Responsible Disclosure

If you discover a security vulnerability in PIChain, please report it responsibly:

1. **Email:** security@pichain.network
2. **PGP:** Available at https://pichain.network/.well-known/security.txt
3. **Scope:** All code in the `pichain` repository, including crates, tools, and
   deployment configurations.
4. **Out of scope:** Third-party dependencies (report upstream), social engineering,
   DoS against test infrastructure.

### Disclosure Timeline

| Phase | Duration |
|-------|----------|
| Acknowledgment | Within 48 hours |
| Initial assessment | Within 7 days |
| Fix development | Within 30 days for critical, 90 days for others |
| Public disclosure | 90 days after report, or after fix is deployed |

### Severity Classification

| Severity | Description | Examples |
|----------|-------------|---------|
| Critical | Loss of funds, consensus break, total system compromise | Double-spend, bridge drain, validator key extraction |
| High | Significant impact requiring coordinated response | DoS of all nodes, state corruption, partial fund loss |
| Medium | Limited impact or requires unlikely preconditions | Information leak, non-critical DoS, edge-case bugs |
| Low | Minimal impact, defense-in-depth improvements | Code quality, non-exploitable edge cases |

### Bug Bounty

A formal bug bounty program will be announced prior to mainnet launch. Researchers
who responsibly disclose valid vulnerabilities before the program launches will be
retroactively eligible for rewards.

---

## Audit History

See [AUDIT_LOG.md](../AUDIT_LOG.md) for a detailed record of internal audit rounds,
findings, fixes, and third-party audit status.

---

## References

- [PIChain Whitepaper](../WHITEPAPER.md)
- [Investor Deck](../INVESTOR_DECK.md)
- [Devnet Test Script](../scripts/devnet-test.sh)
- [Bridge Configuration](../bridge-config.toml)
