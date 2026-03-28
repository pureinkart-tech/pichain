/**
 * Solana RPC + Jupiter DEX client for PiBot.
 *
 * Handles:
 * - Wallet creation/import (Ed25519 → base58 address)
 * - SOL balance + SPL token balances
 * - Jupiter v1 swap quotes + execution
 * - Token metadata lookups
 * - Transaction signing + submission
 */

const { Connection, Keypair, VersionedTransaction, PublicKey, LAMPORTS_PER_SOL } = require('@solana/web3.js');
const bs58 = require('bs58').default || require('bs58');
const nacl = require('tweetnacl');

// ── Constants ──────────────────────────────────────────────────────────────
// lite-api is the free public endpoint (no API key required)
const JUPITER_API = process.env.JUPITER_API || 'https://lite-api.jup.ag';
const SOL_MINT = 'So11111111111111111111111111111111111111112';
const USDC_MINT = 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v';

class SolanaClient {
  constructor(rpcUrl = 'https://api.mainnet-beta.solana.com') {
    this.rpcUrl = rpcUrl;
    this.connection = new Connection(rpcUrl, 'confirmed');
    this.jupiterApi = JUPITER_API;
  }

  // ── Wallet ───────────────────────────────────────────────────────────
  createWallet() {
    const kp = Keypair.generate();
    return {
      publicKey: kp.publicKey.toBase58(),
      secretKey: bs58.encode(kp.secretKey),
      keypair: kp,
    };
  }

  walletFromSecret(secretBase58) {
    const secretBytes = bs58.decode(secretBase58);
    const kp = Keypair.fromSecretKey(secretBytes);
    return {
      publicKey: kp.publicKey.toBase58(),
      secretKey: secretBase58,
      keypair: kp,
    };
  }

  walletFromSeed(seedHex) {
    // 32-byte seed → Ed25519 keypair (compatible with PIChain seeds)
    const seed = Buffer.from(seedHex, 'hex');
    if (seed.length !== 32) throw new Error('Seed must be 32 bytes (64 hex chars)');
    const naclKp = nacl.sign.keyPair.fromSeed(seed);
    const kp = Keypair.fromSecretKey(naclKp.secretKey);
    return {
      publicKey: kp.publicKey.toBase58(),
      secretKey: bs58.encode(kp.secretKey),
      seedHex,
      keypair: kp,
    };
  }

  // ── Account queries ──────────────────────────────────────────────────
  async getBalance(publicKey) {
    try {
      const pk = new PublicKey(publicKey);
      const lamports = await this.connection.getBalance(pk);
      return lamports;
    } catch { return 0; }
  }

  async getBalanceSOL(publicKey) {
    const lamports = await this.getBalance(publicKey);
    return lamports / LAMPORTS_PER_SOL;
  }

  async getTokenAccounts(publicKey) {
    try {
      const pk = new PublicKey(publicKey);
      const parse = (resp) => resp.value.map(a => {
        const info = a.account.data.parsed.info;
        return {
          mint: info.mint,
          balance: parseInt(info.tokenAmount.amount),
          decimals: info.tokenAmount.decimals,
          uiBalance: parseFloat(info.tokenAmount.uiAmountString || '0'),
        };
      }).filter(t => t.balance > 0);

      // Query BOTH standard SPL tokens AND Token-2022 tokens
      const [spl, t22] = await Promise.all([
        this.connection.getParsedTokenAccountsByOwner(pk, {
          programId: new PublicKey('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA'),
        }).catch(() => ({ value: [] })),
        this.connection.getParsedTokenAccountsByOwner(pk, {
          programId: new PublicKey('TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb'),
        }).catch(() => ({ value: [] })),
      ]);

      return [...parse(spl), ...parse(t22)];
    } catch { return []; }
  }

  // ── Jupiter swap quote ───────────────────────────────────────────────
  async getQuote(inputMint, outputMint, amount, slippageBps = 50) {
    const params = new URLSearchParams({
      inputMint,
      outputMint,
      amount: String(amount),
      slippageBps: String(slippageBps),
      restrictIntermediateTokens: 'true',
      maxAccounts: '64',
    });
    const url = `${this.jupiterApi}/swap/v1/quote?${params}`;
    const res = await fetch(url, {
      signal: AbortSignal.timeout(10000),
    });
    if (!res.ok) {
      const text = await res.text().catch(() => '');
      throw new Error(`Jupiter quote failed (${res.status}): ${text.slice(0, 200)}`);
    }
    return res.json();
  }

