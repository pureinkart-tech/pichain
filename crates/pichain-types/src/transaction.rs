//! PIChain transaction types.

use pichain_crypto::{ed25519::Address, Hash, PublicKey, Signature};
use serde::{Deserialize, Serialize};

use crate::{Gas, Nonce, PiAmount};

/// Raw transaction data before signing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionData {
    /// Sender address.
    pub sender: Address,
    /// Transaction nonce (monotonically increasing per sender).
    pub nonce: Nonce,
    /// What this transaction does.
    pub kind: TransactionKind,
    /// Maximum gas the sender is willing to pay.
    pub gas_limit: Gas,
    /// Maximum base fee per gas unit (in base PI units).
    pub max_base_fee: PiAmount,
    /// Priority fee per gas unit (tip to block producer).
    pub max_priority_fee: PiAmount,
    /// Chain ID to prevent cross-chain replay attacks.
    pub chain_id: u64,
}

/// What a transaction does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionKind {
    /// Transfer PI tokens from sender to recipient.
    Transfer {
        recipient: Address,
        amount: PiAmount,
    },
    /// Deploy a smart contract (WASM bytecode).
    DeployContract {
        code: Vec<u8>,
        init_data: Vec<u8>,
    },
    /// Call a smart contract function.
    ContractCall {
        contract: Address,
        function: String,
        args: Vec<u8>,
    },
    /// Stake PI tokens to a validator.
    Stake {
        validator: Address,
        amount: PiAmount,
    },
    /// Unstake PI tokens from a validator.
    Unstake {
        validator: Address,
        amount: PiAmount,
    },
    /// Submit a mining proof (PI digit computation + PoW nonce).
    MiningProof {
        /// Starting digit position computed.
        start_position: u64,
        /// Number of digits computed.
        digit_count: u32,
        /// The computed hex digits.
        digits: Vec<u8>,
        /// Legacy proof field (kept for serialization compat).
        proof: Vec<u8>,
        /// PoW nonce: blake3(digits || pow_nonce || anchor_block_hash) < difficulty target.
        #[serde(default)]
        pow_nonce: u64,
        /// Anchor block hash — ties proof to a recent block (prevents pre-computation).
        #[serde(default)]
        anchor_block_hash: Vec<u8>,
    },

    // --- Token Program (native SPL-like token operations) ---

    /// Create a new custom token.
    CreateToken {
        /// Token name.
        name: String,
        /// Token symbol (e.g., "DOPI").
        symbol: String,
        /// Decimal places.
        decimals: u8,
        /// Maximum supply (0 = unlimited).
        max_supply: u64,
        /// Metadata URI (logo, description).
        metadata_uri: String,
    },

    /// Mint new tokens (only callable by mint authority).
    MintToken {
        /// Token mint ID.
        mint: crate::MintId,
        /// Recipient of minted tokens.
        recipient: Address,
        /// Amount to mint.
        amount: u64,
    },

    /// Transfer custom tokens between accounts.
    TransferToken {
        /// Token mint ID.
        mint: crate::MintId,
        /// Recipient address.
        recipient: Address,
        /// Amount to transfer.
        amount: u64,
    },

    /// Burn tokens (reduce supply).
    BurnToken {
        /// Token mint ID.
        mint: crate::MintId,
        /// Amount to burn.
        amount: u64,
    },

    /// Approve a delegate to spend tokens on your behalf.
    ApproveToken {
        /// Token mint ID.
        mint: crate::MintId,
        /// Delegate address.
        delegate: Address,
        /// Maximum amount the delegate can spend.
        amount: u64,
    },

    /// Revoke the mint authority (permanently freeze supply).
    RevokeMintAuthority {
        /// Token mint ID.
        mint: crate::MintId,
    },

    /// Freeze a token account (only callable by freeze authority).
    FreezeTokenAccount {
        /// Token mint ID.
        mint: crate::MintId,
        /// Account to freeze.
        target: Address,
    },

    /// Thaw a frozen token account (only callable by freeze authority).
    ThawTokenAccount {
        /// Token mint ID.
        mint: crate::MintId,
        /// Account to thaw.
        target: Address,
    },

    // --- DEX/AMM (native constant-product AMM) ---

    /// Create a new liquidity pool for a token pair.
    CreatePool {
        /// First token mint.
        mint_a: crate::MintId,
        /// Second token mint.
        mint_b: crate::MintId,
    },

    /// Add liquidity to a pool.
    AddLiquidity {
        /// First token mint.
        mint_a: crate::MintId,
        /// Second token mint.
        mint_b: crate::MintId,
        /// Amount of token A to deposit.
        amount_a: u64,
        /// Amount of token B to deposit.
        amount_b: u64,
        /// Minimum LP tokens to receive (slippage protection).
        min_lp_tokens: u64,
    },

    /// Remove liquidity from a pool.
    RemoveLiquidity {
        /// First token mint.
        mint_a: crate::MintId,
        /// Second token mint.
        mint_b: crate::MintId,
        /// LP tokens to burn.
        lp_amount: u64,
        /// Minimum amount of token A to receive.
        min_amount_a: u64,
        /// Minimum amount of token B to receive.
        min_amount_b: u64,
    },

    /// Swap tokens through an AMM pool.
    Swap {
        /// Token to sell.
        mint_in: crate::MintId,
        /// Token to buy.
        mint_out: crate::MintId,
        /// Amount of input token.
        amount_in: u64,
        /// Minimum output amount (slippage protection).
        min_amount_out: u64,
    },

    // --- Launchpad (one-click token launch with bonding curves) ---

    /// Create a new token launch.
    CreateLaunch {
        /// Token mint ID.
        mint: crate::MintId,
        /// Launch type (pricing mechanism).
        launch_type: crate::launchpad::LaunchType,
        /// Total tokens allocated for sale.
        tokens_for_sale: u64,
        /// Target PI to raise.
        target_pi: u64,
        /// Maximum PI per address.
        max_per_address: u64,
    },

    /// Participate in a token launch (buy tokens with PI).
    ParticipateInLaunch {
        /// Token mint ID.
        mint: crate::MintId,
        /// PI amount to contribute.
        pi_amount: u64,
    },

    /// Finalize a token launch (create AMM pool).
    FinalizeLaunch {
        /// Token mint ID.
        mint: crate::MintId,
    },

    // --- NFTs (native NFT standard with collections, royalties, marketplace) ---

    /// Create a new NFT collection.
    CreateNftCollection {
        /// Collection name.
        name: String,
        /// Collection symbol.
        symbol: String,
        /// Maximum NFTs in this collection (0 = unlimited).
        max_supply: u64,
        /// Royalty in basis points (e.g., 500 = 5%).
        royalty_bps: u16,
        /// Base URI for metadata.
        base_uri: String,
    },

    /// Mint a new NFT in a collection.
    MintNft {
        /// Collection ID.
        collection: crate::CollectionId,
        /// Recipient of the minted NFT.
        recipient: Address,
        /// NFT name.
        name: String,
        /// Metadata URI (e.g., IPFS link).
        metadata_uri: String,
        /// NFT attributes.
        attributes: Vec<crate::nft::NftAttribute>,
    },

    /// Transfer an NFT to a new owner.
    TransferNft {
        /// NFT ID.
        nft_id: crate::NftId,
        /// Recipient address.
        recipient: Address,
    },

    /// List an NFT for sale on the marketplace.
    ListNft {
        /// NFT ID.
        nft_id: crate::NftId,
        /// Listing price in base PI units.
        price: u64,
    },

    /// Buy a listed NFT (with protocol-enforced royalties).
    BuyNft {
        /// NFT ID.
        nft_id: crate::NftId,
    },

    /// Delist an NFT from the marketplace.
    DelistNft {
        /// NFT ID.
        nft_id: crate::NftId,
    },
}

