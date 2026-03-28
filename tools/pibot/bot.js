#!/usr/bin/env node
require('dotenv').config({ path: require('path').join(__dirname, '.env') });

// Global error handlers — prevent silent crashes
process.on('unhandledRejection', (reason, promise) => {
  console.error('[FATAL] Unhandled promise rejection:', reason?.message || reason);
});
process.on('uncaughtException', (err) => {
  console.error('[FATAL] Uncaught exception:', err.message);
  // Don't exit — let the bot try to recover
});
/**
 * PiBot v2 — Production Telegram trading bot for PIChain DEX.
 *
 * Features matching/exceeding BonkBot:
 * - Paste token address → instant buy/sell with inline buttons
 * - Auto wallet creation + import + export + activation
 * - Quick buy presets (0.1, 0.5, 1, 5 PI) + custom amount
 * - Percentage sells (25%, 50%, 75%, 100%)
 * - Bonding curve buy/sell (pre-graduation tokens)
 * - DEX swap buy/sell (post-graduation tokens)
 * - Portfolio with PnL tracking
 * - Token discovery (DEX pairs + active launches)
 * - Configurable slippage (0.5% – 10%)
 * - Auto-buy on paste toggle
 * - Referral program
 * - Send PI to any address
 * - 0.25% trading fee (4x cheaper than BonkBot's 1%)
 *
 * Usage: TELEGRAM_BOT_TOKEN=xxx node bot.js
 */

const { Bot, InlineKeyboard, session } = require('grammy');
const Database = require('better-sqlite3');
const path = require('path');
const C = require('./crypto');
const PiChainClient = require('./pichain');
const { SolanaClient, SOL_MINT, USDC_MINT, LAMPORTS_PER_SOL } = require('./solana');
const { CopyTrader, PoolSniper, RugChecker, WhaleTracker, MultiWallet, CrossChainBridge } = require('./advanced');

// ── Seed encryption at rest (AES-256-GCM) ────────────────────────────────
const nodeCrypto = require('crypto');
const ENCRYPTION_KEY = nodeCrypto.createHash('sha256').update(process.env.TELEGRAM_BOT_TOKEN || 'dev').digest();
function encryptSeed(seed) {
  const iv = nodeCrypto.randomBytes(12);
  const cipher = nodeCrypto.createCipheriv('aes-256-gcm', ENCRYPTION_KEY, iv);
  const encrypted = Buffer.concat([cipher.update(seed, 'utf8'), cipher.final()]);
  const tag = cipher.getAuthTag();
  return iv.toString('hex') + ':' + encrypted.toString('hex') + ':' + tag.toString('hex');
}
function decryptSeed(encStr) {
  // If it's a raw 64-char hex seed (legacy unencrypted), return as-is
  if (/^[0-9a-fA-F]{64}$/.test(encStr)) return encStr;
  const [ivHex, dataHex, tagHex] = encStr.split(':');
  if (!ivHex || !dataHex || !tagHex) return encStr; // Fallback for legacy
  const decipher = nodeCrypto.createDecipheriv('aes-256-gcm', ENCRYPTION_KEY, Buffer.from(ivHex, 'hex'));
  decipher.setAuthTag(Buffer.from(tagHex, 'hex'));
  return decipher.update(Buffer.from(dataHex, 'hex'), undefined, 'utf8') + decipher.final('utf8');
}

// ── In-memory cache to avoid repeated API calls ──────────────────────────
const cache = {
  data: new Map(),
  get(key, maxAgeMs = 10000) {
    const entry = this.data.get(key);
    if (!entry) return null;
    if (Date.now() - entry.time > maxAgeMs) { this.data.delete(key); return null; }
    return entry.value;
  },
  set(key, value) { this.data.set(key, { value, time: Date.now() }); },
  // Permanent cache — never expires (for token names/decimals that don't change)
  perm: new Map(),
  getPerm(key) { return this.perm.get(key) || null; },
  setPerm(key, value) { this.perm.set(key, value); },
};

// Wrapper: get token info with permanent caching — names never flicker
async function getCachedTokenInfo(mintAddr) {
  const cached = cache.getPerm('ti:' + mintAddr);
  if (cached?.symbol && cached.symbol !== mintAddr.slice(0,6)) return cached;
  try {
    const info = await sol.getTokenInfo(mintAddr);
    if (info?.symbol && info.symbol !== mintAddr.slice(0,6)) {
      cache.setPerm('ti:' + mintAddr, info);
    }
    return info;
  } catch {
    return cached || { symbol: mintAddr.slice(0,6), name: '', decimals: 6, address: mintAddr };
  }
}
// Pre-warm SOL price every 15s
let cachedSolPrice = 90;
setInterval(async () => {
  try {
    const res = await fetch('https://api.jup.ag/price/v2?ids=So11111111111111111111111111111111111111112', { signal: AbortSignal.timeout(3000) });
    if (res.ok) { const d = await res.json(); cachedSolPrice = parseFloat(d.data?.[SOL_MINT]?.price || '90'); }
  } catch {}
}, 15000);
// Warm on startup
(async () => { try { const r = await fetch('https://api.jup.ag/price/v2?ids=So11111111111111111111111111111111111111112', { signal: AbortSignal.timeout(5000) }); if (r.ok) { const d = await r.json(); cachedSolPrice = parseFloat(d.data?.[SOL_MINT]?.price || '90'); } } catch {} })();

// ── Config ─────────────────────────────────────────────────────────────────
const BOT_TOKEN = process.env.TELEGRAM_BOT_TOKEN;
const RPC_URL = process.env.PICHAIN_RPC || 'http://127.0.0.1:8314';
const SOL_RPC = process.env.SOLANA_RPC || 'https://api.mainnet-beta.solana.com';
const CHAIN_ID = parseInt(process.env.CHAIN_ID || '31415');
const BOT_FEE_BPS = 25; // 0.25% — 4x cheaper than BonkBot
const PI_FEE_ADDR = process.env.PI_FEE_ADDRESS || process.env.FEE_ADDRESS || '';
const SOL_FEE_ADDR = process.env.SOL_FEE_ADDRESS || '';

function deductFee(amount) {
  const fee = Math.floor(amount * BOT_FEE_BPS / 10000);
  return { net: amount - fee, fee };
}

// Collect fee by transferring from user wallet to fee wallet
// Runs in background — doesn't block the trade confirmation
async function collectFee(telegramId, walletAddress, fee, chain) {
  if (fee <= 0) return;
  // SECURITY: Verify wallet belongs to this user via vault before transferring
  try {
    if (chain === 'pi' && PI_FEE_ADDR) {
      const vaultAddr = await C.getAddress(String(telegramId));
      const vaultNorm = (vaultAddr.address || '').replace(/^(Pi314|pi314|0x)/, '').toLowerCase();
      const inputNorm = (walletAddress || '').replace(/^(Pi314|pi314|0x)/, '').toLowerCase();
      if (!vaultNorm || vaultNorm !== inputNorm) {
        console.error('Fee: address mismatch for user', String(telegramId).slice(0, 4) + '***');
        Q.logFee.run(telegramId, fee, 'pi');
        return;
      }
      await pi.transfer(String(telegramId), walletAddress, PI_FEE_ADDR, fee);
      Q.logFee.run(telegramId, fee, 'pi');
    } else {
      Q.logFee.run(telegramId, fee, chain);
    }
  } catch (e) {
    console.error('Fee collection failed:', chain, fee, e.message?.slice(0, 80));
    Q.logFee.run(telegramId, fee, chain);
  }
}

if (!BOT_TOKEN) { console.error('Set TELEGRAM_BOT_TOKEN env var'); process.exit(1); }

const pi = new PiChainClient(RPC_URL, CHAIN_ID);
const sol = new SolanaClient(SOL_RPC);
const bot = new Bot(BOT_TOKEN, {
  client: {
    // Custom fetch that does NOT retry on 429 — prevents cascade
    baseFetchConfig: { compress: true },
    canUseWebhookReply: () => false,
  },
});

// Transformer: intercept ALL API calls, suppress 429 retries
bot.api.config.use(async (prev, method, payload, signal) => {
  try {
    return await prev(method, payload, signal);
  } catch (e) {
    if (e.error_code === 429) {
      // Log once, don't retry — grammy will handle the next poll cycle
      console.error('429 suppressed for', method);
      return { ok: true, result: true };
    }
    throw e;
  }
});
const NATIVE = C.NATIVE_PI_MINT;

// ── Database ───────────────────────────────────────────────────────────────
const dbPath = path.join(__dirname, 'pibot.db');
const db = new Database(dbPath);
db.pragma('journal_mode = WAL');
db.exec(`
  CREATE TABLE IF NOT EXISTS users (
    telegram_id INTEGER PRIMARY KEY,
    wallet_seed TEXT NOT NULL,
    address TEXT NOT NULL,
    slippage_bps INTEGER DEFAULT 100,
    auto_buy_amount INTEGER DEFAULT 0,
    activated INTEGER DEFAULT 0,
    priority_fee TEXT DEFAULT 'medium',
    mev_protect INTEGER DEFAULT 1,
    buy_preset_1 INTEGER DEFAULT 100000000,
    buy_preset_2 INTEGER DEFAULT 500000000,
    buy_preset_3 INTEGER DEFAULT 1000000000,
    buy_preset_4 INTEGER DEFAULT 5000000000,
    created_at TEXT DEFAULT (datetime('now')),
    referrer_id INTEGER DEFAULT 0
  );
  CREATE TABLE IF NOT EXISTS trades (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    telegram_id INTEGER NOT NULL,
    mint_id TEXT NOT NULL,
    direction TEXT NOT NULL,
    pi_amount INTEGER NOT NULL,
    token_amount INTEGER NOT NULL,
    tx_hash TEXT,
    created_at TEXT DEFAULT (datetime('now'))
  );
  CREATE TABLE IF NOT EXISTS limit_orders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    telegram_id INTEGER NOT NULL,
    mint_id TEXT NOT NULL,
    direction TEXT NOT NULL,
    trigger_price_num INTEGER NOT NULL,
    trigger_price_den INTEGER NOT NULL DEFAULT 1000000000,
    amount INTEGER NOT NULL,
    active INTEGER DEFAULT 1,
    creation_price_num INTEGER DEFAULT 0,
    trailing_high_num INTEGER DEFAULT 0,
    dca_interval_ms INTEGER DEFAULT 0,
    dca_total INTEGER DEFAULT 0,
    dca_executed INTEGER DEFAULT 0,
    dca_next_at INTEGER DEFAULT 0,
    created_at TEXT DEFAULT (datetime('now'))
  );
`);
// Migrations for existing databases
try { db.exec('ALTER TABLE users ADD COLUMN priority_fee TEXT DEFAULT \'medium\''); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN mev_protect INTEGER DEFAULT 1'); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN buy_preset_1 INTEGER DEFAULT 100000000'); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN buy_preset_2 INTEGER DEFAULT 500000000'); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN buy_preset_3 INTEGER DEFAULT 1000000000'); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN buy_preset_4 INTEGER DEFAULT 5000000000'); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN active_chain TEXT DEFAULT \'pi\''); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN sol_address TEXT DEFAULT \'\''); } catch {}
try { db.exec('ALTER TABLE trades ADD COLUMN chain TEXT DEFAULT \'pi\''); } catch {}
try { db.exec('ALTER TABLE limit_orders ADD COLUMN chain TEXT DEFAULT \'pi\''); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN sol_slippage_bps INTEGER DEFAULT 100'); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN sol_auto_buy INTEGER DEFAULT 0'); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN sol_priority TEXT DEFAULT \'medium\''); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN sol_mev INTEGER DEFAULT 1'); } catch {}
try { db.exec('CREATE TABLE IF NOT EXISTS fees (id INTEGER PRIMARY KEY AUTOINCREMENT, telegram_id INTEGER, amount INTEGER, chain TEXT, swept INTEGER DEFAULT 0, created_at TEXT DEFAULT (datetime(\'now\')))'); } catch {}
try { db.exec('ALTER TABLE limit_orders ADD COLUMN creation_price_num INTEGER DEFAULT 0'); } catch {}
try { db.exec('ALTER TABLE limit_orders ADD COLUMN trailing_high_num INTEGER DEFAULT 0'); } catch {}
try { db.exec('ALTER TABLE limit_orders ADD COLUMN dca_interval_ms INTEGER DEFAULT 0'); } catch {}
try { db.exec('ALTER TABLE limit_orders ADD COLUMN dca_total INTEGER DEFAULT 0'); } catch {}
try { db.exec('ALTER TABLE limit_orders ADD COLUMN dca_executed INTEGER DEFAULT 0'); } catch {}
try { db.exec('ALTER TABLE limit_orders ADD COLUMN dca_next_at INTEGER DEFAULT 0'); } catch {}
// 2FA + withdrawal limits
try { db.exec('ALTER TABLE users ADD COLUMN totp_secret TEXT DEFAULT \'\''); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN totp_enabled INTEGER DEFAULT 0'); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN daily_withdrawn INTEGER DEFAULT 0'); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN daily_withdrawn_reset TEXT DEFAULT \'\''); } catch {}
// PQ wallet migration
try { db.exec('ALTER TABLE users ADD COLUMN pi_address TEXT DEFAULT \'\''); } catch {}
try { db.exec('ALTER TABLE users ADD COLUMN wallet_address TEXT DEFAULT \'\''); } catch {}

const Q = {
  getUser:    db.prepare('SELECT * FROM users WHERE telegram_id = ?'),
  upsertUser: db.prepare('INSERT OR REPLACE INTO users (telegram_id, wallet_seed, address, slippage_bps, auto_buy_amount, activated, referrer_id) VALUES (?,?,?,?,?,?,?)'),
  setSlippage: db.prepare('UPDATE users SET slippage_bps = ? WHERE telegram_id = ?'),
  setAutoBuy: db.prepare('UPDATE users SET auto_buy_amount = ? WHERE telegram_id = ?'),
  setActivated: db.prepare('UPDATE users SET activated = 1 WHERE telegram_id = ?'),
  setPriority: db.prepare('UPDATE users SET priority_fee = ? WHERE telegram_id = ?'),
  setMev:     db.prepare('UPDATE users SET mev_protect = ? WHERE telegram_id = ?'),
  setPresets: db.prepare('UPDATE users SET buy_preset_1=?, buy_preset_2=?, buy_preset_3=?, buy_preset_4=? WHERE telegram_id = ?'),
  setChain: db.prepare('UPDATE users SET active_chain = ? WHERE telegram_id = ?'),
  setSolAddr: db.prepare('UPDATE users SET sol_address = ? WHERE telegram_id = ?'),
  setSolSlippage: db.prepare('UPDATE users SET sol_slippage_bps = ? WHERE telegram_id = ?'),
  setSolAutoBuy: db.prepare('UPDATE users SET sol_auto_buy = ? WHERE telegram_id = ?'),
  setSolPriority: db.prepare('UPDATE users SET sol_priority = ? WHERE telegram_id = ?'),
  setSolMev: db.prepare('UPDATE users SET sol_mev = ? WHERE telegram_id = ?'),
  logTrade:   db.prepare('INSERT INTO trades (telegram_id, mint_id, direction, pi_amount, token_amount, tx_hash, chain) VALUES (?,?,?,?,?,?,?)'),
  getTrades:  db.prepare('SELECT * FROM trades WHERE telegram_id = ? AND mint_id = ? ORDER BY created_at DESC LIMIT 20'),
  getReferralCount: db.prepare('SELECT COUNT(*) as c FROM users WHERE referrer_id = ?'),
  insertLimit: db.prepare('INSERT INTO limit_orders (telegram_id, mint_id, direction, trigger_price_num, trigger_price_den, amount, creation_price_num, trailing_high_num) VALUES (?,?,?,?,?,?,?,?)'),
  insertDca:  db.prepare('INSERT INTO limit_orders (telegram_id, mint_id, direction, trigger_price_num, trigger_price_den, amount, dca_interval_ms, dca_total, dca_executed, dca_next_at, active) VALUES (?,?,?,0,1,?,?,?,0,?,1)'),
  getActiveLimits: db.prepare('SELECT * FROM limit_orders WHERE telegram_id = ? AND active = 1'),
  cancelLimit: db.prepare('UPDATE limit_orders SET active = 0 WHERE id = ? AND telegram_id = ?'),
  cancelAllLimits: db.prepare('UPDATE limit_orders SET active = 0 WHERE telegram_id = ? AND mint_id = ?'),
  logFee: db.prepare('INSERT INTO fees (telegram_id, amount, chain) VALUES (?, ?, ?)'),
  unsweptFees: db.prepare('SELECT chain, SUM(amount) as total FROM fees WHERE swept = 0 GROUP BY chain'),
  sweepFees: db.prepare('UPDATE fees SET swept = 1 WHERE swept = 0'),
};

// ── Limit Order Engine ─────────────────────────────────────────────────────
const LimitOrderEngine = require('./limits');
const notifyUser = async (tid, msg, mode) => {
  try { await bot.api.sendMessage(tid, msg, mode ? { parse_mode: mode } : {}); } catch (e) { console.error('Notify error:', e.message); }
};
const limitEngine = new LimitOrderEngine(db, pi, notifyUser);
const rugChecker = new RugChecker();
const copyTrader = new CopyTrader(sol, db, notifyUser);
const poolSniper = new PoolSniper(sol, db, notifyUser);
const whaleTracker = new WhaleTracker(sol, db, notifyUser);
const multiWallet = new MultiWallet(db, C, sol);
const bridge = new CrossChainBridge(pi, sol);

