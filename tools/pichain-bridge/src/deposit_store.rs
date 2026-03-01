use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::PathBuf;

/// Tracks processed deposit transactions to prevent double-minting.
/// Uses an NDJSON (newline-delimited JSON) file for persistence.
pub struct DepositStore {
    /// Set of processed deposit keys: "chain:txhash"
    processed: HashSet<String>,
    /// Path to the NDJSON file
    file_path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcessedDeposit {
    pub chain: String,
    pub tx_hash: String,
    pub amount: u64,
    pub recipient: String,
    pub symbol: String,
    pub timestamp: i64,
}

impl DepositStore {
    /// Load or create a deposit store at the given path.
    pub fn load(file_path: PathBuf) -> Result<Self> {
        let mut processed = HashSet::new();

        if file_path.exists() {
            let file =
                std::fs::File::open(&file_path).context("failed to open deposit store")?;
            let reader = std::io::BufReader::new(file);
            for line in reader.lines() {
                let line = line.context("failed to read deposit store line")?;
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(deposit) = serde_json::from_str::<ProcessedDeposit>(line) {
                    let key = format!("{}:{}", deposit.chain, deposit.tx_hash);
                    processed.insert(key);
                }
            }
            tracing::info!(
                count = processed.len(),
                path = %file_path.display(),
                "loaded deposit store"
            );
        } else {
            // Ensure parent directory exists
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent)
                    .context("failed to create deposit store directory")?;
            }
            tracing::info!(
                path = %file_path.display(),
                "created new deposit store"
            );
        }

        Ok(Self {
            processed,
            file_path,
        })
    }

    /// Check if a deposit has already been processed.
    pub fn is_processed(&self, chain: &str, tx_hash: &str) -> bool {
        let key = format!("{}:{}", chain, tx_hash);
        self.processed.contains(&key)
    }

    /// Record a processed deposit and append to the NDJSON file.
    pub fn record(&mut self, deposit: ProcessedDeposit) -> Result<()> {
        let key = format!("{}:{}", deposit.chain, deposit.tx_hash);
        if self.processed.contains(&key) {
            return Ok(()); // Already recorded
        }

        // Append to file
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_path)
            .context("failed to open deposit store for writing")?;

        let json = serde_json::to_string(&deposit)?;
        writeln!(file, "{}", json).context("failed to write deposit record")?;

        self.processed.insert(key);
        Ok(())
    }

    /// Number of processed deposits.
    pub fn count(&self) -> usize {
        self.processed.len()
    }
}