impl TransactionData {
    /// Produce canonical binary encoding for hashing and signing.
    ///
    /// This uses a deterministic binary format (field-by-field little-endian encoding)
    /// instead of serde_json, which has non-deterministic field ordering and formatting
    /// that would break consensus across nodes.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256);
        // Fixed header fields
        buf.extend_from_slice(&self.sender.0);           // 20 bytes
        buf.extend_from_slice(&self.nonce.to_le_bytes()); // 8 bytes
        buf.extend_from_slice(&self.gas_limit.to_le_bytes());
        buf.extend_from_slice(&self.max_base_fee.to_le_bytes());
        buf.extend_from_slice(&self.max_priority_fee.to_le_bytes());
        buf.extend_from_slice(&self.chain_id.to_le_bytes());

        // Transaction kind — each variant gets a unique tag byte
        match &self.kind {
            TransactionKind::Transfer { recipient, amount } => {
                buf.push(0);
                buf.extend_from_slice(&recipient.0);
                buf.extend_from_slice(&amount.to_le_bytes());
            }
            TransactionKind::DeployContract { code, init_data } => {
                buf.push(1);
                buf.extend_from_slice(&(code.len() as u32).to_le_bytes());
                buf.extend_from_slice(code);
                buf.extend_from_slice(&(init_data.len() as u32).to_le_bytes());
                buf.extend_from_slice(init_data);
            }
            TransactionKind::ContractCall { contract, function, args } => {
                buf.push(2);
                buf.extend_from_slice(&contract.0);
                let fn_bytes = function.as_bytes();
                buf.extend_from_slice(&(fn_bytes.len() as u32).to_le_bytes());
                buf.extend_from_slice(fn_bytes);
                buf.extend_from_slice(&(args.len() as u32).to_le_bytes());
                buf.extend_from_slice(args);
            }
            TransactionKind::Stake { validator, amount } => {
                buf.push(3);
                buf.extend_from_slice(&validator.0);
                buf.extend_from_slice(&amount.to_le_bytes());
            }
            TransactionKind::Unstake { validator, amount } => {
                buf.push(4);
                buf.extend_from_slice(&validator.0);
                buf.extend_from_slice(&amount.to_le_bytes());
            }
            TransactionKind::MiningProof {
                start_position, digit_count, digits, proof, pow_nonce, anchor_block_hash,
            } => {
                buf.push(5);
                buf.extend_from_slice(&start_position.to_le_bytes());
                buf.extend_from_slice(&digit_count.to_le_bytes());
                buf.extend_from_slice(&(digits.len() as u32).to_le_bytes());
                buf.extend_from_slice(digits);
                buf.extend_from_slice(&(proof.len() as u32).to_le_bytes());
                buf.extend_from_slice(proof);
                buf.extend_from_slice(&pow_nonce.to_le_bytes());
                buf.extend_from_slice(&(anchor_block_hash.len() as u32).to_le_bytes());
                buf.extend_from_slice(anchor_block_hash);
            }
            TransactionKind::CreateToken { name, symbol, decimals, max_supply, metadata_uri } => {
                buf.push(6);
                let name_b = name.as_bytes();
                buf.extend_from_slice(&(name_b.len() as u32).to_le_bytes());
                buf.extend_from_slice(name_b);
                let sym_b = symbol.as_bytes();
                buf.extend_from_slice(&(sym_b.len() as u32).to_le_bytes());
                buf.extend_from_slice(sym_b);
                buf.push(*decimals);
                buf.extend_from_slice(&max_supply.to_le_bytes());
                let uri_b = metadata_uri.as_bytes();
                buf.extend_from_slice(&(uri_b.len() as u32).to_le_bytes());
                buf.extend_from_slice(uri_b);
            }
            TransactionKind::MintToken { mint, recipient, amount } => {
                buf.push(7);
                buf.extend_from_slice(&mint.0);
                buf.extend_from_slice(&recipient.0);
                buf.extend_from_slice(&amount.to_le_bytes());
            }
            TransactionKind::TransferToken { mint, recipient, amount } => {
                buf.push(8);
                buf.extend_from_slice(&mint.0);
                buf.extend_from_slice(&recipient.0);
                buf.extend_from_slice(&amount.to_le_bytes());
            }
            TransactionKind::BurnToken { mint, amount } => {
                buf.push(9);
                buf.extend_from_slice(&mint.0);
                buf.extend_from_slice(&amount.to_le_bytes());
            }
            TransactionKind::ApproveToken { mint, delegate, amount } => {
                buf.push(10);
                buf.extend_from_slice(&mint.0);
                buf.extend_from_slice(&delegate.0);
                buf.extend_from_slice(&amount.to_le_bytes());
            }
            TransactionKind::RevokeMintAuthority { mint } => {
                buf.push(11);
                buf.extend_from_slice(&mint.0);
            }
            TransactionKind::FreezeTokenAccount { mint, target } => {
                buf.push(12);
                buf.extend_from_slice(&mint.0);
                buf.extend_from_slice(&target.0);
            }
            TransactionKind::ThawTokenAccount { mint, target } => {
                buf.push(13);
                buf.extend_from_slice(&mint.0);
                buf.extend_from_slice(&target.0);
            }
            TransactionKind::CreatePool { mint_a, mint_b } => {
                buf.push(14);
                buf.extend_from_slice(&mint_a.0);
                buf.extend_from_slice(&mint_b.0);
            }
            TransactionKind::AddLiquidity { mint_a, mint_b, amount_a, amount_b, min_lp_tokens } => {
                buf.push(15);
                buf.extend_from_slice(&mint_a.0);
                buf.extend_from_slice(&mint_b.0);
                buf.extend_from_slice(&amount_a.to_le_bytes());
                buf.extend_from_slice(&amount_b.to_le_bytes());
                buf.extend_from_slice(&min_lp_tokens.to_le_bytes());
            }
            TransactionKind::RemoveLiquidity { mint_a, mint_b, lp_amount, min_amount_a, min_amount_b } => {
                buf.push(16);
                buf.extend_from_slice(&mint_a.0);
                buf.extend_from_slice(&mint_b.0);
                buf.extend_from_slice(&lp_amount.to_le_bytes());
                buf.extend_from_slice(&min_amount_a.to_le_bytes());
                buf.extend_from_slice(&min_amount_b.to_le_bytes());
            }
            TransactionKind::Swap { mint_in, mint_out, amount_in, min_amount_out } => {
                buf.push(17);
                buf.extend_from_slice(&mint_in.0);
                buf.extend_from_slice(&mint_out.0);
                buf.extend_from_slice(&amount_in.to_le_bytes());
                buf.extend_from_slice(&min_amount_out.to_le_bytes());
            }
            TransactionKind::CreateLaunch { mint, launch_type, tokens_for_sale, target_pi, max_per_address } => {
                buf.push(18);
                buf.extend_from_slice(&mint.0);
                // Encode launch_type deterministically
                match launch_type {
                    crate::launchpad::LaunchType::FairLaunch { price_per_token } => {
                        buf.push(0);
                        buf.extend_from_slice(&price_per_token.to_le_bytes());
                    }
                    crate::launchpad::LaunchType::BondingCurve { base_price, slope } => {
                        buf.push(1);
                        buf.extend_from_slice(&base_price.to_le_bytes());
                        buf.extend_from_slice(&slope.to_le_bytes());
                    }
                }
                buf.extend_from_slice(&tokens_for_sale.to_le_bytes());
                buf.extend_from_slice(&target_pi.to_le_bytes());
                buf.extend_from_slice(&max_per_address.to_le_bytes());
            }
            TransactionKind::ParticipateInLaunch { mint, pi_amount } => {
                buf.push(19);
                buf.extend_from_slice(&mint.0);
                buf.extend_from_slice(&pi_amount.to_le_bytes());
            }
            TransactionKind::FinalizeLaunch { mint } => {
                buf.push(20);
                buf.extend_from_slice(&mint.0);
            }
            TransactionKind::CreateNftCollection { name, symbol, max_supply, royalty_bps, base_uri } => {
                buf.push(21);
                let name_b = name.as_bytes();
                buf.extend_from_slice(&(name_b.len() as u32).to_le_bytes());
                buf.extend_from_slice(name_b);
                let sym_b = symbol.as_bytes();
                buf.extend_from_slice(&(sym_b.len() as u32).to_le_bytes());
                buf.extend_from_slice(sym_b);
                buf.extend_from_slice(&max_supply.to_le_bytes());
                buf.extend_from_slice(&royalty_bps.to_le_bytes());
                let uri_b = base_uri.as_bytes();
                buf.extend_from_slice(&(uri_b.len() as u32).to_le_bytes());
                buf.extend_from_slice(uri_b);
            }
            TransactionKind::MintNft { collection, recipient, name, metadata_uri, attributes } => {
                buf.push(22);
                buf.extend_from_slice(&collection.0);
                buf.extend_from_slice(&recipient.0);
                let name_b = name.as_bytes();
                buf.extend_from_slice(&(name_b.len() as u32).to_le_bytes());
                buf.extend_from_slice(name_b);
                let uri_b = metadata_uri.as_bytes();
                buf.extend_from_slice(&(uri_b.len() as u32).to_le_bytes());
                buf.extend_from_slice(uri_b);
                buf.extend_from_slice(&(attributes.len() as u32).to_le_bytes());
                for attr in attributes {
                    let tt = attr.trait_type.as_bytes();
                    buf.extend_from_slice(&(tt.len() as u32).to_le_bytes());
                    buf.extend_from_slice(tt);
                    let val = attr.value.as_bytes();
                    buf.extend_from_slice(&(val.len() as u32).to_le_bytes());
                    buf.extend_from_slice(val);
                }
            }
            TransactionKind::TransferNft { nft_id, recipient } => {
                buf.push(23);
                buf.extend_from_slice(&nft_id.0);
                buf.extend_from_slice(&recipient.0);
            }
            TransactionKind::ListNft { nft_id, price } => {
                buf.push(24);
                buf.extend_from_slice(&nft_id.0);
                buf.extend_from_slice(&price.to_le_bytes());
            }
            TransactionKind::BuyNft { nft_id } => {
                buf.push(25);
                buf.extend_from_slice(&nft_id.0);
            }
            TransactionKind::DelistNft { nft_id } => {
                buf.push(26);
                buf.extend_from_slice(&nft_id.0);
            }
        }

