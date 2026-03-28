/**
 * PIChain AMM — Constant Product Market Maker (x * y = k)
 *
 * Virtual liquidity pool for PI/SOL trading.
 * Price adjusts automatically based on supply and demand.
 * Never runs out — price just gets more expensive.
 *
 * Same math as Uniswap v2: x * y = k
 * Buy PI: send SOL → receive PI → PI price goes up
 * Sell PI: send PI → receive SOL → PI price goes down
 */

const fs = require('fs');
const path = require('path');

const POOL_FILE = path.join(__dirname, 'amm-pool.json');
const FEE_BPS = 30; // 0.3% swap fee (standard AMM fee)

class AMMPool {
  constructor() {
    this.load();
  }

  /**
   * Load pool state from disk, or initialize with defaults.
   */
  load() {
    try {
      if (fs.existsSync(POOL_FILE)) {
        const data = JSON.parse(fs.readFileSync(POOL_FILE, 'utf8'));
        this.piReserve = data.piReserve || 0;      // PI in pool (base units, 1e9)
        this.solReserve = data.solReserve || 0;     // SOL in pool (lamports, 1e9)
        this.k = data.k || 0;                       // constant product
        this.totalVolumePi = data.totalVolumePi || 0;
        this.totalVolumeSol = data.totalVolumeSol || 0;
        this.totalTrades = data.totalTrades || 0;
        return;
      }
    } catch {}
    // Default: empty pool
    this.piReserve = 0;
    this.solReserve = 0;
    this.k = 0;
    this.totalVolumePi = 0;
    this.totalVolumeSol = 0;
    this.totalTrades = 0;
  }

  /**
   * Save pool state to disk.
   */
  save() {
    fs.writeFileSync(POOL_FILE, JSON.stringify({
      piReserve: this.piReserve,
      solReserve: this.solReserve,
      k: this.k,
      totalVolumePi: this.totalVolumePi,
      totalVolumeSol: this.totalVolumeSol,
      totalTrades: this.totalTrades,
      updatedAt: new Date().toISOString(),
    }, null, 2));
  }

  /**
   * Initialize or add liquidity to the pool.
   * @param {number} piAmount - PI in base units (1 PI = 1e9)
   * @param {number} solAmount - SOL in lamports (1 SOL = 1e9)
   */
  addLiquidity(piAmount, solAmount) {
    this.piReserve += piAmount;
    this.solReserve += solAmount;
    this.k = this.piReserve * this.solReserve;
    this.save();
    return { piReserve: this.piReserve, solReserve: this.solReserve, k: this.k };
  }

  /**
   * Check if pool is initialized.
   */
  isInitialized() {
    return this.piReserve > 0 && this.solReserve > 0;
  }

  /**
   * Get current PI price in SOL (lamports per base unit).
   */
  priceInSol() {
    if (this.piReserve === 0) return 0;
    return this.solReserve / this.piReserve;
  }

  /**
   * Get current PI price in USD.
   * Both reserves use 1e9 scaling (base units / lamports), so the ratio
   * directly gives SOL-per-PI without additional conversion.
   * @param {number} solPriceUsd - Current SOL price in USD
   */
  priceInUsd(solPriceUsd) {
    return this.priceInSol() * solPriceUsd;
  }

  /**
   * Calculate how much PI you get for a given SOL input (buy PI).
   * Uses constant product formula: (x + dx)(y - dy) = k
   * @param {number} solIn - SOL input in lamports
   * @returns {{ piOut: number, fee: number, priceImpact: number, newPrice: number }}
   */
  quoteBuyPI(solIn) {
    if (!this.isInitialized()) return null;
    const fee = Math.floor(solIn * FEE_BPS / 10000);
    const solInAfterFee = solIn - fee;
    // dy = y - k / (x + dx)
    const newSolReserve = this.solReserve + solInAfterFee;
    const newPiReserve = Math.ceil(this.k / newSolReserve);
    const piOut = this.piReserve - newPiReserve;
    if (piOut <= 0) return null;
    const priceImpact = (piOut / this.piReserve) * 100;
    const newPrice = newSolReserve / newPiReserve;
    return { piOut, fee, priceImpact, newPrice, solInAfterFee };
  }

  /**
   * Calculate how much SOL you get for a given PI input (sell PI).
   * @param {number} piIn - PI input in base units
   * @returns {{ solOut: number, fee: number, priceImpact: number, newPrice: number }}
   */
  quoteSellPI(piIn) {
    if (!this.isInitialized()) return null;
    const fee = Math.floor(piIn * FEE_BPS / 10000);
    const piInAfterFee = piIn - fee;
    const newPiReserve = this.piReserve + piInAfterFee;
    const newSolReserve = Math.ceil(this.k / newPiReserve);
    const solOut = this.solReserve - newSolReserve;
    if (solOut <= 0) return null;
    const priceImpact = (piIn / this.piReserve) * 100;
    const newPrice = newSolReserve / newPiReserve;
    return { solOut, fee, priceImpact, newPrice, piInAfterFee };
  }

  /**
   * Execute a buy: SOL → PI. Updates reserves.
   * @param {number} solIn - SOL in lamports
   * @returns {{ piOut: number, newPriceInSol: number }}
   */
  executeBuy(solIn) {
    const quote = this.quoteBuyPI(solIn);
    if (!quote) throw new Error('Insufficient liquidity');
    this.solReserve += quote.solInAfterFee;
    this.piReserve -= quote.piOut;
    this.k = this.piReserve * this.solReserve; // recalc to avoid drift
    this.totalVolumePi += quote.piOut;
    this.totalVolumeSol += solIn;
    this.totalTrades++;
    this.save();
    return { piOut: quote.piOut, newPriceInSol: this.priceInSol() };
  }

  /**
   * Execute a sell: PI → SOL. Updates reserves.
   * @param {number} piIn - PI in base units
   * @returns {{ solOut: number, newPriceInSol: number }}
   */
  executeSell(piIn) {
    const quote = this.quoteSellPI(piIn);
    if (!quote) throw new Error('Insufficient liquidity');
    this.piReserve += quote.piInAfterFee;
    this.solReserve -= quote.solOut;
    this.k = this.piReserve * this.solReserve;
    this.totalVolumePi += piIn;
    this.totalVolumeSol += quote.solOut;
    this.totalTrades++;
    this.save();
    return { solOut: quote.solOut, newPriceInSol: this.priceInSol() };
  }

  /**
   * Get pool stats for display.
   * @param {number} solPriceUsd
   */
  stats(solPriceUsd) {
    const priceUsd = this.priceInUsd(solPriceUsd);
    const piReservePi = this.piReserve / 1e9;
    const solReserveSol = this.solReserve / 1e9;
    const tvlUsd = (solReserveSol * solPriceUsd) * 2; // both sides
    return {
      piReserve: piReservePi,
      solReserve: solReserveSol,
      priceUsd,
      priceSol: this.priceInSol(),
      tvlUsd,
      totalTrades: this.totalTrades,
      totalVolumePi: this.totalVolumePi / 1e9,
      totalVolumeSol: this.totalVolumeSol / 1e9,
      piPerSol: this.solReserve > 0 ? this.piReserve / this.solReserve : 0,
    };
  }
}

module.exports = { AMMPool };
