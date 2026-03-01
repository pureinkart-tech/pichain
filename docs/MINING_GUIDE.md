# PIChain Mining Guide

PIChain uses Proof of Useful Work -- miners compute hexadecimal digits of PI using the
Bailey-Borwein-Plouffe (BBP) formula. Unlike traditional mining, rewards are proportional
to the number of digits computed, not hashpower. Any hardware can mine profitably.

---

## How Mining Works

### The BBP Formula

The BBP (Bailey-Borwein-Plouffe) formula can compute any individual hexadecimal digit of
PI without computing all preceding digits. This property makes digit computation
embarrassingly parallel -- each position is independent.

Miners compute batches of hex digits at positions beyond the network's current
**frontier** (the highest verified digit position).

### Mining Cycle

1. The miner queries the node for the current mining frontier via `GET /api/v1/mining/status`
2. The miner computes a batch of hex digits at the next available positions using BBP
3. The miner finds a nonce satisfying the Proof of Work requirement:
   `blake3(digits || nonce || anchor_block_hash) < difficulty_target`
4. The miner submits a `SubmitMiningProof` transaction to the node
5. The node's `MiningProcessor` verifies the digits (recomputes them) and validates the PoW
6. On success, the miner receives PI tokens proportional to the number of digits in the batch

### Fixed 8-Bit PoW

The Proof of Work component is intentionally minimal:

- **Minimum difficulty**: 8 bits (1 leading zero byte)
- **Average nonce attempts**: ~256
- **Purpose**: Anti-spam only -- prevents flooding the network with junk proofs
- **Not competitive**: PoW difficulty does not determine who earns more

This means any machine -- from a Raspberry Pi to a data center -- can satisfy the PoW
requirement almost instantly. What matters is how many PI digits you compute, not how
fast you can hash.

---

## Getting Started

### 1. Generate a Wallet

```bash
./target/release/pichain-miner --keypair ./miner-wallet.json --generate-keypair
```

This creates a new Ed25519 keypair. Back up this file securely -- it contains your
private key and receives all mining rewards.

### 2. Start Mining

Connect to a running PIChain node (local or remote):

```bash
# Connect to local node
./target/release/pichain-miner \
    --keypair ./miner-wallet.json \
    --rpc-url http://127.0.0.1:8314 \
    --profile desktop

# Connect to public node
./target/release/pichain-miner \
    --keypair ./miner-wallet.json \
    --rpc-url https://pichain.net \
    --profile desktop
```

### 3. Choose a Profile

Profiles auto-configure thread count, batch size, and concurrency for your hardware:

| Profile | Threads | Digits/Batch | Concurrent Batches | Best For |
|---------|---------|-------------|-------------------|----------|
| `laptop` | 2 | 200 | 1 | Laptops, low-power devices |
| `desktop` | half cores | 1,000 | 2 | Desktop PCs, workstations |
| `server` | all cores | 2,000 | 4 | Dedicated mining servers |
| `max` | all cores | 5,000 | 8 | Maximum throughput rigs |

```bash
./target/release/pichain-miner --keypair wallet.json --profile laptop
./target/release/pichain-miner --keypair wallet.json --profile server
```

---

## Fine-Tuning

Override individual settings after the profile (or without a profile):

```bash
./target/release/pichain-miner \
    --keypair ./miner-wallet.json \
    --rpc-url http://127.0.0.1:8314 \
    --threads 4 \
    --digits-per-batch 1500 \
    --concurrent-batches 3 \
    --max-cpu 50
```

### Parameters

| Flag | Description | Default |
|------|-------------|---------|
| `--threads N` | CPU threads for parallel BBP computation | Profile-dependent |
| `--digits-per-batch N` | Hex digits computed per proof | Profile-dependent |
| `--concurrent-batches N` | Batches computed simultaneously | Profile-dependent |
| `--max-cpu N` | Approximate CPU usage cap (percentage) | Unlimited |
| `--interval-secs N` | Delay between mining rounds (seconds) | 1 |
| `--position-offset N` | Offset for multi-miner setups (0, 1, 2...) | 0 |
| `--start-at N` | Override: start at a specific digit position | Auto (from frontier) |
| `--chain-id N` | Chain ID (314159 mainnet, 31415 devnet) | 31415 |

### Tuning Tips

- **More digits per batch** = larger reward per proof but longer computation time
- **More concurrent batches** = higher throughput but more memory usage
- **More threads** = faster computation per batch but higher CPU usage
- **Use `--max-cpu 50`** to mine in the background without impacting other workloads
- Start with `--profile desktop` and adjust from there

---

## Monitoring

### Live Dashboard

Run the built-in monitoring script:

```bash
bash monitor.sh
```

The dashboard refreshes every 3 seconds and shows:

- Block height and mempool size
- Mining frontier (highest verified digit position)
- Total verified digits and proof count
- Current difficulty and reward per digit
- Miner wallet balance and transaction count
- Miner process CPU and memory usage