        buf
    }
}

/// A transaction with its signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedTransaction {
    /// The transaction data.
    pub data: TransactionData,
    /// Ed25519 signature over the serialized TransactionData.
    pub signature: Signature,
    /// Sender's public key (for verification without state lookup).
    pub public_key: PublicKey,
}

impl SignedTransaction {
    /// Compute the transaction hash (unique identifier).
    /// Includes the signature to prevent transaction malleability — even though
    /// Ed25519 signatures are deterministic, including them ensures the hash
    /// uniquely identifies both the intent and its authorization.
    pub fn hash(&self) -> Hash {
        let data_bytes = self.data.canonical_bytes();
        let sig_bytes = self.signature.to_bytes();
        pichain_crypto::hash_concat(&[&data_bytes, &sig_bytes])
    }

    /// Verify the transaction signature.
    /// Uses canonical binary encoding for deterministic verification.
    pub fn verify(&self) -> Result<(), pichain_crypto::CryptoError> {
        // Verify pubkey matches sender address
        let derived_addr = self.public_key.to_address();
        if derived_addr != self.data.sender {
            return Err(pichain_crypto::CryptoError::InvalidPublicKey);
        }
        let bytes = self.data.canonical_bytes();
        self.public_key.verify(&bytes, &self.signature)
    }