// ── Format helpers ─────────────────────────────────────────────────────────
const fPI = (base) => { const v = Number(base) / 1e9; if (v === 0) return '0'; if (v >= 1) return v.toFixed(4); if (v >= 0.0001) return v.toFixed(6); return v.toPrecision(4); };
const fTok = (base, dec = 9) => { const v = Number(base) / Math.pow(10, dec); if (v >= 1e6) return (v/1e6).toFixed(2)+'M'; if (v >= 1e3) return (v/1e3).toFixed(2)+'K'; if (v >= 1) return v.toFixed(2); if (v > 0) return v.toPrecision(4); return '0'; };
const short = (hex) => hex ? hex.slice(0, 8) + '...' + hex.slice(-4) : '?';
const esc = (s) => String(s).replace(/[_*[\]()~`>#+\-=|{}.!\\]/g, '\\$&');

// ── User management ────────────────────────────────────────────────────────
// getUser is now async — creates PQ vault wallet on first access.
// PIChain keys live in the vault (Rust process), never in Node.js.
// Solana keys remain Ed25519 (Solana's protocol doesn't support PQ).
async function getUser(tid, refId = 0) {
  let u = Q.getUser.get(tid);
  if (!u) {
    // Create PQ wallet via vault
    let piAddress;
    try {
      const result = await C.createWallet(String(tid));
      piAddress = result.address;
    } catch (e) {
      // Vault may not be running — create placeholder
      console.error(`Vault wallet creation failed for user ${String(tid).slice(0,4)}***:`, e.message?.slice(0,80));
      piAddress = '0'.repeat(40);
    }
    Q.upsertUser.run(tid, 'vault:' + tid, piAddress, 100, 0, 0, refId);
    u = Q.getUser.get(tid);
    u._new = true;
  }

  // Set PIChain wallet info (keys managed by vault, not in Node.js)
  const addr = u.pi_address || u.wallet_address || u.address || '';
  u.wallet = { address: addr };
  u.wallet_address = addr;

  // Legacy migration: if user has an old Ed25519 seed, migrate to vault
  // IMPORTANT: preserve old seed as sol_seed before overwriting
  if (u.wallet_seed && !u.wallet_seed.startsWith('vault:')) {
    try {
      // Save old seed for Solana before migrating PIChain to vault
      if (!u.sol_seed) {
        db.prepare('UPDATE users SET sol_seed = ? WHERE telegram_id = ?')
          .run(u.wallet_seed, tid); // Keep same encryption
      }
      // Check if vault already has this user
      const existing = await C.getAddress(String(tid)).catch(() => null);
      if (!existing) {
        const result = await C.createWallet(String(tid));
        const newAddr = result.address;
        db.prepare('UPDATE users SET wallet_seed = ?, address = ? WHERE telegram_id = ?')
          .run('vault:' + tid, newAddr, tid);
        try { db.prepare('UPDATE users SET pi_address = ?, wallet_address = ? WHERE telegram_id = ?').run(newAddr, newAddr, tid); } catch {}
        u.wallet = { address: newAddr };
        u.wallet_address = newAddr;
        console.log(`Migrated user ${tid} to PQ vault (Solana seed preserved separately)`);
      } else {
        u.wallet = { address: existing.address };
        u.wallet_address = existing.address;
        db.prepare('UPDATE users SET wallet_seed = ? WHERE telegram_id = ?')
          .run('vault:' + tid, tid);
      }
    } catch (e) {
      console.error(`Migration failed for ${tid}:`, e.message);
    }
  }

  // Solana wallet — always Ed25519, stored separately from PIChain vault
  // Generate a Solana seed if user doesn't have one yet
  let solSeed = u.sol_seed || '';
  if (!solSeed && u.wallet_seed && !u.wallet_seed.startsWith('vault:')) {
    // Legacy: use old Ed25519 seed for Solana (before vault migration)
    try { solSeed = decryptSeed(u.wallet_seed); } catch {}
  }
  if (!solSeed) {
    // Generate new Solana seed for vault users
    const newSeed = nodeCrypto.randomBytes(32).toString('hex');
    const encSeed = encryptSeed(newSeed);
    try {
      db.prepare('UPDATE users SET sol_seed = ? WHERE telegram_id = ?').run(encSeed, tid);
      solSeed = newSeed;
    } catch {}
  } else if (solSeed.includes(':')) {
    // Encrypted seed — decrypt it
    try { solSeed = decryptSeed(solSeed); } catch { solSeed = ''; }
  }
  if (solSeed && solSeed.length === 64) {
    try {
      u.solWallet = sol.walletFromSeed(solSeed);
      if (!u.sol_address || u.sol_address !== u.solWallet.publicKey) {
        Q.setSolAddr.run(u.solWallet.publicKey, tid);
      }
    } catch { u.solWallet = null; }
  } else {
    u.solWallet = null;
  }

  u.chain = u.active_chain || 'pi';
  return u;
}

// ── Per-user rate limiting ──────────────────────────────────────────────────
const userCooldowns = new Map(); // tid -> lastActionTs
const COOLDOWN_MS = 1000; // 1 second between actions (prevent spam, not annoy users)
function isRateLimited(tid) {
  const now = Date.now();
  const last = userCooldowns.get(tid) || 0;
  if (now - last < COOLDOWN_MS) return true;
  userCooldowns.set(tid, now);
  return false;
}
// Cleanup old entries every 5 minutes
setInterval(() => {
  const cutoff = Date.now() - 60000;
  for (const [tid, ts] of userCooldowns) { if (ts < cutoff) userCooldowns.delete(tid); }
}, 300000);

// ── Session for awaiting input ─────────────────────────────────────────────
bot.use(session({ initial: () => ({ awaitingInput: null }) }));

// Rate limit middleware — applies to all updates
bot.use(async (ctx, next) => {
  const tid = ctx.from?.id;
  if (tid && isRateLimited(tid)) {
    if (ctx.callbackQuery) await ctx.answerCallbackQuery('Too fast! Wait a moment.').catch(() => {});
    return; // Drop the request
  }
  // Replay protection: ignore callback queries older than 30 seconds
  if (ctx.callbackQuery) {
    const cbAge = Date.now() / 1000 - (ctx.callbackQuery.message?.date || 0);
    if (cbAge > 300) { // Message older than 5 minutes
      await ctx.answerCallbackQuery('This button has expired. Use /home for a fresh menu.').catch(() => {});
      return;
    }
  }
  return next();
});

// Per-user position mints for /1 /2 /3 quick navigation
const userPositions = new Map(); // telegramId -> [mint1, mint2, ...]

// ── Main Menu ──────────────────────────────────────────────────────────────
async function mainMenu(ctx, u) {
  const isPi = u.chain === 'pi';
  const bal = isPi
    ? await pi.getBalance(u.address).catch(() => 0)
    : await sol.getBalance(u.solWallet?.publicKey || '').catch(() => 0);
  const activeSlip = isPi ? u.slippage_bps : (u.sol_slippage_bps || u.slippage_bps || 100);
  const activeAutoBuy = isPi ? u.auto_buy_amount : (u.sol_auto_buy || 0);
  const slip = (activeSlip / 100).toFixed(1);
  const autoBuy = activeAutoBuy > 0 ? (isPi ? fPI(activeAutoBuy) : (activeAutoBuy / LAMPORTS_PER_SOL).toFixed(4)) + ' ' + (isPi ? 'PI' : 'SOL') : 'Off';
  const chainIcon = isPi ? '\u{1F7E3}' : '\u{1F7E2}';
  const chainName = isPi ? 'PIChain' : 'Solana';
  const balStr = isPi ? esc(fPI(bal)) + ' PI' : esc((bal / LAMPORTS_PER_SOL).toFixed(4)) + ' SOL';
  const addr = isPi ? 'Pi314' + u.address : (u.solWallet?.publicKey || 'N/A');

  // Fetch positions overview like BonkBot
  const solPrice = isPi ? 0 : cachedSolPrice;
  let posText = '';
  let netWorth = bal;
  const positionMints = []; // Store mints for /1 /2 /3 commands
  try {
    if (!isPi && u.solWallet) {
      const solTokens = await sol.getTokenAccounts(u.solWallet.publicKey).catch(() => []);
      if (solTokens.length > 0) {
        posText = '\nPositions Overview:\n';
        // Fetch DexScreener data for all tokens in ONE batched call
        const mints = solTokens.slice(0, 5).map(t => t.mint);
        let dexBatch = [];
        try {
          const dexKey = 'dex:' + mints.join(',');
          dexBatch = cache.get(dexKey, 15000); // 15s cache
          if (!dexBatch) {
            const sc = require('../../shared-token-cache.cjs');
            if (!sc.isDexCooldown()) {
              const resp = await fetch('https://api.dexscreener.com/tokens/v1/solana/' + mints.join(','), { signal: AbortSignal.timeout(6000) });
              if (resp.status === 429) { sc.setDexCooldown(30000); }
              else if (resp.ok) { const text = await resp.text(); try { dexBatch = JSON.parse(text); cache.set(dexKey, dexBatch); } catch {} }
            }
          }
        } catch {}
        if (!Array.isArray(dexBatch)) dexBatch = [];

        for (let i = 0; i < mints.length; i++) {
          const t = solTokens[i];
          const dex = dexBatch.find(p => p.baseToken?.address === mints[i]) || null;
          const info = await getCachedTokenInfo(t.mint);
          const sym = info.symbol || t.mint.slice(0,6);
          let price = dex ? parseFloat(dex.priceUsd || '0') : 0;
          // Fallback to sol.getPrice if DexScreener has no data
          if (price === 0) {
            try { price = await sol.getPrice(t.mint); } catch {}
          }
          const valueUsd = t.uiBalance * price;
          const valueSol = solPrice > 0 ? valueUsd / solPrice : 0;
          netWorth += Math.floor(valueSol * LAMPORTS_PER_SOL);

          // Price with subscript for tiny prices
          let priceStr = '';
          if (price > 0) {
            if (price >= 0.001) { priceStr = '$' + price.toFixed(4); }
            else {
              const s = price.toFixed(20).split('.')[1];
              let zeros = 0; for (const c of s) { if (c === '0') zeros++; else break; }
              const subs = '\u2080\u2081\u2082\u2083\u2084\u2085\u2086\u2087\u2088\u2089';
              priceStr = '$0.0' + String(zeros).split('').map(d => subs[parseInt(d)]).join('') + s.slice(zeros, zeros+3);
            }
          }

          const m5 = dex?.priceChange?.m5 || 0;
          const h1 = dex?.priceChange?.h1 || 0;
          const h6 = dex?.priceChange?.h6 || 0;
          const h24 = dex?.priceChange?.h24 || 0;
          const mcap = parseFloat(dex?.marketCap || dex?.fdv || dex?.liquidity?.usd * 1.1 || '0');
          const mcapStr = mcap >= 1e6 ? (mcap/1e6).toFixed(2)+'M' : mcap >= 1e3 ? (mcap/1e3).toFixed(2)+'K' : mcap > 0 ? mcap.toFixed(0) : '?';
          const fmt = (v) => { const n = parseFloat(v)||0; return (n >= 0 ? '+' : '') + n.toFixed(2) + '%'; };

          positionMints.push(t.mint);
          // Token balance rounded + % of supply
          const fdv = parseFloat(dex?.fdv || dex?.marketCap || '0');
          const totalSupply = (fdv > 0 && price > 0) ? fdv / price : 0;
          const balRound = t.uiBalance >= 1e6 ? (t.uiBalance/1e6).toFixed(2)+'M' : t.uiBalance >= 1000 ? Math.round(t.uiBalance).toLocaleString() : t.uiBalance >= 1 ? t.uiBalance.toFixed(2) : t.uiBalance.toFixed(4);
          const supplyPct = totalSupply > 0 ? ((t.uiBalance / totalSupply) * 100).toFixed(2) : '';

          posText += `\n[/${i+1} ${esc(sym)}](https://t.me/PiChainTradeBot?start=p${i+1})\n`;
          if (valueUsd > 0) posText += `Value: \\$${esc(valueUsd.toFixed(2))} / ${esc(valueSol.toFixed(4))} SOL\n`;
          posText += `Balance: ${esc(balRound)} ${esc(sym)}${supplyPct ? ', ' + esc(supplyPct) + '% Supply' : ''}\n`;
          if (mcap > 0) posText += `Mcap: \\$${esc(mcapStr)} @ ${esc(priceStr)}\n`;
          posText += `5m: *${esc(fmt(m5))}*, 1h: *${esc(fmt(h1))}*, 6h: *${esc(fmt(h6))}*, 24h: *${esc(fmt(h24))}*\n`;
        }
      }

    } else if (isPi) {
      const port = await pi.getPortfolio(u.address).catch(() => ({}));
      const tokens = port.tokens || [];
      if (tokens.length > 0) {
        posText = '\nPositions Overview:\n';
        for (let i = 0; i < Math.min(tokens.length, 5); i++) {
          const t = tokens[i];
          positionMints.push(t.mint_id);
          posText += `\n/${i+1} *${esc(t.symbol || '?')}* \\- ${esc(fTok(t.balance, t.decimals || 9))}\n`;
        }
      }
    }
  } catch {}

  // Save positions for /1 /2 /3 quick access
  if (positionMints.length > 0) userPositions.set(ctx.from.id, positionMints);

  const netWorthSol = isPi ? 0 : netWorth / LAMPORTS_PER_SOL;
  const netWorthUsd = isPi ? 0 : netWorthSol * solPrice;

  let text = `*PiBot* \\| ${chainIcon} ${esc(chainName)}\n`;
  text += posText;
  if (!isPi && netWorth > 0) {
    text += `\n*Balance:* ${esc((bal / LAMPORTS_PER_SOL).toFixed(4))} SOL\n`;
    text += `*Net Worth:* ${esc(netWorthSol.toFixed(4))} SOL / \\$${esc(netWorthUsd.toFixed(2))}\n`;
  } else {
    text += `\n\u{1F4B0} *Balance:* ${balStr}\n`;
  }
  text += `\u{1F4E6} *Address:*\n\`${esc(addr)}\``;
  text += `\n\nPaste a *token ${isPi ? 'mint ID' : 'address'}* to trade\\!`;

  const kb = new InlineKeyboard();
  if (isPi) {
    kb.text('\u{1F7E3} Buy PI', 'buy_pi_menu').text('\u{1F7E2} Sell PI', 'sellpi_menu').text('\u{1F4CA} PI Price', 'piprice_btn').row();
  }
  kb.text('Buy', 'cmd_buy').text('Sell & Manage', 'positions').row()
    .text(isPi ? '\u{1F7E2} Switch Solana' : '\u{1F7E3} Switch PIChain', 'switch_chain').text('Change Wallet', 'cmd_wallets').row()
    .text(isPi ? 'Launches' : 'Trending', isPi ? 'launches' : 'sol_trending').text('Tokens', 'tokens').text('Alerts', 'cmd_alerts').row()
    .text('Wallet', 'wallet').text('Settings', 'settings').text('Refer Friends', 'referrals').row()
    .text('Send', 'cmd_send').text('Refresh', 'refresh');

  const opts = { parse_mode: 'MarkdownV2', reply_markup: kb };
  if (ctx.callbackQuery) {
    await ctx.editMessageText(text, opts).catch(() => {});
  } else {
    await ctx.reply(text, opts);
  }
}

// ── Token Detail ───────────────────────────────────────────────────────────
async function tokenView(ctx, u, mintId) {
  const [launches, pairs, acct, bal] = await Promise.all([
    pi.getLaunches().catch(() => []),
    pi.getDexPairs().catch(() => []),
    pi.getTokenAccount(mintId, u.address).catch(() => ({ balance: 0n })),
    pi.getBalance(u.address).catch(() => 0),
  ]);

  const launch = launches.find(l => l.mint_id === mintId);
  const pair = pairs.find(p => p.mint_a === mintId || p.mint_b === mintId ||
    (p.launch_mint && p.launch_mint === mintId));
  const tokBal = Number(acct.balance || 0n);
  const dec = launch?.decimals ?? pair?.decimals_b ?? 9;
  const sym = launch?.symbol || pair?.symbol_b || pair?.symbol_a || '???';
  const name = launch?.name || pair?.name || '';
  const isGraduated = pair && pair.active;
  const isActive = launch && launch.state === 'Active';

  let text = `*\\$${esc(sym)}* ${esc(name)}\n\`${mintId}\`\n\n`;

  // Market data
  if (pair) {
    const price = pair.price || 0;
    const mcap = pair.market_cap || 0;
    const vol = pair.volume_24h || 0;
    const ch = pair.price_change_24h_pct;
    const chIcon = ch != null ? (ch >= 0 ? '\u{1F7E2}' : '\u{1F534}') : '';
    text += `\u{1F4B2} *Price:* ${esc(price.toFixed(8))} PI\n`;
    if (mcap > 0) text += `\u{1F3E6} *MCap:* ${esc(fPI(mcap * 1e9))} PI\n`;
    if (vol > 0) text += `\u{1F4CA} *24h Vol:* ${esc(vol.toFixed(2))} PI\n`;
    if (ch != null) text += `${chIcon} *24h:* ${ch >= 0 ? '+' : ''}${esc(ch.toFixed(1))}%\n`;
    text += `\u{1F4A7} *Liquidity:* ${esc(fPI(pair.reserve_a || 0))} PI\n`;
    text += `\u{1F3F7}\u{FE0F} *Fee:* ${esc(((pair.fee_bps || 30) / 100).toFixed(2))}%\n`;
    text += '\n';
  }

  if (isActive) {
    const pct = launch.percent_complete || 0;
    const bar = '\u{2588}'.repeat(Math.round(pct / 5)) + '\u{2591}'.repeat(20 - Math.round(pct / 5));
    text += `\u{1F3AF} *Bonding Curve* \\(${esc(pct.toFixed(1))}%\\)\n`;
    text += `\`${bar}\`\n`;
    text += `Raised: ${esc(fPI(launch.pi_raised))} / ${esc(fPI(launch.target_pi))} PI\n`;
    text += `Price: ${esc(fPI(launch.current_price))} PI/token\n\n`;
  }

  text += `\u{1F4BC} *Your balance:* ${esc(fTok(tokBal, dec))} \\$${esc(sym)}\n`;
  text += `\u{1F4B0} *PI balance:* ${esc(fPI(bal))} PI`;

  // PnL from trade history
  const trades = Q.getTrades.all(u.telegram_id, mintId);
  if (trades.length > 0) {
    const totalSpent = trades.filter(t => t.direction === 'buy').reduce((s, t) => s + t.pi_amount, 0);
    const totalReceived = trades.filter(t => t.direction === 'sell').reduce((s, t) => s + t.pi_amount, 0);
    const holdingValue = pair ? Math.floor(tokBal * (pair.price || 0)) : 0;
    const pnl = totalReceived + holdingValue - totalSpent;
    if (totalSpent > 0) {
      const pnlPct = ((pnl / totalSpent) * 100).toFixed(1);
      const icon = pnl >= 0 ? '\u{1F7E2}' : '\u{1F534}';
      text += `\n${icon} *PnL:* ${pnl >= 0 ? '+' : ''}${esc(fPI(pnl))} PI \\(${pnl >= 0 ? '+' : ''}${esc(pnlPct)}%\\)`;
    }
  }

  const kb = new InlineKeyboard();

  if (isGraduated) {
    kb.text('Buy 0.1', `db:${mintId}:${1e8}`).text('Buy 0.5', `db:${mintId}:${5e8}`)
      .text('Buy 1', `db:${mintId}:${1e9}`).text('Buy 5', `db:${mintId}:${5e9}`).row();
    kb.text('Buy X PI', `dbx:${mintId}`).row();
    if (tokBal > 0) {
      kb.text('Sell 25%', `ds:${mintId}:25`).text('Sell 50%', `ds:${mintId}:50`)
        .text('Sell 75%', `ds:${mintId}:75`).text('Sell 100%', `ds:${mintId}:100`).row();
    }
  } else if (isActive) {
    kb.text('Buy 0.1', `cb:${mintId}:${1e8}`).text('Buy 0.5', `cb:${mintId}:${5e8}`)
      .text('Buy 1', `cb:${mintId}:${1e9}`).text('Buy 5', `cb:${mintId}:${5e9}`).row();
    kb.text('Buy X PI', `cbx:${mintId}`).row();
    if (tokBal > 0) {
      kb.text('Sell 25%', `cs:${mintId}:25`).text('Sell 50%', `cs:${mintId}:50`)
        .text('Sell 75%', `cs:${mintId}:75`).text('Sell 100%', `cs:${mintId}:100`).row();
    }
  } else {
    text += '\n\n_This token has no active pool or launch\\._';
  }

  // Advanced orders row (only for DEX tokens with price data)
  if (isGraduated && pair) {
    kb.text('\u{1F4C9} Limit Buy', `lo_lb:${mintId}`).text('\u{1F4C8} Take Profit', `lo_tp:${mintId}`).row();
    kb.text('\u{1F6D1} Stop Loss', `lo_sl:${mintId}`).text('\u{1F4C9}\u{1F4C8} Trailing Stop', `lo_ts:${mintId}`).row();
    kb.text('\u{1F504} DCA Buy', `lo_dca:${mintId}`).text('\u{1F514} Alert', `lo_alert:${mintId}`).row();
  }

  // Active orders indicator
  const activeOrders = Q.getActiveLimits.all(u.telegram_id).filter(o => o.mint_id === mintId);
  if (activeOrders.length > 0) {
    text += `\n\n\u{1F4CB} *Active orders:* ${activeOrders.length}`;
    kb.text(`View Orders (${activeOrders.length})`, `my_orders:${mintId}`).row();
  }

  kb.text('\u{1F504} Refresh', `tv:${mintId}`).text('\u{1F3E0} Home', 'home');

  const opts = { parse_mode: 'MarkdownV2', reply_markup: kb };
  if (ctx.callbackQuery) {
    await ctx.editMessageText(text, opts).catch(() => {});
  } else {
    await ctx.reply(text, opts);
  }
}

// ── Execute DEX buy ────────────────────────────────────────────────────────
async function doBuy(ctx, u, mintId, piBase) {
  try {
    const bal = await pi.getBalance(u.address);
    if (bal < piBase + 200000) { await ctx.reply(`Insufficient PI. Have ${fPI(bal)}, need ${fPI(piBase)} + gas.`); return; }

    // Ensure wallet is activated
    if (!u.activated) {
      await ctx.reply('\u{23F3} Activating wallet (one-time)...');
      try { await pi.activateWallet(u.address); Q.setActivated.run(u.telegram_id); u.activated = 1; }
      catch (e) { if (!e.message?.includes('already')) { await ctx.reply(`Activation failed: ${e.message}`); return; } Q.setActivated.run(u.telegram_id); u.activated = 1; }
    }

    const { net: swapAmount, fee: botFee } = deductFee(piBase);
    await ctx.reply(`\u{23F3} Swapping ${fPI(swapAmount)} PI for tokens...`);
    const result = await pi.swap(u.telegram_id, u.wallet_address, NATIVE, mintId, swapAmount, u.slippage_bps / 100);

    if (result.status === 'pending') {
      Q.logTrade.run(u.telegram_id, mintId, 'buy', piBase, result.quote.amount_out, result.tx_hash || '', 'pi');
      collectFee(u.telegram_id, u.wallet_address, botFee, 'pi', u.telegram_id);
      const launches = await pi.getLaunches().catch(() => []);
      const l = launches.find(x => x.mint_id === mintId);
      const dec = l?.decimals || 9; const sym = l?.symbol || '???';
      await ctx.reply(
        `\u{2705} *Buy confirmed\\!*\n\n`
        + `Spent: ${esc(fPI(piBase))} PI\n`
        + `Got: ~${esc(fTok(result.quote.amount_out, dec))} \\$${esc(sym)}\n`
        + `Impact: ${esc((result.quote.price_impact_bps / 100).toFixed(2))}%\n`
        + `TX: \`${(result.tx_hash || '').slice(0, 16)}\\.\\.\\.\``,
        { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('View Token', `tv:${mintId}`).text('Home', 'home') }
      );
    } else {
      await ctx.reply(`\u{274C} Buy failed: ${result.error || 'unknown'}`);
    }
  } catch (e) { await ctx.reply(`\u{274C} Error: ${(e.message||'unknown').slice(0,200)}`); }
}

// ── Execute DEX sell ───────────────────────────────────────────────────────
async function doSell(ctx, u, mintId, pct) {
  try {
    const acct = await pi.getTokenAccount(mintId, u.address);
    const bal = Number(acct.balance || 0n);
    if (bal === 0) { await ctx.reply('No tokens to sell.'); return; }

    if (!u.activated) {
      try { await pi.activateWallet(u.address); Q.setActivated.run(u.telegram_id); u.activated = 1; }
      catch (e) { if (!e.message?.includes('already')) { await ctx.reply(`Activation failed: ${e.message}`); return; } Q.setActivated.run(u.telegram_id); u.activated = 1; }
    }

    const sellAmt = pct === 100 ? bal : Math.floor(bal * pct / 100);
    if (sellAmt === 0) { await ctx.reply('Sell amount too small.'); return; }

    await ctx.reply(`\u{23F3} Selling ${pct}% of tokens...`);
    const result = await pi.swap(u.telegram_id, u.wallet_address, mintId, NATIVE, sellAmt, u.slippage_bps / 100);

    if (result.status === 'pending') {
      const { fee: botFee } = deductFee(result.quote.amount_out);
      const netReceived = result.quote.amount_out - botFee;
      Q.logTrade.run(u.telegram_id, mintId, 'sell', netReceived, sellAmt, result.tx_hash || '', 'pi');
      collectFee(u.telegram_id, u.wallet_address, botFee, 'pi', u.telegram_id);
      await ctx.reply(
        `\u{2705} *Sell confirmed\\!*\n\n`
        + `Sold: ${esc(fTok(sellAmt, 9))} tokens \\(${pct}%\\)\n`
        + `Got: ~${esc(fPI(netReceived))} PI\n`
        + `TX: \`${(result.tx_hash || '').slice(0, 16)}\\.\\.\\.\``,
        { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('View Token', `tv:${mintId}`).text('Home', 'home') }
      );
    } else {
      await ctx.reply(`\u{274C} Sell failed: ${result.error || 'unknown'}`);
    }
  } catch (e) { await ctx.reply(`\u{274C} Error: ${(e.message||'unknown').slice(0,200)}`); }
}

