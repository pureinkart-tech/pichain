/**
 * Advanced trading features for PiBot:
 * 1. Copy Trading — mirror another wallet's swaps
 * 2. Auto-Snipe — detect new Raydium LP pools and buy instantly
 * 3. Anti-Rug Detection — RugCheck API integration
 * 4. Whale Tracking — monitor large transactions + alerts
 * 5. Multi-Wallet — manage multiple wallets per user
 * 6. Cross-Chain Bridge — PI↔SOL conceptual swap via wrapped tokens
 */

const { Connection, PublicKey } = require('@solana/web3.js');

// ═══════════════════════════════════════════════════════════════════
// 1. COPY TRADING — mirror another wallet's trades
// ═══════════════════════════════════════════════════════════════════
class CopyTrader {
  constructor(solClient, db, notifyFn) {
    this.sol = solClient;
    this.db = db;
    this.notify = notifyFn;
    this.running = false;
    this.pollMs = 30000;
    this.lastSigs = new Map(); // targetWallet -> lastSignature

    // DB setup
    try { db.exec(`CREATE TABLE IF NOT EXISTS copy_targets (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      telegram_id INTEGER NOT NULL,
      target_wallet TEXT NOT NULL,
      max_sol_per_trade INTEGER DEFAULT 100000000,
      active INTEGER DEFAULT 1,
      copy_buys INTEGER DEFAULT 1,
      copy_sells INTEGER DEFAULT 1,
      created_at TEXT DEFAULT (datetime('now'))
    )`); } catch {}
    try { db.exec('ALTER TABLE copy_targets ADD COLUMN copy_sells INTEGER DEFAULT 1'); } catch {}

    this.stmts = {
      getActive: db.prepare('SELECT * FROM copy_targets WHERE active = 1'),
      getForUser: db.prepare('SELECT * FROM copy_targets WHERE telegram_id = ? AND active = 1'),
      insert: db.prepare('INSERT INTO copy_targets (telegram_id, target_wallet, max_sol_per_trade) VALUES (?, ?, ?)'),
      deactivate: db.prepare('UPDATE copy_targets SET active = 0 WHERE id = ? AND telegram_id = ?'),
      deactivateAll: db.prepare('UPDATE copy_targets SET active = 0 WHERE telegram_id = ?'),
    };
  }

  start() {
    if (this.running) return;
    this.running = true;
    console.log('Copy trader started');
    this._poll();
  }

  stop() { this.running = false; }

  async _poll() {
    while (this.running) {
      try { await this._check(); } catch (e) { /* silent */ }
      await new Promise(r => setTimeout(r, this.pollMs));
    }
  }

  async _check() {
    const targets = this.stmts.getActive.all();
    if (targets.length === 0) return;

    // Group by target wallet to avoid duplicate RPC calls
    const byWallet = {};
    for (const t of targets) {
      if (!byWallet[t.target_wallet]) byWallet[t.target_wallet] = [];
      byWallet[t.target_wallet].push(t);
    }

    for (const [wallet, subscribers] of Object.entries(byWallet)) {
      try {
        const pk = new PublicKey(wallet);
        const sigs = await this.sol.connection.getSignaturesForAddress(pk, { limit: 5 });
        if (sigs.length === 0) continue;

        const lastSeen = this.lastSigs.get(wallet);
        if (!lastSeen) {
          // First run — just record, don't fire
          this.lastSigs.set(wallet, sigs[0].signature);
          continue;
        }

        // Find new transactions since last check
        const newTxs = [];
        for (const sig of sigs) {
          if (sig.signature === lastSeen) break;
          newTxs.push(sig);
        }
        if (newTxs.length > 0) this.lastSigs.set(wallet, sigs[0].signature);

        // Analyze new transactions for swaps
        for (const sig of newTxs.slice(0, 3)) { // Max 3 per check to avoid spam
          try {
            const tx = await this.sol.connection.getParsedTransaction(sig.signature, { maxSupportedTransactionVersion: 0 });
            if (!tx) continue;

            // Look for token balance changes (swap indicator)
            const preBalances = tx.meta?.preTokenBalances || [];
            const postBalances = tx.meta?.postTokenBalances || [];
            if (preBalances.length === 0 && postBalances.length === 0) continue;

            // Detect swap: token balance changed for the target wallet
            const changes = [];
            for (const post of postBalances) {
              if (post.owner !== wallet) continue;
              const pre = preBalances.find(p => p.accountIndex === post.accountIndex);
              const preAmt = parseInt(pre?.uiTokenAmount?.amount || '0');
              const postAmt = parseInt(post.uiTokenAmount?.amount || '0');
              const diff = postAmt - preAmt;
              if (diff !== 0) {
                changes.push({
                  mint: post.mint,
                  diff,
                  symbol: post.uiTokenAmount?.uiAmountString || '?',
                  decimals: post.uiTokenAmount?.decimals || 9,
                });
              }
            }

            if (changes.length > 0) {
              const isBuy = changes.some(c => c.diff > 0 && c.mint !== 'So11111111111111111111111111111111111111112');
              const action = isBuy ? 'BUY' : 'SELL';
              const tokenChange = changes.find(c => c.mint !== 'So11111111111111111111111111111111111111112') || changes[0];

              for (const sub of subscribers) {
                if (isBuy && !sub.copy_buys) continue;
                if (!isBuy && !sub.copy_sells) continue;

                const shortWallet = wallet.slice(0, 6) + '...' + wallet.slice(-4);
                const maxSol = (sub.max_sol_per_trade / 1e9).toFixed(2);
                await this.notify(sub.telegram_id,
                  `\u{1F4CB} *Copy Trade Alert*\n\n` +
                  `Wallet \`${shortWallet}\` did a *${action}*\n` +
                  `Token: \`${tokenChange.mint.slice(0, 12)}\\.\\.\\.\`\n` +
                  `TX: \`${sig.signature.slice(0, 16)}\\.\\.\\.\`\n` +
                  `Max per trade: ${esc(maxSol)} SOL\n\n` +
                  `\`${tokenChange.mint}\`\n` +
                  `_Paste the address above to copy this trade\\!_`,
                  'MarkdownV2'
                );
              }
            }
          } catch { /* skip individual tx errors */ }
        }
      } catch { /* skip wallet errors */ }
    }
  }
}