    /// Check if this is a simple transfer (eligible for Avalanche fast-path).
    pub fn is_simple(&self) -> bool {
        matches!(
            self.data.kind,
            TransactionKind::Transfer { .. } | TransactionKind::TransferToken { .. }
        )
    }

    /// Gas cost estimate for this transaction type.
    pub fn estimated_gas(&self) -> Gas {
        match &self.data.kind {
            TransactionKind::Transfer { .. } => 21_000,
            TransactionKind::DeployContract { code, .. } => {
                50_000u64.saturating_add((code.len() as Gas).saturating_mul(200))
            }
            TransactionKind::ContractCall { args, .. } => {
                30_000u64.saturating_add((args.len() as Gas).saturating_mul(16))
            }
            TransactionKind::Stake { .. } => 25_000,
            TransactionKind::Unstake { .. } => 25_000,
            TransactionKind::MiningProof { digits, proof, .. } => {
                100_000u64
                    .saturating_add((digits.len() as Gas).saturating_mul(100))
                    .saturating_add((proof.len() as Gas).saturating_mul(50))
            }
            // Token operations
            TransactionKind::CreateToken { .. } => 50_000,
            TransactionKind::MintToken { .. } => 25_000,
            TransactionKind::TransferToken { .. } => 25_000,
            TransactionKind::BurnToken { .. } => 25_000,
            TransactionKind::ApproveToken { .. } => 25_000,
            TransactionKind::RevokeMintAuthority { .. } => 25_000,
            TransactionKind::FreezeTokenAccount { .. } => 25_000,
            TransactionKind::ThawTokenAccount { .. } => 25_000,
            // DEX operations
            TransactionKind::CreatePool { .. } => 50_000,
            TransactionKind::AddLiquidity { .. } => 60_000,
            TransactionKind::RemoveLiquidity { .. } => 60_000,
            TransactionKind::Swap { .. } => 50_000,
            // Launchpad operations
            TransactionKind::CreateLaunch { .. } => 75_000,
            TransactionKind::ParticipateInLaunch { .. } => 50_000,
            TransactionKind::FinalizeLaunch { .. } => 100_000,
            // NFT operations
            TransactionKind::CreateNftCollection { .. } => 50_000,
            TransactionKind::MintNft { .. } => 40_000,
            TransactionKind::TransferNft { .. } => 25_000,
            TransactionKind::ListNft { .. } => 25_000,
            TransactionKind::BuyNft { .. } => 60_000,
            TransactionKind::DelistNft { .. } => 25_000,
        }
    }
}