  // ── Jupiter swap execution ───────────────────────────────────────────
  async swap(wallet, inputMint, outputMint, amount, slippageBps = 50) {
    // Step 1: Get quote
    const quote = await this.getQuote(inputMint, outputMint, amount, slippageBps);

    // Step 2: Get swap transaction
    const swapRes = await fetch(`${this.jupiterApi}/swap/v1/swap`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        quoteResponse: quote,
        userPublicKey: wallet.publicKey,
        dynamicComputeUnitLimit: true,
        dynamicSlippage: true,
        prioritizationFeeLamports: 'auto',
      }),
      signal: AbortSignal.timeout(15000),
    });

    if (!swapRes.ok) {
      const text = await swapRes.text().catch(() => '');
      throw new Error(`Jupiter swap failed (${swapRes.status}): ${text.slice(0, 200)}`);
    }

    const { swapTransaction, lastValidBlockHeight } = await swapRes.json();

    // Step 3: Deserialize, sign, serialize
    const txBuf = Buffer.from(swapTransaction, 'base64');
    const tx = VersionedTransaction.deserialize(txBuf);
    tx.sign([wallet.keypair]);

    // Step 4: Submit
    const rawTx = tx.serialize();
    const txid = await this.connection.sendRawTransaction(rawTx, {
      skipPreflight: true,
      maxRetries: 3,
    });

    return {
      txid,
      quote,
      lastValidBlockHeight,
    };
  }

  // ── Wait for confirmation ────────────────────────────────────────────
  async waitConfirm(txid, timeoutMs = 30000) {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
      try {
        const status = await this.connection.getSignatureStatus(txid);
        if (status?.value?.confirmationStatus === 'confirmed' ||
            status?.value?.confirmationStatus === 'finalized') {
          return { confirmed: true, status: status.value };
        }
        if (status?.value?.err) {
          return { confirmed: false, error: JSON.stringify(status.value.err) };
        }
      } catch {}
      await new Promise(r => setTimeout(r, 1000));
    }
    throw new Error('Confirmation timeout');
  }

  // ── SOL transfer ─────────────────────────────────────────────────────
  async transfer(wallet, recipientBase58, lamports) {
    const { SystemProgram, Transaction } = require('@solana/web3.js');
    const tx = new Transaction().add(
      SystemProgram.transfer({
        fromPubkey: wallet.keypair.publicKey,
        toPubkey: new PublicKey(recipientBase58),
        lamports,
      })
    );
    tx.feePayer = wallet.keypair.publicKey;
    tx.recentBlockhash = (await this.connection.getLatestBlockhash()).blockhash;
    tx.sign(wallet.keypair);
    const txid = await this.connection.sendRawTransaction(tx.serialize());
    return txid;
  }

  // ── Token metadata from Jupiter ──────────────────────────────────────
  async getTokenInfo(mintAddress) {
    // Well-known tokens (instant, no API call)
    const KNOWN = {
      [SOL_MINT]: { symbol: 'SOL', name: 'Solana', decimals: 9, address: SOL_MINT },
      [USDC_MINT]: { symbol: 'USDC', name: 'USD Coin', decimals: 6, address: USDC_MINT },
      'DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263': { symbol: 'BONK', name: 'Bonk', decimals: 5, address: 'DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263' },
      'EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm': { symbol: 'WIF', name: 'dogwifhat', decimals: 6, address: 'EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm' },
      'JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN': { symbol: 'JUP', name: 'Jupiter', decimals: 6, address: 'JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN' },
      'bf7BTmV7qUY1jiZA9FybL1tAswhsqhnYXMML8Frpump': { symbol: 'UFC', name: 'UFC Token', decimals: 6, address: 'bf7BTmV7qUY1jiZA9FybL1tAswhsqhnYXMML8Frpump' },
    };
    if (KNOWN[mintAddress]) return KNOWN[mintAddress];

    // Check shared cache first (shared with Crypto Bros bot)
    const sharedCache = require('../../shared-token-cache.cjs');
    const cached = sharedCache.get(mintAddress);
    if (cached?.symbol) return { symbol: cached.symbol, name: cached.name || '', decimals: cached.decimals || 9, address: mintAddress };

    // Try DexScreener API (new chain-specific endpoint, safe JSON parsing)
    try {
      const data = await sharedCache.safeFetchJSON(`https://api.dexscreener.com/tokens/v1/solana/${mintAddress}`, 5000);
      if (Array.isArray(data) && data.length > 0) {
        const pair = data[0];
        const token = pair.baseToken?.address === mintAddress ? pair.baseToken : pair.quoteToken;
        const result = { symbol: token?.symbol || mintAddress.slice(0,6), name: token?.name || '', decimals: 6, address: mintAddress };
        sharedCache.set(mintAddress, result);
        return result;
      }
    } catch {}

    // Fallback: old DexScreener endpoint
    try {
      const data = await sharedCache.safeFetchJSON(`https://api.dexscreener.com/latest/dex/tokens/${mintAddress}`, 5000);
      if (data?.pairs?.[0]) {
        const token = data.pairs[0].baseToken?.address === mintAddress ? data.pairs[0].baseToken : data.pairs[0].quoteToken;
        const result = { symbol: token?.symbol || mintAddress.slice(0,6), name: token?.name || '', decimals: 6, address: mintAddress };
        sharedCache.set(mintAddress, result);
        return result;
      }
    } catch {}

    // Try Moralis (paid API — better decimals/metadata)
    try {
      const moralisKey = process.env.MORALIS_API_KEY;
      if (moralisKey) {
        const d = await sharedCache.safeFetchJSON(`https://solana-gateway.moralis.io/token/mainnet/${mintAddress}/metadata`, 5000);
        if (d?.symbol) {
          const result = { symbol: d.symbol, name: d.name || '', decimals: parseInt(d.decimals || '9'), address: mintAddress };
          sharedCache.set(mintAddress, result);
          return result;
        }
      }
    } catch {}

    // Try Jupiter token list
    try {
      const d = await sharedCache.safeFetchJSON(`https://tokens.jup.ag/token/${mintAddress}`, 3000);
      if (d?.symbol) { sharedCache.set(mintAddress, d); return d; }
    } catch {}

    return { symbol: mintAddress.slice(0,6), name: '', decimals: 6, address: mintAddress };
  }

  // ── Trending tokens ──────────────────────────────────────────────────
  async getTrendingTokens() {
    // Try Jupiter verified token list
    try {
      const res = await fetch('https://tokens.jup.ag/tokens?tags=verified&sortBy=volume24hUSD&limit=20', { signal: AbortSignal.timeout(10000) });
      if (res.ok) { const data = await res.json(); if (data.length > 0) return data; }
    } catch {}

    // Try Pump.fun trending (live meme coins)
    try {
      const res = await fetch('https://frontend-api-v3.pump.fun/coins/currently-live?limit=15&offset=0&includeNsfw=false', { signal: AbortSignal.timeout(8000) });
      if (res.ok) {
        const data = await res.json();
        if (data && data.length > 0) {
          return data.map(t => ({
            symbol: t.symbol || t.mint?.slice(0,6) || '?',
            name: t.name || '',
            address: t.mint,
            decimals: 6,
            source: 'pump.fun',
          }));
        }
      }
    } catch {}

    // Try DexScreener trending
    try {
      const sharedCache = require('../../shared-token-cache.cjs');
      if (sharedCache.isDexCooldown()) throw new Error('cooldown');
      const res = await fetch('https://api.dexscreener.com/token-boosts/top/v1', { signal: AbortSignal.timeout(8000) });
      if (res.ok) {
        const data = await res.json();
        const solTokens = (data || []).filter(t => t.chainId === 'solana').slice(0, 15);
        if (solTokens.length > 0) {
          return solTokens.map(t => ({
            symbol: t.tokenAddress?.slice(0, 6) || '?',
            name: t.description || '',
            address: t.tokenAddress,
            decimals: 9,
            source: 'dexscreener',
          }));
        }
      }
    } catch {}

    // Fallback: well-known Solana tokens
    return [
      { symbol: 'SOL', name: 'Solana', address: SOL_MINT, decimals: 9 },
      { symbol: 'USDC', name: 'USD Coin', address: USDC_MINT, decimals: 6 },
      { symbol: 'BONK', name: 'Bonk', address: 'DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263', decimals: 5 },
      { symbol: 'WIF', name: 'dogwifhat', address: 'EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm', decimals: 6 },
      { symbol: 'JUP', name: 'Jupiter', address: 'JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN', decimals: 6 },
      { symbol: 'RAY', name: 'Raydium', address: '4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R', decimals: 6 },
      { symbol: 'ORCA', name: 'Orca', address: 'orcaEKTdK7LKz57vaAYr9QeNsVEPfiu6QeMU1kektZE', decimals: 6 },
    ];
  }

  // ── Token price from Jupiter ─────────────────────────────────────────
  async getPrice(mintAddress) {
    // Try price API
    for (const base of ['https://api.jup.ag', 'https://lite-api.jup.ag']) {
      try {
        const res = await fetch(`${base}/price/v2?ids=${mintAddress}`, { signal: AbortSignal.timeout(3000) });
        if (res.ok) {
          const data = await res.json();
          const p = parseFloat(data.data?.[mintAddress]?.price || '0');
          if (p > 0) return p;
        }
      } catch {}
    }
    // Fallback: derive from quote (1 SOL -> USDC to get SOL price, then token -> SOL)
    try {
      if (mintAddress === SOL_MINT) {
        const q = await this.getQuote(SOL_MINT, USDC_MINT, LAMPORTS_PER_SOL, 100);
        return parseInt(q.outAmount) / 1e6; // USDC has 6 decimals
      }
      // For other tokens: get token/SOL price, multiply by SOL/USD
      const q = await this.getQuote(mintAddress, SOL_MINT, 1e9, 100); // 1 token worth of SOL
      const solPerToken = parseInt(q.outAmount) / LAMPORTS_PER_SOL;
      const solPrice = await this.getPrice(SOL_MINT);
      return solPerToken * solPrice;
    } catch {}
    return 0;
  }

  async getPrices(mintAddresses) {
    try {
      const ids = mintAddresses.join(',');
      const res = await fetch(`https://lite-api.jup.ag/price/v2?ids=${ids}`, { signal: AbortSignal.timeout(10000) });
      if (res.ok) {
        const data = await res.json();
        const result = {};
        for (const mint of mintAddresses) result[mint] = parseFloat(data.data?.[mint]?.price || '0');
        return result;
      }
    } catch {}
    return {};
  }
}

module.exports = { SolanaClient, SOL_MINT, USDC_MINT, LAMPORTS_PER_SOL };
