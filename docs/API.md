# PIChain RPC API Reference

Base URL: `http://localhost:8314`

All addresses are 40-character hex strings (no `0x` prefix). Hash values are 64-character hex strings.
Mint IDs and collection IDs are 64-character hex strings (32 bytes). Balances are in base units
(1 PI = 1,000,000,000 base units, 9 decimal places).

Rate limits: 100 req/sec for reads, 20 req/sec for writes (tx submit, faucet, bridge), 10 req/sec
for expensive queries (swap quotes, block ranges, richlist, events, proofs, portfolio).

---

## Chain Info

### GET /api/v1/info

Returns current node status.

```bash
curl http://localhost:8314/api/v1/info
```

**Response:**

```json
{
  "chain_id": 31415,
  "version": "0.1.0",
  "block_height": 1042,
  "peer_count": 3,
  "is_syncing": false,
  "state_root": "a1b2c3d4e5f6...64 hex chars...",
  "base_fee": 1000,
  "total_burned": 50000,
  "mempool_size": 7
}
```

### GET /api/v1/block/:height_or_latest

Fetch a full block by height, or use `latest` for the most recent block.

```bash
# By height
curl http://localhost:8314/api/v1/block/42

# Latest block
curl http://localhost:8314/api/v1/block/latest
```

**Response:**

```json
{
  "found": true,
  "height": 42,
  "epoch": 1,
  "round": 42,
  "parent_hash": "abcdef...64 hex chars...",
  "tx_root": "123456...64 hex chars...",
  "state_root": "789abc...64 hex chars...",
  "proposer": "aabbccdd...40 hex chars...",
  "timestamp_ms": 1709312400000,
  "gas_used": 21000,
  "base_fee": 1000,
  "tx_count": 3,
  "pi_burned": 500,
  "tx_hashes": [
    "deadbeef...64 hex chars...",
    "cafebabe...64 hex chars..."
  ]
}
```

When block is not found, `found` is `false` and optional fields are omitted. Transaction hashes are
capped at 1000 per block response.

### GET /api/v1/blocks

Fetch a range of blocks (summaries). Returns newest-first.

| Query Param | Type | Default | Description |
|-------------|------|---------|-------------|
| `from`      | u64  | auto    | Start height |
| `to`        | u64  | latest  | End height |
| `limit`     | u64  | 20      | Max blocks (clamped to 1-50) |

```bash
curl "http://localhost:8314/api/v1/blocks?limit=5"
curl "http://localhost:8314/api/v1/blocks?from=100&to=110"
```

**Response:**

```json
{
  "blocks": [
    {
      "height": 110,
      "tx_count": 2,
      "gas_used": 42000,
      "base_fee": 1000,
      "pi_burned": 200,
      "timestamp_ms": 1709312400000
    }
  ],
  "total_height": 1042
}
```

### GET /api/v1/header/:height

Block header only (for light clients).

```bash
curl http://localhost:8314/api/v1/header/42
```

**Response:**

```json
{
  "height": 42,
  "epoch": 1,
  "parent_hash": "abcdef...64 hex chars...",
  "state_root": "789abc...64 hex chars...",
  "tx_root": "123456...64 hex chars...",
  "tx_count": 3,
  "gas_used": 21000,
  "base_fee": 1000,
  "pi_burned": 500,
  "proposer": "aabbccdd...40 hex chars...",
  "timestamp_ms": 1709312400000
}
```

### GET /health

Structured health check.

```bash
curl http://localhost:8314/health
```

**Response:**

```json
{
  "status": "healthy",
  "version": "0.1.0",
  "chain_id": 31415,
  "block_height": 1042,
  "is_syncing": false,
  "uptime_secs": 86400,
  "requests_total": 150000
}
```

### GET /metrics

Prometheus-compatible metrics. **Restricted to localhost only** for security.

```bash
curl http://localhost:8314/metrics
```

**Response (text/plain):**