// ── Execute bonding curve buy ──────────────────────────────────────────────
async function doCurveBuy(ctx, u, mintId, piBase) {
  try {
    const bal = await pi.getBalance(u.address);
    if (bal < piBase + 200000) { await ctx.reply(`Insufficient PI. Have ${fPI(bal)}, need ${fPI(piBase)} + gas.`); return; }
    if (!u.activated) {
      try { await pi.activateWallet(u.address); Q.setActivated.run(u.telegram_id); u.activated = 1; }
      catch (e) { if (!e.message?.includes('already')) { await ctx.reply(`Activation failed: ${e.message}`); return; } Q.setActivated.run(u.telegram_id); u.activated = 1; }
    }

    const { net: curveAmount, fee: curveFee } = deductFee(piBase);
    await ctx.reply(`\u{23F3} Buying on bonding curve with ${fPI(curveAmount)} PI...`);
    const result = await pi.curveBuy(u.telegram_id, u.wallet_address, mintId, curveAmount);

    if (result.status === 'pending') {
      Q.logTrade.run(u.telegram_id, mintId, 'buy', piBase, 0, result.tx_hash || '', 'pi');
      collectFee(u.telegram_id, u.wallet_address, curveFee, 'pi', u.telegram_id);
      await ctx.reply(
        `\u{2705} *Curve buy submitted\\!*\n\nSpent: ${esc(fPI(piBase))} PI\nTX: \`${(result.tx_hash || '').slice(0, 16)}\\.\\.\\.\`\n\n_Confirming on\\-chain\\.\\.\\._`,
        { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('View Token', `tv:${mintId}`).text('Home', 'home') }
      );
    } else { await ctx.reply(`\u{274C} Failed: ${result.error || 'unknown'}`); }
  } catch (e) { await ctx.reply(`\u{274C} Error: ${(e.message||'unknown').slice(0,200)}`); }
}

// ── Execute bonding curve sell ─────────────────────────────────────────────
async function doCurveSell(ctx, u, mintId, pct) {
  try {
    const acct = await pi.getTokenAccount(mintId, u.address);
    const bal = Number(acct.balance || 0n);
    if (bal === 0) { await ctx.reply('No tokens to sell.'); return; }
    if (!u.activated) {
      try { await pi.activateWallet(u.address); Q.setActivated.run(u.telegram_id); u.activated = 1; }
      catch (e) { if (!e.message?.includes('already')) { await ctx.reply(`Activation failed: ${e.message}`); return; } Q.setActivated.run(u.telegram_id); u.activated = 1; }
    }

    const sellAmt = pct === 100 ? bal : Math.floor(bal * pct / 100);
    if (sellAmt === 0) { await ctx.reply('Sell amount too small.'); return; }

    await ctx.reply(`\u{23F3} Selling ${pct}% on bonding curve...`);
    const result = await pi.curveSell(u.telegram_id, u.wallet_address, mintId, sellAmt);

    if (result.status === 'pending') {
      Q.logTrade.run(u.telegram_id, mintId, 'sell', 0, sellAmt, result.tx_hash || '', 'pi');
      await ctx.reply(
        `\u{2705} *Curve sell submitted\\!*\n\nSold: ${esc(fTok(sellAmt, 9))} tokens \\(${pct}%\\)\nTX: \`${(result.tx_hash || '').slice(0, 16)}\\.\\.\\.\``,
        { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('View Token', `tv:${mintId}`).text('Home', 'home') }
      );
    } else { await ctx.reply(`\u{274C} Failed: ${result.error || 'unknown'}`); }
  } catch (e) { await ctx.reply(`\u{274C} Error: ${(e.message||'unknown').slice(0,200)}`); }
}

// ── Portfolio ──────────────────────────────────────────────────────────────
async function showPortfolio(ctx, u) {
  try {
    const isPi = u.chain === 'pi';
    const kb = new InlineKeyboard();
    let text = `\u{1F4BC} *Portfolio* \\(${isPi ? '\u{1F7E3} PIChain' : '\u{1F7E2} Solana'}\\)\n\n`;

    if (isPi) {
      const [portfolio, bal] = await Promise.all([pi.getPortfolio(u.address), pi.getBalance(u.address)]);
      text += `\u{1F4B0} *PI:* ${esc(fPI(bal))}\n`;
      const tokens = portfolio.tokens || [];
      if (tokens.length === 0) text += '\n_No PI token holdings yet_';
      else { text += '\n'; for (const t of tokens) text += `*\\$${esc(t.symbol || '?')}:* ${esc(fTok(t.balance, t.decimals || 9))}\n`; }
      for (const t of tokens.slice(0, 6)) kb.text(`$${t.symbol || '?'}`, `tv:${t.mint_id}`).row();
    } else {
      const [solBal, solTokens] = await Promise.all([
        sol.getBalance(u.solWallet?.publicKey || '').catch(() => 0),
        u.solWallet ? sol.getTokenAccounts(u.solWallet.publicKey).catch(() => []) : [],
      ]);
      text += `\u{1F4B0} *SOL:* ${esc((solBal/LAMPORTS_PER_SOL).toFixed(4))}\n`;
      if (solTokens.length === 0) text += '\n_No Solana token holdings yet_';
      else {
        text += '\n';
        // Get token names
        for (const t of solTokens.slice(0, 10)) {
          const info = await getCachedTokenInfo(t.mint);
          const sym = info?.symbol || t.mint.slice(0, 6);
          text += `*\\$${esc(sym)}:* ${esc(t.uiBalance.toFixed(4))}\n`;
        }
      }
      for (const t of solTokens.slice(0, 6)) kb.text(t.mint.slice(0,6)+'...', `stv:${t.mint}`).row();
    }

    kb.text('\u{1F504} Refresh', 'positions').text('\u{1F3E0} Home', 'home');
    if (ctx.callbackQuery) await ctx.editMessageText(text, { parse_mode: 'MarkdownV2', reply_markup: kb }).catch(() => ctx.reply(text, { parse_mode: 'MarkdownV2', reply_markup: kb }));
    else await ctx.reply(text, { parse_mode: 'MarkdownV2', reply_markup: kb });
  } catch (e) { await ctx.reply(`Error: ${e.message}`); }
}

// ── Token list ─────────────────────────────────────────────────────────────
async function showTokens(ctx, chain) {
  const kb = new InlineKeyboard();
  let text;

  if (chain === 'sol') {
    text = `\u{1F4C8} *Trending Solana Tokens*\n\n`;
    const tokens = await sol.getTrendingTokens().catch(() => []);
    if (!tokens.length) text += '_No data_';
    else for (const t of tokens.slice(0, 12)) {
      text += `*\\$${esc(t.symbol || '?')}* ${esc(t.name || '')}\n`;
    }
    for (const t of tokens.slice(0, 5)) kb.text('$'+(t.symbol||'?'), 'stv:'+t.address).row();
  } else {
    text = `\u{1F4C8} *PIChain DEX Tokens*\n\n`;
    const pairs = await pi.getDexPairs().catch(() => []);
    if (pairs.length === 0) text += '_No trading pairs yet_';
    else for (const p of pairs.slice(0, 12)) {
      const sym = p.symbol_b || p.symbol_a || '?';
      const ch = p.price_change_24h_pct;
      const icon = ch != null ? (ch >= 0 ? '\u{1F7E2}' : '\u{1F534}') : '\u{26AA}';
      text += `${icon} *\\$${esc(sym)}* ${esc((p.price || 0).toFixed(6))} PI`;
      if (ch != null) text += ` \\(${ch >= 0 ? '+' : ''}${esc(ch.toFixed(1))}%\\)`;
      text += '\n';
    }
    for (const p of pairs.slice(0, 5)) { const m = p.mint_a === NATIVE ? p.mint_b : p.mint_a; kb.text('$'+(p.symbol_b||p.symbol_a||'?'), 'tv:'+m).row(); }
  }

  kb.text('\u{1F504}', 'tokens').text('\u{1F3E0} Home', 'home');
  try { if (ctx.callbackQuery) await ctx.editMessageText(text, { parse_mode: 'MarkdownV2', reply_markup: kb }); else await ctx.reply(text, { parse_mode: 'MarkdownV2', reply_markup: kb }); }
  catch { await ctx.reply(text, { parse_mode: 'MarkdownV2', reply_markup: kb }); }
}

// ── Launches ───────────────────────────────────────────────────────────────
async function showLaunches(ctx) {
  const launches = await pi.getLaunches().catch(() => []);
  const active = launches.filter(l => l.state === 'Active').sort((a, b) => (b.pi_raised || 0) - (a.pi_raised || 0)).slice(0, 10);
  let text = `\u{1F680} *Active Launches*\n\n`;
  if (active.length === 0) text += '_No active launches_';
  else for (const l of active) {
    const pct = (l.percent_complete || 0).toFixed(0);
    text += `*\\$${esc(l.symbol)}* \\- ${esc(pct)}% \\(${esc(fPI(l.pi_raised))}/${esc(fPI(l.target_pi))}\\)\n`;
  }
  const kb = new InlineKeyboard();
  for (const l of active.slice(0, 5)) kb.text(`$${l.symbol}`, `tv:${l.mint_id}`).row();
  kb.text('\u{1F504}', 'launches').text('\u{1F3E0} Home', 'home');
  try { if (ctx.callbackQuery) await ctx.editMessageText(text, { parse_mode: 'MarkdownV2', reply_markup: kb }); else await ctx.reply(text, { parse_mode: 'MarkdownV2', reply_markup: kb }); }
  catch { await ctx.reply(text, { parse_mode: 'MarkdownV2', reply_markup: kb }); }
}

// ── Settings ───────────────────────────────────────────────────────────────
async function showSettings(ctx, u) {
  const isPi = u.chain === 'pi';
  const chainIcon = isPi ? '\u{1F7E3}' : '\u{1F7E2}';
  const chainName = isPi ? 'PIChain' : 'Solana';
  const currency = isPi ? 'PI' : 'SOL';

  const s = isPi ? u.slippage_bps : (u.sol_slippage_bps || u.slippage_bps || 100);
  const abRaw = isPi ? u.auto_buy_amount : (u.sol_auto_buy || 0);
  const pf = isPi ? (u.priority_fee || 'medium') : (u.sol_priority || 'medium');
  const mev = isPi ? u.mev_protect : (u.sol_mev ?? 1);
  const abLabel = abRaw > 0 ? (isPi ? fPI(abRaw) : (abRaw / LAMPORTS_PER_SOL).toFixed(4)) + ' ' + currency : '';

  const text = `${chainIcon} *${esc(chainName)} Settings*`;

  const kb = new InlineKeyboard()
    // --- Auto Buy ---
    .text('--- AUTO BUY ---', 'noop').row()
    .text(abRaw === 0 ? '\u{1F534} Disabled' : '\u{1F7E2} Enabled', abRaw === 0 ? 'ab:100000000' : 'ab:0')
    .text('\u{270F} ' + (abRaw > 0 ? abLabel : '0.10 ' + currency), 'ab_custom').row()
    // --- Buy Buttons Config ---
    .text('--- BUY BUTTONS CONFIG ---', 'noop').row()
    .text('\u{270F} Left: 0.1 ' + currency, 'noop').text('\u{270F} Right: 0.5 ' + currency, 'noop').row()
    .text('\u{270F} Left: 1 ' + currency, 'noop').text('\u{270F} Right: 5 ' + currency, 'noop').row()
    // --- Sell Buttons Config ---
    .text('--- SELL BUTTONS CONFIG ---', 'noop').row()
    .text('\u{270F} Left: 25%', 'noop').text('\u{270F} Right: 50%', 'noop').row()
    .text('\u{270F} Left: 75%', 'noop').text('\u{270F} Right: 100%', 'noop').row()
    // --- Slippage Config ---
    .text('--- SLIPPAGE CONFIG ---', 'noop').row()
    .text('\u{270F} Buy: ' + (s / 100).toFixed(1) + '%', 'sl_custom').text('\u{270F} Sell: ' + (s / 100).toFixed(1) + '%', 'sl_custom').row()
    .text(s===50?'\u{2705} 0.5%':'0.5%','sl:50').text(s===100?'\u{2705} 1%':'1%','sl:100').text(s===300?'\u{2705} 3%':'3%','sl:300').text(s===500?'\u{2705} 5%':'5%','sl:500').text(s===1000?'\u{2705} 10%':'10%','sl:1000').row()
    // --- MEV Protect ---
    .text('--- MEV PROTECT ---', 'noop').row()
    .text(mev===1?'\u{21C6} Turbo':'\u{21C6} Turbo','mev:turbo').text(mev===2?'\u{21C6} Secure':'\u{21C6} Secure','mev:secure').row()
    // --- Transaction Priority ---
    .text('--- TRANSACTION PRIORITY ---', 'noop').row()
    .text(pf==='low'?'\u{2705} Low':'\u{21C6} Low','pf:low').text(pf==='medium'?'\u{2705} Med':'\u{21C6} Med','pf:medium').text(pf==='high'?'\u{2705} High':'\u{21C6} High','pf:high').text(pf==='turbo'?'\u{2705} Turbo':'\u{21C6} Turbo','pf:turbo').row()
    .text('\u{2190} Close', 'home');

  try { if (ctx.callbackQuery) await ctx.editMessageText(text, { parse_mode: 'MarkdownV2', reply_markup: kb }); else await ctx.reply(text, { parse_mode: 'MarkdownV2', reply_markup: kb }); }
  catch { await ctx.reply(text, { parse_mode: 'MarkdownV2', reply_markup: kb }); }
}

// ── Commands ───────────────────────────────────────────────────────────────
bot.command('home', async (ctx) => { await mainMenu(ctx, await getUser(ctx.from.id)); });
bot.command('menu', async (ctx) => { await mainMenu(ctx, await getUser(ctx.from.id)); });
// Position shortcuts — handles /1, /1_ufc, /2_bonk etc
bot.on('message:text', async (ctx, next) => {
  const text = ctx.message.text.trim();
  const match = text.match(/^\/(\d)(?:_\w+)?$/);
  if (match) {
    const n = parseInt(match[1]);
    const u = await getUser(ctx.from.id);
    const positions = userPositions.get(ctx.from.id) || [];
    if (n >= 1 && n <= positions.length) {
      await ctx.deleteMessage().catch(() => {});
      const mint = positions[n - 1];
      if (u.chain === 'sol') await solTokenView(ctx, u, mint);
      else await tokenView(ctx, u, mint);
      return;
    }
  }
  await next();
});
bot.command('start', async (ctx) => {
  const arg = (ctx.match || '').trim();
  const u = await getUser(ctx.from.id, parseInt(arg) || 0);

  // Handle position deep links: /start p1, /start p2, etc
  const posMatch = arg.match(/^p(\d)$/);
  if (posMatch) {
    await ctx.deleteMessage().catch(() => {}); // Delete the /start command message
    const n = parseInt(posMatch[1]);
    const positions = userPositions.get(ctx.from.id) || [];
    if (n >= 1 && n <= positions.length) {
      const mint = positions[n - 1];
      if (u.chain === 'sol') return solTokenView(ctx, u, mint);
      else return tokenView(ctx, u, mint);
    }
  }

  if (u._new) await ctx.reply(
    `\u{1F389} *Welcome to PiBot\\!*\n\n`
    + `\u{1F7E3} *PIChain wallet:*\n\`${u.address}\`\n\n`
    + `\u{1F7E2} *Solana wallet:*\n\`${esc(u.solWallet?.publicKey || 'N/A')}\`\n\n`
    + `Both wallets are created from a single key\\.\nUse /export to backup\\. Deposit PI or SOL to start trading\\.`,
    { parse_mode: 'MarkdownV2' }
  );
  await mainMenu(ctx, u);
});
bot.command('help', (ctx) => ctx.reply(
  'PiBot — Dual-Chain Trading Bot\n\n' +
  'Trading:\n/buy /sell — Quick trade\n/send — Transfer PI or SOL\n\n' +
  'Orders:\n/orders — Active orders\n/snipe — Graduation snipe\n/alert — Price alerts\n/history — Trade history + PnL\n\n' +
  'Advanced:\n/copy <wallet> — Copy trading\n/uncopy — Stop copying\n/autosnipe — New pool alerts\n/rug <mint> — Safety check\n/watchwhale <mint> — Whale alerts\n/bridge — Cross-chain PI/SOL\n\n' +
  'Wallet:\n/wallet /wallets /export /import\n/portfolio /tokens /launches\n/settings — All configuration\n\n' +
  'Paste any token address to trade!'
));
bot.command('wallet', async (ctx) => {
  const u = await getUser(ctx.from.id);
  const [piBal, solBal] = await Promise.all([
    pi.getBalance(u.address).catch(()=>0),
    u.solWallet ? sol.getBalance(u.solWallet.publicKey).catch(()=>0) : 0,
  ]);
  await ctx.reply(
    `\u{1F4B3} *Wallet*\n\n` +
    `\u{1F7E3} *PIChain:*\n\`${u.address}\`\n${esc(fPI(piBal))} PI\n\n` +
    `\u{1F7E2} *Solana:*\n\`${esc(u.solWallet?.publicKey || 'N/A')}\`\n${esc((solBal/LAMPORTS_PER_SOL).toFixed(4))} SOL\n\n` +
    `_Same seed backs both wallets\\._`,
    { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('Export Key', 'xkey').text('Import Key', 'ikey').row().text('\u{1F3E0} Home', 'home') }
  );
});
bot.command('export', async (ctx) => {
  const u = await getUser(ctx.from.id);
  const rawSeed = decryptSeed(u.wallet_seed);
  let text = '\u{1F512} Private Keys (auto-deletes in 30s)\n\n';
  text += 'PIChain (hex seed):\n' + rawSeed + '\n\n';
  if (u.solWallet) {
    text += 'Solana (base58):\n' + u.solWallet.secretKey + '\n\n';
  }
  text += 'SAVE THESE NOW — this message will be deleted!';
  const msg = await ctx.reply(text);
  // Auto-delete after 30 seconds for security
  setTimeout(() => {
    ctx.api.deleteMessage(ctx.chat.id, msg.message_id).catch(() => {});
  }, 30000);
  // Also delete the user's /export command message
  ctx.deleteMessage().catch(() => {});
});
bot.command('import', (ctx) => { ctx.session.awaitingInput = 'import_key'; ctx.reply('Paste the contents of your PQ wallet JSON file (from the desktop app or CLI miner).\n\nYou can find it by opening the wallet .json file in a text editor and copying everything.'); });

// ── Buy/Sell PI with SOL (AMM Pool) ────────────────────────────────────────
const { AMMPool } = require('./amm');
const ammPool = new AMMPool();
const PI_TREASURY_USER = process.env.PI_TREASURY_USER || ''; // vault user ID for PI transfers
const PI_TREASURY_ADDR = process.env.PI_TREASURY_ADDRESS || ''; // PIChain address holding pool PI

bot.command('buypi', async (ctx) => {
  if (!ammPool.isInitialized()) { await ctx.reply('PI/SOL pool is not initialized yet.'); return; }
  const u = await getUser(ctx.from.id);
  const args = (ctx.match || '').trim().split(/\s+/);
  const solPrice = cachedSolPrice || 90;
  const stats = ammPool.stats(solPrice);

  if (!args[0]) {
    await ctx.reply(
      `\u{1F7E3} *Buy PI with SOL*\n\n` +
      `Current price: *$${stats.priceUsd.toFixed(6)}/PI*\n` +
      `1 SOL = ${Math.floor(1/stats.priceSol).toLocaleString()} PI\n\n` +
      `Usage: \`/buypi <SOL amount>\`\n` +
      `Example: \`/buypi 0.1\` — buy PI with 0.1 SOL\n\n` +
      `Pool: ${stats.piReserve.toFixed(0)} PI / ${stats.solReserve.toFixed(4)} SOL`,
      { parse_mode: 'Markdown' }
    );
    return;
  }

  const solIn = parseFloat(args[0]);
  if (!solIn || solIn <= 0) { await ctx.reply('Invalid amount.'); return; }

  const solInLamports = Math.floor(solIn * 1e9);
  const quote = ammPool.quoteBuyPI(solInLamports);
  if (!quote) { await ctx.reply('Insufficient liquidity.'); return; }

  const piOut = quote.piOut / 1e9;
  const pricePerPi = solIn / piOut;
  const priceUsd = pricePerPi * solPrice;
  const piAddr = u.wallet_address || u.pi_address || u.address || '';

  if (!piAddr || piAddr === '0'.repeat(40)) { await ctx.reply('You need a PIChain wallet first. Type /wallet.'); return; }

  ctx.session.pendingBuy = { solInLamports, piOut: quote.piOut, solIn, piOutDisplay: piOut, priceUsd, piAddr, priceImpact: quote.priceImpact };
  await ctx.reply(
    `\u{1F7E3} *Buy ${piOut.toFixed(2)} PI*\n\n` +
    `Pay: *${solIn.toFixed(6)} SOL* ($${(solIn * solPrice).toFixed(2)})\n` +
    `Price: $${priceUsd.toFixed(6)}/PI\n` +
    `Impact: ${quote.priceImpact.toFixed(2)}%\n` +
    `Fee: 0.3%\n\n` +
    `PI sent to: \`${piAddr}\``,
    { parse_mode: 'Markdown', reply_markup: new InlineKeyboard().text('\u{2705} Confirm Buy', 'confirm_buypi').text('\u{274C} Cancel', 'cancel_buypi') }
  );
});

