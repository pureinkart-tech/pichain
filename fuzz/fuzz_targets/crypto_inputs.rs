//! Fuzz target for cryptographic primitive inputs.
//!
//! Exercises signature verification, key derivation, and hash functions
//! with arbitrary inputs. These are security-critical paths — any panic
//! is a potential DoS vector since signature verification happens at
//! mempool ingress (untrusted data from the network).

#![no_main]

use libfuzzer_sys::fuzz_target;
use pichain_crypto::{Keypair, ed25519};

fuzz_target!(|data: &[u8]| {
    // 1. Signature verification with arbitrary public key, message, and signature
    if data.len() >= 32 + 64 {
        let pk_bytes: [u8; 32] = data[..32].try_into().unwrap();
        let sig_bytes: [u8; 64] = data[32..96].try_into().unwrap();
        let message = &data[96..];

        // Try to construct a public key — should not panic on invalid bytes
        if let Ok(pk) = ed25519::PublicKey::from_bytes(&pk_bytes) {
            let sig = ed25519::Signature::from_bytes(&sig_bytes);
            // Verification should return Ok/Err, never panic
            let _ = pk.verify(message, &sig);
        }
    }

    // 2. Keypair from arbitrary secret bytes
    if data.len() >= 32 {
        let secret: [u8; 32] = data[..32].try_into().unwrap();
        let kp = Keypair::from_secret_bytes(&secret);
        // Derived address should always be valid
        let _ = kp.address();
        // Sign and verify round-trip
        let msg = if data.len() > 32 { &data[32..] } else { b"test" as &[u8] };
        let sig = kp.sign(msg);
        let _ = kp.public.verify(msg, &sig);
    }

    // 3. Hash function with arbitrary input (should never panic)
    let _ = pichain_crypto::hash(data);

    // 4. Hex decode of arbitrary data (address parsing path)
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = hex::decode(s);
    }
});