```
# HELP pichain_block_height Current block height
# TYPE pichain_block_height gauge
pichain_block_height 1042
# HELP pichain_peer_count Connected peers
# TYPE pichain_peer_count gauge
pichain_peer_count 3
# HELP pichain_base_fee Current base fee in base units
# TYPE pichain_base_fee gauge
pichain_base_fee 1000
# HELP pichain_total_burned Total PI burned in base units
# TYPE pichain_total_burned counter
pichain_total_burned 50000
# HELP pichain_mining_frontier Current mining frontier position
# TYPE pichain_mining_frontier gauge
pichain_mining_frontier 100000
# HELP pichain_mining_digits_verified Total PI hex digits verified
# TYPE pichain_mining_digits_verified counter
pichain_mining_digits_verified 500000
# HELP pichain_mining_unique_miners Unique miner addresses
# TYPE pichain_mining_unique_miners gauge
pichain_mining_unique_miners 42
# HELP pichain_uptime_seconds Node uptime in seconds
# TYPE pichain_uptime_seconds counter
pichain_uptime_seconds 86400
# HELP pichain_requests_total Total RPC requests served
# TYPE pichain_requests_total counter
pichain_requests_total 150000
# HELP pichain_requests_errors Total RPC request errors
# TYPE pichain_requests_errors counter
pichain_requests_errors 12
# HELP pichain_ws_subscribers Active WebSocket subscribers
# TYPE pichain_ws_subscribers gauge
pichain_ws_subscribers 5
# HELP pichain_consensus_round Current consensus round
# TYPE pichain_consensus_round gauge
pichain_consensus_round 1042
# HELP pichain_bullshark_commits Total Bullshark commits
# TYPE pichain_bullshark_commits counter
pichain_bullshark_commits 1040
# HELP pichain_certificates_produced Total certificates produced
# TYPE pichain_certificates_produced counter
pichain_certificates_produced 4160
# HELP pichain_validator_count Active validators
# TYPE pichain_validator_count gauge
pichain_validator_count 4
# HELP pichain_total_stake Total staked PI
# TYPE pichain_total_stake gauge
pichain_total_stake 10000000000000
```

---

## Accounts

### GET /api/v1/account/:address

Get account balance, nonce, and staking info.

```bash
curl http://localhost:8314/api/v1/account/aabbccdd11223344556677889900aabbccddeeff
```

**Response:**

```json
{
  "address": "aabbccdd11223344556677889900aabbccddeeff",
  "balance": 3140000000,
  "balance_pi": "3.140000000",
  "nonce": 5,
  "staked": 0,
  "locked_balance": 0,
  "found": true
}
```

When account is not found, `found` is `false`, all numeric fields are `0`.

### GET /api/v1/address/:address/transactions

Get transaction history for an address (newest-first).

| Query Param | Type  | Default | Description |
|-------------|-------|---------|-------------|
| `before`    | u64   | none    | Only return txs before this block height |
| `limit`     | usize | 50      | Max results (clamped to 200) |

```bash
curl "http://localhost:8314/api/v1/address/aabbccdd11223344556677889900aabbccddeeff/transactions?limit=10"

# Pagination: get next page using the lowest height from previous results
curl "http://localhost:8314/api/v1/address/aabbccdd11223344556677889900aabbccddeeff/transactions?before=500&limit=10"
```

**Response:**

```json
{
  "address": "aabbccdd11223344556677889900aabbccddeeff",
  "transactions": [
    {
      "tx_hash": "deadbeef...64 hex chars...",
      "height": 1040,
      "tx_index": 0
    },
    {
      "tx_hash": "cafebabe...64 hex chars...",
      "height": 1035,
      "tx_index": 2
    }
  ],
  "count": 2
}
```

### GET /api/v1/portfolio/:address

Get PI balance and all token balances for an address.

```bash
curl http://localhost:8314/api/v1/portfolio/aabbccdd11223344556677889900aabbccddeeff
```

**Response:**

```json
{
  "address": "aabbccdd11223344556677889900aabbccddeeff",
  "pi_balance": 3140000000,
  "tokens": [
    {
      "mint_id": "abcdef01...64 hex chars...",
      "balance": 1000000000
    }
  ]
}
```

---

## Transactions

### POST /api/v1/tx/submit

Submit a signed transaction. The transaction must be JSON-serialized, then hex-encoded.

```bash
curl -X POST http://localhost:8314/api/v1/tx/submit \
  -H "Content-Type: application/json" \
  -d '{"signed_tx_hex": "7b22646174...hex encoded JSON..."}'
```

**Request body:**

```json
{
  "signed_tx_hex": "<hex-encoded JSON of SignedTransaction>"
}
```

The hex payload decodes to a JSON `SignedTransaction` with these fields:

```json
{
  "data": {
    "sender": "aabbccdd11223344556677889900aabbccddeeff",
    "nonce": 0,
    "chain_id": 31415,
    "max_gas": 100000,
    "max_fee_per_gas": 2000,
    "kind": {
      "Transfer": {
        "to": "1122334455667788990011223344556677889900",
        "amount": 1000000000
      }
    }
  },
  "signature": "base64-encoded-ed25519-signature",
  "public_key": "base64-encoded-ed25519-pubkey"
}
```