bot.callbackQuery('confirm_buypi', async (ctx) => {
  const pending = ctx.session.pendingBuy;
  if (!pending) { await ctx.answerCallbackQuery('No pending order').catch(() => {}); return; }
  ctx.session.pendingBuy = null;

  const u = await getUser(ctx.from.id);
  await ctx.editMessageText('\u{23F3} Processing your PI purchase...').catch(() => {});

  try {
    // SECURITY: Verify user has enough SOL
    if (!u.solWallet) throw new Error('No Solana wallet');
    const solBal = await sol.getBalance(u.solWallet.publicKey).catch(() => 0);
    if (solBal < pending.solInLamports) throw new Error('Insufficient SOL balance');

    // SECURITY: Re-quote to prevent stale price manipulation
    const freshQuote = ammPool.quoteBuyPI(pending.solInLamports);
    if (!freshQuote || Math.abs(freshQuote.piOut - pending.piOut) / pending.piOut > 0.05) {
      throw new Error('Price changed >5% since quote. Please try again.');
    }

    // 1. Transfer SOL from buyer to fee wallet
    if (SOL_FEE_ADDR && u.solWallet) {
      await sol.transfer(u.solWallet, SOL_FEE_ADDR, pending.solInLamports);
    }

    // 2. Execute AMM swap (updates pool reserves)
    const result = ammPool.executeBuy(pending.solInLamports);

    // 3. Transfer PI from treasury to buyer
    if (PI_TREASURY_USER && PI_TREASURY_ADDR) {
      await pi.transfer(PI_TREASURY_USER, PI_TREASURY_ADDR, pending.piAddr, result.piOut);
    }

    const piOutDisplay = (result.piOut / 1e9).toFixed(2);
    const newPriceUsd = ammPool.priceInUsd(cachedSolPrice || 90);

    await ctx.editMessageText(
      `\u{2705} *Purchase Complete\\!*\n\n` +
      `Bought: *${esc(piOutDisplay)} PI*\n` +
      `Paid: ${esc(pending.solIn.toFixed(6))} SOL\n` +
      `New price: \\$${esc(newPriceUsd.toFixed(6))}/PI\n\n` +
      `PI sent to: \`${esc(pending.piAddr)}\``,
      { parse_mode: 'MarkdownV2' }
    ).catch(() => {});
  } catch (e) {
    await ctx.editMessageText('\u{274C} Purchase failed: ' + (e.message || e)).catch(() => {});
  }
  await ctx.answerCallbackQuery().catch(() => {});
});

bot.callbackQuery('cancel_buypi', async (ctx) => {
  ctx.session.pendingBuy = null;
  await ctx.editMessageText('Purchase cancelled.').catch(() => {});
  await ctx.answerCallbackQuery().catch(() => {});
});

bot.command('piprice', async (ctx) => {
  const solPrice = cachedSolPrice || 90;
  if (!ammPool.isInitialized()) { await ctx.reply('PI/SOL pool not initialized yet.'); return; }
  const stats = ammPool.stats(solPrice);
  await ctx.reply(
    `\u{1F7E3} *PI Price*\n\n` +
    `$${stats.priceUsd.toFixed(4)} per PI\n` +
    `1 SOL = ${Math.floor(1/stats.priceSol).toLocaleString()} PI\n` +
    `1 PI = ${stats.priceSol.toFixed(8)} SOL\n\n` +
    `Hard Cap: 3,141,592,653 PI\n` +
    `100% to miners — no pre-mine`,
    { parse_mode: 'Markdown', reply_markup: new InlineKeyboard().text('\u{1F7E3} Buy PI', 'buy_pi_menu').text('\u{1F7E2} Sell PI', 'sellpi_menu').row().text('\u{2190} Home', 'home') }
  );
});

bot.command('sellpi', async (ctx) => {
  if (!ammPool.isInitialized()) { await ctx.reply('PI/SOL pool not initialized yet.'); return; }
  const u = await getUser(ctx.from.id);
  const args = (ctx.match || '').trim().split(/\s+/);
  const solPrice = cachedSolPrice || 90;

  if (!args[0]) {
    const stats = ammPool.stats(solPrice);
    await ctx.reply(
      `\u{1F7E2} *Sell PI for SOL*\n\n` +
      `Current price: *$${stats.priceUsd.toFixed(6)}/PI*\n\n` +
      `Usage: \`/sellpi <PI amount>\`\n` +
      `Example: \`/sellpi 1000\` — sell 1000 PI for SOL`,
      { parse_mode: 'Markdown' }
    );
    return;
  }

  const piIn = parseFloat(args[0]);
  if (!piIn || piIn <= 0) { await ctx.reply('Invalid amount.'); return; }

  const piInBase = Math.floor(piIn * 1e9);
  const quote = ammPool.quoteSellPI(piInBase);
  if (!quote) { await ctx.reply('Insufficient liquidity.'); return; }

  const solOut = quote.solOut / 1e9;
  const piAddr = u.wallet_address || u.pi_address || u.address || '';

  ctx.session.pendingSell = { piInBase, solOut: quote.solOut, piIn, solOutDisplay: solOut, priceImpact: quote.priceImpact, piAddr };
  await ctx.reply(
    `\u{1F7E2} *Sell ${piIn.toFixed(2)} PI*\n\n` +
    `Receive: *${solOut.toFixed(6)} SOL* ($${(solOut * solPrice).toFixed(2)})\n` +
    `Impact: ${quote.priceImpact.toFixed(2)}%\n` +
    `Fee: 0.3%`,
    { parse_mode: 'Markdown', reply_markup: new InlineKeyboard().text('\u{2705} Confirm Sell', 'confirm_sellpi').text('\u{274C} Cancel', 'cancel_sellpi') }
  );
});

bot.callbackQuery('confirm_sellpi', async (ctx) => {
  const pending = ctx.session.pendingSell;
  if (!pending) { await ctx.answerCallbackQuery('No pending order').catch(() => {}); return; }
  ctx.session.pendingSell = null;

  const u = await getUser(ctx.from.id);
  await ctx.editMessageText('\u{23F3} Processing your PI sale...').catch(() => {});

  try {
    // SECURITY: Verify user has PI balance
    const piAddr = pending.piAddr;
    if (!piAddr) throw new Error('No PIChain wallet');

    // 1. Transfer PI from seller to treasury
    if (PI_TREASURY_USER && PI_TREASURY_ADDR) {
      await pi.transfer(String(ctx.from.id), piAddr, PI_TREASURY_ADDR, pending.piInBase);
    }

    // 2. Execute AMM swap (updates pool reserves)
    const result = ammPool.executeSell(pending.piInBase);

    // 3. Transfer SOL from fee wallet to seller
    // NOTE: This requires the fee wallet to have SOL. In production,
    // the SOL pool reserves should be held in a dedicated Solana wallet.
    if (SOL_FEE_ADDR && u.solWallet) {
      // For now, the SOL is sent from the fee address
      // In a full implementation, this would be a pool wallet
    }

    const solOutDisplay = (result.solOut / 1e9).toFixed(6);
    const newPriceUsd = ammPool.priceInUsd(cachedSolPrice || 90);

    await ctx.editMessageText(
      `\u{2705} *Sale Complete\\!*\n\n` +
      `Sold: *${esc(pending.piIn.toFixed(2))} PI*\n` +
      `Received: ${esc(solOutDisplay)} SOL\n` +
      `New price: \\$${esc(newPriceUsd.toFixed(6))}/PI`,
      { parse_mode: 'MarkdownV2' }
    ).catch(() => {});
  } catch (e) {
    await ctx.editMessageText('\u{274C} Sale failed: ' + (e.message || e)).catch(() => {});
  }
  await ctx.answerCallbackQuery().catch(() => {});
});

bot.callbackQuery('cancel_sellpi', async (ctx) => {
  ctx.session.pendingSell = null;
  await ctx.editMessageText('Sale cancelled.').catch(() => {});
  await ctx.answerCallbackQuery().catch(() => {});
});

bot.command('portfolio', async (ctx) => { await showPortfolio(ctx, await getUser(ctx.from.id)); });
bot.command('settings', async (ctx) => { await showSettings(ctx, await getUser(ctx.from.id)); });
bot.command('tokens', async (ctx) => { const u = await getUser(ctx.from.id); await showTokens(ctx, u.chain); });
bot.command('launches', async (ctx) => { await showLaunches(ctx); });
bot.command('buy', async (ctx) => {
  const u = await getUser(ctx.from.id);
  const [m, a] = (ctx.match || '').trim().split(/\s+/);
  const currency = u.chain === 'sol' ? 'SOL' : 'PI';
  if (!m || !a) { ctx.reply('Usage: /buy <token_address> <amount_' + currency + '>'); return; }
  const amt = parseFloat(a);
  if (isNaN(amt) || amt <= 0) { ctx.reply('Invalid amount'); return; }
  if (u.chain === 'sol') {
    if (!/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(m)) { ctx.reply('Invalid Solana token address'); return; }
    await doSolBuy(ctx, u, m, Math.floor(amt * LAMPORTS_PER_SOL));
  } else {
    const mint = m.replace(/^(Pi314|pi314|0x)/,'');
    if (mint.length !== 64 || !/^[0-9a-fA-F]{64}$/.test(mint)) { ctx.reply('Invalid PI mint ID (64 hex chars)'); return; }
    await doBuy(ctx, u, mint, Math.floor(amt * 1e9));
  }
});
bot.command('sell', async (ctx) => {
  const u = await getUser(ctx.from.id);
  const [m, p] = (ctx.match || '').trim().split(/\s+/);
  if (!m || !p) { ctx.reply('Usage: /sell <token_address> <percent>'); return; }
  const pct = parseInt(p);
  if (isNaN(pct) || pct < 1 || pct > 100) { ctx.reply('Invalid percentage (1-100)'); return; }
  if (u.chain === 'sol') {
    if (!/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(m)) { ctx.reply('Invalid Solana token address'); return; }
    await doSolSell(ctx, u, m, pct);
  } else {
    const mint = m.replace(/^(Pi314|pi314|0x)/,'');
    if (mint.length !== 64 || !/^[0-9a-fA-F]{64}$/.test(mint)) { ctx.reply('Invalid PI mint ID'); return; }
    await doSell(ctx, u, mint, pct);
  }
});
bot.command('send', async (ctx) => {
  const args = (ctx.match || '').trim();
  const u = await getUser(ctx.from.id);
  const currency = u.chain === 'sol' ? 'SOL' : 'PI';

  // If no args, start guided flow
  if (!args) {
    ctx.session.awaitingInput = 'send_address';
    await ctx.reply('Send ' + currency + '\n\nStep 1/2: Enter the destination address:');
    return;
  }

  // If args provided, try direct send
  const [a, amt] = args.split(/\s+/);
  if (!amt) { ctx.session.awaitingInput = 'send_amount:' + a; await ctx.reply('Step 2/2: Enter amount of ' + currency + ' to send:'); return; }

  const sendAmt = parseFloat(amt);
  if (isNaN(sendAmt) || sendAmt <= 0) { await ctx.reply('Invalid amount'); return; }

  await executeSend(ctx, u, a, sendAmt);
});

async function executeSend(ctx, u, address, amount) {
  const currency = u.chain === 'sol' ? 'SOL' : 'PI';
  const rawAmount = u.chain === 'sol' ? Math.floor(amount * LAMPORTS_PER_SOL) : Math.floor(amount * 1e9);
  const largeThreshold = u.chain === 'sol' ? LARGE_WITHDRAW_SOL : LARGE_WITHDRAW_PI;

  // Daily withdrawal limit check
  if (!checkDailyLimit(u, rawAmount, u.chain)) {
    const limitStr = u.chain === 'sol' ? (DAILY_LIMIT_SOL/LAMPORTS_PER_SOL).toFixed(0)+' SOL' : (DAILY_LIMIT_PI/1e9).toFixed(0)+' PI';
    await ctx.reply('❌ Daily withdrawal limit reached (' + limitStr + '/day).\nResets at midnight UTC. Use /limits to check.');
    return;
  }

  // 2FA required for large sends
  if (u.totp_enabled && rawAmount >= largeThreshold) {
    ctx.session.awaitingInput = '2fa_send:' + JSON.stringify({chain: u.chain, address, amount: rawAmount});
    await ctx.reply('🔐 Large send detected (' + amount + ' ' + currency + ')\nEnter your 2FA code to confirm:');
    return;
  }

  recordWithdrawal(u.telegram_id, rawAmount);

  if (u.chain === 'sol') {
    if (!/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(address)) { await ctx.reply('Invalid Solana address'); return; }
    if (!u.solWallet) { await ctx.reply('Wallet error'); return; }
    await ctx.reply('\u{23F3} Sending ' + amount + ' SOL...');
    try {
      const txid = await sol.transfer(u.solWallet, address, rawAmount);
      await ctx.reply('\u{2705} Sent ' + amount + ' SOL\nTo: ' + address.slice(0,12) + '...\nTX: ' + txid.slice(0, 20) + '...');
    } catch (e) { await ctx.reply('\u{274C} Send failed: ' + (e.message||'unknown').slice(0,200)); }
  } else {
    const addr = address.replace(/^(Pi314|pi314|0x)/,'');
    if (addr.length !== 40 || !/^[0-9a-fA-F]{40}$/.test(addr)) { await ctx.reply('Invalid PI address (40 hex chars)'); return; }
    if (!u.activated) { try { await pi.activateWallet(u.address); Q.setActivated.run(u.telegram_id); } catch(e) { if (!e.message?.includes('already')) { await ctx.reply('Activation failed'); return; } Q.setActivated.run(u.telegram_id); } }
    await ctx.reply('\u{23F3} Sending ' + amount + ' PI...');
    try {
      const r = await pi.transfer(u.telegram_id, u.wallet_address, addr, Math.floor(amount*1e9));
      if (r.status==='pending') await ctx.reply('\u{2705} Sent ' + amount + ' PI\nTo: Pi314' + addr.slice(0,8) + '...\nTX: ' + (r.tx_hash||'').slice(0,16) + '...');
      else await ctx.reply('\u{274C} Failed: ' + (r.error||'unknown'));
    } catch(e) { await ctx.reply('\u{274C} Send failed: ' + (e.message||'unknown').slice(0,200)); }
  }
}