// ═══════════════════════════════════════════════════════════════════
// 2. AUTO-SNIPE — detect new LP pools
// ═══════════════════════════════════════════════════════════════════
const RAYDIUM_V4 = '675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8';

class PoolSniper {
  constructor(solClient, db, notifyFn) {
    this.sol = solClient;
    this.db = db;
    this.notify = notifyFn;
    this.running = false;
    this.seenPools = new Set();
    this.pollMs = 30000;

    try { db.exec(`CREATE TABLE IF NOT EXISTS snipe_config (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      telegram_id INTEGER NOT NULL UNIQUE,
      active INTEGER DEFAULT 1,
      max_sol INTEGER DEFAULT 50000000,
      min_liq_sol INTEGER DEFAULT 1000000000,
      auto_buy INTEGER DEFAULT 0,
      created_at TEXT DEFAULT (datetime('now'))
    )`); } catch {}
    // Migration: add UNIQUE constraint if table already exists without it
    try { db.exec('CREATE UNIQUE INDEX IF NOT EXISTS idx_snipe_tid ON snipe_config(telegram_id)'); } catch {}

    this.stmts = {
      getActive: db.prepare('SELECT * FROM snipe_config WHERE active = 1'),
      getForUser: db.prepare('SELECT * FROM snipe_config WHERE telegram_id = ? AND active = 1'),
      upsert: db.prepare('INSERT INTO snipe_config (telegram_id, active, max_sol, min_liq_sol, auto_buy) VALUES (?, 1, ?, ?, ?) ON CONFLICT(telegram_id) DO UPDATE SET active=1, max_sol=excluded.max_sol, min_liq_sol=excluded.min_liq_sol, auto_buy=excluded.auto_buy'),
      deactivate: db.prepare('UPDATE snipe_config SET active = 0 WHERE telegram_id = ?'),
    };
  }

  start() {
    if (this.running) return;
    this.running = true;
    console.log('Pool sniper started');
    this._poll();
  }

  stop() { this.running = false; }

  async _poll() {
    while (this.running) {
      try { await this._check(); } catch (e) { /* silent */ }
      await new Promise(r => setTimeout(r, this.pollMs));
    }
  }

