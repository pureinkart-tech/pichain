/**
 * Limit Order + DCA + Snipe Engine for PiBot.
 *
 * Order types:
 * - limit_buy:       Buy when price drops to target
 * - take_profit:     Sell when price rises to target
 * - stop_loss:       Sell when price drops to target
 * - trailing_stop:   Dynamic stop following price up, sells on reversal
 * - dca_buy/sell:    Periodic buys/sells at fixed intervals
 *
 * Features:
 * - Partial fills: if full amount fails (liquidity), tries 50%, then 25%
 * - Graduation snipe: auto-buy when bonding curve tokens graduate to DEX
 * - Price alerts: notify when price hits user target (no execution)
 */

const C = require('./crypto');
const NATIVE = C.NATIVE_PI_MINT;

class LimitOrderEngine {
  constructor(db, piClient, notifyFn) {
    this.db = db;
    this.pi = piClient;
    this.notify = notifyFn;
    this.running = false;
    this.pollMs = 15000;
    this.lastLaunches = new Map(); // mint_id -> state (for graduation detection)

    this.stmts = {
      getActive: db.prepare('SELECT * FROM limit_orders WHERE active = 1'),
      deactivate: db.prepare('UPDATE limit_orders SET active = 0 WHERE id = ?'),
      reduceAmount: db.prepare('UPDATE limit_orders SET amount = ? WHERE id = ?'),
      updateTrailingHigh: db.prepare('UPDATE limit_orders SET trailing_high_num = ? WHERE id = ?'),
      updateDcaNext: db.prepare('UPDATE limit_orders SET dca_next_at = ?, dca_executed = dca_executed + 1 WHERE id = ?'),
      getUser: db.prepare('SELECT * FROM users WHERE telegram_id = ?'),
      logTrade: db.prepare('INSERT INTO trades (telegram_id, mint_id, direction, pi_amount, token_amount, tx_hash) VALUES (?,?,?,?,?,?)'),
      getSnipeUsers: db.prepare("SELECT * FROM limit_orders WHERE direction = 'snipe_graduation' AND active = 1"),
      getAlerts: db.prepare("SELECT * FROM limit_orders WHERE direction = 'price_alert' AND active = 1"),
    };
  }

  start() {
    if (this.running) return;
    this.running = true;
    console.log('Limit/DCA/Snipe engine started (poll ' + this.pollMs + 'ms)');
    this._poll();
  }

  stop() { this.running = false; }

  async _poll() {
    while (this.running) {
      try { await this._tick(); } catch (e) { console.error('Limit engine:', e.message); }
      await new Promise(r => setTimeout(r, this.pollMs));
    }
  }

  async _tick() {
    const orders = this.stmts.getActive.all();
    if (orders.length === 0) return; // Nothing to monitor

    const [pairs, launches] = await Promise.all([
      this.pi.getDexPairs().catch(() => []),
      this.pi.getLaunches().catch(() => []),
    ]);

    // Price map: mintId -> price in PI
    const priceMap = {};
    for (const p of pairs) {
      const m = p.mint_a === NATIVE ? p.mint_b : p.mint_a;
      if (p.price > 0) priceMap[m] = p.price;
      if (p.launch_mint && p.price > 0) priceMap[p.launch_mint] = p.price;
    }

    // ── Graduation detection ───────────────────────────────────────────
    for (const launch of launches) {
      const prev = this.lastLaunches.get(launch.mint_id);
      if (prev && prev !== 'Finalized' && launch.state === 'Finalized') {
        // Token just graduated — notify snipe orders
        await this._handleGraduation(launch, orders, priceMap);
      }
      this.lastLaunches.set(launch.mint_id, launch.state);
    }

    // ── Price alerts ───────────────────────────────────────────────────
    for (const order of orders.filter(o => o.direction === 'price_alert')) {
      const price = priceMap[order.mint_id];
      if (!price) continue;
      const target = order.trigger_price_num / order.trigger_price_den;
      const above = order.amount > 0; // amount > 0 means alert when above, <= 0 when below
      if ((above && price >= target) || (!above && price <= target)) {
        this.stmts.deactivate.run(order.id);
        await this.notify(order.telegram_id,
          `\u{1F514} *Price Alert\\!*\n\n` +
          `Token \`${order.mint_id.slice(0,12)}\\.\\.\\.\` hit ${esc(price.toFixed(8))} PI\n` +
          `Target was: ${esc(target.toFixed(8))} PI \\(${above ? 'above' : 'below'}\\)`,
          'MarkdownV2'
        );
      }
    }

    const now = Date.now();

    // ── Order execution ────────────────────────────────────────────────
    for (const order of orders) {
      if (order.direction === 'price_alert' || order.direction === 'snipe_graduation') continue;

      const price = priceMap[order.mint_id];
      if (!price && !order.direction.startsWith('dca_')) continue;

      // DCA
      if (order.direction === 'dca_buy' || order.direction === 'dca_sell') {
        if (now >= order.dca_next_at) await this._executeDca(order, price);
        continue;
      }

      const triggerPrice = order.trigger_price_num / order.trigger_price_den;
      let fire = false;

      switch (order.direction) {
        case 'limit_buy': fire = price <= triggerPrice; break;
        case 'take_profit': fire = price >= triggerPrice; break;
        case 'stop_loss': fire = price <= triggerPrice; break;
        case 'trailing_stop': {
          const currentHigh = order.trailing_high_num / order.trigger_price_den;
          if (price > currentHigh) {
            this.stmts.updateTrailingHigh.run(Math.floor(price * order.trigger_price_den), order.id);
          }
          const highWater = Math.max(price, currentHigh);
          const dropPct = highWater > 0 ? (1 - price / highWater) * 100 : 0;
          fire = dropPct >= triggerPrice && highWater > 0;
          break;
        }
      }

      if (fire) await this._executeWithPartialFill(order, price);
    }
  }

