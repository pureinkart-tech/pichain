# PIChain MEV Research Bot

**INTERNAL USE ONLY** — This tool exists to study MEV attack vectors on PIChain
and validate that protocol-level protections are effective. It is NOT designed
for deployment against other users.

## MEV Attack Surface Analysis

### Already Protected
1. **No public mempool** — RPC only exposes `mempool_size`, not pending txs
2. **Block-start reserve snapshots** — Price impact measured against pre-block state
3. **LP lock period** (1,911 blocks) — No atomic add+remove manipulation
4. **Ceiling-division fees** — No zero-fee micro-swaps
5. **Per-sender rate limiting** — 10 tx/sec max
6. **Slippage protection** — User-set `min_amount_out`

### Attack Vectors This Bot Studies
1. **Launch graduation sniping** — Detect launches near target, buy immediately on DEX
2. **New token first-buy** — Monitor for new launches and be first buyer on curve
3. **Post-block swap arbitrage** — Detect price deviations after confirmed swaps

### Why This Bot Is Hard to Replicate
- Requires deep knowledge of PIChain's transaction format and signing
- No standard MEV infrastructure (no Flashbots, no mempool API)
- Chain-specific canonical byte encoding for transaction signing
- 314ms block time makes timing extremely tight
- Block-start reserve snapshots prevent classical sandwich attacks
