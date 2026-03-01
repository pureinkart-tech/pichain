use std::time::Duration;
use tracing::{error, info, warn};

use crate::chains::{ChainMonitor, Deposit};
use crate::deposit_store::{DepositStore, ProcessedDeposit};
use crate::pichain_client::PiChainClient;

/// Core bridge relayer that polls chain monitors and triggers minting.
pub struct Relayer {
    client: PiChainClient,
    store: DepositStore,
    monitors: Vec<Box<dyn ChainMonitor>>,
    poll_interval: Duration,
}

impl Relayer {
    pub fn new(
        client: PiChainClient,
        store: DepositStore,
        monitors: Vec<Box<dyn ChainMonitor>>,
        poll_interval_secs: u64,
    ) -> Self {
        Self {
            client,
            store,
            monitors,
            poll_interval: Duration::from_secs(poll_interval_secs),
        }
    }

    /// Run the main polling loop.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        // Check PIChain node connectivity
        if !self.client.health_check().await? {
            warn!("PIChain node is not reachable — will retry on each poll");
        } else {
            info!("connected to PIChain node");
        }

        let enabled: Vec<&str> = self
            .monitors
            .iter()
            .filter(|m| m.is_enabled())
            .map(|m| m.chain_name())
            .collect();

        if enabled.is_empty() {
            warn!("no chain monitors enabled — bridge relayer has nothing to do");
            return Ok(());
        }

        info!(
            chains = ?enabled,
            poll_interval = ?self.poll_interval,
            processed = self.store.count(),
            "bridge relayer started"
        );

        loop {
            self.poll_all_chains().await;
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// Poll all enabled chain monitors and process deposits.
    async fn poll_all_chains(&mut self) {
        // Collect all deposits first to avoid borrow conflict
        let mut all_deposits = Vec::new();

        for monitor in &mut self.monitors {
            if !monitor.is_enabled() {
                continue;
            }

            match monitor.poll_deposits().await {
                Ok(deposits) => all_deposits.extend(deposits),
                Err(e) => {
                    warn!(
                        chain = monitor.chain_name(),
                        error = %e,
                        "failed to poll chain"
                    );
                }
            };
        }

        for deposit in all_deposits {
            self.process_deposit(deposit).await;
        }
    }

    /// Process a single deposit: check if already processed, mint, record.
    async fn process_deposit(&mut self, deposit: Deposit) {
        // Skip if already processed
        if self.store.is_processed(&deposit.chain, &deposit.tx_hash) {
            return;
        }

        info!(
            chain = %deposit.chain,
            tx = %deposit.tx_hash,
            symbol = %deposit.symbol,
            amount = deposit.amount,
            recipient = %deposit.recipient,
            "processing new deposit"
        );

        // Call PIChain bridge/mint
        match self
            .client
            .mint(
                &deposit.symbol,
                &deposit.recipient,
                deposit.amount,
                &deposit.chain,
                &deposit.tx_hash,
            )
            .await
        {
            Ok(()) => {
                // Record as processed
                let record = ProcessedDeposit {
                    chain: deposit.chain,
                    tx_hash: deposit.tx_hash,
                    amount: deposit.amount,
                    recipient: deposit.recipient,
                    symbol: deposit.symbol,
                    timestamp: chrono::Utc::now().timestamp(),
                };

                if let Err(e) = self.store.record(record) {
                    error!(error = %e, "failed to record processed deposit — RISK OF DOUBLE MINT");
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "failed to mint — will retry next poll"
                );
                // Don't record: will retry on next poll
            }
        }
    }
}