  // ── Partial fill execution ───────────────────────────────────────────
  async _executeWithPartialFill(order, currentPrice) {
    const user = this.stmts.getUser.get(order.telegram_id);
    if (!user) { this.stmts.deactivate.run(order.id); return; }
    // PQ vault signing — use telegram_id as vault user ID
    const userId = String(order.telegram_id);
    const walletAddress = user.pi_address || user.wallet_address || '';
    const labels = { limit_buy:'Limit Buy', take_profit:'Take Profit', stop_loss:'Stop Loss', trailing_stop:'Trailing Stop' };
    const label = labels[order.direction] || order.direction;
    const isBuy = order.direction === 'limit_buy';

    // Try full amount, then 50%, then 25%
    for (const fraction of [1, 0.5, 0.25]) {
      const amount = Math.floor(order.amount * fraction);
      if (amount <= 0) continue;

      try {
        let result;
        if (isBuy) {
          result = await this.pi.swap(userId, walletAddress, NATIVE, order.mint_id, amount, user.slippage_bps / 100);
        } else {
          result = await this.pi.swap(userId, walletAddress, order.mint_id, NATIVE, amount, user.slippage_bps / 100);
        }

        if (result.status === 'pending') {
          const piAmt = isBuy ? amount : result.quote.amount_out;
          const tokAmt = isBuy ? result.quote.amount_out : amount;
          this.stmts.logTrade.run(order.telegram_id, order.mint_id, isBuy ? 'buy' : 'sell', piAmt, tokAmt, result.tx_hash || '');

          const fPI = (v) => (Number(v) / 1e9).toFixed(4);
          const partial = fraction < 1 ? ` \\(partial: ${Math.round(fraction*100)}%\\)` : '';

          if (fraction < 1) {
            // Partial fill — reduce remaining amount and keep order active
            const remaining = order.amount - amount;
            this.stmts.reduceAmount.run(remaining, order.id);
            await this.notify(order.telegram_id,
              `\u{1F514} *${esc(label)} partial fill${partial}*\n\n` +
              `Price: ${esc(currentPrice.toFixed(8))} PI\n` +
              (isBuy ? `Spent: ${esc(fPI(amount))} PI` : `Received: ~${esc(fPI(result.quote.amount_out))} PI`) +
              `\nRemaining: ${esc(fPI(remaining))} ${isBuy ? 'PI' : 'tokens'}\n` +
              `TX: \`${(result.tx_hash || '').slice(0, 16)}\\.\\.\\.\``,
              'MarkdownV2'
            );
          } else {
            // Full fill
            this.stmts.deactivate.run(order.id);
            await this.notify(order.telegram_id,
              `\u{1F514} *${esc(label)} filled\\!*\n\n` +
              `Price: ${esc(currentPrice.toFixed(8))} PI\n` +
              (isBuy ? `Spent: ${esc(fPI(amount))} PI` : `Received: ~${esc(fPI(result.quote.amount_out))} PI`) +
              `\nTX: \`${(result.tx_hash || '').slice(0, 16)}\\.\\.\\.\``,
              'MarkdownV2'
            );
          }
          return; // Success — done
        }
        // If rejected (not pending), try smaller amount
      } catch (e) {
        if (fraction === 0.25) {
          // All attempts failed
          this.stmts.deactivate.run(order.id);
          await this.notify(order.telegram_id, `\u{274C} ${label} failed after partial fill attempts: ${e.message}`);
          return;
        }
        // Try smaller
      }
    }

    this.stmts.deactivate.run(order.id);
    await this.notify(order.telegram_id, `\u{274C} ${label} failed — insufficient liquidity`);
  }