  async _check() {
    const subs = this.stmts.getActive.all();
    if (subs.length === 0) return;

    try {
      const raydiumPk = new PublicKey(RAYDIUM_V4);
      const sigs = await this.sol.connection.getSignaturesForAddress(raydiumPk, { limit: 10 });

      for (const sig of sigs) {
        if (this.seenPools.has(sig.signature)) continue;
        this.seenPools.add(sig.signature);

        // Keep set bounded
        if (this.seenPools.size > 1000) {
          const arr = [...this.seenPools];
          this.seenPools = new Set(arr.slice(-500));
        }

        try {
          const tx = await this.sol.connection.getParsedTransaction(sig.signature, { maxSupportedTransactionVersion: 0 });
          if (!tx) continue;

          // Check if this is a pool creation (initialize2 instruction)
          const logs = tx.meta?.logMessages || [];
          const isPoolCreate = logs.some(l => l.includes('initialize2') || l.includes('Initialize2'));
          if (!isPoolCreate) continue;

          // Extract token mints from the pool
          const postBalances = tx.meta?.postTokenBalances || [];
          const mints = [...new Set(postBalances.map(b => b.mint))];
          const tokenMint = mints.find(m => m !== 'So11111111111111111111111111111111111111112');
          if (!tokenMint) continue;

          // Get token info
          const tokenInfo = await this.sol.getTokenInfo(tokenMint).catch(() => ({ symbol: tokenMint.slice(0, 6) }));

          // Notify all subscribers
          for (const sub of subs) {
            await this.notify(sub.telegram_id,
              `\u{1F6A8} *New Pool Detected\\!*\n\n` +
              `Token: *\\$${esc(tokenInfo.symbol || '?')}*\n` +
              `Mint: \`${esc(tokenMint)}\`\n` +
              `TX: \`${sig.signature.slice(0, 20)}\\.\\.\\.\`\n\n` +
              `_Paste the mint address to buy\\!_`,
              'MarkdownV2'
            );
          }
        } catch { /* skip individual tx parse errors */ }
      }
    } catch { /* skip RPC errors */ }
  }
}

// ═══════════════════════════════════════════════════════════════════
// 3. ANTI-RUG DETECTION — RugCheck API
// ═══════════════════════════════════════════════════════════════════
class RugChecker {
  constructor() {
    this.cache = new Map(); // mint -> { result, timestamp }
    this.cacheTtlMs = 60000; // 1 minute cache
  }

  async check(mintAddress) {
    // Check cache first
    const cached = this.cache.get(mintAddress);
    if (cached && Date.now() - cached.timestamp < this.cacheTtlMs) return cached.result;

    try {
      const res = await fetch(`https://api.rugcheck.xyz/v1/tokens/${mintAddress}/report/summary`, {
        signal: AbortSignal.timeout(8000),
      });
      if (!res.ok) {
        // If rate limited or error, return unknown
        return { safe: null, score: -1, risks: [], error: `API ${res.status}` };
      }
      const data = await res.json();
      const score = data.score != null ? data.score : -1;
      const risks = (data.risks || []).map(r => ({
        name: r.name || 'Unknown',
        level: r.level || 'unknown',
        description: r.description || '',
        score: r.score || 0,
      }));
      const lpLocked = data.lpLockedPct || data.totalMarketLiquidity > 0 ? 100 : 0;

      // Check for danger signals from the raw data
      const hasMintAuth = data.mintAuthority != null;
      const hasFreezeAuth = data.freezeAuthority != null;
      if (hasMintAuth && !risks.some(r => r.name.includes('mint'))) {
        risks.push({ name: 'Mint Authority Active', level: 'warn', description: 'Token can mint more supply', score: 100 });
      }
      if (hasFreezeAuth && !risks.some(r => r.name.includes('freeze'))) {
        risks.push({ name: 'Freeze Authority Active', level: 'warn', description: 'Accounts can be frozen', score: 50 });
      }

      const effectiveScore = score >= 0 ? score : (risks.length > 0 ? 500 : 0);

      const result = {
        safe: effectiveScore < 500,
        score: effectiveScore,
        scoreLabel: effectiveScore < 200 ? 'Good' : effectiveScore < 500 ? 'OK' : effectiveScore < 800 ? 'Risky' : 'Danger',
        risks,
        lpLockedPct: lpLocked,
        topRisks: risks.filter(r => r.level === 'danger' || r.level === 'warn').slice(0, 3),
      };

      this.cache.set(mintAddress, { result, timestamp: Date.now() });
      return result;
    } catch (e) {
      return { safe: null, score: -1, risks: [], error: e.message };
    }
  }

  formatReport(result) {
    if (!result || result.score < 0) return '\u{26A0}\u{FE0F} RugCheck unavailable' + (result?.error ? ': ' + esc(String(result.error)) : '');

    const icon = result.safe ? '\u{1F7E2}' : result.score < 500 ? '\u{1F7E1}' : '\u{1F534}';
    let text = `${icon} *RugCheck: ${esc(result.scoreLabel)}* \\(${result.score}/1000\\)\n`;
    text += `LP Locked: ${result.lpLockedPct.toFixed(0)}%\n`;

    if (result.topRisks.length > 0) {
      for (const r of result.topRisks) {
        const ri = r.level === 'danger' ? '\u{1F534}' : '\u{1F7E1}';
        text += `${ri} ${esc(r.name)}\n`;
      }
    } else {
      text += '\u{2705} No major risks detected\n';
    }
    return text;
  }
}

