/**
 * PIChain RPC client — all blockchain interactions for PiBot.
 *
 * Transaction signing goes through the vault API (pichain-signer).
 * NO private keys are handled in this Node.js process.
 */
const C = require('./crypto');

class PiChainClient {
  constructor(rpcUrl = 'http://127.0.0.1:8314', chainId = 31415) {
    this.rpcUrl = rpcUrl;
    this.chainId = chainId;
    this.baseFee = 1000;
    this.cacheUrl = 'http://127.0.0.1:8350';
    // Per-address nonce tracking to prevent race conditions.
    // After a successful submission, we increment locally rather than
    // re-fetching from the chain (which may not have confirmed yet).
    this._nonceCache = new Map(); // address -> last known nonce
    this._nonceLocks = new Map(); // address -> Promise (serializes per-address txs)
  }

  // Get nonce with local caching — prevents race when two txs fire in parallel
  async _getNextNonce(address) {
    const cached = this._nonceCache.get(address);
    if (cached !== undefined) return cached;
    const onChain = await this.getNonce(address);
    this._nonceCache.set(address, onChain);
    return onChain;
  }

  _incrementNonce(address) {
    const current = this._nonceCache.get(address) || 0;
    this._nonceCache.set(address, current + 1);
  }

  // Serialize transactions per-address to prevent nonce collisions.
  // On error, the lock is released cleanly so subsequent txs can proceed.
  // On timeout/failure, the local nonce cache is invalidated to force re-fetch.
  async _withNonceLock(address, fn) {
    const prev = this._nonceLocks.get(address) || Promise.resolve();
    const execute = async () => {
      try {
        return await fn();
      } catch (e) {
        // On failure, invalidate nonce cache so next tx re-fetches from chain
        this._nonceCache.delete(address);
        throw e;
      }
    };
    // Chain: wait for previous tx to complete (success or failure), then run ours
    const current = prev.catch(() => {}).then(() => execute());
    this._nonceLocks.set(address, current.catch(() => {}));
    return current;
  }

  async get(path) {
    const res = await fetch(`${this.rpcUrl}${path}`, { signal: AbortSignal.timeout(10000) });
    if (!res.ok) {
      try { return await res.json(); } catch {}
      return { found: false, error: `RPC ${res.status}` };
    }
    return res.json();
  }

  async cachedGet(path) {
    try {
      const res = await fetch(`${this.cacheUrl}${path}`, { signal: AbortSignal.timeout(3000) });
      if (res.ok) return res.json();
    } catch {}
    return this.get(path);
  }