**Validation order:** hex decode -> JSON deserialize -> chain_id check -> signature verify -> mempool insert.

Max transaction size: 512KB (1MB hex-encoded).

**Response (success):**

```json
{
  "tx_hash": "abc123...64 hex chars...",
  "status": "pending",
  "error": null
}
```

**Response (rejected):**

```json
{
  "tx_hash": "abc123...64 hex chars...",
  "status": "rejected",
  "error": "nonce too low: expected 5, got 3"
}
```

**Transaction kinds:**

| Kind | Description |
|------|-------------|
| `Transfer` | Send PI to another address |
| `Stake` | Stake PI with a validator |
| `Unstake` | Unstake PI from a validator |
| `MiningProof` | Submit BBP digit computation proof |
| `CreateToken` | Create a new token mint |
| `MintToken` | Mint tokens (requires mint authority) |
| `TransferToken` | Transfer tokens between accounts |
| `BurnToken` | Burn tokens |
| `ApproveToken` | Approve token spending allowance |
| `RevokeMintAuthority` | Permanently revoke minting capability |
| `FreezeTokenAccount` | Freeze a token account |
| `ThawTokenAccount` | Unfreeze a token account |
| `CreatePool` | Create a DEX liquidity pool |
| `AddLiquidity` | Add liquidity to a pool |
| `RemoveLiquidity` | Remove liquidity from a pool |
| `Swap` | Swap tokens on the DEX |
| `CreateLaunch` | Create a token launchpad |
| `ParticipateInLaunch` | Buy tokens from a launchpad |
| `SellFromLaunch` | Sell tokens back to a launchpad |
| `FinalizeLaunch` | Finalize a completed launch |
| `CreateNftCollection` | Create an NFT collection |
| `MintNft` | Mint an NFT |
| `TransferNft` | Transfer an NFT |
| `ListNft` | List an NFT for sale |
| `BuyNft` | Purchase a listed NFT |
| `DelistNft` | Remove an NFT listing |
| `CreateMultisig` | Create a multisig wallet |
| `ExecuteMultisig` | Execute a multisig transaction |
| `BridgeWithdraw` | Withdraw wrapped tokens to external chain |
| `DeployContract` | Deploy a smart contract |
| `ContractCall` | Call a smart contract |

### GET /api/v1/tx/:hash

Get transaction details and execution status.

```bash
curl http://localhost:8314/api/v1/tx/deadbeef0123456789abcdef0123456789abcdef0123456789abcdef01234567
```

**Response:**

```json
{
  "tx_hash": "deadbeef...64 hex chars...",
  "sender": "aabbccdd11223344556677889900aabbccddeeff",
  "nonce": 5,
  "kind": "Transfer",
  "status": "success",
  "gas_used": 21000,
  "block_height": 1042,
  "found": true
}
```

Possible `status` values: `"success"`, `"reverted: <reason>"`, `"out_of_gas"`, or `null` if no receipt.

### GET /api/v1/receipt/:hash

Get detailed transaction receipt with events.

```bash
curl http://localhost:8314/api/v1/receipt/deadbeef0123456789abcdef0123456789abcdef0123456789abcdef01234567
```

**Response:**

```json
{
  "tx_hash": "deadbeef...64 hex chars...",
  "status": "success",
  "gas_used": 21000,
  "base_fee": 1000,
  "events": [
    {
      "emitter": "aabbccdd11223344556677889900aabbccddeeff",
      "event_type": "transfer",
      "data_hex": "0123456789abcdef..."
    }
  ],
  "found": true
}
```

---

## Mining

### GET /api/v1/mining/status

Get current mining frontier, rewards, and difficulty info. This is the primary endpoint miners
use to determine what work to do.

```bash
curl http://localhost:8314/api/v1/mining/status
```

**Response:**

```json
{
  "frontier_position": 100000,
  "total_digits_verified": 500000,
  "next_position": 100000,
  "max_batch_at_position": 10000,
  "total_ranges": 50,
  "unique_miners": 42,
  "remaining_pool": 78539816339744830,
  "total_mined": 160000000000,
  "reward_per_digit": 3141,
  "emission_year": 1,
  "difficulty_bits": 8,
  "difficulty_target_hex": "00ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
  "anchor_block_hash": "abcdef...64 hex chars...",
  "base_fee": 1000
}
```