/// Account access declaration for parallel scheduling (Sealevel-style).
/// Every transaction declares which accounts it reads/writes BEFORE execution,
/// enabling the scheduler to run non-conflicting transactions in parallel.
#[derive(Clone, Debug)]
pub struct AccountAccess {
    /// The account address.
    pub address: Address,
    /// Whether this transaction writes to the account.
    pub writable: bool,
}

impl TransactionKind {
    /// Declare which accounts this transaction will access.
    /// Used by the parallel scheduler to detect conflicts without executing.
    pub fn account_accesses(&self, sender: &Address) -> Vec<AccountAccess> {
        let mut accesses = vec![AccountAccess {
            address: *sender,
            writable: true, // Sender always writable (nonce, balance)
        }];

        match self {
            TransactionKind::Transfer { recipient, .. } => {
                accesses.push(AccountAccess {
                    address: *recipient,
                    writable: true,
                });
            }
            TransactionKind::Stake { validator, .. } | TransactionKind::Unstake { validator, .. } => {
                accesses.push(AccountAccess {
                    address: *validator,
                    writable: false,
                });
            }
            TransactionKind::TransferToken { mint, recipient, .. }
            | TransactionKind::MintToken { mint, recipient, .. } => {
                accesses.push(AccountAccess {
                    address: *recipient,
                    writable: true,
                });
                // Token ops touch shared DashMap state keyed by mint.
                // Derive virtual address so concurrent ops on the same token
                // are serialized by the scheduler.
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&mint.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
            }
            TransactionKind::ApproveToken { mint, delegate, .. } => {
                accesses.push(AccountAccess {
                    address: *delegate,
                    writable: false,
                });
                // Approve touches shared token account state.
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&mint.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
            }
            TransactionKind::FreezeTokenAccount { mint, target, .. }
            | TransactionKind::ThawTokenAccount { mint, target, .. } => {
                accesses.push(AccountAccess {
                    address: *target,
                    writable: true,
                });
                // Freeze/thaw touches shared token account state.
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&mint.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
            }
            TransactionKind::TransferNft { nft_id, recipient } => {
                accesses.push(AccountAccess {
                    address: *recipient,
                    writable: true,
                });
                // TransferNft touches shared NFT state. Derive a virtual writable
                // address from the NftId so concurrent transfers of the same NFT
                // are correctly serialized by the scheduler.
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&nft_id.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
            }
            TransactionKind::ContractCall { contract, .. } => {
                accesses.push(AccountAccess {
                    address: *contract,
                    writable: true,
                });
            }
            TransactionKind::MintNft { collection, recipient, .. } => {
                accesses.push(AccountAccess {
                    address: *recipient,
                    writable: true,
                });
                // MintNft touches shared collection state (increments `minted` counter).
                // Derive virtual address from CollectionId so concurrent mints to the
                // same collection are serialized by the scheduler.
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&collection.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
            }
            TransactionKind::BurnToken { mint, .. } => {
                // Burns touch shared mint state (total_supply). Derive virtual
                // address from mint ID to serialize concurrent burns on same token.
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&mint.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
            }
            TransactionKind::ListNft { nft_id, .. }
            | TransactionKind::DelistNft { nft_id } => {
                // List/delist touch shared NFT state.
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&nft_id.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
            }
            TransactionKind::ParticipateInLaunch { mint, .. }
            | TransactionKind::CreateLaunch { mint, .. } => {
                // Launchpad operations touch shared launch state keyed by mint.
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&mint.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
            }
            TransactionKind::FinalizeLaunch { mint } => {
                // FinalizeLaunch touches launch state AND creates a DEX pool
                // (mint/PI pair). Must declare both virtual addresses so the
                // scheduler serializes with concurrent Swap/AddLiquidity ops.
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&mint.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
                // DEX pool virtual address (R25-FIX: was missing).
                // Without this, FinalizeLaunch could run in parallel with
                // Swap/AddLiquidity on the newly-created pool.
                let pi_mint = crate::MintId::ZERO;
                let pool_id = crate::dex::PoolId::derive(mint, &pi_mint);
                let mut pool_vaddr = [0u8; 20];
                pool_vaddr.copy_from_slice(&pool_id.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(pool_vaddr),
                    writable: true,
                });
            }
            TransactionKind::RevokeMintAuthority { mint } => {
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&mint.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
            }
            TransactionKind::CreateToken { .. }
            | TransactionKind::DeployContract { .. }
            | TransactionKind::CreateNftCollection { .. } => {
                // These only touch the sender's state or create new unique state.
                // Sender is already declared above.
            }
            TransactionKind::MiningProof { .. } => {
                // Mining proofs touch the shared mining processor (global frontier
                // state, digit registry). Declare a virtual writable address so the
                // scheduler serializes all mining proofs within a block, preventing
                // frontier race conditions and duplicate digit registration.
                accesses.push(AccountAccess {
                    address: Address([0xFF; 20]), // sentinel: global mining processor lock
                    writable: true,
                });
            }
            TransactionKind::Swap { mint_in, mint_out, .. }
            | TransactionKind::AddLiquidity { mint_a: mint_in, mint_b: mint_out, .. }
            | TransactionKind::RemoveLiquidity { mint_a: mint_in, mint_b: mint_out, .. }
            | TransactionKind::CreatePool { mint_a: mint_in, mint_b: mint_out } => {
                // DEX operations touch shared pool state. Derive a virtual
                // writable address from the canonical PoolId so the scheduler
                // correctly serializes all transactions on the same pool.
                let pool_id = crate::dex::PoolId::derive(mint_in, mint_out);
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&pool_id.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
                // R26-FIX: Also declare mint virtual addresses so the scheduler
                // detects conflicts with TransferToken/BurnToken/MintToken on the
                // same mints. Without this, a Swap and a BurnToken on the same
                // mint could execute in parallel, causing supply inconsistencies.
                let mut mint_in_vaddr = [0u8; 20];
                mint_in_vaddr.copy_from_slice(&mint_in.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(mint_in_vaddr),
                    writable: true,
                });
                let mut mint_out_vaddr = [0u8; 20];
                mint_out_vaddr.copy_from_slice(&mint_out.0[..20]);
                // Avoid duplicate if mint_in == mint_out (shouldn't happen but safe)
                if mint_out_vaddr != mint_in_vaddr {
                    accesses.push(AccountAccess {
                        address: Address(mint_out_vaddr),
                        writable: true,
                    });
                }
            }
            TransactionKind::BuyNft { nft_id } => {
                // BuyNft touches shared NFT state. Derive a virtual writable
                // address from the NftId so concurrent buys for the same NFT
                // are correctly serialized by the scheduler.
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&nft_id.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
                // R27-FIX: BuyNft performs PI transfers to the seller and
                // royalty recipient whose addresses are only known at execution
                // time (stored in NFT state). Use a global marketplace sentinel
                // to serialize all BuyNft transactions, preventing PI balance
                // races between BuyNft and concurrent Transfer/Stake/etc.
                accesses.push(AccountAccess {
                    address: Address([0xFE; 20]), // sentinel: global NFT marketplace lock
                    writable: true,
                });
            }
        }

        accesses
    }
}

