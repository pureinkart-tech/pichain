# PIChain Validator Guide

This guide covers running a PIChain validator node in production, including hardware
requirements, key generation, staking, monitoring, and operational procedures.

---

## Hardware Requirements

### Minimum (Devnet / Testnet)

| Resource | Requirement |
|----------|-------------|
| CPU | 4 cores (x86_64 or aarch64) |
| RAM | 8 GB |
| Disk | 50 GB SSD |
| Network | 100 Mbps symmetric |

### Recommended (Mainnet)

| Resource | Requirement |
|----------|-------------|
| CPU | 16+ cores (AMD EPYC / Intel Xeon recommended) |
| RAM | 64 GB |
| Disk | 1 TB NVMe SSD |
| Network | 1 Gbps symmetric, low latency |
| OS | Ubuntu 22.04 LTS or newer |

PIChain's Block-STM parallel executor and Turbine erasure coding benefit directly from
higher core counts and fast storage. NVMe is strongly recommended over SATA SSD for
the RocksDB workload.

---

## Automated Setup

The fastest way to set up a validator is the included setup script:

```bash
# Clone and build
git clone https://github.com/pureinkart-tech/pichain.git
cd pichain
cargo build --release

# Run setup (creates user, directories, keys, systemd services)
sudo bash deploy/setup-validator.sh mainnet
```

The script performs 7 steps:
1. Creates a `pichain` system user
2. Creates the data directory at `/var/lib/pichain`
3. Installs binaries to `/usr/local/bin/`
4. Initializes chain data
5. Generates validator Ed25519 keypair
6. Generates miner wallet
7. Installs systemd service files

---

## Manual Setup

### 1. Create System User

```bash
sudo useradd -m -s /bin/bash pichain
sudo mkdir -p /var/lib/pichain
sudo chown pichain:pichain /var/lib/pichain
```

### 2. Install Binaries

```bash
sudo cp target/release/pichain /usr/local/bin/
sudo cp target/release/pichain-cli /usr/local/bin/
sudo cp target/release/pichain-miner /usr/local/bin/
sudo chmod +x /usr/local/bin/pichain*
```

### 3. Generate Validator Keys

PIChain validators need two key pairs:

**Ed25519 keypair** (transaction signing and identity):

```bash
sudo -u pichain pichain keygen > /var/lib/pichain/validator-keys.txt
sudo chmod 600 /var/lib/pichain/validator-keys.txt
```

**BLS12-381 keypair** (consensus attestations):

BLS keys are generated automatically when the node starts for the first time. They are
stored in the validator key file at `{data_dir}/validator.key`. The key file contains
both the Ed25519 secret key and the BLS secret key.

The BLS public key is 48 bytes and the BLS proof-of-possession is 96 bytes. Both are
required for multi-validator consensus to prevent rogue-key attacks.

### 4. Configure for Mainnet

Create a configuration file:

```bash
sudo -u pichain tee /var/lib/pichain/config.toml <<'EOF'
# PIChain Mainnet Validator Configuration
data_dir = "/var/lib/pichain/mainnet"
rpc_addr = "0.0.0.0:8314"
p2p_addr = "/ip4/0.0.0.0/tcp/9314"
chain_id = 314159
max_mempool_size = 100000
validator_stake_pi = 100000
log_level = "info"

# Connect to existing validators
bootstrap_peers = [
    # Add known validator multiaddrs here
    # "/ip4/1.2.3.4/tcp/9314/p2p/12D3KooW...",
]
EOF
```

Key configuration values:

| Parameter | Mainnet | Devnet |
|-----------|---------|--------|
| `chain_id` | 314159 | 31415 |
| `rpc_addr` | `0.0.0.0:8314` | `127.0.0.1:8314` |
| `p2p_addr` | `/ip4/0.0.0.0/tcp/9314` | `/ip4/0.0.0.0/tcp/9314` |
| `log_level` | `info` | `debug` |

### 5. Install Systemd Service

Copy the provided service file:

```bash
sudo cp deploy/pichain.service /etc/systemd/system/pichain.service
```

The service file includes security hardening:

```ini
[Service]
Type=simple
User=pichain
Group=pichain
ExecStart=/usr/local/bin/pichain run \
    --data-dir /var/lib/pichain \
    --rpc-addr 0.0.0.0:8314 \
    --p2p-addr /ip4/0.0.0.0/tcp/9314 \
    --chain-id 314159
Restart=on-failure
RestartSec=5
LimitNOFILE=65536
LimitNPROC=4096

# Security hardening
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/pichain
NoNewPrivileges=true
PrivateTmp=true

Environment=RUST_LOG=info
```