| Field | Description |
|-------|-------------|
| `frontier_position` | Lowest unverified hex digit position |
| `next_position` | Suggested starting position for new miners |
| `max_batch_at_position` | Max digits to compute at `next_position` without overlap |
| `difficulty_bits` | PoW difficulty (leading zero bits required, minimum 8) |
| `difficulty_target_hex` | PoW target hash as hex |
| `anchor_block_hash` | Latest block hash for PoW nonce grinding |
| `reward_per_digit` | Base units of PI earned per verified digit |
| `remaining_pool` | Remaining mining reward pool (base units) |
| `fee_income` | Cumulative transaction fee income added to mining pool |

---

## Tokens

### GET /api/v1/token/:mint_id

Get token mint information.

```bash
curl http://localhost:8314/api/v1/token/abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789
```

**Response:**

```json
{
  "mint_id": "abcdef01...64 hex chars...",
  "name": "PiCoin",
  "symbol": "PIC",
  "decimals": 9,
  "total_supply": 1000000000000,
  "max_supply": 10000000000000,
  "creator": "aabbccdd11223344556677889900aabbccddeeff",
  "has_mint_authority": true,
  "active": true,
  "found": true
}
```

### GET /api/v1/token/:mint_id/account/:address

Get a specific token balance for an address.

```bash
curl http://localhost:8314/api/v1/token/abcdef01...64hex.../account/aabbccdd...40hex...
```

**Response:**

```json
{
  "owner": "aabbccdd11223344556677889900aabbccddeeff",
  "mint_id": "abcdef01...64 hex chars...",
  "balance": 5000000000,
  "frozen": false,
  "found": true
}
```

### GET /api/v1/tokens

List all token mints on the chain.

```bash
curl http://localhost:8314/api/v1/tokens
```

**Response:**

```json
{
  "tokens": [
    {
      "mint_id": "abcdef01...64 hex chars...",
      "name": "PiCoin",
      "symbol": "PIC",
      "decimals": 9,
      "total_supply": 1000000000000,
      "max_supply": 10000000000000,
      "creator": "aabbccdd...40 hex chars...",
      "metadata_uri": "https://example.com/metadata.json",
      "has_mint_authority": true,
      "active": true
    }
  ]
}
```

### GET /api/v1/mint-nonce/:address

Get the mint creation nonce for an address (used for deterministic MintId derivation when
creating new tokens).

```bash
curl http://localhost:8314/api/v1/mint-nonce/aabbccdd11223344556677889900aabbccddeeff
```

**Response:**

```json
{
  "address": "aabbccdd11223344556677889900aabbccddeeff",
  "mint_nonce": 3
}
```

---

## DEX

### GET /api/v1/pools

List all liquidity pools.

```bash
curl http://localhost:8314/api/v1/pools
```

**Response:**

```json
{
  "pools": [
    {
      "pool_id": "fedcba98...64 hex chars...",
      "mint_a": "abcdef01...64 hex chars...",
      "mint_b": "01234567...64 hex chars...",
      "reserve_a": 500000000000,
      "reserve_b": 1000000000000,
      "lp_supply": 707106781186,
      "fee_bps": 30,
      "active": true
    }
  ]
}
```

### GET /api/v1/pool/:mint_a/:mint_b

Get pool details by token pair.

```bash
curl http://localhost:8314/api/v1/pool/abcdef01...64hex.../01234567...64hex...
```

**Response:**

```json
{
  "pool_id": "fedcba98...64 hex chars...",
  "mint_a": "abcdef01...64 hex chars...",
  "mint_b": "01234567...64 hex chars...",
  "reserve_a": 500000000000,
  "reserve_b": 1000000000000,
  "lp_supply": 707106781186,
  "fee_bps": 30,
  "active": true,
  "found": true
}
```

### GET /api/v1/swap/quote/:mint_in/:mint_out/:amount_in

Get a swap quote. Amount is in base units of the input token.

```bash
curl http://localhost:8314/api/v1/swap/quote/abcdef01...64hex.../01234567...64hex.../1000000000
```

**Response:**

```json
{
  "mint_in": "abcdef01...64 hex chars...",
  "mint_out": "01234567...64 hex chars...",
  "amount_in": 1000000000,
  "amount_out": 1980000000,
  "fee": 3000000,
  "price_impact_bps": 15,
  "found": true
}
```

`price_impact_bps` is the price impact in basis points (1 bps = 0.01%).