/// Unsigned transaction builder (convenience).
pub struct Transaction;

impl Transaction {
    pub fn transfer(
        sender: Address,
        nonce: Nonce,
        recipient: Address,
        amount: PiAmount,
        chain_id: u64,
    ) -> TransactionData {
        TransactionData {
            sender,
            nonce,
            kind: TransactionKind::Transfer { recipient, amount },
            gas_limit: 21_000,
            max_base_fee: 1_000, // 0.000001 PI
            max_priority_fee: 100,
            chain_id,
        }
    }

    pub fn sign(data: TransactionData, keypair: &pichain_crypto::Keypair) -> SignedTransaction {
        let bytes = data.canonical_bytes();
        let signature = keypair.sign(&bytes);
        SignedTransaction {
            data,
            signature,
            public_key: keypair.public,
        }
    }
}

/// Effect of executing a transaction (state changes).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionEffect {
    /// Transaction hash.
    pub tx_hash: Hash,
    /// Execution status.
    pub status: TransactionStatus,
    /// Gas actually consumed.
    pub gas_used: Gas,
    /// Base fee per gas at time of execution.
    pub base_fee: PiAmount,
    /// Objects created by this transaction.
    pub created_objects: Vec<crate::ObjectId>,
    /// Objects modified by this transaction.
    pub modified_objects: Vec<crate::ObjectId>,
    /// Objects deleted by this transaction.
    pub deleted_objects: Vec<crate::ObjectId>,
    /// Events emitted during execution.
    pub events: Vec<TransactionEvent>,
}