  async post(path, body) {
    const res = await fetch(`${this.rpcUrl}${path}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(10000),
    });
    if (!res.ok) {
      try { return await res.json(); } catch {}
      return { success: false, error: `RPC ${res.status}` };
    }
    return res.json();
  }

  // ── Account ────────────────────────────────────────────────────────────
  async getAccount(addr) { return this.get(`/api/v1/account/${addr}`); }
  async getBalance(addr) { const d = await this.getAccount(addr); return d.found ? d.balance : 0; }
  async getNonce(addr) { const d = await this.getAccount(addr); return d.found ? d.nonce : 0; }

  // ── Chain ──────────────────────────────────────────────────────────────
  async syncInfo() {
    const info = await this.get('/api/v1/info');
    this.baseFee = info.base_fee || 1000;
    this.chainId = info.chain_id || this.chainId;
    return info;
  }

  // ── Tokens (cached via dex-api) ─────────────────────────────────────
  async getTokens() { const d = await this.cachedGet('/api/v1/tokens'); return d.tokens || []; }
  async getPortfolio(addr) { return this.get(`/api/v1/portfolio/${addr}`); }
  async getTokenAccount(mint, addr) {
    try { const r = await fetch(`${this.cacheUrl}/api/v1/token/${mint}/account/${addr}`, { signal: AbortSignal.timeout(3000) }).catch(() => null);
      if (r?.ok) { const t = await r.text(); const m = t.match(/"balance"\s*:\s*(\d+)/); return { balance: m ? BigInt(m[1]) : 0n, raw: JSON.parse(t) }; }
      const r2 = await fetch(`${this.rpcUrl}/api/v1/token/${mint}/account/${addr}`, { signal: AbortSignal.timeout(5000) });
      if (r2.ok) { const t = await r2.text(); const m = t.match(/"balance"\s*:\s*(\d+)/); return { balance: m ? BigInt(m[1]) : 0n, raw: JSON.parse(t) }; }
    } catch {} return { balance: 0n };
  }
  async getMintNonce(addr) { const d = await this.get(`/api/v1/mint-nonce/${addr}`); return d.mint_nonce || 0; }

  // ── DEX (cached) ─────────────────────────────────────────────────────
  async getPools() { const d = await this.cachedGet('/api/v1/pools'); return d.pools || []; }
  async getDexPairs() { const d = await this.cachedGet('/api/v1/dex/pairs'); return d.pairs || []; }
  async getSwapQuote(mintIn, mintOut, amountIn) { return this.cachedGet(`/api/v1/swap/quote/${mintIn}/${mintOut}/${amountIn}`); }

  // ── Launches (cached) ─────────────────────────────────────────────────
  async getLaunches() { const d = await this.cachedGet('/api/v1/launches'); return d.launches || []; }
  async getLaunchDetail(mint) { return this.get(`/api/v1/launch/${mint}`); }

  // ── TX submission ──────────────────────────────────────────────────────
  async submitTx(hex) { return this.post('/api/v1/tx/submit', { signed_tx_hex: hex }); }

  async waitConfirm(txHash, timeoutMs = 20000) {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
      try { const d = await this.get(`/api/v1/tx/${txHash}`); if (d.found) return d; } catch {}
      await new Promise(r => setTimeout(r, 400));
    }
    throw new Error('TX not confirmed within timeout');
  }

  // ── Wallet activation ──────────────────────────────────────────────────
  async activateWallet(addr) {
    const ch = await this.post('/api/v1/wallet/challenge', { address: addr });
    if (!ch.success) {
      if (ch.error && ch.error.includes('already activated')) return { alreadyActive: true };
      throw new Error(ch.error || 'Challenge failed');
    }
    const challengeBytes = C.hexDecode(ch.challenge);
    const diffBits = ch.difficulty_bits || 20;
    const nonce = await C.solveActivationPoW(challengeBytes, diffBits);
    const result = await this.post('/api/v1/wallet/activate', { address: addr, challenge: ch.challenge, nonce });
    if (!result.success) throw new Error(result.error || 'Activation failed');
    return result;
  }

  // ── High-level: DEX swap (via vault — NO local signing) ──────────────
  async swap(userId, address, mintIn, mintOut, amountIn, slippagePct) {
    return this._withNonceLock(address, async () => {
      await this.syncInfo();
      const nonce = await this._getNextNonce(address);
      const quote = await this.getSwapQuote(mintIn, mintOut, amountIn);
      if (!quote.found) throw new Error('No liquidity pool for this pair');

      const minOut = Math.floor(quote.amount_out * (1 - slippagePct / 100));
      const { signedTxHex } = await C.signTransaction(userId, address, 'Swap', {
        mint_in: mintIn, mint_out: mintOut,
        amount_in: amountIn, min_amount_out: minOut,
      }, nonce, 50000, this.baseFee, this.chainId);

      const result = await this.submitTx(signedTxHex);
      if (result.status === 'pending') this._incrementNonce(address);
      return { ...result, quote, minOut, nonce };
    });
  }

  async curveBuy(userId, address, mintId, piAmount) {
    return this._withNonceLock(address, async () => {
      await this.syncInfo();
      const nonce = await this._getNextNonce(address);
      const { signedTxHex } = await C.signTransaction(userId, address, 'ParticipateInLaunch', {
        mint: mintId, pi_amount: piAmount,
      }, nonce, 50000, this.baseFee, this.chainId);
      const result = await this.submitTx(signedTxHex);
      if (result.status === 'pending') this._incrementNonce(address);
      return result;
    });
  }

  async curveSell(userId, address, mintId, tokenAmount) {
    return this._withNonceLock(address, async () => {
      await this.syncInfo();
      const nonce = await this._getNextNonce(address);
      const { signedTxHex } = await C.signTransaction(userId, address, 'SellFromLaunch', {
        mint: mintId, token_amount: tokenAmount,
      }, nonce, 50000, this.baseFee, this.chainId);
      const result = await this.submitTx(signedTxHex);
      if (result.status === 'pending') this._incrementNonce(address);
      return result;
    });
  }

  async transfer(userId, address, recipientHex, amount) {
    return this._withNonceLock(address, async () => {
      await this.syncInfo();
      const nonce = await this._getNextNonce(address);
      const { signedTxHex } = await C.signTransaction(userId, address, 'Transfer', {
        recipient: recipientHex, amount,
      }, nonce, 21000, this.baseFee, this.chainId);
      const result = await this.submitTx(signedTxHex);
      if (result.status === 'pending') this._incrementNonce(address);
      return result;
    });
  }
}

module.exports = PiChainClient;