---

## Launchpad

### GET /api/v1/launches

List all token launches (sorted newest-first).

```bash
curl http://localhost:8314/api/v1/launches
```

**Response:**

```json
{
  "launches": [
    {
      "mint_id": "abcdef01...64 hex chars...",
      "launch_id": "fedcba98...64 hex chars...",
      "creator": "aabbccdd...40 hex chars...",
      "state": "Active",
      "tokens_for_sale": 1000000000000,
      "tokens_sold": 250000000000,
      "pi_raised": 500000000,
      "target_pi": 2000000000,
      "current_price": 2000,
      "percent_complete": 25.0,
      "max_per_address": 100000000000,
      "contributors": 12,
      "created_at_ms": 1709312400000,
      "name": "MyToken",
      "symbol": "MTK",
      "metadata_uri": "https://example.com/token.json",
      "decimals": 9,
      "launch_type": "BondingCurve",
      "base_price": 1000,
      "slope": 1,
      "price_scale": 1000000000
    }
  ]
}
```

Launch states: `"Active"`, `"TargetReached"`, `"Finalized"`, `"Cancelled"`.

Launch types: `"FairLaunch"` (fixed price), `"BondingCurve"` (price increases with demand).

### GET /api/v1/launch/:mint_id

Get detailed launch info for a specific token.

```bash
curl http://localhost:8314/api/v1/launch/abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789
```

**Response:**

```json
{
  "found": true,
  "mint_id": "abcdef01...64 hex chars...",
  "launch_id": "fedcba98...64 hex chars...",
  "creator": "aabbccdd...40 hex chars...",
  "state": "Active",
  "tokens_for_sale": 1000000000000,
  "tokens_sold": 250000000000,
  "pi_raised": 500000000,
  "target_pi": 2000000000,
  "current_price": 2000,
  "percent_complete": 25.0,
  "max_per_address": 100000000000,
  "contributors": 12,
  "created_at_ms": 1709312400000,
  "launch_type": "BondingCurve",
  "base_price": 1000,
  "slope": 1,
  "price_scale": 1000000000,
  "liquidity_bps": 8000,
  "token_liquidity_bps": 2000,
  "name": "MyToken",
  "symbol": "MTK",
  "metadata_uri": "https://example.com/token.json",
  "decimals": 9
}
```

---

## NFTs

### GET /api/v1/nft/collections

List all NFT collections.

```bash
curl http://localhost:8314/api/v1/nft/collections
```

**Response:**

```json
{
  "collections": [
    {
      "id": "abcdef01...64 hex chars...",
      "name": "PIChain Founders",
      "symbol": "PIFO",
      "creator": "aabbccdd...40 hex chars...",
      "max_supply": 1000,
      "total_minted": 42,
      "royalty_bps": 500
    }
  ],
  "count": 1
}
```

`royalty_bps` is the creator royalty in basis points (500 = 5%).

### GET /api/v1/nft/collection/:collection_id/items

List all NFTs in a collection.

```bash
curl http://localhost:8314/api/v1/nft/collection/abcdef01...64hex.../items
```

**Response:**

```json
{
  "items": [
    {
      "nft_id": "11223344...64 hex chars...",
      "collection_id": "abcdef01...64 hex chars...",
      "name": "Founder #1",
      "owner": "aabbccdd...40 hex chars...",
      "metadata_uri": "https://example.com/nft/1.json"
    }
  ],
  "count": 1
}
```

### GET /api/v1/nft/owner/:address

Get all NFTs owned by an address.

```bash
curl http://localhost:8314/api/v1/nft/owner/aabbccdd11223344556677889900aabbccddeeff
```

**Response:**

```json
{
  "nfts": [
    {
      "nft_id": "11223344...64 hex chars...",
      "collection_id": "abcdef01...64 hex chars...",
      "name": "Founder #1",
      "owner": "aabbccdd...40 hex chars...",
      "metadata_uri": "https://example.com/nft/1.json"
    }
  ],
  "count": 1
}
```

---

## Staking

### GET /api/v1/staking/validators

List all validators.

```bash
curl http://localhost:8314/api/v1/staking/validators
```

**Response:**

```json
{
  "validators": [
    {
      "address": "aabbccdd11223344556677889900aabbccddeeff",
      "stake": 1000000000000,
      "delegated": 500000000000,
      "commission_bps": 1000,
      "active": true,
      "uptime_bps": 9950,
      "blocks_proposed": 520
    }
  ],
  "count": 1
}
```

