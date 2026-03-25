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
    DeployContract { code: Vec<u8>, init_data: Vec<u8> },
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

    /// Sell tokens back to the bonding curve for PI.
    SellFromLaunch {
        /// Token mint ID.
        mint: crate::MintId,
        /// Token amount to sell (in base units).
        token_amount: u64,
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

    // --- Multi-Signature ---
    /// Create a new multi-signature wallet.
    CreateMultisig {
        /// Authorized signer addresses.
        signers: Vec<Address>,
        /// Required number of signatures (M-of-N).
        threshold: u8,
    },

    /// Execute a transaction from a multisig wallet.
    ExecuteMultisig {
        /// The multisig wallet address.
        multisig_address: Address,
        /// Serialized inner transaction to execute.
        inner_tx_data: Vec<u8>,
        /// Signatures from authorized signers.
        signatures: Vec<(Address, Vec<u8>)>,
    },

    // --- Bridge ---
    /// Burn wrapped tokens for cross-chain withdrawal.
    BridgeWithdraw {
        /// Wrapped token mint ID to burn.
        mint: crate::MintId,
        /// Amount to burn (in token base units).
        amount: u64,
        /// Destination chain identifier.
        dest_chain: String,
        /// Destination address on the target chain.
        dest_address: String,
    },

    // --- Betting / Gaming ---
    /// Create a new betting match (PVP, Poker, or Arcade).
    /// Creator's wager is escrowed immediately.
    CreateMatch {
        /// Game category tag (0=PVP, 1=Poker, 2=Arcade).
        game_category: u8,
        /// Game type identifier (e.g., "fps", "poker", "snake").
        game_id: String,
        /// Wager per player (base PI units).
        wager: u64,
        /// Maximum number of players (2-10).
        max_players: u8,
        /// Server seed commitment: blake3(server_seed).
        server_seed_hash: [u8; 32],
    },

    /// Join an existing match (escrows joiner's wager).
    JoinMatch {
        /// Match to join.
        match_id: [u8; 32],
        /// Client seed for provably fair randomness.
        client_seed: [u8; 32],
    },

    /// Start a match (creator only, transitions Open -> InProgress).
    /// Captures the current block hash as entropy.
    StartMatch {
        /// Match to start.
        match_id: [u8; 32],
    },

    /// Resolve a match — verify seed, distribute pot to winners.
    ResolveMatch {
        /// Match to resolve.
        match_id: [u8; 32],
        /// Winner addresses.
        winners: Vec<Address>,
        /// Revealed server seed (must hash to committed server_seed_hash).
        server_seed: Vec<u8>,
    },

    /// Cancel a match — refund all escrowed wagers.
    CancelMatch {
        /// Match to cancel.
        match_id: [u8; 32],
    },

    /// Remove a participant from a match — refund their wager individually.
    /// Creator-only. Cannot remove self (use CancelMatch instead).
    RemoveParticipant {
        /// Match to remove participant from.
        match_id: [u8; 32],
        /// Address of participant to remove and refund.
        participant: Address,
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
        buf.extend_from_slice(&self.sender.0); // 20 bytes
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
            TransactionKind::ContractCall {
                contract,
                function,
                args,
            } => {
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
                start_position,
                digit_count,
                digits,
                proof,
                pow_nonce,
                anchor_block_hash,
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
            TransactionKind::CreateToken {
                name,
                symbol,
                decimals,
                max_supply,
                metadata_uri,
            } => {
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
            TransactionKind::MintToken {
                mint,
                recipient,
                amount,
            } => {
                buf.push(7);
                buf.extend_from_slice(&mint.0);
                buf.extend_from_slice(&recipient.0);
                buf.extend_from_slice(&amount.to_le_bytes());
            }
            TransactionKind::TransferToken {
                mint,
                recipient,
                amount,
            } => {
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
            TransactionKind::ApproveToken {
                mint,
                delegate,
                amount,
            } => {
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
            TransactionKind::AddLiquidity {
                mint_a,
                mint_b,
                amount_a,
                amount_b,
                min_lp_tokens,
            } => {
                buf.push(15);
                buf.extend_from_slice(&mint_a.0);
                buf.extend_from_slice(&mint_b.0);
                buf.extend_from_slice(&amount_a.to_le_bytes());
                buf.extend_from_slice(&amount_b.to_le_bytes());
                buf.extend_from_slice(&min_lp_tokens.to_le_bytes());
            }
            TransactionKind::RemoveLiquidity {
                mint_a,
                mint_b,
                lp_amount,
                min_amount_a,
                min_amount_b,
            } => {
                buf.push(16);
                buf.extend_from_slice(&mint_a.0);
                buf.extend_from_slice(&mint_b.0);
                buf.extend_from_slice(&lp_amount.to_le_bytes());
                buf.extend_from_slice(&min_amount_a.to_le_bytes());
                buf.extend_from_slice(&min_amount_b.to_le_bytes());
            }
            TransactionKind::Swap {
                mint_in,
                mint_out,
                amount_in,
                min_amount_out,
            } => {
                buf.push(17);
                buf.extend_from_slice(&mint_in.0);
                buf.extend_from_slice(&mint_out.0);
                buf.extend_from_slice(&amount_in.to_le_bytes());
                buf.extend_from_slice(&min_amount_out.to_le_bytes());
            }
            TransactionKind::CreateLaunch {
                mint,
                launch_type,
                tokens_for_sale,
                target_pi,
                max_per_address,
            } => {
                buf.push(18);
                buf.extend_from_slice(&mint.0);
                // Encode launch_type deterministically
                match launch_type {
                    crate::launchpad::LaunchType::FairLaunch { price_per_token } => {
                        buf.push(0);
                        buf.extend_from_slice(&price_per_token.to_le_bytes());
                    }
                    crate::launchpad::LaunchType::BondingCurve {
                        base_price,
                        slope,
                        price_scale,
                    } => {
                        buf.push(1);
                        buf.extend_from_slice(&base_price.to_le_bytes());
                        buf.extend_from_slice(&slope.to_le_bytes());
                        buf.extend_from_slice(&price_scale.to_le_bytes());
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
            TransactionKind::SellFromLaunch { mint, token_amount } => {
                buf.push(30);
                buf.extend_from_slice(&mint.0);
                buf.extend_from_slice(&token_amount.to_le_bytes());
            }
            TransactionKind::FinalizeLaunch { mint } => {
                buf.push(20);
                buf.extend_from_slice(&mint.0);
            }
            TransactionKind::CreateNftCollection {
                name,
                symbol,
                max_supply,
                royalty_bps,
                base_uri,
            } => {
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
            TransactionKind::MintNft {
                collection,
                recipient,
                name,
                metadata_uri,
                attributes,
            } => {
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
            TransactionKind::CreateMultisig { signers, threshold } => {
                buf.push(27);
                buf.push(*threshold);
                buf.extend_from_slice(&(signers.len() as u32).to_le_bytes());
                for signer in signers {
                    buf.extend_from_slice(&signer.0);
                }
            }
            TransactionKind::ExecuteMultisig {
                multisig_address,
                inner_tx_data,
                signatures,
            } => {
                buf.push(28);
                buf.extend_from_slice(&multisig_address.0);
                buf.extend_from_slice(&(inner_tx_data.len() as u32).to_le_bytes());
                buf.extend_from_slice(inner_tx_data);
                buf.extend_from_slice(&(signatures.len() as u32).to_le_bytes());
                for (addr, sig) in signatures {
                    buf.extend_from_slice(&addr.0);
                    buf.extend_from_slice(&(sig.len() as u32).to_le_bytes());
                    buf.extend_from_slice(sig);
                }
            }
            TransactionKind::BridgeWithdraw {
                mint,
                amount,
                dest_chain,
                dest_address,
            } => {
                buf.push(29);
                buf.extend_from_slice(&mint.0);
                buf.extend_from_slice(&amount.to_le_bytes());
                let chain_b = dest_chain.as_bytes();
                buf.extend_from_slice(&(chain_b.len() as u32).to_le_bytes());
                buf.extend_from_slice(chain_b);
                let addr_b = dest_address.as_bytes();
                buf.extend_from_slice(&(addr_b.len() as u32).to_le_bytes());
                buf.extend_from_slice(addr_b);
            }

            // --- Betting / Gaming (tags 31-35) ---
            TransactionKind::CreateMatch {
                game_category,
                game_id,
                wager,
                max_players,
                server_seed_hash,
            } => {
                buf.push(31);
                buf.push(*game_category);
                let gid = game_id.as_bytes();
                buf.extend_from_slice(&(gid.len() as u32).to_le_bytes());
                buf.extend_from_slice(gid);
                buf.extend_from_slice(&wager.to_le_bytes());
                buf.push(*max_players);
                buf.extend_from_slice(server_seed_hash);
            }
            TransactionKind::JoinMatch {
                match_id,
                client_seed,
            } => {
                buf.push(32);
                buf.extend_from_slice(match_id);
                buf.extend_from_slice(client_seed);
            }
            TransactionKind::StartMatch { match_id } => {
                buf.push(33);
                buf.extend_from_slice(match_id);
            }
            TransactionKind::ResolveMatch {
                match_id,
                winners,
                server_seed,
            } => {
                buf.push(34);
                buf.extend_from_slice(match_id);
                buf.extend_from_slice(&(winners.len() as u32).to_le_bytes());
                for w in winners {
                    buf.extend_from_slice(&w.0);
                }
                buf.extend_from_slice(&(server_seed.len() as u32).to_le_bytes());
                buf.extend_from_slice(server_seed);
            }
            TransactionKind::CancelMatch { match_id } => {
                buf.push(35);
                buf.extend_from_slice(match_id);
            }
            TransactionKind::RemoveParticipant {
                match_id,
                participant,
            } => {
                buf.push(36);
                buf.extend_from_slice(match_id);
                buf.extend_from_slice(&participant.0);
            }
        }

        buf
    }
}

/// A transaction with its signature.
///
/// Supports two authentication modes via [`CryptoVersion`]:
/// - **Ed25519Legacy** (v0): Classical Ed25519 signature + public key.
/// - **PqDualV1** (v1): Dual post-quantum ML-DSA-65 + SLH-DSA-SHAKE-128f.
///   Both PQ signatures must verify. Address derived via quantum-safe hash combiner.
///
/// The `crypto_version` field enables crypto agility — new schemes can be added
/// without hard forks. Old versions remain verifiable for historical blocks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedTransaction {
    /// The transaction data.
    pub data: TransactionData,
    /// Ed25519 signature over the serialized TransactionData.
    pub signature: Signature,
    /// Sender's public key (for verification without state lookup).
    pub public_key: PublicKey,
    /// Cryptographic scheme version (default: Ed25519Legacy for backwards compat).
    #[serde(default)]
    pub crypto_version: pichain_crypto::CryptoVersion,
    /// Post-quantum dual signature (ML-DSA-65 + SLH-DSA-SHAKE-128f).
    /// Present only when `crypto_version == PqDualV1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pq_signature: Option<pichain_crypto::PqDualSignature>,
    /// Post-quantum public keys (ML-DSA-65 + SLH-DSA-SHAKE-128f).
    /// Present only when `crypto_version == PqDualV1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pq_public_keys: Option<pichain_crypto::PqPublicKeys>,
}

impl SignedTransaction {
    /// Compute the transaction hash (unique identifier).
    /// Includes the signature to prevent transaction malleability — even though
    /// Ed25519 signatures are deterministic, including them ensures the hash
    /// uniquely identifies both the intent and its authorization.
    ///
    /// For PQ transactions, the hash also includes the ML-DSA signature component
    /// to bind the PQ auth to the transaction identity.
    pub fn hash(&self) -> Hash {
        let data_bytes = self.data.canonical_bytes();
        match self.crypto_version {
            pichain_crypto::CryptoVersion::Ed25519Legacy => {
                let sig_bytes = self.signature.to_bytes();
                pichain_crypto::hash_concat(&[&data_bytes, &sig_bytes])
            }
            pichain_crypto::CryptoVersion::PqDualV1 => {
                // Include ML-DSA sig in hash (3,309 bytes — smaller than SLH-DSA's 17KB)
                if let Some(ref pq_sig) = self.pq_signature {
                    pichain_crypto::hash_concat(&[&data_bytes, pq_sig.ml_sig.as_bytes()])
                } else {
                    // Fallback: should not happen for valid PQ txs
                    pichain_crypto::hash_concat(&[&data_bytes, &[1u8]])
                }
            }
        }
    }

    /// Verify the transaction signature based on the crypto version.
    ///
    /// - **Ed25519Legacy**: Verifies Ed25519 signature and checks pubkey→address derivation.
    /// - **PqDualV1**: Verifies BOTH ML-DSA-65 and SLH-DSA-SHAKE-128f signatures,
    ///   and checks that the PQ public keys derive the sender address via the
    ///   quantum-safe hash combiner (Blake3 XOR SHA3-256).
    pub fn verify(&self) -> Result<(), pichain_crypto::CryptoError> {
        match self.crypto_version {
            pichain_crypto::CryptoVersion::Ed25519Legacy => {
                // Ed25519 verification retained for internal/test use.
                // The mempool and RPC layer reject Ed25519 transactions from external users.
                let derived_addr = self.public_key.to_address();
                if derived_addr != self.data.sender {
                    return Err(pichain_crypto::CryptoError::InvalidPublicKey);
                }
                let bytes = self.data.canonical_bytes();
                self.public_key.verify(&bytes, &self.signature)
            }
            pichain_crypto::CryptoVersion::PqDualV1 => {
                // SECURITY: Both PQ components MUST be present for PqDualV1.
                // Reject transactions that claim PQ version but are missing components.
                let pq_sig = self.pq_signature.as_ref().ok_or_else(|| {
                    pichain_crypto::CryptoError::Serialization(
                        "PqDualV1 transaction missing pq_signature".into(),
                    )
                })?;
                let pq_pks = self.pq_public_keys.as_ref().ok_or_else(|| {
                    pichain_crypto::CryptoError::Serialization(
                        "PqDualV1 transaction missing pq_public_keys".into(),
                    )
                })?;

                // Verify PQ public keys derive the sender address
                let derived_addr = pichain_crypto::pq_address(&pq_pks.ml_pk, &pq_pks.slh_pk);
                if derived_addr != self.data.sender {
                    return Err(pichain_crypto::CryptoError::InvalidPublicKey);
                }

                // Verify BOTH PQ signatures over canonical tx data
                let bytes = self.data.canonical_bytes();
                pq_sig.verify(&bytes, &pq_pks.ml_pk, &pq_pks.slh_pk)
            }
        }
    }

    /// Returns true if this transaction uses post-quantum signatures.
    pub fn is_post_quantum(&self) -> bool {
        self.crypto_version.is_post_quantum()
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
            TransactionKind::MiningProof { digits, .. } => {
                // Gas scales with digit count so small miners stay profitable.
                // At base_fee=1000: 10 digits → 550 gas → 0.00055 PI fee (3.3% of reward)
                // 2000 digits → 10,500 gas → 0.0105 PI fee (0.3% of reward)
                500u64.saturating_add((digits.len() as Gas).saturating_mul(5))
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
            TransactionKind::SellFromLaunch { .. } => 50_000,
            TransactionKind::FinalizeLaunch { .. } => 100_000,
            // NFT operations
            TransactionKind::CreateNftCollection { .. } => 50_000,
            TransactionKind::MintNft { .. } => 40_000,
            TransactionKind::TransferNft { .. } => 25_000,
            TransactionKind::ListNft { .. } => 25_000,
            TransactionKind::BuyNft { .. } => 60_000,
            TransactionKind::DelistNft { .. } => 25_000,
            TransactionKind::CreateMultisig { .. } => 50_000,
            TransactionKind::ExecuteMultisig { inner_tx_data, .. } => {
                75_000u64.saturating_add((inner_tx_data.len() as Gas).saturating_mul(16))
            }
            TransactionKind::BridgeWithdraw { .. } => 50_000,
            // Betting
            TransactionKind::CreateMatch { .. } => 50_000,
            TransactionKind::JoinMatch { .. } => 30_000,
            TransactionKind::StartMatch { .. } => 25_000,
            TransactionKind::ResolveMatch { winners, .. } => {
                50_000u64.saturating_add((winners.len() as Gas).saturating_mul(10_000))
            }
            TransactionKind::CancelMatch { .. } => 30_000,
            TransactionKind::RemoveParticipant { .. } => 35_000,
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
    /// Return the primary recipient address of this transaction, if any.
    /// Used for tx history indexing (index both sender and recipient).
    pub fn recipient_address(&self) -> Option<Address> {
        match self {
            TransactionKind::Transfer { recipient, .. } => Some(*recipient),
            TransactionKind::ContractCall { contract, .. } => Some(*contract),
            TransactionKind::Stake { validator, .. } => Some(*validator),
            TransactionKind::Unstake { validator, .. } => Some(*validator),
            TransactionKind::MintToken { recipient, .. } => Some(*recipient),
            TransactionKind::TransferToken { recipient, .. } => Some(*recipient),
            TransactionKind::TransferNft { recipient, .. } => Some(*recipient),
            TransactionKind::RemoveParticipant { participant, .. } => Some(*participant),
            _ => None,
        }
    }

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
            TransactionKind::Stake { validator, .. }
            | TransactionKind::Unstake { validator, .. } => {
                accesses.push(AccountAccess {
                    address: *validator,
                    writable: false,
                });
            }
            TransactionKind::TransferToken {
                mint, recipient, ..
            }
            | TransactionKind::MintToken {
                mint, recipient, ..
            } => {
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
            TransactionKind::MintNft {
                collection,
                recipient,
                ..
            } => {
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
            TransactionKind::ListNft { nft_id, .. } | TransactionKind::DelistNft { nft_id } => {
                // List/delist touch shared NFT state.
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&nft_id.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
            }
            TransactionKind::ParticipateInLaunch { mint, .. }
            | TransactionKind::SellFromLaunch { mint, .. }
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
            TransactionKind::Swap {
                mint_in, mint_out, ..
            }
            | TransactionKind::AddLiquidity {
                mint_a: mint_in,
                mint_b: mint_out,
                ..
            }
            | TransactionKind::RemoveLiquidity {
                mint_a: mint_in,
                mint_b: mint_out,
                ..
            }
            | TransactionKind::CreatePool {
                mint_a: mint_in,
                mint_b: mint_out,
            } => {
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
            TransactionKind::CreateMultisig { .. } => {
                // Only touches sender state (creates new multisig wallet).
            }
            TransactionKind::ExecuteMultisig {
                multisig_address, ..
            } => {
                accesses.push(AccountAccess {
                    address: *multisig_address,
                    writable: true,
                });
            }
            TransactionKind::BridgeWithdraw { mint, .. } => {
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&mint.0[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
            }
            // Betting: use a sentinel address derived from match_id to serialize
            // operations on the same match while allowing parallel match operations.
            TransactionKind::CreateMatch { .. } => {
                // New match — only touches sender, no sentinel needed.
            }
            TransactionKind::JoinMatch { match_id, .. }
            | TransactionKind::StartMatch { match_id }
            | TransactionKind::ResolveMatch { match_id, .. }
            | TransactionKind::CancelMatch { match_id }
            | TransactionKind::RemoveParticipant { match_id, .. } => {
                let mut vaddr = [0u8; 20];
                vaddr.copy_from_slice(&match_id[..20]);
                accesses.push(AccountAccess {
                    address: Address(vaddr),
                    writable: true,
                });
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

    /// DO NOT USE IN PRODUCTION. Ed25519 is quantum-vulnerable.
    /// Exists only for test infrastructure across crates. The mempool
    /// rejects Ed25519 transactions (require_pq=true in production).
    #[doc(hidden)]
    pub fn sign_ed25519_for_tests_only(
        data: TransactionData,
        keypair: &pichain_crypto::Keypair,
    ) -> SignedTransaction {
        let bytes = data.canonical_bytes();
        let signature = keypair.sign(&bytes);
        SignedTransaction {
            data,
            signature,
            public_key: keypair.public,
            crypto_version: pichain_crypto::CryptoVersion::Ed25519Legacy,
            pq_signature: None,
            pq_public_keys: None,
        }
    }

    /// Sign with dual post-quantum signatures (ML-DSA-65 + SLH-DSA-SHAKE-128f).
    /// This is the ONLY signing method accepted by the network for external transactions.
    pub fn sign_pq(
        data: TransactionData,
        pq_keypair: &pichain_crypto::PqKeypair,
    ) -> SignedTransaction {
        let bytes = data.canonical_bytes();
        let pq_sig = pq_keypair.sign(&bytes);
        let pq_pks = pichain_crypto::PqPublicKeys {
            ml_pk: pq_keypair.ml_dsa_pk.clone(),
            slh_pk: pq_keypair.slh_dsa_pk.clone(),
        };
        SignedTransaction {
            data,
            // Placeholder Ed25519 fields (not used for PQ verification)
            signature: Signature::from_bytes(&[0u8; 64]),
            public_key: PublicKey::from_bytes(&[0u8; 32]),
            crypto_version: pichain_crypto::CryptoVersion::PqDualV1,
            pq_signature: Some(pq_sig),
            pq_public_keys: Some(pq_pks),
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

        let signed = Transaction::sign_ed25519_for_tests_only(tx_data, &sender_kp);
        assert!(signed.verify().is_ok());
        assert!(signed.is_simple());
        assert_eq!(signed.estimated_gas(), 21_000);
    }

    #[test]
    fn wrong_signer_fails_verification() {
        let sender_kp = Keypair::generate();
        let wrong_kp = Keypair::generate();

        let tx_data = Transaction::transfer(sender_kp.address(), 0, wrong_kp.address(), 1_000, 1);

        // Sign with wrong key
        let bytes = serde_json::to_vec(&tx_data).unwrap();
        let sig = wrong_kp.sign(&bytes);

        let signed = SignedTransaction {
            data: tx_data,
            signature: sig,
            public_key: wrong_kp.public, // wrong pubkey doesn't match sender
            crypto_version: pichain_crypto::CryptoVersion::Ed25519Legacy,
            pq_signature: None,
            pq_public_keys: None,
        };

        assert!(signed.verify().is_err());
    }

    #[test]
    fn tx_hash_is_deterministic() {
        let kp = Keypair::generate();
        let tx_data = Transaction::transfer(kp.address(), 0, Address::ZERO, 100, 1);
        let signed = Transaction::sign_ed25519_for_tests_only(tx_data, &kp);
        let h1 = signed.hash();
        let h2 = signed.hash();
        assert_eq!(h1, h2);
    }
}