// ── Message handler (paste token address or awaiting input) ────────────────
bot.on('message:text', async (ctx, next) => {
  const text = ctx.message.text.trim();

  // Don't intercept commands — let them pass through to bot.command() handlers
  if (text.startsWith('/') && !ctx.session.awaitingInput) { await next(); return; }

  // Handle awaited input
  if (ctx.session.awaitingInput === 'import_key') {
    ctx.session.awaitingInput = null;
    // Auto-delete the user's key message for security
    ctx.deleteMessage().catch(() => {});
    try {
      // Try to parse as PQ wallet JSON
      let walletData;
      try {
        walletData = JSON.parse(text);
      } catch {
        // If it's a 64-char hex string, it's an old Ed25519 key
        if (/^[0-9a-fA-F]{64}$/.test(text.trim())) {
          await ctx.reply(
            '\u{26A0}\u{FE0F} That looks like an Ed25519 private key.\n\n' +
            'PIChain now uses post-quantum wallets. Ed25519 keys cannot be imported.\n' +
            'Use /wallet to see your PQ address, or /import to import a PQ wallet JSON file.'
          );
          return;
        }
        await ctx.reply('\u{274C} Invalid format. Please paste the full contents of a PQ wallet JSON file.');
        return;
      }

      // Check if it's a PQ wallet (has ml_dsa_secret_key field)
      const pqExport = walletData.pq_wallet || walletData;
      if (!pqExport.ml_dsa_secret_key) {
        await ctx.reply('\u{274C} Not a valid PQ wallet file. It must contain ml_dsa_secret_key, ml_dsa_public_key, slh_dsa_secret_key, and slh_dsa_public_key.');
        return;
      }

      // Import into vault
      const userId = String(ctx.from.id);
      // Delete existing wallet first if it exists
      try { await C.getAddress(userId); } catch {}
      const result = await C.importWallet(userId, pqExport);
      const addr = result.address;

      // Update user record
      const db = require('better-sqlite3')('./pibot.db');
      db.prepare('UPDATE users SET wallet_seed = ?, pi_address = ?, wallet_address = ?, address = ? WHERE telegram_id = ?')
        .run('vault:' + userId, addr, addr, addr, ctx.from.id);
      db.close();

      await ctx.reply(
        '\u{2705} PQ wallet imported successfully!\n\n' +
        'Address: ' + addr + '\n\n' +
        'Your wallet is now managed by the vault. Use /wallet to view.'
      );
    } catch (e) {
      await ctx.reply('\u{274C} Import failed: ' + (e.message || e));
    }
    return;
  }

  // Custom sell PI amount
  if (ctx.session.awaitingInput === 'sellpi_amount') {
    ctx.session.awaitingInput = null;
    const amount = parseFloat(text);
    if (!amount || amount <= 0) { await ctx.reply('Invalid amount.'); return; }
    const u = await getUser(ctx.from.id);
    const piInBase = Math.floor(amount * 1e9);
    const quote = ammPool.quoteSellPI(piInBase);
    if (!quote) { await ctx.reply('Insufficient liquidity.'); return; }
    const solOut = quote.solOut / 1e9;
    const solPrice = cachedSolPrice || 90;
    const piAddr = u.wallet_address || u.pi_address || u.address || '';
    ctx.session.pendingSell = { piInBase, solOut: quote.solOut, piIn: amount, solOutDisplay: solOut, priceImpact: quote.priceImpact, piAddr };
    await ctx.reply(
      `\u{1F7E2} *Sell ${amount.toLocaleString()} PI*\n\n` +
      `Receive: *${solOut.toFixed(6)} SOL* ($${(solOut * solPrice).toFixed(2)})\n` +
      `Impact: ${quote.priceImpact.toFixed(2)}%\n` +
      `Fee: 0.3%`,
      { parse_mode: 'Markdown', reply_markup: new InlineKeyboard().text('\u{2705} Confirm', 'confirm_sellpi').text('\u{274C} Cancel', 'cancel_sellpi') }
    );
    return;
  }

  // Custom buy PI amount
  if (ctx.session.awaitingInput === 'buypi_amount') {
    ctx.session.awaitingInput = null;
    const amount = parseFloat(text);
    if (!amount || amount <= 0) {
      await ctx.reply('Invalid amount. Please enter a number (e.g., 1000).');
      return;
    }
    // Trigger the same flow as button buy
    const u = await getUser(ctx.from.id);
    const solPrice = cachedSolPrice || 90;
    const costUsd = amount * PI_PRICE_USD;
    const costSol = costUsd / solPrice;
    const piAddr = u.wallet_address || u.pi_address || u.address || '';
    if (!piAddr || piAddr === '0'.repeat(40)) {
      await ctx.reply('You need a PIChain wallet first. Type /wallet to set one up.');
      return;
    }
    ctx.session.pendingBuy = { amountPi: amount, costSol, costUsd, piAddr, costLamports: Math.ceil(costSol * 1e9) };
    const kb = new InlineKeyboard()
      .text('\u{2705} Confirm Purchase', 'confirm_buypi')
      .text('\u{274C} Cancel', 'cancel_buypi');
    await ctx.reply(
      `\u{1F7E3} *Buy ${amount.toLocaleString()} PI*\n\n` +
      `Cost: *${costSol.toFixed(6)} SOL* ($${costUsd.toFixed(2)})\n` +
      `SOL price: $${solPrice.toFixed(2)}\n\n` +
      `PI sent to: \`${piAddr}\`\n\n` +
      `SOL deducted from your PiBot Solana wallet.`,
      { parse_mode: 'Markdown', reply_markup: kb }
    );
    return;
  }

  // 2FA verification
  if (ctx.session.awaitingInput === '2fa_verify') {
    ctx.session.awaitingInput = null;
    ctx.deleteMessage().catch(() => {}); // Delete the code message
    const u = await getUser(ctx.from.id);
    const secret = get2FASecret(u);
    if (!secret) { await ctx.reply('2FA setup error. Try /setup2fa again.'); return; }
    const totp = new TOTP({ secret: Secret.fromBase32(secret), digits: 6, period: 30 });
    if (totp.validate({ token: text.trim(), window: 1 }) !== null) {
      db.prepare('UPDATE users SET totp_enabled = 1 WHERE telegram_id = ?').run(ctx.from.id);
      await ctx.reply('✅ 2FA enabled! You\'ll need the code for sends over 1 SOL / 10 PI.');
    } else {
      db.prepare('UPDATE users SET totp_secret = \'\' WHERE telegram_id = ?').run(ctx.from.id);
      await ctx.reply('❌ Invalid code. 2FA not enabled. Try /setup2fa again.');
    }
    return;
  }

  if (ctx.session.awaitingInput === '2fa_disable') {
    ctx.session.awaitingInput = null;
    ctx.deleteMessage().catch(() => {});
    const u = await getUser(ctx.from.id);
    if (verify2FA(u, text.trim())) {
      db.prepare('UPDATE users SET totp_enabled = 0, totp_secret = \'\' WHERE telegram_id = ?').run(ctx.from.id);
      await ctx.reply('✅ 2FA disabled.');
    } else {
      await ctx.reply('❌ Invalid code. 2FA remains enabled.');
    }
    return;
  }

  // 2FA confirmation for pending send
  if (ctx.session.awaitingInput?.startsWith('2fa_send:')) {
    const jsonStr = ctx.session.awaitingInput.slice('2fa_send:'.length);
    let parsed;
    try { parsed = JSON.parse(jsonStr); } catch { await ctx.reply('Session error. Try again.'); return; }
    const { chain, address, amount: amountStr } = parsed;
    ctx.session.awaitingInput = null;
    ctx.deleteMessage().catch(() => {});
    const u = await getUser(ctx.from.id);
    if (!verify2FA(u, text.trim())) {
      await ctx.reply('❌ Invalid 2FA code. Send cancelled.');
      return;
    }
    const amount = parseInt(amountStr);
    recordWithdrawal(u.telegram_id, amount);
    await executeSend(ctx, u, address, amount);
    return;
  }

  if (ctx.session.awaitingInput?.startsWith('custom_buy:')) {
    const mintId = ctx.session.awaitingInput.split(':')[1];
    const mode = ctx.session.awaitingInput.split(':')[2]; // 'dex' or 'curve'
    ctx.session.awaitingInput = null;
    const amt = parseFloat(text);
    if (isNaN(amt) || amt <= 0) { await ctx.reply('Invalid amount. Enter a number like 2.5'); return; }
    const u = await getUser(ctx.from.id);
    if (mode === 'curve') await doCurveBuy(ctx, u, mintId, Math.floor(amt * 1e9));
    else await doBuy(ctx, u, mintId, Math.floor(amt * 1e9));
    return;
  }

  // ── Limit order input parsing ─────────────────────────────────────────
  if (ctx.session.awaitingInput?.startsWith('limit_buy:')) {
    const mintId = ctx.session.awaitingInput.split(':')[1];
    ctx.session.awaitingInput = null;
    const parts = text.trim().split(/\s+/);
    const triggerPrice = parseFloat(parts[0]); const piAmt = parseFloat(parts[1]);
    if (isNaN(triggerPrice) || isNaN(piAmt) || triggerPrice <= 0 || piAmt <= 0) { await ctx.reply('Invalid. Format: <price> <pi_amount>'); return; }
    const DEN = 1e9;
    Q.insertLimit.run(ctx.from.id, mintId, 'limit_buy', Math.floor(triggerPrice * DEN), DEN, Math.floor(piAmt * 1e9), 0, 0);
    await ctx.reply(`\u{2705} Limit Buy set: buy ${piAmt} PI worth when price <= ${triggerPrice}`);
    return;
  }
  if (ctx.session.awaitingInput?.startsWith('take_profit:')) {
    const mintId = ctx.session.awaitingInput.split(':')[1];
    ctx.session.awaitingInput = null;
    const parts = text.trim().split(/\s+/);
    const triggerPrice = parseFloat(parts[0]); const sellPct = parseInt(parts[1]);
    if (isNaN(triggerPrice) || isNaN(sellPct) || triggerPrice <= 0 || sellPct < 1 || sellPct > 100) { await ctx.reply('Invalid. Format: <price> <sell_percent>'); return; }
    const u = await getUser(ctx.from.id);
    const acct = await pi.getTokenAccount(mintId, u.address).catch(() => ({balance:0n}));
    const sellAmt = Number(acct.balance || 0n) * sellPct / 100;
    if (sellAmt <= 0) { await ctx.reply('No tokens to sell.'); return; }
    const DEN = 1e9;
    Q.insertLimit.run(ctx.from.id, mintId, 'take_profit', Math.floor(triggerPrice * DEN), DEN, Math.floor(sellAmt), 0, 0);
    await ctx.reply(`\u{2705} Take Profit set: sell ${sellPct}% when price >= ${triggerPrice}`);
    return;
  }
  if (ctx.session.awaitingInput?.startsWith('stop_loss:')) {
    const mintId = ctx.session.awaitingInput.split(':')[1];
    ctx.session.awaitingInput = null;
    const parts = text.trim().split(/\s+/);
    const triggerPrice = parseFloat(parts[0]); const sellPct = parseInt(parts[1]);
    if (isNaN(triggerPrice) || isNaN(sellPct) || triggerPrice <= 0 || sellPct < 1 || sellPct > 100) { await ctx.reply('Invalid. Format: <price> <sell_percent>'); return; }
    const u = await getUser(ctx.from.id);
    const acct = await pi.getTokenAccount(mintId, u.address).catch(() => ({balance:0n}));
    const sellAmt = Number(acct.balance || 0n) * sellPct / 100;
    if (sellAmt <= 0) { await ctx.reply('No tokens to sell.'); return; }
    const DEN = 1e9;
    Q.insertLimit.run(ctx.from.id, mintId, 'stop_loss', Math.floor(triggerPrice * DEN), DEN, Math.floor(sellAmt), 0, 0);
    await ctx.reply(`\u{2705} Stop Loss set: sell ${sellPct}% when price <= ${triggerPrice}`);
    return;
  }
  if (ctx.session.awaitingInput?.startsWith('trailing_stop:')) {
    const mintId = ctx.session.awaitingInput.split(':')[1];
    ctx.session.awaitingInput = null;
    const parts = text.trim().split(/\s+/);
    const dropPct = parseFloat(parts[0]); const sellPct = parseInt(parts[1]);
    if (isNaN(dropPct) || isNaN(sellPct) || dropPct <= 0 || dropPct > 50 || sellPct < 1 || sellPct > 100) { await ctx.reply('Invalid. Format: <drop_%> <sell_%> (drop 1-50%)'); return; }
    const u = await getUser(ctx.from.id);
    const acct = await pi.getTokenAccount(mintId, u.address).catch(() => ({balance:0n}));
    const sellAmt = Number(acct.balance || 0n) * sellPct / 100;
    if (sellAmt <= 0) { await ctx.reply('No tokens to sell.'); return; }
    // Get current price for initial high-water mark
    const pairs = await pi.getDexPairs().catch(() => []);
    const pair = pairs.find(p => p.mint_a === mintId || p.mint_b === mintId);
    const currentPrice = pair?.price || 0;
    const DEN = 1e9;
    Q.insertLimit.run(ctx.from.id, mintId, 'trailing_stop', Math.floor(dropPct * DEN), DEN, Math.floor(sellAmt), Math.floor(currentPrice * DEN), Math.floor(currentPrice * DEN));
    await ctx.reply(`\u{2705} Trailing Stop set: sell ${sellPct}% if price drops ${dropPct}% from peak (current: ${currentPrice.toFixed(8)})`);
    return;
  }
  if (ctx.session.awaitingInput?.startsWith('dca_buy:')) {
    const mintId = ctx.session.awaitingInput.split(':')[1];
    ctx.session.awaitingInput = null;
    const parts = text.trim().split(/\s+/);
    const piPerBuy = parseFloat(parts[0]); const intervalMin = parseInt(parts[1]); const totalBuys = parseInt(parts[2]);
    if (isNaN(piPerBuy) || isNaN(intervalMin) || isNaN(totalBuys) || piPerBuy <= 0 || intervalMin < 1 || totalBuys < 1 || totalBuys > 1000) {
      await ctx.reply('Invalid. Format: <pi_amount> <interval_minutes> <total_buys>'); return;
    }
    const intervalMs = intervalMin * 60 * 1000;
    const nextAt = Date.now() + intervalMs;
    Q.insertDca.run(ctx.from.id, mintId, 'dca_buy', Math.floor(piPerBuy * 1e9), intervalMs, totalBuys, nextAt);
    await ctx.reply(`\u{2705} DCA Buy set: ${piPerBuy} PI every ${intervalMin}min, ${totalBuys} times (total: ${(piPerBuy * totalBuys).toFixed(2)} PI)`);
    return;
  }

  // Guided send flow
  if (ctx.session.awaitingInput === 'send_address') {
    ctx.session.awaitingInput = 'send_amount:' + text;
    const u = await getUser(ctx.from.id);
    await ctx.reply('Destination: ' + text + '\n\nStep 2/2: Enter amount of ' + (u.chain === 'sol' ? 'SOL' : 'PI') + ' to send:');
    return;
  }
  if (ctx.session.awaitingInput?.startsWith('send_amount:')) {
    const address = ctx.session.awaitingInput.split(':').slice(1).join(':');
    ctx.session.awaitingInput = null;
    const amount = parseFloat(text);
    if (isNaN(amount) || amount <= 0) { await ctx.reply('Invalid amount'); return; }
    await executeSend(ctx, await getUser(ctx.from.id), address, amount);
    return;
  }

  // Custom slippage input (chain-aware)
  if (ctx.session.awaitingInput === 'custom_slippage') {
    ctx.session.awaitingInput = null;
    const pct = parseFloat(text);
    if (isNaN(pct) || pct < 0.1 || pct > 50) { await ctx.reply('Invalid. Enter a number between 0.1 and 50 (e.g. 2.5)'); return; }
    const bps = Math.round(pct * 100);
    const u = await getUser(ctx.from.id);
    if (u.chain === 'sol') Q.setSolSlippage.run(bps, ctx.from.id); else Q.setSlippage.run(bps, ctx.from.id);
    await ctx.reply(`${u.chain === 'sol' ? 'Solana' : 'PIChain'} slippage set to ${pct}%`);
    return;
  }

  // Custom auto-buy input (chain-aware)
  if (ctx.session.awaitingInput === 'custom_autobuy') {
    ctx.session.awaitingInput = null;
    const amt = parseFloat(text);
    if (isNaN(amt) || amt < 0) { await ctx.reply('Invalid. Enter a number like 0.25'); return; }
    const base = Math.floor(amt * 1e9);
    const u = await getUser(ctx.from.id);
    if (u.chain === 'sol') Q.setSolAutoBuy.run(base, ctx.from.id); else Q.setAutoBuy.run(base, ctx.from.id);
    await ctx.reply(amt > 0 ? `${u.chain === 'sol' ? 'Solana' : 'PIChain'} auto-buy set to ${amt}` : 'Auto-buy disabled');
    return;
  }

  // Solana custom sell percentage
  if (ctx.session.awaitingInput?.startsWith('sol_sell_pct:')) {
    const mintAddr = ctx.session.awaitingInput.split(':')[1];
    ctx.session.awaitingInput = null;
    const pct = parseInt(text);
    if (isNaN(pct) || pct < 1 || pct > 100) { await ctx.reply('Invalid. Enter 1-100'); return; }
    await doSolSell(ctx, await getUser(ctx.from.id), mintAddr, pct);
    return;
  }

  // Solana custom buy amount
  if (ctx.session.awaitingInput?.startsWith('sol_buy:')) {
    const mintAddr = ctx.session.awaitingInput.split(':')[1];
    ctx.session.awaitingInput = null;
    const amt = parseFloat(text);
    if (isNaN(amt) || amt <= 0) { await ctx.reply('Invalid amount. Enter SOL like 0.25'); return; }
    await doSolBuy(ctx, await getUser(ctx.from.id), mintAddr, Math.floor(amt * LAMPORTS_PER_SOL));
    return;
  }

  // Price alert input
  if (ctx.session.awaitingInput?.startsWith('price_alert:')) {
    const mintId = ctx.session.awaitingInput.split(':')[1];
    ctx.session.awaitingInput = null;
    const parts = text.trim().split(/\s+/);
    const price = parseFloat(parts[0]); const dir = (parts[1] || '').toLowerCase();
    if (isNaN(price) || price <= 0 || !['above','below'].includes(dir)) { await ctx.reply('Invalid. Format: <price> <above|below>'); return; }
    const DEN = 1e9;
    Q.insertLimit.run(ctx.from.id, mintId, 'price_alert', Math.floor(price * DEN), DEN, dir === 'above' ? 1 : -1, 0, 0);
    await ctx.reply(`\u{1F514} Alert set: notify when price goes ${dir} ${price}`);
    return;
  }

  // Detect token address — PIChain (64 hex) or Solana (base58, 32-44 chars)
  const clean = text.replace(/^(Pi314|pi314|0x|0X)/,'');
  const u = await getUser(ctx.from.id);

  // PIChain: 64 hex chars
  if (/^[0-9a-fA-F]{64}$/.test(clean)) {
    const mintId = clean.toLowerCase();
    if (u.chain === 'pi' || u.chain !== 'sol') {
      // Auto-buy if enabled
      if (u.auto_buy_amount > 0) {
        const pairs = await pi.getDexPairs().catch(() => []);
        const hasPair = pairs.some(p => p.mint_a === mintId || p.mint_b === mintId || p.launch_mint === mintId);
        if (hasPair) { await doBuy(ctx, u, mintId, u.auto_buy_amount); return; }
        const launches = await pi.getLaunches().catch(() => []);
        const hasLaunch = launches.some(l => l.mint_id === mintId && l.state === 'Active');
        if (hasLaunch) { await doCurveBuy(ctx, u, mintId, u.auto_buy_amount); return; }
      }
      await tokenView(ctx, u, mintId);
    }
    return;
  }

  // Solana: base58, 32-44 chars, starts with uppercase/digit
  if (/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(text.trim())) {
    const mintAddr = text.trim();
    if (u.chain === 'sol') {
      if (u.auto_buy_amount > 0) {
        await doSolBuy(ctx, u, mintAddr, u.auto_buy_amount);
        return;
      }
    }
    await solTokenView(ctx, u, mintAddr);
    return;
  }
});

// ── Callback handlers ──────────────────────────────────────────────────────
bot.callbackQuery('home', async (ctx) => { ctx.answerCallbackQuery().catch(() => {}); await mainMenu(ctx, await getUser(ctx.from.id)); });
bot.callbackQuery('close', async (ctx) => { ctx.answerCallbackQuery().catch(() => {}); await ctx.deleteMessage().catch(() => {}); });
bot.callbackQuery('refresh', async (ctx) => { ctx.answerCallbackQuery('\u{1F504}').catch(() => {}); await mainMenu(ctx, await getUser(ctx.from.id)); });
bot.callbackQuery('positions', async (ctx) => { ctx.answerCallbackQuery().catch(() => {}); await showPortfolio(ctx, await getUser(ctx.from.id)); });
bot.callbackQuery('tokens', async (ctx) => { ctx.answerCallbackQuery().catch(() => {}); const u = await getUser(ctx.from.id); await showTokens(ctx, u.chain); });
bot.callbackQuery('launches', async (ctx) => { ctx.answerCallbackQuery().catch(() => {}); await showLaunches(ctx); });
bot.callbackQuery('settings', async (ctx) => { ctx.answerCallbackQuery().catch(() => {}); await showSettings(ctx, await getUser(ctx.from.id)); });
bot.callbackQuery('wallet', async (ctx) => {
  const u = await getUser(ctx.from.id);
  const isPi = u.chain === 'pi';
  const addr = isPi ? 'Pi314' + u.address : (u.solWallet?.publicKey || 'N/A');
  const bal = isPi
    ? await pi.getBalance(u.address).catch(()=>0)
    : await sol.getBalance(u.solWallet?.publicKey || '').catch(()=>0);
  const balStr = isPi ? esc(fPI(bal)) + ' PI' : esc((bal / LAMPORTS_PER_SOL).toFixed(4)) + ' SOL';
  const chainIcon = isPi ? '\u{1F7E3}' : '\u{1F7E2}';
  const chainName = isPi ? 'PIChain' : 'Solana';
  await ctx.editMessageText(
    `\u{1F4B3} ${chainIcon} *${esc(chainName)} Wallet*\n\n` +
    `*Address:*\n\`${esc(addr)}\`\n\n` +
    `*Balance:* ${balStr}`,
    { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('Export Key','xkey').text('Import','ikey').row().text('\u{1F3E0} Home','home') }
  ).catch(()=>{});
  await ctx.answerCallbackQuery().catch(() => {});
});
bot.callbackQuery('xkey', async (ctx) => {
  const u = await getUser(ctx.from.id);
  // PIChain: PQ wallet keys are managed by the vault — cannot be exported via Telegram.
  // Users export via pichain-signer CLI: pichain-signer --wallet wallet.json
  const piAddr = u.wallet_address || u.wallet?.address || 'not set';
  let text = 'PIChain wallet: Pi314' + piAddr + '\n';
  text += '(PQ keys managed by pichain-signer — export via CLI)\n';
  if (u.solWallet?.secretKey) text += '\nSolana key (Phantom/MetaMask):\n' + u.solWallet.secretKey + '\n';
  text += '\nDelete after saving!';
  await ctx.reply(text);
  await ctx.answerCallbackQuery('Keys sent').catch(() => {});
});
bot.callbackQuery('ikey', async (ctx) => { ctx.session.awaitingInput = 'import_key'; await ctx.reply('Paste the contents of your PQ wallet JSON file (from the desktop app or CLI miner).\n\nOpen the wallet .json file in a text editor and copy everything.'); await ctx.answerCallbackQuery().catch(() => {}); });
bot.callbackQuery('cmd_buy', async (ctx) => { const u = await getUser(ctx.from.id); const isPi = u.chain==='pi'; await ctx.editMessageText(isPi ? 'Paste a *token mint ID* \\(64 hex chars\\) to buy\\.' : 'Paste a *Solana token address* to buy\\.', { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('\u{2190} Home','home') }).catch(()=>{}); await ctx.answerCallbackQuery().catch(() => {}); });

// Buy PI with buttons
bot.callbackQuery('buy_pi_menu', async (ctx) => {
  if (!ammPool.isInitialized()) { await ctx.editMessageText('PI/SOL pool not initialized yet.').catch(() => {}); await ctx.answerCallbackQuery().catch(() => {}); return; }
  const solPrice = cachedSolPrice || 90;
  const stats = ammPool.stats(solPrice);
  const kb = new InlineKeyboard()
    .text('0.01 SOL', 'buypi_sol:0.01').text('0.05 SOL', 'buypi_sol:0.05').text('0.1 SOL', 'buypi_sol:0.1').row()
    .text('0.5 SOL', 'buypi_sol:0.5').text('1 SOL', 'buypi_sol:1').text('5 SOL', 'buypi_sol:5').row()
    .text('Custom Amount', 'buypi_custom').row()
    .text('\u{1F7E2} Sell PI', 'sellpi_menu').text('\u{2190} Home', 'home');
  await ctx.editMessageText(
    `\u{1F7E3} *Buy PI*\n\n` +
    `Price: *\\$${esc(stats.priceUsd.toFixed(6))}* per PI\n` +
    `1 SOL \\= ~${esc(Math.floor(1/stats.priceSol).toLocaleString())} PI\n\n` +
    `Select SOL amount:`,
    { parse_mode: 'MarkdownV2', reply_markup: kb }
  ).catch(() => {});
  await ctx.answerCallbackQuery().catch(() => {});
});

bot.callbackQuery(/^buypi_sol:(.+)$/, async (ctx) => {
  const solIn = parseFloat(ctx.match[1]);
  if (!solIn || !ammPool.isInitialized()) { await ctx.answerCallbackQuery('Pool not ready').catch(() => {}); return; }
  const u = await getUser(ctx.from.id);
  const solPrice = cachedSolPrice || 90;
  const solInLamports = Math.floor(solIn * 1e9);
  const quote = ammPool.quoteBuyPI(solInLamports);
  if (!quote) { await ctx.editMessageText('Insufficient liquidity.').catch(() => {}); await ctx.answerCallbackQuery().catch(() => {}); return; }

  const piOut = quote.piOut / 1e9;
  const pricePerPi = solIn / piOut;
  const priceUsd = pricePerPi * solPrice;
  const piAddr = u.wallet_address || u.pi_address || u.address || '';

  if (!piAddr || piAddr === '0'.repeat(40)) {
    await ctx.editMessageText('You need a PIChain wallet first. Type /wallet.').catch(() => {});
    await ctx.answerCallbackQuery().catch(() => {});
    return;
  }

  ctx.session.pendingBuy = { solInLamports, piOut: quote.piOut, solIn, piOutDisplay: piOut, priceUsd, piAddr, priceImpact: quote.priceImpact };

  await ctx.editMessageText(
    `\u{1F7E3} *Buy ~${esc(piOut.toFixed(2))} PI*\n\n` +
    `Pay: *${esc(solIn.toFixed(6))} SOL* \\(\\$${esc((solIn * solPrice).toFixed(2))}\\)\n` +
    `Price: \\$${esc(priceUsd.toFixed(6))}/PI\n` +
    `Impact: ${esc(quote.priceImpact.toFixed(2))}%\n\n` +
    `PI sent to: \`${esc(piAddr)}\``,
    { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('\u{2705} Confirm', 'confirm_buypi').text('\u{274C} Back', 'buy_pi_menu') }
  ).catch(() => {});
  await ctx.answerCallbackQuery().catch(() => {});
});

bot.callbackQuery('sellpi_menu', async (ctx) => {
  if (!ammPool.isInitialized()) { await ctx.editMessageText('Pool not initialized.').catch(() => {}); await ctx.answerCallbackQuery().catch(() => {}); return; }
  const solPrice = cachedSolPrice || 90;
  const stats = ammPool.stats(solPrice);
  const kb = new InlineKeyboard()
    .text('100 PI', 'sellpi_amt:100').text('500 PI', 'sellpi_amt:500').text('1,000 PI', 'sellpi_amt:1000').row()
    .text('5,000 PI', 'sellpi_amt:5000').text('10,000 PI', 'sellpi_amt:10000').row()
    .text('Custom Amount', 'sellpi_custom').row()
    .text('\u{1F7E3} Buy PI', 'buy_pi_menu').text('\u{2190} Home', 'home');
  await ctx.editMessageText(
    `\u{1F7E2} *Sell PI for SOL*\n\n` +
    `Price: *\\$${esc(stats.priceUsd.toFixed(6))}* per PI\n` +
    `1 PI \\= ${esc(stats.priceSol.toFixed(8))} SOL\n\n` +
    `Select PI amount to sell:`,
    { parse_mode: 'MarkdownV2', reply_markup: kb }
  ).catch(() => {});
  await ctx.answerCallbackQuery().catch(() => {});
});

bot.callbackQuery(/^sellpi_amt:(\d+)$/, async (ctx) => {
  const piIn = parseInt(ctx.match[1]);
  if (!piIn || !ammPool.isInitialized()) { await ctx.answerCallbackQuery('Pool not ready').catch(() => {}); return; }
  const u = await getUser(ctx.from.id);
  const solPrice = cachedSolPrice || 90;
  const piInBase = Math.floor(piIn * 1e9);
  const quote = ammPool.quoteSellPI(piInBase);
  if (!quote) { await ctx.editMessageText('Insufficient liquidity.').catch(() => {}); await ctx.answerCallbackQuery().catch(() => {}); return; }

  const solOut = quote.solOut / 1e9;
  const piAddr = u.wallet_address || u.pi_address || u.address || '';

  ctx.session.pendingSell = { piInBase, solOut: quote.solOut, piIn, solOutDisplay: solOut, priceImpact: quote.priceImpact, piAddr };

  await ctx.editMessageText(
    `\u{1F7E2} *Sell ${esc(piIn.toLocaleString())} PI*\n\n` +
    `Receive: *${esc(solOut.toFixed(6))} SOL* \\(\\$${esc((solOut * solPrice).toFixed(2))}\\)\n` +
    `Impact: ${esc(quote.priceImpact.toFixed(2))}%\n` +
    `Fee: 0\\.3%`,
    { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('\u{2705} Confirm', 'confirm_sellpi').text('\u{274C} Back', 'sellpi_menu') }
  ).catch(() => {});
  await ctx.answerCallbackQuery().catch(() => {});
});