`commission_bps` is the validator commission in basis points (1000 = 10%).
`uptime_bps` is uptime percentage in basis points (9950 = 99.50%).

### GET /api/v1/staking/delegations/:address

Get delegations for a delegator address.

```bash
curl http://localhost:8314/api/v1/staking/delegations/aabbccdd11223344556677889900aabbccddeeff
```

**Response:**

```json
{
  "address": "aabbccdd11223344556677889900aabbccddeeff",
  "delegations": [
    {
      "validator": "1122334455667788990011223344556677889900",
      "amount": 500000000000,
      "rewards_earned": 12500000000
    }
  ]
}
```

### GET /api/v1/staking/rewards/:address

Get pending staking rewards for an address.

```bash
curl http://localhost:8314/api/v1/staking/rewards/aabbccdd11223344556677889900aabbccddeeff
```

**Response:**

```json
{
  "address": "aabbccdd11223344556677889900aabbccddeeff",
  "pending_rewards": 12500000000
}
```

---

## Bridge

The bridge supports wrapping ETH, SOL, BTC, and USDT into wrapped tokens on PIChain.

### GET /api/v1/bridge/status

Get bridge TVL and transfer stats.

```bash
curl http://localhost:8314/api/v1/bridge/status
```

**Response:**

```json
{
  "tokens": [
    {
      "symbol": "wETH",
      "total_supply": 10000000000000000000,
      "decimals": 18
    },
    {
      "symbol": "wBTC",
      "total_supply": 100000000,
      "decimals": 8
    }
  ],
  "total_transfers": 150,
  "addresses": {
    "eth": "0x742d35Cc6634C0532925a3b844Bc9e7595f2bD18",
    "sol": "BRjpCHtyQLeSJjRKsMdbbQWfQ2EH3ZTfHxKBxJfRzakr",
    "btc": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
    "usdt": "0x742d35Cc6634C0532925a3b844Bc9e7595f2bD18"
  }
}
```

### POST /api/v1/bridge/deposit-address

Get the custodial deposit address for a specific chain.

```bash
curl -X POST http://localhost:8314/api/v1/bridge/deposit-address \
  -H "Content-Type: application/json" \
  -d '{"chain": "eth", "pichain_address": "aabbccdd11223344556677889900aabbccddeeff"}'
```

**Request body:**

```json
{
  "chain": "eth",
  "pichain_address": "aabbccdd11223344556677889900aabbccddeeff"
}
```

Supported chains: `eth`, `sol`, `btc`, `usdt`.

**Response:**

```json
{
  "chain": "eth",
  "address": "0x742d35Cc6634C0532925a3b844Bc9e7595f2bD18",
  "pichain_address": "aabbccdd11223344556677889900aabbccddeeff",
  "note": "Send ETH to this address with your PIChain address as calldata. wETH will be minted after 12 confirmations."
}
```

### POST /api/v1/bridge/deposit-intent

Register a deposit intent (maps your external chain address to your PIChain address so the
bridge relayer can automatically credit you).

```bash
curl -X POST http://localhost:8314/api/v1/bridge/deposit-intent \
  -H "Content-Type: application/json" \
  -d '{
    "chain": "eth",
    "external_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f2bD18",
    "pichain_address": "aabbccdd11223344556677889900aabbccddeeff"
  }'
```

**Response:**

```json
{
  "success": true,
  "chain": "eth",
  "external_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f2bD18",
  "pichain_address": "aabbccdd11223344556677889900aabbccddeeff"
}
```

### POST /api/v1/bridge/withdraw

Request withdrawal of wrapped tokens to an external chain.

```bash
curl -X POST http://localhost:8314/api/v1/bridge/withdraw \
  -H "Content-Type: application/json" \
  -d '{
    "symbol": "wETH",
    "amount": 1000000000000000000,
    "destination_chain": "eth",
    "destination_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f2bD18",
    "pichain_address": "aabbccdd11223344556677889900aabbccddeeff"
  }'
```

Supported destination chains: `eth`, `sol`, `btc`.

**Response:**

```json
{
  "success": true,
  "status": "pending",
  "message": "Withdrawal queued. The bridge relayer will process it shortly.",
  "symbol": "wETH",
  "amount": 1000000000000000000,
  "destination_chain": "eth",
  "destination_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f2bD18"
}
```

### GET /api/v1/bridge/transfers

List recent bridge transfers.