/// Transaction execution status.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransactionStatus {
    Success,
    /// Execution reverted with a reason.
    Reverted(String),
    /// Out of gas.
    OutOfGas,
}

/// Event emitted during transaction execution.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionEvent {
    pub emitter: Address,
    pub event_type: String,
    pub data: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pichain_crypto::Keypair;

    #[test]
    fn create_and_sign_transfer() {
        let sender_kp = Keypair::generate();
        let recipient_kp = Keypair::generate();

        let tx_data = Transaction::transfer(
            sender_kp.address(),
            0,
            recipient_kp.address(),
            1_000_000_000, // 1 PI
            1,             // chain_id
        );

        let signed = Transaction::sign(tx_data, &sender_kp);
        assert!(signed.verify().is_ok());
        assert!(signed.is_simple());
        assert_eq!(signed.estimated_gas(), 21_000);
    }

    #[test]
    fn wrong_signer_fails_verification() {
        let sender_kp = Keypair::generate();
        let wrong_kp = Keypair::generate();

        let tx_data = Transaction::transfer(
            sender_kp.address(),
            0,
            wrong_kp.address(),
            1_000,
            1,
        );

        // Sign with wrong key
        let bytes = serde_json::to_vec(&tx_data).unwrap();
        let sig = wrong_kp.sign(&bytes);

        let signed = SignedTransaction {
            data: tx_data,
            signature: sig,
            public_key: wrong_kp.public, // wrong pubkey doesn't match sender
        };

        assert!(signed.verify().is_err());
    }

    #[test]
    fn tx_hash_is_deterministic() {
        let kp = Keypair::generate();
        let tx_data = Transaction::transfer(kp.address(), 0, Address::ZERO, 100, 1);
        let signed = Transaction::sign(tx_data, &kp);
        let h1 = signed.hash();
        let h2 = signed.hash();
        assert_eq!(h1, h2);
    }
}
