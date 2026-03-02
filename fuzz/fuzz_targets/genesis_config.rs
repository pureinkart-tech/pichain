//! Fuzz target for genesis configuration parsing and validation.
//!
//! Exercises the genesis config deserialization and validation paths
//! to ensure malformed genesis files cannot cause panics.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pichain_types::GenesisConfig;

fuzz_target!(|data: &[u8]| {
    // 1. Direct JSON deserialization
    if let Ok(genesis) = serde_json::from_slice::<GenesisConfig>(data) {
        // If it parses, run validation — this should never panic
        let _ = genesis.validate();
        let _ = genesis.verify_supply();
        let _ = genesis.genesis_hash();

        // Round-trip: serialize and deserialize again
        if let Ok(json) = serde_json::to_string(&genesis) {
            let _ = serde_json::from_str::<GenesisConfig>(&json);
        }
    }

    // 2. Try as UTF-8 string (simulates reading a config file)
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<GenesisConfig>(s);
    }
});