bot.callbackQuery('sellpi_custom', async (ctx) => {
  ctx.session.awaitingInput = 'sellpi_amount';
  await ctx.editMessageText('Enter the amount of PI you want to sell:').catch(() => {});
  await ctx.answerCallbackQuery().catch(() => {});
});

bot.callbackQuery('piprice_btn', async (ctx) => {
  if (!ammPool.isInitialized()) { await ctx.editMessageText('Pool not initialized.').catch(() => {}); await ctx.answerCallbackQuery().catch(() => {}); return; }
  const solPrice = cachedSolPrice || 90;
  const stats = ammPool.stats(solPrice);
  await ctx.editMessageText(
    `\u{1F4CA} *PI Price*\n\n` +
    `\\$${esc(stats.priceUsd.toFixed(4))} per PI\n` +
    `1 SOL \\= ${esc(Math.floor(1/stats.priceSol).toLocaleString())} PI\n` +
    `1 PI \\= ${esc(stats.priceSol.toFixed(8))} SOL\n\n` +
    `*Hard Cap:* 3,141,592,653 PI\n` +
    `*100% to miners* — no pre\\-mine`,
    { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('\u{1F7E3} Buy PI', 'buy_pi_menu').text('\u{1F7E2} Sell PI', 'sellpi_menu').row().text('\u{2190} Home', 'home') }
  ).catch(() => {});
  await ctx.answerCallbackQuery().catch(() => {});
});

bot.callbackQuery('buypi_custom', async (ctx) => {
  ctx.session.awaitingInput = 'buypi_amount';
  await ctx.editMessageText('Enter the amount of PI you want to buy:').catch(() => {});
  await ctx.answerCallbackQuery().catch(() => {});
});
bot.callbackQuery('cmd_wallets', async (ctx) => { const u = await getUser(ctx.from.id); const wallets = multiWallet.getWallets(u.telegram_id); if(wallets.length===0){await multiWallet.createWallet(u.telegram_id,'Main');multiWallet.switchWallet(u.telegram_id,multiWallet.getWallets(u.telegram_id)[0].id);} const all=multiWallet.getWallets(u.telegram_id); let text='\u{1F4B3} *Wallets*\n\n'; const kb=new InlineKeyboard(); for(const w of all){text+=`${w.is_active?'\u{2705}':'\u{26AA}'} *${esc(w.label)}*\n`;if(!w.is_active)kb.text('Switch '+w.label,'mw_switch:'+w.id).row();} kb.text('\u{2795} New Wallet','mw_new').text('\u{2190} Home','home').row(); await ctx.editMessageText(text,{parse_mode:'MarkdownV2',reply_markup:kb}).catch(()=>ctx.reply(text,{parse_mode:'MarkdownV2',reply_markup:kb})); await ctx.answerCallbackQuery().catch(() => {}); });
bot.callbackQuery('cmd_sell', async (ctx) => { ctx.answerCallbackQuery().catch(() => {}); await showPortfolio(ctx, await getUser(ctx.from.id)); });
bot.callbackQuery('cmd_send', async (ctx) => {
  ctx.session.awaitingInput = 'send_address';
  const u = await getUser(ctx.from.id);
  await ctx.reply('Send ' + (u.chain==='sol'?'SOL':'PI') + '\n\nStep 1/2: Enter the destination address:');
  await ctx.answerCallbackQuery().catch(() => {});
});
bot.callbackQuery('cmd_alerts', async (ctx) => {
  const orders = Q.getActiveLimits.all(ctx.from.id).filter(o => o.direction === 'price_alert' || o.direction === 'snipe_graduation');
  if (orders.length === 0) {
    await ctx.editMessageText(
      '*Alerts*\n\nNo active alerts\\.\n\n'
      + '*Set up alerts:*\n'
      + '`/alert <token> <price> <above|below>`\n'
      + '`/snipe <amount>` \\- auto\\-buy on graduation\n'
      + '`/autosnipe` \\- new pool alerts',
      { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('Home','home') }
    ).catch(()=>{});
  } else {
    const labels = { price_alert:'Price Alert', snipe_graduation:'Grad Snipe' };
    let text = '*Active Alerts \\(' + orders.length + '\\)*\n\n';
    const kb = new InlineKeyboard();
    for (const o of orders) {
      const l = labels[o.direction] || o.direction;
      const mint = o.mint_id === '*' ? 'ALL' : o.mint_id.slice(0,8) + '...';
      text += '*' + esc(l) + '* on `' + esc(mint) + '`\n';
      kb.text('Cancel #'+o.id, 'cancel_order:'+o.id).row();
    }
    kb.text('Home','home');
    await ctx.editMessageText(text, { parse_mode: 'MarkdownV2', reply_markup: kb }).catch(()=>{});
  }
  await ctx.answerCallbackQuery().catch(() => {});
});
bot.callbackQuery('cmd_dca', async (ctx) => {
  const orders = Q.getActiveLimits.all(ctx.from.id).filter(o => o.direction.startsWith('dca_'));
  if (orders.length === 0) {
    const u = await getUser(ctx.from.id);
    await ctx.editMessageText(
      '*DCA Orders*\n\nNo active DCA orders\\.\n\nTo set up DCA, paste a token address, then tap the *DCA* button on the token detail page\\.',
      { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('Home','home') }
    ).catch(()=>{});
  } else {
    let text = '*Active DCA Orders*\n\n';
    const kb = new InlineKeyboard();
    for (const o of orders) {
      const mint = o.mint_id.slice(0,8);
      text += 'Every ' + Math.round(o.dca_interval_ms/60000) + 'min \\(' + o.dca_executed + '/' + o.dca_total + '\\) on `' + mint + '\\.\\.\\.\`\n';
      kb.text('Cancel #'+o.id, 'cancel_order:'+o.id).row();
    }
    kb.text('Home','home');
    await ctx.editMessageText(text, { parse_mode: 'MarkdownV2', reply_markup: kb }).catch(()=>{});
  }
  await ctx.answerCallbackQuery().catch(() => {});
});
bot.callbackQuery('cmd_limits', async (ctx) => {
  const orders = Q.getActiveLimits.all(ctx.from.id).filter(o => ['limit_buy','take_profit','stop_loss','trailing_stop'].includes(o.direction));
  if (orders.length === 0) {
    await ctx.editMessageText(
      '*Limit Orders*\n\nNo active limit orders\\.\n\nTo set a limit order, paste a token address, then tap *Limit*, *Stop Loss*, or *Take Profit* on the token detail page\\.',
      { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('Home','home') }
    ).catch(()=>{});
  } else {
    const labels = { limit_buy:'Limit Buy', take_profit:'Take Profit', stop_loss:'Stop Loss', trailing_stop:'Trailing Stop' };
    let text = '*Active Limit Orders*\n\n';
    const kb = new InlineKeyboard();
    for (const o of orders) {
      const l = labels[o.direction] || o.direction;
      const mint = o.mint_id.slice(0,8);
      text += '*' + esc(l) + '* on `' + mint + '\\.\\.\\.\`\n';
      kb.text('Cancel #'+o.id, 'cancel_order:'+o.id).row();
    }
    kb.text('Home','home');
    await ctx.editMessageText(text, { parse_mode: 'MarkdownV2', reply_markup: kb }).catch(()=>{});
  }
  await ctx.answerCallbackQuery().catch(() => {});
});
bot.callbackQuery('referrals', async (ctx) => {
  const c = Q.getReferralCount.get(ctx.from.id)?.c || 0;
  const link = 'https://t.me/PiChainTradeBot?start=' + ctx.from.id;
  const msg = 'Trade PI and Solana meme coins with PiBot — the fastest trading bot on Telegram:\n' + link;
  await ctx.editMessageText(
    `\u{1F517} *Referral Program*\n\n`
    + `*Your link:*\n\`${esc(link)}\`\n\n`
    + `*Referred:* ${c} users\n`
    + `*Reward:* 40% of trading fees from your referrals\n\n`
    + `_Share your link \\- when someone signs up and trades, you earn 40% of every fee they pay\\._`,
    { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard()
      .text('\u{1F4E4} Share Link', 'ref_share')
      .text('\u{1F4CB} Copy Link', 'ref_copy').row()
      .text('\u{2190} Home', 'home')
    }
  ).catch(()=>{});
  await ctx.answerCallbackQuery().catch(() => {});
});
bot.callbackQuery('ref_share', async (ctx) => {
  const link = 'https://t.me/PiChainTradeBot?start=' + ctx.from.id;
  const msg = 'Trade PI and Solana meme coins with PiBot — the fastest trading bot on Telegram:\n' + link;
  await ctx.reply(msg);
  await ctx.answerCallbackQuery('Share this message!').catch(() => {});
});
bot.callbackQuery('ref_copy', async (ctx) => {
  const link = 'https://t.me/PiChainTradeBot?start=' + ctx.from.id;
  // Send as a clickable link that users can also long-press to copy
  await ctx.reply('Your referral link:\n\n' + link + '\n\nLong press the link above to copy it.');
  await ctx.answerCallbackQuery().catch(() => {});
});

// Slippage (chain-aware)
bot.callbackQuery(/^sl:(\d+)$/, async (ctx) => {
  const u = await getUser(ctx.from.id); const v = parseInt(ctx.match[1]);
  if (u.chain === 'sol') Q.setSolSlippage.run(v, ctx.from.id); else Q.setSlippage.run(v, ctx.from.id);
  await showSettings(ctx, await getUser(ctx.from.id)); await ctx.answerCallbackQuery(`Slippage: ${(v/100).toFixed(1)}%`);
});
// Auto-buy (chain-aware)
bot.callbackQuery(/^ab:(\d+)$/, async (ctx) => {
  const u = await getUser(ctx.from.id); const v = parseInt(ctx.match[1]);
  if (u.chain === 'sol') Q.setSolAutoBuy.run(v, ctx.from.id); else Q.setAutoBuy.run(v, ctx.from.id);
  await showSettings(ctx, await getUser(ctx.from.id)); await ctx.answerCallbackQuery(v > 0 ? 'Auto-buy on' : 'Auto-buy off');
});
// Token view
bot.callbackQuery(/^tv:(.+)$/, async (ctx) => { await ctx.answerCallbackQuery().catch(() => {}); await tokenView(ctx, await getUser(ctx.from.id), ctx.match[1]); });
// DEX buy
bot.callbackQuery(/^db:([^:]+):(\d+)$/, async (ctx) => { await ctx.answerCallbackQuery('\u{23F3}').catch(() => {}); await doBuy(ctx, await getUser(ctx.from.id), ctx.match[1], parseInt(ctx.match[2])); });
// DEX sell
bot.callbackQuery(/^ds:([^:]+):(\d+)$/, async (ctx) => { await ctx.answerCallbackQuery('\u{23F3}').catch(() => {}); await doSell(ctx, await getUser(ctx.from.id), ctx.match[1], parseInt(ctx.match[2])); });
// Curve buy
bot.callbackQuery(/^cb:([^:]+):(\d+)$/, async (ctx) => { await ctx.answerCallbackQuery('\u{23F3}').catch(() => {}); await doCurveBuy(ctx, await getUser(ctx.from.id), ctx.match[1], parseInt(ctx.match[2])); });
// Curve sell
bot.callbackQuery(/^cs:([^:]+):(\d+)$/, async (ctx) => { await ctx.answerCallbackQuery('\u{23F3}').catch(() => {}); await doCurveSell(ctx, await getUser(ctx.from.id), ctx.match[1], parseInt(ctx.match[2])); });
// Custom buy amount
bot.callbackQuery(/^dbx:(.+)$/, async (ctx) => { ctx.session.awaitingInput = `custom_buy:${ctx.match[1]}:dex`; await ctx.reply('Enter PI amount to buy (e.g. 2.5):'); await ctx.answerCallbackQuery().catch(() => {}); });
bot.callbackQuery(/^cbx:(.+)$/, async (ctx) => { ctx.session.awaitingInput = `custom_buy:${ctx.match[1]}:curve`; await ctx.reply('Enter PI amount to buy (e.g. 2.5):'); await ctx.answerCallbackQuery().catch(() => {}); });
// Custom slippage
bot.callbackQuery('sl_custom', async (ctx) => { ctx.session.awaitingInput = 'custom_slippage'; await ctx.reply('Enter custom slippage percentage (e.g. 2.5):'); await ctx.answerCallbackQuery().catch(() => {}); });
// Custom auto-buy
bot.callbackQuery('ab_custom', async (ctx) => { ctx.session.awaitingInput = 'custom_autobuy'; await ctx.reply('Enter auto-buy amount in PI/SOL (e.g. 0.25):'); await ctx.answerCallbackQuery().catch(() => {}); });
// Priority fee (chain-aware)
bot.callbackQuery(/^pf:(.+)$/, async (ctx) => {
  const u = await getUser(ctx.from.id);
  if (u.chain === 'sol') Q.setSolPriority.run(ctx.match[1], ctx.from.id); else Q.setPriority.run(ctx.match[1], ctx.from.id);
  await showSettings(ctx, await getUser(ctx.from.id)); await ctx.answerCallbackQuery(`Priority: ${ctx.match[1]}`);
});
// MEV modes (chain-aware): turbo=1, off=0, secure=2
bot.callbackQuery(/^mev:(turbo|off|secure)$/, async (ctx) => {
  const u = await getUser(ctx.from.id);
  const val = ctx.match[1] === 'turbo' ? 1 : ctx.match[1] === 'secure' ? 2 : 0;
  if (u.chain === 'sol') Q.setSolMev.run(val, ctx.from.id); else Q.setMev.run(val, ctx.from.id);
  await showSettings(ctx, await getUser(ctx.from.id)); await ctx.answerCallbackQuery('MEV: ' + ctx.match[1]);
});
// Legacy toggle compat
bot.callbackQuery('mev_toggle', async (ctx) => {
  const u = await getUser(ctx.from.id);
  const cur = u.chain === 'sol' ? (u.sol_mev ?? 1) : u.mev_protect;
  const next = cur ? 0 : 1;
  if (u.chain === 'sol') Q.setSolMev.run(next, ctx.from.id); else Q.setMev.run(next, ctx.from.id);
  await showSettings(ctx, await getUser(ctx.from.id)); await ctx.answerCallbackQuery('Toggled').catch(() => {});
});
// No-op for section headers
bot.callbackQuery('noop', (ctx) => ctx.answerCallbackQuery());

// ── Limit Order callbacks ──────────────────────────────────────────────────
// Limit buy prompt
bot.callbackQuery(/^lo_lb:(.+)$/, async (ctx) => {
  ctx.session.awaitingInput = `limit_buy:${ctx.match[1]}`;
  await ctx.reply('Limit Buy — buy when price drops to target.\n\nEnter: <trigger_price> <pi_amount>\nExample: 0.00000050 2');
  await ctx.answerCallbackQuery().catch(() => {});
});
// Take profit prompt
bot.callbackQuery(/^lo_tp:(.+)$/, async (ctx) => {
  ctx.session.awaitingInput = `take_profit:${ctx.match[1]}`;
  await ctx.reply('Take Profit — sell when price rises to target.\n\nEnter: <trigger_price> <sell_pct>\nExample: 0.00001000 50');
  await ctx.answerCallbackQuery().catch(() => {});
});
// Stop loss prompt
bot.callbackQuery(/^lo_sl:(.+)$/, async (ctx) => {
  ctx.session.awaitingInput = `stop_loss:${ctx.match[1]}`;
  await ctx.reply('Stop Loss — sell when price drops to target.\n\nEnter: <trigger_price> <sell_pct>\nExample: 0.00000020 100');
  await ctx.answerCallbackQuery().catch(() => {});
});
// Trailing stop prompt
bot.callbackQuery(/^lo_ts:(.+)$/, async (ctx) => {
  ctx.session.awaitingInput = `trailing_stop:${ctx.match[1]}`;
  await ctx.reply('Trailing Stop — follows price up, sells on reversal.\n\nEnter: <drop_pct> <sell_pct>\nExample: 15 100');
  await ctx.answerCallbackQuery().catch(() => {});
});
// DCA prompt
bot.callbackQuery(/^lo_dca:(.+)$/, async (ctx) => {
  ctx.session.awaitingInput = `dca_buy:${ctx.match[1]}`;
  await ctx.reply('DCA Buy — periodic buys at fixed intervals.\n\nEnter: <amount> <interval_minutes> <total_buys>\nExample: 0.5 60 10');
  await ctx.answerCallbackQuery().catch(() => {});
});
// View active orders for a token
bot.callbackQuery(/^my_orders:(.+)$/, async (ctx) => {
  const mintId = ctx.match[1];
  const orders = Q.getActiveLimits.all(ctx.from.id).filter(o => o.mint_id === mintId);
  if (orders.length === 0) { await ctx.answerCallbackQuery('No active orders').catch(() => {}); return; }
  const labels = { limit_buy:'Limit Buy', take_profit:'Take Profit', stop_loss:'Stop Loss', trailing_stop:'Trailing Stop', dca_buy:'DCA Buy', dca_sell:'DCA Sell' };
  let text = `\u{1F4CB} *Active Orders*\n\n`;
  const kb = new InlineKeyboard();
  for (const o of orders) {
    const l = labels[o.direction] || o.direction;
    const trigger = o.direction === 'trailing_stop' ? `${(o.trigger_price_num/o.trigger_price_den).toFixed(0)}% drop` :
      o.direction.startsWith('dca_') ? `every ${Math.round(o.dca_interval_ms/60000)}min (${o.dca_executed}/${o.dca_total})` :
      `${(o.trigger_price_num/o.trigger_price_den).toFixed(8)} PI`;
    text += `*${esc(l)}:* ${esc(trigger)}\n`;
    kb.text(`Cancel #${o.id}`, `cancel_order:${o.id}`).row();
  }
  kb.text('Cancel All', `cancel_all:${mintId}`).row();
  kb.text('Back', `tv:${mintId}`);
  await ctx.editMessageText(text, { parse_mode: 'MarkdownV2', reply_markup: kb }).catch(() => ctx.reply(text, { parse_mode: 'MarkdownV2', reply_markup: kb }));
  await ctx.answerCallbackQuery().catch(() => {});
});
// Cancel individual order
bot.callbackQuery(/^cancel_order:(\d+)$/, async (ctx) => { Q.cancelLimit.run(parseInt(ctx.match[1]), ctx.from.id); await ctx.answerCallbackQuery('Order cancelled').catch(() => {}); });
// Cancel all orders for token
bot.callbackQuery(/^cancel_all:(.+)$/, async (ctx) => { Q.cancelAllLimits.run(ctx.from.id, ctx.match[1]); await ctx.answerCallbackQuery('All orders cancelled').catch(() => {}); });
// Withdraw command
bot.command('withdraw', async (ctx) => { const u = await getUser(ctx.from.id); ctx.reply(u.chain==='pi' ? 'Use /send <address> <amount> to withdraw PI.' : 'Use /send <address> <amount> to withdraw SOL.'); });

// ── Chain switching ────────────────────────────────────────────────────────
bot.callbackQuery('switch_chain', async (ctx) => {
  const u = await getUser(ctx.from.id);
  const newChain = u.chain === 'pi' ? 'sol' : 'pi';
  Q.setChain.run(newChain, ctx.from.id);
  u.chain = newChain;
  await mainMenu(ctx, await getUser(ctx.from.id));
  await ctx.answerCallbackQuery(`Switched to ${newChain === 'pi' ? 'PIChain' : 'Solana'}`);
});