// ═══════════════════════════════════════════════════════════════════
// 4. WHALE TRACKING — monitor large transactions
// ═══════════════════════════════════════════════════════════════════
class WhaleTracker {
  constructor(solClient, db, notifyFn) {
    this.sol = solClient;
    this.db = db;
    this.notify = notifyFn;
    this.running = false;
    this.pollMs = 60000;
    this.lastSigs = new Map();

    try { db.exec(`CREATE TABLE IF NOT EXISTS whale_watches (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      telegram_id INTEGER NOT NULL,
      mint TEXT NOT NULL,
      min_sol_value INTEGER DEFAULT 10000000000,
      active INTEGER DEFAULT 1,
      created_at TEXT DEFAULT (datetime('now'))
    )`); } catch {}

    this.stmts = {
      getActive: db.prepare('SELECT * FROM whale_watches WHERE active = 1'),
      getForUser: db.prepare('SELECT * FROM whale_watches WHERE telegram_id = ? AND active = 1'),
      insert: db.prepare('INSERT INTO whale_watches (telegram_id, mint, min_sol_value) VALUES (?, ?, ?)'),
      deactivate: db.prepare('UPDATE whale_watches SET active = 0 WHERE id = ? AND telegram_id = ?'),
      deactivateAll: db.prepare('UPDATE whale_watches SET active = 0 WHERE telegram_id = ? AND mint = ?'),
    };
  }

  start() {
    if (this.running) return;
    this.running = true;
    console.log('Whale tracker started');
    this._poll();
  }

  stop() { this.running = false; }

  async _poll() {
    while (this.running) {
      try { await this._check(); } catch { /* silent */ }
      await new Promise(r => setTimeout(r, this.pollMs));
    }
  }

  async _check() {
    const watches = this.stmts.getActive.all();
    if (watches.length === 0) return;

    // Group by mint
    const byMint = {};
    for (const w of watches) {
      if (!byMint[w.mint]) byMint[w.mint] = [];
      byMint[w.mint].push(w);
    }

    for (const [mint, subs] of Object.entries(byMint)) {
      try {
        const pk = new PublicKey(mint);
        const sigs = await this.sol.connection.getSignaturesForAddress(pk, { limit: 5 });
        const lastSeen = this.lastSigs.get(mint);
        if (!lastSeen) { this.lastSigs.set(mint, sigs[0]?.signature || ''); continue; }

        for (const sig of sigs) {
          if (sig.signature === lastSeen) break;

          try {
            const tx = await this.sol.connection.getParsedTransaction(sig.signature, { maxSupportedTransactionVersion: 0 });
            if (!tx) continue;

            // Check SOL balance changes (whale = large SOL movement)
            const preSOL = tx.meta?.preBalances || [];
            const postSOL = tx.meta?.postBalances || [];
            let maxSolChange = 0;
            for (let i = 0; i < preSOL.length; i++) {
              const diff = Math.abs((postSOL[i] || 0) - (preSOL[i] || 0));
              if (diff > maxSolChange) maxSolChange = diff;
            }

            const minThreshold = Math.min(...subs.map(s => s.min_sol_value));
            if (maxSolChange >= minThreshold) {
              const solAmount = maxSolChange / 1e9;
              for (const sub of subs) {
                if (maxSolChange >= sub.min_sol_value) {
                  await this.notify(sub.telegram_id,
                    `\u{1F40B} *Whale Alert\\!*\n\n` +
                    `${esc(solAmount.toFixed(2))} SOL moved on \`${mint.slice(0,12)}\\.\\.\\.\`\n` +
                    `TX: \`${sig.signature.slice(0,16)}\\.\\.\\.\``,
                    'MarkdownV2'
                  );
                }
              }
            }
          } catch { /* skip */ }
        }
        if (sigs.length > 0) this.lastSigs.set(mint, sigs[0].signature);
      } catch { /* skip mint errors */ }
    }
  }
}