  // ── Graduation snipe ─────────────────────────────────────────────────
  async _handleGraduation(launch, orders, priceMap) {
    const snipeOrders = orders.filter(o => o.direction === 'snipe_graduation' && (o.mint_id === launch.mint_id || o.mint_id === '*'));

    for (const order of snipeOrders) {
      const user = this.stmts.getUser.get(order.telegram_id);
      if (!user) continue;
      const userId = String(order.telegram_id); const walletAddress = user.pi_address || user.wallet_address || "";

      // Notify about graduation
      await this.notify(order.telegram_id,
        `\u{1F680} *$${esc(launch.symbol)} graduated to DEX\\!*\n` +
        `Raised: ${esc((launch.pi_raised/1e9).toFixed(2))} PI\n` +
        `Auto\\-buying with ${esc((order.amount/1e9).toFixed(2))} PI\\.\\.\\.`,
        'MarkdownV2'
      );

      // Wait a moment for the pool to be indexed
      await new Promise(r => setTimeout(r, 2000));

      try {
        const result = await this.pi.swap(userId, walletAddress, NATIVE, launch.mint_id, order.amount, user.slippage_bps / 100);
        if (result.status === 'pending') {
          this.stmts.logTrade.run(order.telegram_id, launch.mint_id, 'buy', order.amount, result.quote.amount_out, result.tx_hash || '');
          await this.notify(order.telegram_id,
            `\u{2705} *Graduation snipe success\\!*\n` +
            `Got: ~${esc((result.quote.amount_out / 1e9).toFixed(2))} \\$${esc(launch.symbol)}\n` +
            `TX: \`${(result.tx_hash || '').slice(0, 16)}\\.\\.\\.\``,
            'MarkdownV2'
          );
        } else {
          await this.notify(order.telegram_id, `Graduation snipe failed: ${result.error || 'unknown'}`);
        }
      } catch (e) {
        await this.notify(order.telegram_id, `Graduation snipe error: ${e.message}`);
      }

      // Deactivate if specific mint (keep active if wildcard '*')
      if (order.mint_id !== '*') this.stmts.deactivate.run(order.id);
    }
  }

  // ── DCA execution ────────────────────────────────────────────────────
  async _executeDca(order, currentPrice) {
    const user = this.stmts.getUser.get(order.telegram_id);
    if (!user) return;
    const userId = String(order.telegram_id); const walletAddress = user.pi_address || user.wallet_address || "";
    const isBuy = order.direction === 'dca_buy';

    try {
      let result;
      if (isBuy) {
        result = await this.pi.swap(userId, walletAddress, NATIVE, order.mint_id, order.amount, user.slippage_bps / 100);
      } else {
        result = await this.pi.swap(userId, walletAddress, order.mint_id, NATIVE, order.amount, user.slippage_bps / 100);
      }

      const executed = order.dca_executed + 1;

      if (result.status === 'pending') {
        const piAmt = isBuy ? order.amount : result.quote.amount_out;
        const tokAmt = isBuy ? result.quote.amount_out : order.amount;
        this.stmts.logTrade.run(order.telegram_id, order.mint_id, isBuy ? 'buy' : 'sell', piAmt, tokAmt, result.tx_hash || '');

        const fPI = (v) => (Number(v) / 1e9).toFixed(4);
        await this.notify(order.telegram_id,
          `\u{1F504} *DCA ${isBuy ? 'Buy' : 'Sell'} \\#${executed}/${order.dca_total}*\n` +
          (isBuy ? `Bought with ${esc(fPI(order.amount))} PI` : `Sold for ~${esc(fPI(result.quote.amount_out))} PI`) +
          `\nTX: \`${(result.tx_hash || '').slice(0, 16)}\\.\\.\\.\``,
          'MarkdownV2'
        );
      }

      if (executed >= order.dca_total) {
        this.stmts.deactivate.run(order.id);
        await this.notify(order.telegram_id, `\u{2705} DCA complete! ${executed}/${order.dca_total} orders done.`);
      } else {
        this.stmts.updateDcaNext.run(Date.now() + order.dca_interval_ms, order.id);
      }
    } catch (e) {
      this.stmts.updateDcaNext.run(Date.now() + order.dca_interval_ms, order.id);
      console.error(`DCA error user ${order.telegram_id}:`, e.message);
    }
  }
}

function esc(s) { return String(s).replace(/[_*[\]()~`>#+\-=|{}.!\\]/g, '\\$&'); }

module.exports = LimitOrderEngine;