Enable and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable pichain
sudo systemctl start pichain
```

---

## Staking Requirements

### Mainnet

| Parameter | Value |
|-----------|-------|
| Minimum validator stake | 10,000 PI |
| Recommended stake | 100,000 PI |
| Minimum delegation | 1 PI |
| Maximum commission rate | 20% |
| Maximum validator stake share | 33.33% of total staked |
| Maximum address stake share | 10% of total staked |
| Maximum stake growth per epoch | 50% |
| Unbonding period | 62 epochs (~7 days) |

### Stake Registration

Stake is declared via the `validator_stake_pi` field in the configuration file or
the `--validator-stake` CLI flag. The node registers the validator in the consensus
engine at startup.

### Delegation

Non-validators can delegate PI to validators to earn staking rewards:

- Minimum delegation: 1 PI
- Rewards are distributed proportionally to stake
- Delegators can redelegate or unbond at any time (subject to unbonding period)

---

## Slashing

Validators can be slashed for the following offenses:

| Offense | Penalty | Jail Duration |
|---------|---------|---------------|
| Double-signing (equivocation) | 33% of stake | 88 epochs (~10 days) |
| Extended downtime | 1% of stake | 88 epochs (~10 days) |
| Invalid block proposal | 5% of stake | 88 epochs (~10 days) |

Equivocation evidence is propagated through GossipSub and all nodes process the same
evidence deterministically. Evidence must be submitted within 42 epochs (less than the
unbonding period of 62 epochs, ensuring misbehaving validators still have locked funds).

### Anti-Concentration Protections

- No single validator can hold more than 33.33% of total staked PI
- No single address can control more than 10% of total staked PI
- Stake growth is capped at 50% per epoch to prevent flash-staking attacks
- During bootstrap (fewer than 4 validators), a relaxed 41.3% cap applies

---

## Monitoring

### Health Checks

```bash
# Basic health
curl http://localhost:8314/health

# Detailed node info
curl http://localhost:8314/api/v1/info

# Mining status
curl http://localhost:8314/api/v1/mining/status
```

### Prometheus Metrics

The `/metrics` endpoint is available on localhost and exposes Prometheus-compatible
metrics. Configure your Prometheus instance to scrape it:

```yaml
# prometheus.yml
global:
  scrape_interval: 10s

scrape_configs:
  - job_name: "pichain-nodes"
    metrics_path: "/metrics"
    static_configs:
      - targets:
          - "localhost:8314"
        labels:
          network: "mainnet"
```

### Available Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `pichain_block_height` | gauge | Current block height |
| `pichain_peer_count` | gauge | Connected P2P peers |
| `pichain_base_fee` | gauge | Current base fee in base units |
| `pichain_total_burned` | counter | Total PI burned (deflationary) |
| `pichain_mining_frontier` | gauge | Highest verified PI digit position |
| `pichain_mining_digits_verified` | counter | Total hex digits verified |
| `pichain_mining_unique_miners` | gauge | Unique miner addresses |
| `pichain_uptime_seconds` | counter | Node uptime |
| `pichain_requests_total` | counter | Total RPC requests served |
| `pichain_requests_errors` | counter | RPC request errors |
| `pichain_ws_subscribers` | gauge | Active WebSocket subscribers |
| `pichain_consensus_round` | gauge | Current DAG consensus round |
| `pichain_bullshark_commits` | counter | Total Bullshark commits |
| `pichain_certificates_produced` | counter | DAG certificates produced |
| `pichain_validator_count` | gauge | Active validators in the set |
| `pichain_total_stake` | gauge | Total PI staked across all validators |

### Recommended Alerts

- `pichain_block_height` has not increased in 60 seconds
- `pichain_peer_count` drops below 3
- `pichain_consensus_round` stalls for more than 30 seconds
- `pichain_requests_errors / pichain_requests_total` exceeds 5%

### Logs

```bash
# Follow node logs
journalctl -u pichain -f

# Follow miner logs
journalctl -u pichain-miner -f
```

Set log verbosity via the `RUST_LOG` environment variable or the `log_level` config
option. Levels: `trace`, `debug`, `info`, `warn`, `error`.

---

## Firewall Configuration

Open the following ports:

| Port | Protocol | Purpose |
|------|----------|---------|
| 8314 | TCP | RPC API (restrict to trusted sources on mainnet) |
| 9314 | TCP | P2P communication (must be open to all peers) |
| 9314 | UDP/QUIC | P2P QUIC transport |

Example (UFW):

```bash
sudo ufw allow 9314/tcp   # P2P
sudo ufw allow 9314/udp   # QUIC
# Only allow RPC from trusted IPs
sudo ufw allow from 10.0.0.0/8 to any port 8314
```

---

## Backup and Recovery

### What to Back Up

- `/var/lib/pichain/validator.key` -- validator Ed25519 + BLS private keys (critical)
- `/var/lib/pichain/miner-wallet.json` -- miner keypair
- `/var/lib/pichain/config.toml` -- node configuration

The blockchain data directory can be rebuilt by syncing from peers. Private keys
cannot be recovered if lost.

### Key Security

- Store validator keys in a hardware security module (HSM) for mainnet
- Keep encrypted offline backups of key files
- Never commit key files to version control
- Key files should have permissions `600` (readable only by the pichain user)

---

## Operational Procedures

### Graceful Shutdown

```bash
sudo systemctl stop pichain
```

The node handles `SIGTERM` gracefully, flushing RocksDB before exit.

### Upgrading

```bash
cd pichain
git pull
cargo build --release
sudo systemctl stop pichain
sudo cp target/release/pichain /usr/local/bin/
sudo systemctl start pichain
```

RocksDB schema migrations run automatically on startup when the database version
changes. Migrations are forward-only.

### Running a Co-Located Miner

Install and enable the miner service:

```bash
sudo cp deploy/pichain-miner.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable pichain-miner
sudo systemctl start pichain-miner
```

The miner service depends on the node service and will start automatically after it.