### API Queries

```bash
# Mining status
curl -s http://localhost:8314/api/v1/mining/status | python3 -m json.tool

# Miner balance
curl -s http://localhost:8314/api/v1/account/<your-address> | python3 -m json.tool
```

The mining status response includes:

- `frontier_position` -- highest verified digit position
- `total_digits_verified` -- total hex digits computed by all miners
- `total_ranges` -- number of proof ranges submitted
- `difficulty_bits` -- current PoW difficulty (8 = minimum)
- `reward_per_digit` -- current reward per hex digit (in base units)

---

## Reward Economics

### Supply Allocation

85% of the total PI supply (2,670,353,755 PI out of 3,141,592,653 PI) is allocated
to the mining pool.

### Emission Schedule

Mining rewards follow a 2pi% (6.28%) geometric decay per year:

| Year | Annual Emission | Cumulative % Distributed |
|------|----------------|-------------------------|
| 1 | ~167.6M PI | ~6.3% |
| 2 | ~157.1M PI | ~12.2% |
| 5 | ~129.0M PI | ~28.8% |
| 10 | ~91.5M PI | ~50.0% |
| 20 | ~45.9M PI | ~75.0% |
| 31 (pi decades) | ~27.8M PI | ~82.0% |

The pool never fully drains because each year emits a fixed percentage of the
*remaining* pool (geometric series).

### Fee Recycling

25% of all transaction base fees are recycled back into the mining pool. As network
usage grows, this fee income extends the pool's effective lifetime, ensuring mining
remains viable indefinitely even after the geometric emission approaches zero.

### Reward Calculation

Rewards are proportional to digits computed:

```
reward = reward_per_digit * digit_count
```

The `reward_per_digit` decreases over time as the annual emission shrinks. It is
calculated as:

```
reward_per_digit = annual_emission / (blocks_per_year * expected_digits_per_block)
```

---

## Running as a Service

For long-running mining, install the systemd service:

```bash
sudo cp deploy/pichain-miner.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable pichain-miner
sudo systemctl start pichain-miner
```

The default service configuration:

```ini
[Service]
ExecStart=/usr/local/bin/pichain-miner \
    --rpc-url http://127.0.0.1:8314 \
    --keypair /var/lib/pichain/miner-wallet.json \
    --digits-per-batch 1000 \
    --chain-id 31415 \
    --interval-secs 1
Restart=on-failure
RestartSec=10
```

Edit the service file to change the profile or parameters, then reload:

```bash
sudo systemctl daemon-reload
sudo systemctl restart pichain-miner
```

View logs:

```bash
journalctl -u pichain-miner -f
```

---

## Multi-Miner Setup

To run multiple miners on the same machine (or across machines) without computing
overlapping digit ranges, use `--position-offset`:

```bash
# Miner 1 (offset 0 -- computes at frontier + 0)
./target/release/pichain-miner --keypair wallet1.json --position-offset 0 --profile server

# Miner 2 (offset 1 -- computes at frontier + batch_size)
./target/release/pichain-miner --keypair wallet2.json --position-offset 1 --profile server
```

Each miner should use a different keypair and offset to maximize coverage of the
digit space and avoid duplicate computation.

---

## Difficulty Adjustment

While the minimum PoW is fixed at 8 bits, the network adjusts difficulty dynamically
to maintain a target proof rate:

| Parameter | Value |
|-----------|-------|
| Target proof interval | 10 seconds |
| Adjustment interval | Every 10 proofs |
| Rolling window | 144 proofs (~24 minutes) |
| Max adjustment per period | 2x (up or down) |
| Minimum difficulty | 8 bits |
| Maximum difficulty | 240 bits |

If proofs arrive faster than target, difficulty increases. If slower, it decreases.
The 2x cap prevents runaway adjustments. In practice, with the fixed 8-bit minimum,
difficulty stays low and any hardware can mine.

---

## FAQ

**Can I mine on a laptop?**
Yes. Use `--profile laptop` for light CPU usage (2 threads, small batches). Rewards
are proportional to digits computed, so you earn less than a server but you still earn.

**Does GPU mining help?**
No. BBP digit computation is CPU-bound integer arithmetic. GPUs provide no advantage
over CPUs for this workload.

**Will I compete against ASICs?**
No. The 8-bit PoW is trivial for any hardware. Rewards scale with digit throughput
(CPU core count), not with specialized hashing hardware.

**How much PI can I earn?**
Check the current `reward_per_digit` via `GET /api/v1/mining/status`. Multiply by
your digits-per-batch and your proof submission rate. A desktop with 8 cores running
`--profile desktop` might submit a proof every few seconds.

**What happens if two miners compute the same range?**
The first proof to be included in a block claims the range. Subsequent proofs for
already-computed ranges are rejected. Use `--position-offset` in multi-miner setups
to avoid overlap.