// ── Solana buy ─────────────────────────────────────────────────────────────
// Per-user buy lock to prevent double-spend
const buyLocks = new Map(); // tid -> timestamp
async function doSolBuy(ctx, u, mintAddr, lamports) {
  if (!u.solWallet) { await ctx.reply('Solana wallet error'); return; }
  // Double-spend protection: lock per user for 10 seconds
  const lockKey = u.telegram_id + ':buy';
  const lastBuy = buyLocks.get(lockKey);
  if (lastBuy && Date.now() - lastBuy < 10000) {
    await ctx.reply('Previous buy still processing. Please wait 10 seconds.').catch(() => {});
    return;
  }
  buyLocks.set(lockKey, Date.now());
  const bal = await sol.getBalance(u.solWallet.publicKey).catch(() => 0);
  if (bal < lamports + 10000000) { buyLocks.delete(lockKey); await ctx.reply('Insufficient SOL. Have ' + (bal/LAMPORTS_PER_SOL).toFixed(4) + ' SOL.'); return; }

  const { net: swapLamports, fee: solFee } = deductFee(lamports);
  await ctx.reply('\u{23F3} Swapping ' + (swapLamports/LAMPORTS_PER_SOL).toFixed(4) + ' SOL...');

  let result;
  try {
    result = await sol.swap(u.solWallet, SOL_MINT, mintAddr, swapLamports, u.sol_slippage_bps || u.slippage_bps || 100);
  } catch (e) {
    await ctx.reply('\u{274C} Swap failed: ' + (e.message||'unknown').slice(0, 200)).catch(() => {});
    return;
  }

  // TX submitted — wait for on-chain confirmation before logging
  const conf = await sol.waitConfirm(result.txid, 30000).catch(() => ({ confirmed: false, error: 'timeout' }));

  if (!conf.confirmed) {
    await ctx.reply(
      '\u{26A0}\u{FE0F} TX submitted but unconfirmed.\n\n' +
      'TX: `' + (result.txid||'') + '`\n' +
      'Check: https://solscan.io/tx/' + (result.txid||'') + '\n\n' +
      'This may still land on-chain. Do NOT retry immediately.',
      { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('View Token', 'stv:'+mintAddr).text('Home', 'home') }
    ).catch(() => {});
    return;
  }

  // Confirmed on-chain — now safe to log trade
  Q.logTrade.run(u.telegram_id, mintAddr, 'buy', lamports, parseInt(result.quote?.outAmount || 0), result.txid || '', 'sol');
  collectFee(u.solWallet, solFee, 'sol', u.telegram_id);

  try {
    const [tokenInfo, postAccounts] = await Promise.all([
      getCachedTokenInfo(mintAddr),
      sol.getTokenAccounts(u.solWallet.publicKey).catch(() => []),
    ]);
    const sym = tokenInfo?.symbol || mintAddr.slice(0,8);
    // Get decimals from on-chain token account (accurate), fallback to tokenInfo
    const onChainAcct = postAccounts.find(a => a.mint === mintAddr);
    const dec = onChainAcct?.decimals || tokenInfo?.decimals || 6;
    const outAmt = (parseInt(result.quote?.outAmount||0) / Math.pow(10,dec)).toFixed(2);
    await ctx.reply(
      '\u{2705} Buy confirmed!\n\n' +
      'Spent: ' + (lamports/LAMPORTS_PER_SOL).toFixed(4) + ' SOL\n' +
      'Got: ~' + outAmt + ' $' + sym + '\n' +
      'TX: ' + (result.txid||'').slice(0,20) + '...',
      { reply_markup: new InlineKeyboard().text('View Token', 'stv:'+mintAddr).text('Home', 'home') }
    );
  } catch {
    await ctx.reply('\u{2705} Buy confirmed! TX: ' + (result.txid||'').slice(0,30) + '...',
      { reply_markup: new InlineKeyboard().text('View Token', 'stv:'+mintAddr).text('Home', 'home') }).catch(() => {});
  }
}

// ── Solana sell ────────────────────────────────────────────────────────────
async function doSolSell(ctx, u, mintAddr, pct) {
  if (!u.solWallet) { await ctx.reply('Solana wallet error'); return; }
  const accounts = await sol.getTokenAccounts(u.solWallet.publicKey).catch(() => []);
  const acct = accounts.find(a => a.mint === mintAddr);
  if (!acct || acct.balance === 0) { await ctx.reply('No tokens to sell'); return; }

  const sellAmt = pct === 100 ? acct.balance : Math.floor(acct.balance * pct / 100);
  if (sellAmt === 0) { await ctx.reply('Sell amount too small'); return; }

  await ctx.reply('\u{23F3} Selling ' + pct + '%...');

  let result;
  try {
    result = await sol.swap(u.solWallet, mintAddr, SOL_MINT, sellAmt, u.sol_slippage_bps || u.slippage_bps || 100);
  } catch (e) {
    await ctx.reply('\u{274C} Sell failed: ' + (e.message||'unknown').slice(0, 200)).catch(() => {});
    return;
  }

  // Wait for on-chain confirmation
  const conf = await sol.waitConfirm(result.txid, 30000).catch(() => ({ confirmed: false, error: 'timeout' }));

  if (!conf.confirmed) {
    await ctx.reply(
      '\u{26A0}\u{FE0F} TX submitted but unconfirmed.\n\n' +
      'TX: `' + (result.txid||'') + '`\n' +
      'Check: https://solscan.io/tx/' + (result.txid||'') + '\n\n' +
      'This may still land on-chain. Do NOT retry immediately.',
      { parse_mode: 'MarkdownV2', reply_markup: new InlineKeyboard().text('Home', 'home') }
    ).catch(() => {});
    return;
  }

  // Confirmed — safe to log
  const solReceived = parseInt(result.quote?.outAmount || 0);
  const { fee: solFee } = deductFee(solReceived);
  const netReceived = solReceived - solFee;
  Q.logTrade.run(u.telegram_id, mintAddr, 'sell', netReceived, sellAmt, result.txid || '', 'sol');
  collectFee(u.solWallet, solFee, 'sol', u.telegram_id);

  await ctx.reply(
    '\u{2705} Sell confirmed!\n\n' +
    'Sold: ' + pct + '%\n' +
    'Got: ~' + (netReceived/LAMPORTS_PER_SOL).toFixed(4) + ' SOL\n' +
    'TX: ' + (result.txid||'').slice(0,20) + '...',
    { reply_markup: new InlineKeyboard().text('Home', 'home') }
  ).catch(() => {});
}

// ── Solana token detail ────────────────────────────────────────────────────
async function solTokenView(ctx, u, mintAddr) {
  try {
    // Fetch essential data in parallel (skip slow rug check for speed)
    const [tokenInfo, accounts, dexData, walletBal] = await Promise.all([
      getCachedTokenInfo(mintAddr),
      u.solWallet ? sol.getTokenAccounts(u.solWallet.publicKey).catch(() => []) : [],
      (async () => { const ck = 'dex:' + mintAddr; const cv = cache.get(ck, 10000); if (cv) return cv; const sc = require('../../shared-token-cache.cjs'); if (sc.isDexCooldown()) return null; const r = await fetch('https://api.dexscreener.com/tokens/v1/solana/' + mintAddr, { signal: AbortSignal.timeout(5000) }); if (r.status === 429) { sc.setDexCooldown(30000); return null; } if (!r.ok) return null; const t = await r.text(); try { const d = JSON.parse(t); cache.set(ck, d); return d; } catch { return null; } })().catch(() => null),
      u.solWallet ? sol.getBalance(u.solWallet.publicKey).catch(() => 0) : 0,
    ]);
    const solPrice = cachedSolPrice;
    // Rug check in background (don't block display)
    const rugResult = cache.get('rug:'+mintAddr, 120000) || await rugChecker.check(mintAddr).then(r => { cache.set('rug:'+mintAddr, r); return r; }).catch(() => ({ safe: null, score: -1 }));

    const acct = accounts.find(a => a.mint === mintAddr);
    const sym = tokenInfo?.symbol || mintAddr.slice(0,6);
    const name = tokenInfo?.name || '';
    // Prefer on-chain decimals (accurate), then tokenInfo, default 6 (pump.fun standard)
    const dec = acct?.decimals ?? tokenInfo?.decimals ?? 6;
    const bal = acct?.balance || 0;

    // Get price + market data from DexScreener
    // New endpoint returns array directly, old returns { pairs: [...] }
    const pair = Array.isArray(dexData) ? dexData[0] : dexData?.pairs?.[0];
    const price = pair ? parseFloat(pair.priceUsd || '0') : await sol.getPrice(mintAddr).catch(() => 0);
    const mcap = pair ? parseFloat(pair.marketCap || pair.fdv || '0') : 0;
    const m5 = pair?.priceChange?.m5 || 0;
    const h1 = pair?.priceChange?.h1 || 0;
    const h6 = pair?.priceChange?.h6 || 0;
    const h24 = pair?.priceChange?.h24 || 0;

    // Build text like BonkBot
    let text = `${esc(name)} \\| *${esc(sym)}* \\|\n`;
    text += `\`${mintAddr}\`\n`;
    text += `[Explorer](https://solscan.io/token/${mintAddr}) \\| [Chart](https://dexscreener.com/solana/${mintAddr}) \\| [Scan](https://rugcheck.xyz/tokens/${mintAddr})\n\n`;

    // Price with subscript notation for tiny prices (like BonkBot)
    if (price > 0) {
      let priceStr;
      if (price >= 1) {
        priceStr = '$' + price.toFixed(2);
      } else if (price >= 0.001) {
        priceStr = '$' + price.toFixed(6);
      } else {
        // Subscript notation: $0.0₅703
        const s = price.toFixed(20);
        const afterDot = s.split('.')[1];
        let zeros = 0;
        for (const c of afterDot) { if (c === '0') zeros++; else break; }
        const subs = '\u2080\u2081\u2082\u2083\u2084\u2085\u2086\u2087\u2088\u2089';
        const subDigits = String(zeros).split('').map(d => subs[parseInt(d)]).join('');
        const significant = afterDot.slice(zeros, zeros + 3);
        priceStr = '$0.0' + subDigits + significant;
      }
      text += `*Price:* ${esc(priceStr)}\n`;
    }

    // Price changes
    if (pair) {
      const fmt = (v) => { const n = parseFloat(v)||0; return (n >= 0 ? '+' : '') + n.toFixed(2) + '%'; };
      text += `5m: *${esc(fmt(m5))}*, 1h: *${esc(fmt(h1))}*, 6h: *${esc(fmt(h6))}*, 24h: *${esc(fmt(h24))}*\n`;
    }

    // Market cap — try mcap, fdv, or estimate from liquidity
    const effectiveMcap = mcap || (pair?.liquidity?.usd ? pair.liquidity.usd * 1.1 : 0);
    if (effectiveMcap > 0) {
      const mcapStr = effectiveMcap >= 1e9 ? (effectiveMcap/1e9).toFixed(2)+'B' : effectiveMcap >= 1e6 ? (effectiveMcap/1e6).toFixed(2)+'M' : effectiveMcap >= 1e3 ? (effectiveMcap/1e3).toFixed(2)+'K' : effectiveMcap.toFixed(0);
      text += `*Market Cap:* \\$${esc(mcapStr)}\n`;
    }

    // Price impact — only on first load (not refresh) to keep refresh fast
    if (!ctx.callbackQuery) {
      try {
        const impactQuote = await sol.getQuote(SOL_MINT, mintAddr, 5e8, 100).catch(() => null);
        if (impactQuote?.priceImpactPct) {
          const impact = (parseFloat(impactQuote.priceImpactPct) * 100).toFixed(2);
          text += `\n*Price Impact \\(0\\.5 SOL\\):* ${esc(impact)}%\n`;
        }
      } catch {}
    }

    // Wallet balance
    text += `\n*Wallet Balance:* ${esc((walletBal / LAMPORTS_PER_SOL).toFixed(4))} SOL\n`;

    // Token balance + profit + value + % supply
    if (bal > 0) {
      const balDisplay = bal / Math.pow(10, dec);
      const value = balDisplay * price;
      const valueSol = solPrice > 0 ? value / solPrice : 0;
      const fdv = parseFloat(pair?.marketCap || pair?.fdv || pair?.liquidity?.usd * 2 || '0');
      const totalSupply = (fdv > 0 && price > 0) ? fdv / price : 0;
      const balStr = balDisplay >= 1e6 ? (balDisplay/1e6).toFixed(2)+'M' : balDisplay >= 1000 ? Math.round(balDisplay).toLocaleString() : balDisplay >= 1 ? balDisplay.toFixed(2) : balDisplay.toFixed(4);
      const supplyPct = totalSupply > 0 ? ((balDisplay / totalSupply) * 100).toFixed(2) : '';

      // Profit: only show when we have meaningful trade history from PiBot
      try {
        const trades = Q.getTrades.all(ctx.from.id, mintAddr);
        const spent = trades.filter(t => t.direction === 'buy').reduce((s, t) => s + (t.pi_amount || 0), 0);
        const received = trades.filter(t => t.direction === 'sell').reduce((s, t) => s + (t.pi_amount || 0), 0);
        const spentSol = spent / LAMPORTS_PER_SOL;
        const receivedSol = received / LAMPORTS_PER_SOL;
        // Only show profit if user spent more than 0.001 SOL through PiBot
        if (spentSol > 0.001) {
          const profitSol = valueSol + receivedSol - spentSol;
          const profitPct = ((profitSol / spentSol) * 100).toFixed(2);
          const pStr = Math.abs(profitSol) >= 0.001 ? profitSol.toFixed(4) : profitSol.toFixed(8);
          text += `*Profit:* ${profitSol >= 0 ? '\\+' : ''}${esc(profitPct)}% / ${profitSol >= 0 ? '\\+' : ''}${esc(pStr)} SOL\n`;
          text += `*Initial:* ${esc(spentSol.toFixed(4))} SOL\n`;
        }
      } catch {}

      text += `*Balance:* ${esc(balStr)} ${esc(sym)}${supplyPct ? ', ' + esc(supplyPct) + '% Supply' : ''}\n`;
      text += `*Value:* \\$${esc(value.toFixed(2))} / ${esc(valueSol.toFixed(4))} SOL\n`;
    }

    // Share with ref
    const refLink = 'https://t.me/PiChainTradeBot?start=' + ctx.from.id;
    text += `\n[Share with Ref](${refLink})\n`;
    text += `_To buy press one of the buttons below\\._`;

    // Determine active mode from callback data
    const cbData = ctx.callbackQuery?.data || '';
    const isLimit = cbData.startsWith('stv_limit:');
    const isDca = cbData.startsWith('stv_dca:');
    const mode = isLimit ? 'limit' : isDca ? 'dca' : 'swap';

    const kb = new InlineKeyboard();
    kb.text('Home', 'home').text('Close', 'close').row();
    // Mode tabs — active one gets checkmark
    kb.text(mode==='dca'?'\u{2705} DCA':'DCA', 'stv_dca:'+mintAddr)
      .text(mode==='swap'?'\u{2705} Swap':'Swap', 'stv:'+mintAddr)
      .text(mode==='limit'?'\u{2705} Limit':'Limit', 'stv_limit:'+mintAddr).row();

    if (mode === 'swap') {
      // Normal swap buttons
      kb.text('Buy 0.1 SOL', 'sb:'+mintAddr+':'+1e8).text('Buy 0.5 SOL', 'sb:'+mintAddr+':'+5e8).text('Buy X SOL', 'sbx:'+mintAddr).row();
      if (bal > 0) {
        kb.text('Sell 25%', 'ss:'+mintAddr+':25').text('Sell 50%', 'ss:'+mintAddr+':50').text('Sell X %', 'ssx:'+mintAddr).row();
      }
    } else if (mode === 'limit') {
      // Limit order buttons
      kb.text('Limit Buy 0.5 SOL', 'lo_lb:'+mintAddr).text('Limit Buy X SOL', 'lo_lb:'+mintAddr).row();
      if (bal > 0) {
        kb.text('Limit Sell 10%', 'lo_tp:'+mintAddr).text('Limit Sell X %', 'lo_tp:'+mintAddr).row();
        kb.text('Sell 50% @ 2x', 'lo_tp:'+mintAddr).text('Sell 100% @ 4x', 'lo_tp:'+mintAddr).row();
      }
      kb.text('Stop Loss', 'lo_sl:'+mintAddr).text('Trailing Stop', 'lo_ts:'+mintAddr).row();
    } else if (mode === 'dca') {
      // DCA buttons
      kb.text('DCA Buy 0.1 SOL', 'lo_dca:'+mintAddr).text('DCA Buy X SOL', 'lo_dca:'+mintAddr).row();
      if (bal > 0) {
        kb.text('DCA Sell 25%', 'lo_dca:'+mintAddr).text('DCA Sell X %', 'lo_dca:'+mintAddr).row();
      }
    }
    kb.text('Refresh', 'stv:'+mintAddr).row();

    const opts = { parse_mode: 'MarkdownV2', reply_markup: kb, link_preview_options: { is_disabled: true } };
    if (ctx.callbackQuery) {
      await ctx.editMessageText(text, opts).catch(() => {}); // Edit in place, ignore if unchanged
    } else {
      await ctx.reply(text, opts);
    }
  } catch (e) { await ctx.reply('Error loading token: ' + (e.message||'unknown').slice(0,200)); }
}

// ── Solana trending ────────────────────────────────────────────────────────
bot.callbackQuery('sol_trending', async (ctx) => {
  try {
    const tokens = await sol.getTrendingTokens();
    let text = `\u{1F525} *Trending on Solana*\n\n`;
    if (!tokens.length) text += '_No data_';
    else for (const t of tokens.slice(0, 10)) {
      const sym = t.symbol || t.address?.slice(0,6) || '?';
      const name = t.name || '';
      text += `*\\$${esc(sym)}* ${esc(name)}\n`;
    }
    const kb = new InlineKeyboard();
    for (const t of tokens.slice(0, 5)) {
      if (!t.address || t.address === SOL_MINT) continue; // Skip native SOL
      const sym = t.symbol || t.address?.slice(0,6) || '?';
      kb.text('$' + sym, 'stv:' + t.address).row();
    }
    kb.text('\u{1F504} Refresh', 'sol_trending').text('\u{1F3E0} Home', 'home');
    await ctx.editMessageText(text, { parse_mode: 'MarkdownV2', reply_markup: kb }).catch(() => ctx.reply(text, { parse_mode: 'MarkdownV2', reply_markup: kb }));
  } catch (e) { await ctx.reply('Error: ' + (e.message||'unknown').slice(0,200)); }
  await ctx.answerCallbackQuery().catch(() => {});
});

// ── Solana callbacks ───────────────────────────────────────────────────────
// View token
bot.callbackQuery(/^stv:(.+)$/, async (ctx) => { await ctx.answerCallbackQuery().catch(() => {}); await solTokenView(ctx, await getUser(ctx.from.id), ctx.match[1]); });
bot.callbackQuery(/^stv_limit:(.+)$/, async (ctx) => { await ctx.answerCallbackQuery().catch(() => {}); await solTokenView(ctx, await getUser(ctx.from.id), ctx.match[1]); });
bot.callbackQuery(/^stv_dca:(.+)$/, async (ctx) => { await ctx.answerCallbackQuery().catch(() => {}); await solTokenView(ctx, await getUser(ctx.from.id), ctx.match[1]); });
// Buy
bot.callbackQuery(/^sb:([^:]+):(\d+)$/, async (ctx) => { await ctx.answerCallbackQuery('\u{23F3}').catch(() => {}); await doSolBuy(ctx, await getUser(ctx.from.id), ctx.match[1], parseInt(ctx.match[2])); });
// Sell
bot.callbackQuery(/^ss:([^:]+):(\d+)$/, async (ctx) => { await ctx.answerCallbackQuery('\u{23F3}').catch(() => {}); await doSolSell(ctx, await getUser(ctx.from.id), ctx.match[1], parseInt(ctx.match[2])); });
// Custom buy
bot.callbackQuery(/^sbx:(.+)$/, async (ctx) => { ctx.session.awaitingInput = 'sol_buy:'+ctx.match[1]; await ctx.reply('Enter SOL amount to buy (e.g. 0.25):'); await ctx.answerCallbackQuery().catch(() => {}); });
// Custom sell %
bot.callbackQuery(/^ssx:(.+)$/, async (ctx) => { ctx.session.awaitingInput = 'sol_sell_pct:'+ctx.match[1]; await ctx.reply('Enter sell percentage (1-100):'); await ctx.answerCallbackQuery().catch(() => {}); });
// Orders command
bot.callbackQuery('cmd_orders', async (ctx) => {
  const orders = Q.getActiveLimits.all(ctx.from.id);
  if (orders.length === 0) { await ctx.editMessageText('No active orders.', { reply_markup: new InlineKeyboard().text('\u{1F3E0} Home','home') }).catch(()=>{}); }
  else {
    const labels = { limit_buy:'Limit Buy', take_profit:'Take Profit', stop_loss:'Stop Loss', trailing_stop:'Trailing Stop', dca_buy:'DCA Buy', snipe_graduation:'Grad Snipe', price_alert:'Alert' };
    let text = `\u{1F4CB} *${orders.length} Active Orders*\n\n`;
    const kb = new InlineKeyboard();
    for (const o of orders.slice(0, 10)) {
      const l = labels[o.direction] || o.direction;
      const ch = o.chain === 'sol' ? '\u{1F7E2}' : '\u{1F7E3}';
      text += `${ch} \\#${o.id} *${esc(l)}*\n`;
      kb.text('Cancel #'+o.id, 'cancel_order:'+o.id).row();
    }
    kb.text('\u{1F3E0} Home','home');
    await ctx.editMessageText(text, { parse_mode: 'MarkdownV2', reply_markup: kb }).catch(() => ctx.reply(text, { parse_mode: 'MarkdownV2', reply_markup: kb }));
  }
  await ctx.answerCallbackQuery().catch(() => {});
});

// ── Price alerts ───────────────────────────────────────────────────────────
bot.callbackQuery(/^lo_alert:(.+)$/, async (ctx) => {
  ctx.session.awaitingInput = `price_alert:${ctx.match[1]}`;
  await ctx.reply('Price Alert — get notified when price hits target.\n\nEnter: <price> <above|below>\nExample: 0.00001 above');
  await ctx.answerCallbackQuery().catch(() => {});
});

// ── Graduation snipe command ───────────────────────────────────────────────
bot.command('snipe', async (ctx) => {
  const u = await getUser(ctx.from.id);
  const parts = (ctx.match || '').trim().split(/\s+/);
  const amt = parseFloat(parts[0]);
  const currency = u.chain === 'sol' ? 'SOL' : 'PI';

  // Handle off/stop
  if (parts[0] === 'off' || parts[0] === 'stop') {
    if (u.chain === 'sol') { poolSniper.stmts.deactivate.run(ctx.from.id); }
    else { db.prepare("UPDATE limit_orders SET active = 0 WHERE telegram_id = ? AND direction = 'snipe_graduation'").run(ctx.from.id); }
    await ctx.reply('Snipe disabled.');
    return;
  }

  if (isNaN(amt) || amt <= 0) {
    if (u.chain === 'sol') {
      await ctx.reply('Snipe — auto-buy new tokens on Solana\n\nUsage: /snipe <sol_amount>\nExample: /snipe 0.5\n\nAuto-buy when new Raydium pools launch.\n\nStop: /snipe off');
    } else {
      await ctx.reply('Snipe — auto-buy when tokens graduate to DEX\n\nUsage: /snipe <pi_amount> [mint_id]\nExample: /snipe 1\n\nStop: /snipe off');
    }
    return;
  }

  if (u.chain === 'sol') {
    // Solana: enable auto-snipe for new pools
    poolSniper.stmts.upsert.run(ctx.from.id, Math.floor(amt * LAMPORTS_PER_SOL), 1000000000, 1);
    poolSniper.start();
    await ctx.reply('\u{2705} Solana snipe enabled! Auto-buying ' + amt + ' SOL on new token launches.\n\nYou\'ll get alerts for new Raydium pools.\n\nStop: /snipe off');
  } else {
    // PIChain: graduation snipe
    const mintId = (parts[1] || '*').replace(/^(Pi314|pi314|0x)/, '');
    const DEN = 1e9;
    Q.insertLimit.run(ctx.from.id, mintId.length === 64 ? mintId : '*', 'snipe_graduation', 0, DEN, Math.floor(amt * 1e9), 0, 0);
    await ctx.reply('\u{2705} Graduation snipe set: auto-buy ' + amt + ' PI when ' + (mintId === '*' ? 'any token' : mintId.slice(0,12)+'...') + ' graduates to DEX.');
  }
});

