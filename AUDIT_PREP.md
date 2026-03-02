# PIChain Security Audit Preparation Package

## Overview

This document provides auditors with the information needed to conduct a thorough security audit of PIChain, a Layer 1 blockchain implementation in Rust.

**Codebase**: ~46,000 lines of Rust across 83 source files
**Architecture**: Narwhal DAG + Bullshark consensus, Block-STM parallel execution, RocksDB + JMT storage
**Test Suite**: 540 unit tests + 42 integration tests + 6 fuzz targets

## Scope

### Critical (highest priority)
| Crate | Purpose | Lines | Key Files |
|-------|---------|-------|-----------|
| `pichain-crypto` | Ed25519/BLS signatures, BLAKE3 hashing | ~1K | `keypair.rs`, `multisig.rs` |
| `pichain-execution` | Block-STM executor, token/DEX/NFT/bridge logic | ~8.1K | `executor.rs`, `block_stm.rs`, `dex_executor.rs` |
| `pichain-consensus` | DAG+Bullshark aBFT, Avalanche fast-path, staking | ~3.5K | `engine.rs`, `bullshark.rs`, `avalanche.rs` |
| `pichain-types` | Transaction types, genesis, token/NFT/DEX structs | ~4.2K | `transaction.rs`, `genesis.rs` |

### High priority
| Crate | Purpose | Lines | Key Files |
|-------|---------|-------|-----------|
| `pichain-storage` | RocksDB persistence, JMT state proofs | ~3.2K | `state.rs`, `jmt_db.rs`, `migration.rs` |
| `pichain-network` | libp2p P2P, bridge relayer, circuit breaker | ~4.8K | `swarm.rs`, `bridge.rs`, `sync.rs` |
| `pichain-rpc` | Axum HTTP/WS API, rate limiting | ~12.4K | `server.rs`, `ws.rs` |

### Medium priority
| Crate | Purpose | Lines |
|-------|---------|-------|
| `pichain-mining` | BBP algorithm, proof verification | ~2.3K |
| `pichain-node` | CLI, state coordination, block production | ~16.4K |

## Known Vulnerabilities (from cargo audit)

### Active issues
1. **ring 0.16.20** (RUSTSEC-2025-0009): AES panic with overflow checking. Transitive via libp2p → rcgen. Risk: LOW (we don't use ring's AES directly).

### Resolved
- **wasmtime**: Upgraded from v29 → v36.0.6 (LTS). All 5 advisories (RUSTSEC-2026-0020, 0021, 2025-0046, 0118, 2026-0006) resolved.

### Mitigation plan
- `ring`: Awaiting libp2p upgrade to ring 0.17+

## Security Model

### Trust Boundaries
1. **P2P Network** (untrusted): All peer messages (blocks, transactions, certificates) are fully validated before processing
2. **RPC API** (semi-trusted): Rate-limited, CSP-protected. Write endpoints restricted to localhost where security-critical
3. **WASM/EVM** (untrusted): Sandboxed execution with gas metering and memory limits
4. **Bridge** (multi-party trust): Requires 2/3+1 relayer attestations with Ed25519 signature verification

### Key Invariants
1. **Supply invariant**: Total supply is fixed at 3,141,592,653 PI. All arithmetic uses `checked_add`/`checked_sub`/`checked_mul`.
2. **Nonce ordering**: Transactions execute in strict nonce order per sender. Mempool rejects far-future nonces.
3. **BFT safety**: Consensus requires 3f+1 validators. Equivocation (double-vote) triggers automatic slashing.
4. **Bridge circuit breaker**: Hourly volume capped at 1B PI. Per-transfer max 10% of hourly limit. Auto-recovery after 30-minute cooldown.
5. **Staking anti-concentration**: Max 33.33% of total stake per validator, max 10% per delegator address.

### Attack Surfaces
1. **P2P message injection**: Mitigated by signature verification at ingress, chain_id checks, timestamp bounds
2. **Transaction flooding**: Mitigated by mempool capacity limits, nonce validation, gas fee requirements
3. **Eclipse attacks**: Mitigated by MIN_SAFE_PEER_COUNT=8, per-peer byte rate limiting (100MB/60s), GossipSub mesh diversity
4. **Bridge drain**: Mitigated by circuit breaker (hourly cap), multi-relayer quorum (2/3+1), confirmation delay (5,732 blocks)
5. **Integer overflow**: All critical arithmetic uses checked operations (85+ call sites across 16 files)
6. **Reentrancy**: Block-STM executor uses read-write set conflict detection; no recursive contract calls

## Internal Audit History

34 completed audit rounds documented in `AUDIT_LOG.md`:
- Phase 1 (Foundation): 10 rounds covering crypto, types, genesis, storage, consensus, execution, mining, network, RPC
- Phase 2 (Deep Dive): 10 rounds covering cross-crate unwrap audit, DEX/token/NFT executors, WASM VM, bridge, JMT, staking
- Phase 3 (Hardening): 14 rounds covering safe arithmetic, L1 security analysis, Block-STM, Turbine, state sync, Avalanche, bridge circuit breaker

## Fuzz Targets

6 fuzz targets in `/fuzz/fuzz_targets/`:
1. `tx_deserialize` — SignedTransaction JSON parsing (30+ variant enum)
2. `block_deserialize` — Block + BlockHeader parsing
3. `rpc_json_input` — All RPC request body types
4. `mining_proof` — MiningProof construction and round-trip
5. `genesis_config` — GenesisConfig parsing + validation + hash
6. `crypto_inputs` — Signature verification, key derivation, hashing

Run: `cargo +nightly fuzz run <target> -- -max_total_time=3600`

## Test Coverage

```
pichain-crypto:        76 tests
pichain-types:         24 tests
pichain-storage:       158 tests
pichain-consensus:     2 tests
pichain-execution:     80 tests
pichain-mining:        9 tests
pichain-network:       79 tests (includes 25 bridge tests)
pichain-rpc:           54 tests
pichain-node:          64 tests
Integration:           42 tests (scripts/devnet-test.sh)
Total:                 546+ tests
```

## Files of Interest for Auditors

### Highest risk (money/consensus critical)
- `crates/pichain-execution/src/executor.rs` — All transaction execution logic
- `crates/pichain-execution/src/dex_executor.rs` — AMM swap/liquidity math
- `crates/pichain-consensus/src/engine.rs` — DAG + Bullshark consensus core
- `crates/pichain-consensus/src/bullshark.rs` — BFT commit rule
- `crates/pichain-network/src/bridge.rs` — Cross-chain bridge + circuit breaker
- `crates/pichain-types/src/genesis.rs` — Supply distribution + validation

### Crypto primitives (correctness critical)
- `crates/pichain-crypto/src/keypair.rs` — Ed25519 operations
- `crates/pichain-crypto/src/multisig.rs` — Multi-signature verification

### Network security
- `crates/pichain-network/src/swarm.rs` — P2P security (rate limiting, peer scoring, eclipse defense)
- `crates/pichain-node/src/main.rs` — Block validation pipeline (signature, tx_root, timestamps)

## Build & Test

```bash
# Full test suite
cargo test --workspace

# Clippy (zero warnings)
cargo clippy --workspace

# Audit
cargo audit

# Fuzz (requires nightly)
cd fuzz && cargo +nightly fuzz run tx_deserialize -- -max_total_time=300
```

## Contact

For questions during the audit, contact the development team directly.