| Query Param | Type   | Default | Description |
|-------------|--------|---------|-------------|
| `chain`     | string | none    | Filter by source chain |
| `limit`     | usize  | 50      | Max results (clamped to 200) |

```bash
curl "http://localhost:8314/api/v1/bridge/transfers?limit=10"
curl "http://localhost:8314/api/v1/bridge/transfers?chain=eth&limit=10"
```

**Response:**

```json
{
  "transfers": [
    {
      "chain": "eth",
      "tx_hash": "0xabc123...",
      "symbol": "wETH",
      "recipient": "aabbccdd11223344556677889900aabbccddeeff",
      "amount": 1000000000000000000,
      "timestamp": 1709312400
    }
  ]
}
```

### POST /api/v1/bridge/mint (localhost only)

Mint wrapped tokens. **Restricted to localhost** -- used by the bridge operator/relayer only.

```bash
curl -X POST http://localhost:8314/api/v1/bridge/mint \
  -H "Content-Type: application/json" \
  -d '{
    "symbol": "wETH",
    "recipient": "aabbccdd11223344556677889900aabbccddeeff",
    "amount": 1000000000000000000,
    "chain": "eth",
    "tx_hash": "0xabc123..."
  }'
```

### POST /api/v1/bridge/register-addresses (localhost only)

Register custodial deposit addresses. **Restricted to localhost** -- called by bridge relayer on
startup.

```bash
curl -X POST http://localhost:8314/api/v1/bridge/register-addresses \
  -H "Content-Type: application/json" \
  -d '{
    "eth": "0x742d35Cc6634C0532925a3b844Bc9e7595f2bD18",
    "sol": "BRjpCHtyQLeSJjRKsMdbbQWfQ2EH3ZTfHxKBxJfRzakr",
    "btc": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
    "usdt": "0x742d35Cc6634C0532925a3b844Bc9e7595f2bD18"
  }'
```

---

## Events

### POST /api/v1/events/query

Query on-chain events by topic hash or by address.

**Query by topic:**

```bash
curl -X POST http://localhost:8314/api/v1/events/query \
  -H "Content-Type: application/json" \
  -d '{"topic": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789", "limit": 50}'
```

**Query by address:**

```bash
curl -X POST http://localhost:8314/api/v1/events/query \
  -H "Content-Type: application/json" \
  -d '{"address": "aabbccdd11223344556677889900aabbccddeeff", "limit": 50}'
```

`limit` defaults to 50, max 200. You must provide either `topic` (64 hex chars) or `address`
(40 hex chars), not both.

**Response:**

```json
{
  "events": [
    {
      "tx_hash": "deadbeef...64 hex chars...",
      "height": 1042,
      "tx_index": 0
    }
  ],
  "count": 1
}
```

---

## Light Client

### GET /api/v1/proof/account/:address

Get a Jellyfish Merkle Tree (JMT) proof for an account. Used by light clients to verify account
state against the block header's `state_root`.

```bash
curl http://localhost:8314/api/v1/proof/account/aabbccdd11223344556677889900aabbccddeeff
```

**Response:**

```json
{
  "address": "aabbccdd11223344556677889900aabbccddeeff",
  "proof": "0123456789abcdef...hex-encoded JMT proof bytes...",
  "state_root": "789abc...64 hex chars..."
}
```

---

## Wallet Activation

New wallets can be activated through a PoW challenge to receive 3.14 PI in locked balance
(non-transferable, usable only for transaction fees). Maximum 3,140,000 activations.

### POST /api/v1/wallet/challenge

Request a PoW challenge for wallet activation.

```bash
curl -X POST http://localhost:8314/api/v1/wallet/challenge \
  -H "Content-Type: application/json" \
  -d '{"address": "aabbccdd11223344556677889900aabbccddeeff"}'
```

**Response:**

```json
{
  "success": true,
  "challenge": "fedcba9876543210...64 hex chars...",
  "difficulty_bits": 20,
  "error": null,
  "activations_remaining": 3139958
}
```

The challenge is a 32-byte hex string. You must find a `nonce` (u64) such that
`SHA-256(challenge_bytes || nonce_le_bytes)` has at least 20 leading zero bits. The challenge
expires after 5 minutes.

### POST /api/v1/wallet/activate

Submit the PoW solution to activate a wallet.

```bash
curl -X POST http://localhost:8314/api/v1/wallet/activate \
  -H "Content-Type: application/json" \
  -d '{
    "address": "aabbccdd11223344556677889900aabbccddeeff",
    "challenge": "fedcba9876543210...64 hex chars...",
    "nonce": 12345678
  }'
```