// ═══════════════════════════════════════════════════════════════════
// 5. MULTI-WALLET — manage multiple wallets per user
// ═══════════════════════════════════════════════════════════════════
class MultiWallet {
  constructor(db, cryptoModule, solClient) {
    this.db = db;
    this.C = cryptoModule;
    this.sol = solClient;

    try { db.exec(`CREATE TABLE IF NOT EXISTS wallets (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      telegram_id INTEGER NOT NULL,
      label TEXT DEFAULT 'Wallet',
      seed TEXT NOT NULL,
      pi_address TEXT NOT NULL,
      sol_address TEXT NOT NULL,
      is_active INTEGER DEFAULT 0,
      created_at TEXT DEFAULT (datetime('now'))
    )`); } catch {}

    this.stmts = {
      getAll: db.prepare('SELECT * FROM wallets WHERE telegram_id = ? ORDER BY id'),
      getActive: db.prepare('SELECT * FROM wallets WHERE telegram_id = ? AND is_active = 1'),
      insert: db.prepare('INSERT INTO wallets (telegram_id, label, seed, pi_address, sol_address, is_active) VALUES (?, ?, ?, ?, ?, ?)'),
      setActive: db.prepare('UPDATE wallets SET is_active = 0 WHERE telegram_id = ?'),
      activate: db.prepare('UPDATE wallets SET is_active = 1 WHERE id = ? AND telegram_id = ?'),
      rename: db.prepare('UPDATE wallets SET label = ? WHERE id = ? AND telegram_id = ?'),
      delete: db.prepare('DELETE FROM wallets WHERE id = ? AND telegram_id = ?'),
      count: db.prepare('SELECT COUNT(*) as c FROM wallets WHERE telegram_id = ?'),
    };
  }

  async createWallet(telegramId, label = 'Wallet') {
    const count = this.stmts.count.get(telegramId)?.c || 0;
    if (count >= 10) throw new Error('Maximum 10 wallets per account');

    // Create PQ wallet via vault (keys never touch Node.js)
    const vaultId = `${telegramId}_${count}`;
    const result = await this.C.createWallet(vaultId);
    const isFirst = count === 0 ? 1 : 0;

    this.stmts.insert.run(telegramId, label || `Wallet ${count + 1}`, 'vault:' + vaultId, result.address, '', isFirst);
    return { pi: { address: result.address }, sol: null, label };
  }

  importWallet(telegramId, seedHex, label = 'Imported') {
    throw new Error('Ed25519 key import is no longer supported. PIChain uses post-quantum wallets.');
  }

  switchWallet(telegramId, walletId) {
    this.stmts.setActive.run(telegramId); // deactivate all
    this.stmts.activate.run(walletId, telegramId);
  }

  getWallets(telegramId) { return this.stmts.getAll.all(telegramId); }
  getActiveWallet(telegramId) { return this.stmts.getActive.get(telegramId); }
}

// ═══════════════════════════════════════════════════════════════════
// 6. CROSS-CHAIN BRIDGE — PI↔SOL via wrapped tokens
// ═══════════════════════════════════════════════════════════════════
class CrossChainBridge {
  constructor(piClient, solClient) {
    this.pi = piClient;
    this.sol = solClient;
  }

  // For now, bridge works through the PIChain bridge infrastructure
  // (wSOL on PIChain ↔ native SOL on Solana)
  async getQuote(fromChain, toChain, amount) {
    // Bridge fee: 0.1%
    const feeBps = 10;
    const fee = Math.floor(amount * feeBps / 10000);
    const receive = amount - fee;

    return {
      fromChain,
      toChain,
      amount,
      fee,
      receive,
      feePct: (feeBps / 100).toFixed(2),
      estimatedTime: fromChain === 'sol' ? '~2 min' : '~5 min',
      available: true,
    };
  }

  // Initiate bridge: user deposits on source chain, receives on destination
  async initiateBridge(wallet, fromChain, toChain, amount) {
    // This would interact with the PIChain bridge relayer
    // For now, return the bridge deposit address and instructions
    if (fromChain === 'sol' && toChain === 'pi') {
      return {
        action: 'deposit',
        instruction: 'Send SOL to the PIChain bridge address. wSOL will be credited on PIChain.',
        depositAddress: 'Bridge address — use /bridge on the web interface',
        amount,
      };
    } else if (fromChain === 'pi' && toChain === 'sol') {
      return {
        action: 'withdraw',
        instruction: 'Burn wSOL on PIChain. Native SOL will be sent to your Solana address.',
        amount,
      };
    }
    throw new Error('Unsupported bridge direction');
  }
}

function esc(s) { return String(s).replace(/[_*[\]()~`>#+\-=|{}.!\\]/g, '\\$&'); }

module.exports = { CopyTrader, PoolSniper, RugChecker, WhaleTracker, MultiWallet, CrossChainBridge };