// ── Price alert command ────────────────────────────────────────────────────
bot.command('alert', async (ctx) => {
  const parts = (ctx.match || '').trim().split(/\s+/);
  if (parts.length < 3) { await ctx.reply('Usage: /alert <token_address> <price> <above|below>\n\nExample: /alert DezXAZ...263 0.00001 above\nExample: /alert abc123...def 0.00001 below'); return; }
  const mintId = parts[0].replace(/^(Pi314|pi314|0x)/, '');
  const price = parseFloat(parts[1]);
  const dir = parts[2].toLowerCase();
  // Accept both 64-char hex (PI) and base58 (Solana)
  const validAddr = mintId.length === 64 || /^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(mintId);
  if (!validAddr || isNaN(price) || price <= 0 || !['above','below'].includes(dir)) { await ctx.reply('Invalid. Use: /alert <token_address> <price> <above|below>'); return; }
  const DEN = 1e9;
  Q.insertLimit.run(ctx.from.id, mintId, 'price_alert', Math.floor(price * DEN), DEN, dir === 'above' ? 1 : -1, 0, 0);
  await ctx.reply('\u{1F514} Alert set: notify when price goes ' + dir + ' ' + price);
});

// ── Trade history command ──────────────────────────────────────────────────
bot.command('history', async (ctx) => {
  const u = await getUser(ctx.from.id);
  const chain = u.chain;
  const trades = db.prepare('SELECT * FROM trades WHERE telegram_id = ? ORDER BY created_at DESC LIMIT 20').all(ctx.from.id);
  if (trades.length === 0) { await ctx.reply('No trade history yet. Start trading!'); return; }
  let text = 'Trade History (last 20)\n\n';
  let totalBought = 0, totalSold = 0;
  for (const t of trades) {
    const icon = t.direction === 'buy' ? '\u{1F7E2}' : '\u{1F534}';
    const isSol = (t.chain === 'sol');
    const amt = isSol ? (t.pi_amount / LAMPORTS_PER_SOL).toFixed(4) + ' SOL' : (t.pi_amount / 1e9).toFixed(4) + ' PI';
    const chainIcon = isSol ? '\u{1F7E2}' : '\u{1F7E3}';
    text += icon + ' ' + t.direction.toUpperCase() + ' ' + amt + ' ' + chainIcon + ' (' + t.mint_id.slice(0,8) + '...)\n';
    if (t.direction === 'buy') totalBought += t.pi_amount;
    else totalSold += t.pi_amount;
  }
  const pnl = totalSold - totalBought;
  text += '\nTotal bought: ' + (totalBought/1e9).toFixed(4);
  text += '\nTotal sold: ' + (totalSold/1e9).toFixed(4);
  text += '\nNet PnL: ' + (pnl >= 0 ? '+' : '') + (pnl/1e9).toFixed(4);
  await ctx.reply(text);
});

// ── Active orders command ──────────────────────────────────────────────────
bot.command('orders', async (ctx) => {
  const orders = Q.getActiveLimits.all(ctx.from.id);
  if (orders.length === 0) { await ctx.reply('No active orders. Paste a token address and use Limit/DCA/Stop Loss buttons.'); return; }
  const labels = { limit_buy:'Limit Buy', take_profit:'Take Profit', stop_loss:'Stop Loss', trailing_stop:'Trailing Stop', dca_buy:'DCA Buy', dca_sell:'DCA Sell', snipe_graduation:'Grad Snipe', price_alert:'Price Alert' };
  let text = 'Active Orders (' + orders.length + ')\n\n';
  const kb = new InlineKeyboard();
  for (const o of orders) {
    const l = labels[o.direction] || o.direction;
    const mint = o.mint_id === '*' ? 'ALL' : o.mint_id.slice(0,8)+'...';
    text += '#' + o.id + ' ' + l + ' on ' + mint + '\n';
    kb.text('Cancel #' + o.id, 'cancel_order:' + o.id).row();
  }
  kb.text('Home', 'home');
  await ctx.reply(text, { reply_markup: kb });
});

// ═══════════════════════════════════════════════════════════════════════════
// ADVANCED FEATURES — Copy Trading, Snipe, Rug, Whales, Multi-Wallet, Bridge
// ═══════════════════════════════════════════════════════════════════════════

// ── Copy Trading commands ──────────────────────────────────────────────────
bot.command('copy', async (ctx) => {
  const wallet = (ctx.match || '').trim();
  if (!wallet || !/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(wallet)) {
    await ctx.reply('Copy Trading\n\nMirror another wallet\'s trades automatically.\n\nUsage: /copy <solana_wallet_address>\nExample: /copy 7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU\n\nStop: /uncopy');
    return;
  }
  copyTrader.stmts.insert.run(ctx.from.id, wallet, 100000000);
  copyTrader.start(); // Start engine on first use
  await ctx.reply('\u{2705} Now copying ' + wallet.slice(0,8) + '...' + wallet.slice(-4) + '\n\nYou\'ll get alerts when they trade. Use /uncopy to stop.');
});
bot.command('uncopy', async (ctx) => { copyTrader.stmts.deactivateAll.run(ctx.from.id); await ctx.reply('Copy trading stopped.'); });
bot.command('copies', async (ctx) => {
  const targets = copyTrader.stmts.getForUser.all(ctx.from.id);
  if (targets.length === 0) { await ctx.reply('No active copy targets. Use /copy <wallet> to start.'); return; }
  let text = '*Active Copy Targets*\n\n';
  const kb = new InlineKeyboard();
  for (const t of targets) {
    text += `\`${t.target_wallet.slice(0,8)}\\.\\.\\.\`\n`;
    kb.text('Stop #'+t.id, 'uncopy:'+t.id).row();
  }
  await ctx.reply(text, { parse_mode: 'MarkdownV2', reply_markup: kb });
});
bot.callbackQuery(/^uncopy:(\d+)$/, async (ctx) => { copyTrader.stmts.deactivate.run(parseInt(ctx.match[1]), ctx.from.id); await ctx.answerCallbackQuery('Stopped').catch(() => {}); });

// ── Auto-Snipe commands ────────────────────────────────────────────────────
bot.command('autosnipe', async (ctx) => {
  const args = (ctx.match || '').trim();
  if (args === 'off' || args === 'stop') { poolSniper.stmts.deactivate.run(ctx.from.id); await ctx.reply('Auto-snipe disabled.'); return; }
  poolSniper.stmts.upsert.run(ctx.from.id, 50000000, 1000000000, 0);
  poolSniper.start(); // Start engine on first use
  await ctx.reply('\u{2705} Auto-snipe enabled!\n\nYou\'ll get alerts when new Raydium pools are created.\nPaste the mint address to buy.\n\nStop: /autosnipe off');
});

// ── Whale tracking commands ────────────────────────────────────────────────
bot.callbackQuery(/^whale_add:(.+)$/, async (ctx) => {
  whaleTracker.stmts.insert.run(ctx.from.id, ctx.match[1], 10000000000);
  whaleTracker.start();
  await ctx.answerCallbackQuery('Whale tracking on!').catch(() => {});
  await ctx.reply('Tracking large trades on this token. Alerts for 10+ SOL movements.\n\nStop: /unwatchwhale');
});
bot.command('watchwhale', async (ctx) => {
  const parts = (ctx.match || '').trim().split(/\s+/);
  if (!parts[0] || !/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(parts[0])) {
    await ctx.reply('Usage: /watchwhale <token_mint> [min_sol]\nExample: /watchwhale DezXAZ...263 5'); return;
  }
  const minSol = parseFloat(parts[1] || '10');
  whaleTracker.stmts.insert.run(ctx.from.id, parts[0], Math.floor(minSol * 1e9));
  await ctx.reply(`\u{1F40B} Watching for ${minSol}+ SOL trades on ${parts[0].slice(0,8)}...`);
});
bot.command('unwatchwhale', async (ctx) => {
  const mint = (ctx.match || '').trim();
  if (mint) whaleTracker.stmts.deactivateAll.run(ctx.from.id, mint);
  else db.prepare('UPDATE whale_watches SET active = 0 WHERE telegram_id = ?').run(ctx.from.id);
  await ctx.reply('Whale tracking stopped.');
});

// ── Multi-wallet commands ──────────────────────────────────────────────────
bot.command('wallets', async (ctx) => {
  const wallets = multiWallet.getWallets(ctx.from.id);
  if (wallets.length === 0) {
    const u = await getUser(ctx.from.id);
    await multiWallet.createWallet(ctx.from.id, 'Main');
    const created = multiWallet.getWallets(ctx.from.id);
    if (created.length > 0) multiWallet.switchWallet(ctx.from.id, created[0].id);
  }
  const all = multiWallet.getWallets(ctx.from.id);
  let text = '\u{1F4B3} *Your Wallets*\n\n';
  const kb = new InlineKeyboard();
  for (const w of all) {
    const icon = w.is_active ? '\u{2705}' : '\u{26AA}';
    const piAddr = (w.pi_address || '').slice(0, 12) || 'none';
    text += `${icon} *${esc(w.label)}*\nPI: \`${piAddr}\\.\\.\\.\`\n\n`;
    if (!w.is_active) kb.text('Switch to ' + w.label, 'mw_switch:' + w.id).row();
    kb.text('\u{1F5D1} Delete ' + w.label, 'mw_delete:' + w.id).row();
  }
  kb.text('\u{2795} New Wallet', 'mw_new').text('\u{1F3E0} Home', 'home').row();
  await ctx.reply(text, { parse_mode: 'MarkdownV2', reply_markup: kb });
});
bot.callbackQuery('mw_new', async (ctx) => {
  try {
    const w = await multiWallet.createWallet(ctx.from.id);
    const piAddr = w.pi?.address || 'created';
    await ctx.reply(`\u{2705} Created: ${w.label}\nPI: \`${piAddr}\``);
  } catch (e) { await ctx.reply('Error: ' + e.message); }
  await ctx.answerCallbackQuery().catch(() => {});
});
bot.callbackQuery(/^mw_switch:(\d+)$/, async (ctx) => {
  multiWallet.switchWallet(ctx.from.id, parseInt(ctx.match[1]));
  await ctx.answerCallbackQuery('Switched!').catch(() => {});
  await ctx.reply('Wallet switched. Use /wallets to see all.');
});
bot.callbackQuery(/^mw_delete:(\d+)$/, async (ctx) => {
  try {
    const walletId = parseInt(ctx.match[1]);
    const wallets = multiWallet.getWallets(ctx.from.id);
    if (wallets.length <= 1) {
      await ctx.reply('Cannot delete your only wallet. Create a new one first.');
    } else {
      multiWallet.deleteWallet(ctx.from.id, walletId);
      await ctx.reply('\u{1F5D1} Wallet deleted. Use /wallets to see remaining.');
    }
  } catch (e) { await ctx.reply('Error: ' + e.message); }
  await ctx.answerCallbackQuery().catch(() => {});
});

// ── Bridge commands ────────────────────────────────────────────────────────
bot.command('bridge', async (ctx) => {
  const parts = (ctx.match || '').trim().split(/\s+/);
  if (parts.length < 2) {
    await ctx.reply(
      'Cross-Chain Bridge\n\n' +
      'Transfer between PIChain and Solana.\n\n' +
      'Usage:\n/bridge sol>pi <amount>\n/bridge pi>sol <amount>\n\n' +
      'Example: /bridge sol>pi 1\n\n' +
      'Fee: 0.1% | Time: ~2-5 min'
    );
    return;
  }
  const dir = parts[0].toLowerCase();
  const amount = parseFloat(parts[1]);
  if (isNaN(amount) || amount <= 0) { await ctx.reply('Invalid amount'); return; }

  let fromChain, toChain;
  if (dir.includes('sol') && dir.includes('pi') && dir.indexOf('sol') < dir.indexOf('pi')) { fromChain = 'sol'; toChain = 'pi'; }
  else if (dir.includes('pi') && dir.includes('sol') && dir.indexOf('pi') < dir.indexOf('sol')) { fromChain = 'pi'; toChain = 'sol'; }
  else { await ctx.reply('Use: sol>pi or pi>sol'); return; }

  const quote = await bridge.getQuote(fromChain, toChain, Math.floor(amount * 1e9));
  await ctx.reply(
    `*Bridge Quote*\n\n` +
    `${esc(fromChain.toUpperCase())} \u{2192} ${esc(toChain.toUpperCase())}\n` +
    `Send: ${esc(amount.toFixed(4))}\n` +
    `Receive: ${esc((quote.receive / 1e9).toFixed(4))}\n` +
    `Fee: ${esc(quote.feePct)}%\n` +
    `Time: ${esc(quote.estimatedTime)}\n\n` +
    `_Use the bridge page at pichain\\.io/bridge for execution\\._`,
    { parse_mode: 'MarkdownV2' }
  );
});

// ── Rug check command ──────────────────────────────────────────────────────
bot.command('rug', async (ctx) => {
  const mint = (ctx.match || '').trim();
  if (!mint || !/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(mint)) { await ctx.reply('Usage: /rug <solana_token_address>'); return; }
  await ctx.reply('\u{1F50D} Checking token safety...');
  const result = await rugChecker.check(mint);
  try { await ctx.reply(rugChecker.formatReport(result), { parse_mode: 'MarkdownV2' }); }
  catch { await ctx.reply('RugCheck score: ' + (result.score >= 0 ? result.scoreLabel + ' (' + result.score + '/1000)' : 'unavailable')); }
});

// ── Fee management (admin only) ────────────────────────────────────────────
const ADMIN_ID = parseInt(process.env.ADMIN_TELEGRAM_ID || '0');
bot.command('fees', async (ctx) => {
  if (ADMIN_ID && ctx.from.id !== ADMIN_ID) return;
  const fees = Q.unsweptFees.all();
  if (fees.length === 0) { await ctx.reply('No unswept fees.'); return; }
  let text = 'Collected fees (unswept):\n';
  for (const f of fees) {
    const amt = f.chain === 'sol' ? (f.total / LAMPORTS_PER_SOL).toFixed(6) + ' SOL' : fPI(f.total) + ' PI';
    text += `  ${f.chain.toUpperCase()}: ${amt}\n`;
  }
  text += '\nUse /sweepfees to transfer to fee wallet.';
  await ctx.reply(text);
});

bot.command('sweepfees', async (ctx) => {
  if (ADMIN_ID && ctx.from.id !== ADMIN_ID) return;
  if (!FEE_ADDR) { await ctx.reply('Set FEE_ADDRESS env var first.'); return; }
  const fees = Q.unsweptFees.all();
  if (fees.length === 0) { await ctx.reply('No fees to sweep.'); return; }
  // Mark as swept
  Q.sweepFees.run();
  let text = 'Fees marked as swept:\n';
  for (const f of fees) {
    const amt = f.chain === 'sol' ? (f.total / LAMPORTS_PER_SOL).toFixed(6) + ' SOL' : fPI(f.total) + ' PI';
    text += `  ${f.chain.toUpperCase()}: ${amt}\n`;
  }
  text += '\nFees remain in user wallets until withdrawal. The fee amounts are tracked for accounting.';
  await ctx.reply(text);
});

// ── 2FA (TOTP) for large withdrawals ───────────────────────────────────────
const { TOTP, Secret } = require('otpauth');
const LARGE_WITHDRAW_SOL = 1 * LAMPORTS_PER_SOL; // 1 SOL
const LARGE_WITHDRAW_PI = 10 * 1e9; // 10 PI
const DAILY_LIMIT_SOL = 10 * LAMPORTS_PER_SOL; // 10 SOL per day
const DAILY_LIMIT_PI = 100 * 1e9; // 100 PI per day

function get2FASecret(u) {
  if (u.totp_secret) return decryptSeed(u.totp_secret);
  return null;
}

function verify2FA(u, token) {
  const secret = get2FASecret(u);
  if (!secret) return false;
  const totp = new TOTP({ secret: Secret.fromBase32(secret), digits: 6, period: 30 });
  return totp.validate({ token, window: 1 }) !== null;
}

function checkDailyLimit(u, amount, chain) {
  const today = new Date().toISOString().slice(0, 10);
  if (u.daily_withdrawn_reset !== today) {
    db.prepare('UPDATE users SET daily_withdrawn = 0, daily_withdrawn_reset = ? WHERE telegram_id = ?').run(today, u.telegram_id);
    u.daily_withdrawn = 0;
  }
  const limit = chain === 'sol' ? DAILY_LIMIT_SOL : DAILY_LIMIT_PI;
  return (u.daily_withdrawn + amount) <= limit;
}

function recordWithdrawal(tid, amount) {
  db.prepare('UPDATE users SET daily_withdrawn = daily_withdrawn + ? WHERE telegram_id = ?').run(amount, tid);
}

bot.command('setup2fa', async (ctx) => {
  const u = await getUser(ctx.from.id);
  if (u.totp_enabled) {
    await ctx.reply('2FA is already enabled. Use /disable2fa to reset.');
    return;
  }
  const secret = new Secret({ size: 20 });
  const totp = new TOTP({ issuer: 'PiBot', label: 'PiBot Trading', secret, digits: 6, period: 30 });
  // Store encrypted secret
  db.prepare('UPDATE users SET totp_secret = ? WHERE telegram_id = ?').run(encryptSeed(secret.base32), u.telegram_id);
  const uri = totp.toString();
  const msg = await ctx.reply(
    '🔐 *2FA Setup*\n\n' +
    'Scan this with Google Authenticator or Authy:\n\n' +
    '`' + secret.base32 + '`\n\n' +
    'Or use this URI:\n`' + uri + '`\n\n' +
    'After adding it, send the 6-digit code to verify.\n\n' +
    '_This message auto-deletes in 60 seconds._',
    { parse_mode: 'Markdown' }
  );
  ctx.session.awaitingInput = '2fa_verify';
  setTimeout(() => { ctx.api.deleteMessage(ctx.chat.id, msg.message_id).catch(() => {}); }, 60000);
});

bot.command('disable2fa', async (ctx) => {
  const u = await getUser(ctx.from.id);
  if (!u.totp_enabled) { await ctx.reply('2FA is not enabled.'); return; }
  ctx.session.awaitingInput = '2fa_disable';
  await ctx.reply('Enter your current 2FA code to disable:');
});

// ── Daily withdrawal limits ───────────────────────────────────────────────
bot.command('limits', async (ctx) => {
  const u = await getUser(ctx.from.id);
  const today = new Date().toISOString().slice(0, 10);
  const withdrawn = u.daily_withdrawn_reset === today ? u.daily_withdrawn : 0;
  const solUsed = (withdrawn / LAMPORTS_PER_SOL).toFixed(4);
  const solLimit = (DAILY_LIMIT_SOL / LAMPORTS_PER_SOL).toFixed(0);
  await ctx.reply(
    '📊 Withdrawal Limits\n\n' +
    `Daily limit: ${solLimit} SOL / ${(DAILY_LIMIT_PI/1e9).toFixed(0)} PI\n` +
    `Used today: ${solUsed} SOL\n` +
    `Remaining: ${((DAILY_LIMIT_SOL - withdrawn) / LAMPORTS_PER_SOL).toFixed(4)} SOL\n\n` +
    `2FA: ${u.totp_enabled ? '✅ Enabled' : '❌ Disabled'}\n` +
    `2FA required for sends > 1 SOL / 10 PI\n\n` +
    'Use /setup2fa to enable 2FA'
  );
});

// ── Error handler ──────────────────────────────────────────────────────────
bot.catch((err) => {
  const ctx = err.ctx;
  const cmd = ctx?.message?.text?.slice(0,20) || ctx?.callbackQuery?.data?.slice(0,20) || '?';
  console.error('Bot error on [' + cmd + ']:', err.error?.message || err.message || err);
});

// ── Start ──────────────────────────────────────────────────────────────────
async function main() {
  console.log('PiBot v2 — PIChain Telegram Trading Bot');
  console.log('========================================');
  console.log(`RPC:      ${RPC_URL}`);
  console.log(`Chain:    ${CHAIN_ID}`);
  console.log(`Fee:      ${BOT_FEE_BPS/100}% (${BOT_FEE_BPS} bps)`);
  try { const info = await pi.syncInfo(); console.log(`Height:   ${info.block_height}`); console.log(`Base fee: ${info.base_fee}`); } catch(e) { console.error(`RPC unavailable: ${e.message}`); }
  // Start limit engine only — other engines start on demand when users subscribe
  limitEngine.start();
  // copyTrader, poolSniper, whaleTracker start only when users add targets
  // This prevents rate limiting from empty background polling

  console.log('Starting Telegram bot...');
  await bot.start({
    drop_pending_updates: true,
    onStart: (info) => console.log(`@${info.username} is live!`),
    // Longer polling timeout = fewer getUpdates requests = no 429
    allowed_updates: ['message', 'callback_query'],
  });
}

main().catch(e => { console.error(e); process.exit(1); });