**Response:**

```json
{
  "success": true,
  "locked_amount": 3140000000,
  "locked_amount_pi": "3.140000000",
  "error": null,
  "address": "aabbccdd11223344556677889900aabbccddeeff"
}
```

---

## Faucet (devnet only)

### POST /api/v1/faucet

Request test PI on devnet. One claim per address.

```bash
curl -X POST http://localhost:8314/api/v1/faucet \
  -H "Content-Type: application/json" \
  -d '{"address": "aabbccdd11223344556677889900aabbccddeeff"}'
```

**Response:**

```json
{
  "success": true,
  "amount": 100000000000
}
```

---

## Explorer

### GET /api/v1/richlist

Top accounts by balance.

| Query Param | Type  | Default | Description |
|-------------|-------|---------|-------------|
| `limit`     | usize | 100     | Max results (clamped to 500) |

```bash
curl "http://localhost:8314/api/v1/richlist?limit=10"
```

**Response:**

```json
{
  "richlist": [
    {
      "address": "0000000000000000000000000000000000000001",
      "balance": 78539816339744830
    },
    {
      "address": "aabbccdd11223344556677889900aabbccddeeff",
      "balance": 3140000000
    }
  ],
  "count": 2
}
```

---

## WebSocket Subscriptions

### GET /ws

Upgrade to WebSocket for real-time events. Max 500 global connections, max 10 per IP.

```bash
# Using websocat
websocat ws://localhost:8314/ws
```

By default, all three event types are subscribed. Send a JSON message to filter:

```json
{"subscribe": ["newBlocks", "newTransactions"]}
```

Available topics: `"newBlocks"`, `"newTransactions"`, `"miningStatus"`.

**Event: newBlock**

```json
{
  "type": "newBlock",
  "height": 1043,
  "tx_count": 2,
  "gas_used": 42000,
  "base_fee": 1000,
  "pi_burned": 200,
  "timestamp_ms": 1709312401000,
  "proposer": "aabbccdd...40 hex chars...",
  "block_hash": "abcdef...64 hex chars..."
}
```

**Event: newTransaction**

```json
{
  "type": "newTransaction",
  "tx_hash": "deadbeef...64 hex chars...",
  "sender": "aabbccdd...40 hex chars...",
  "kind": "Transfer",
  "block_height": 1043,
  "status": "success"
}
```

**Event: miningStatus**

```json
{
  "type": "miningStatus",
  "frontier_position": 100500,
  "total_digits_verified": 500500,
  "difficulty_bits": 8,
  "unique_miners": 43
}
```

Idle connections are timed out after 120 seconds. The server sends ping frames every 30 seconds
to detect dead connections.

---

## Web UI Pages

The node serves built-in web pages for browsing and monitoring:

| Route | Description |
|-------|-------------|
| `/` | Homepage / landing page |
| `/explorer` | Block explorer |
| `/mining` | Live mining dashboard |
| `/mine` | Miner setup guide |
| `/download` | Miner binary downloads |
| `/dashboard` | Real-time node dashboard |
| `/launch` | Token launchpad interface |
| `/trade` | DEX swap and bridge interface |
| `/bridge` | Bridge monitor dashboard |

---

## Error Responses

All endpoints return standard HTTP status codes:

| Code | Meaning |
|------|---------|
| 200  | Success |
| 400  | Bad request (invalid address format, missing fields) |
| 403  | Forbidden (localhost-only endpoints called remotely, or cap reached) |
| 404  | Not found (block, transaction, account, token, pool does not exist) |
| 422  | Unprocessable (tx rejected by mempool, activation already done) |
| 429  | Rate limited |
| 503  | Service unavailable (node not ready, no state provider) |

Rate limit error response:

```json
{
  "error": "rate limit exceeded",
  "retry_after_secs": 1
}
```

---

## Address Format

- **Account addresses**: 40 hex characters (20 bytes), e.g. `aabbccdd11223344556677889900aabbccddeeff`
- **Mint IDs / Collection IDs / Hashes**: 64 hex characters (32 bytes)
- The `0x` prefix is accepted and automatically stripped by all endpoints
- The `Pi314` and `pi314` prefixes are also accepted and stripped

## CORS

Allowed origins: `localhost`, `127.0.0.1` (http/https), `pichain.net`, `www.pichain.net`,
`explorer.pichain.net`, `pichain.io`, `explorer.pichain.io` (https only).
