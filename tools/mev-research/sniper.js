#!/usr/bin/env node
/**
 * PIChain MEV Research Bot — Launch Graduation Sniper
 *
 * INTERNAL USE ONLY — studies MEV attack vectors to validate protocol protections.
 *
 * Strategy: Monitor active launches approaching graduation threshold.
 * When a launch graduates (auto-finalizes to DEX), immediately buy on the new pool.
 *
 * This exploits the fact that launch graduation creates a DEX pool at a fixed
 * initial price (based on raised PI / token allocation). Early buyers on the DEX
 * can capture the price discovery premium.
 */

const nacl = require('tweetnacl');
const { createHash } = require('crypto');

// ── Configuration ──────────────────────────────────────────────────────────────
const RPC_URL = process.env.PICHAIN_RPC || 'http://127.0.0.1:8314';
const WALLET_SEED = process.env.PICHAIN_SEED; // 64-char hex private key
const CHAIN_ID = parseInt(process.env.CHAIN_ID || '31415');
const BUY_AMOUNT_PI = parseFloat(process.env.BUY_AMOUNT || '0.5'); // PI to spend per snipe
const GRADUATION_THRESHOLD = parseFloat(process.env.GRAD_THRESHOLD || '95'); // % of target to start watching
const POLL_INTERVAL_MS = parseInt(process.env.POLL_MS || '500'); // Poll every 500ms
const SLIPPAGE_PCT = parseFloat(process.env.SLIPPAGE || '5'); // 5% slippage tolerance

const NATIVE_PI_MINT = '0'.repeat(64);

// ── BLAKE3 (matches Rust blake3::hash exactly) ────────────────────────────────
const BLAKE3 = (() => {
  const IV = new Uint32Array([0x6A09E667,0xBB67AE85,0x3C6EF372,0xA54FF53A,0x510E527F,0x9B05688C,0x1F83D9AB,0x5BE0CD19]);
  const MSG_PERM = [2,6,3,10,7,0,4,13,1,11,12,5,9,14,15,8];
  const CHUNK_START=1, CHUNK_END=2, ROOT=8;
  function rotR(x,n){return(x>>>n)|(x<<(32-n));}
  function g(s,a,b,c,d,mx,my){
    s[a]=(s[a]+s[b]+mx)|0; s[d]=rotR(s[d]^s[a],16);
    s[c]=(s[c]+s[d])|0; s[b]=rotR(s[b]^s[c],12);
    s[a]=(s[a]+s[b]+my)|0; s[d]=rotR(s[d]^s[a],8);
    s[c]=(s[c]+s[d])|0; s[b]=rotR(s[b]^s[c],7);
  }
  function round(s,m){
    g(s,0,4,8,12,m[0],m[1]);g(s,1,5,9,13,m[2],m[3]);
    g(s,2,6,10,14,m[4],m[5]);g(s,3,7,11,15,m[6],m[7]);
    g(s,0,5,10,15,m[8],m[9]);g(s,1,6,11,12,m[10],m[11]);
    g(s,2,7,8,13,m[12],m[13]);g(s,3,4,9,14,m[14],m[15]);
  }
  function compress(cv,block,counter,blockLen,flags){
    const s=new Uint32Array([cv[0],cv[1],cv[2],cv[3],cv[4],cv[5],cv[6],cv[7],
      IV[0],IV[1],IV[2],IV[3],counter&0xFFFFFFFF,(counter/0x100000000)>>>0,blockLen,flags]);
    const m=new Uint32Array(16);
    for(let i=0;i<16;i++) m[i]=block[i*4]|(block[i*4+1]<<8)|(block[i*4+2]<<16)|(block[i*4+3]<<24);
    for(let i=0;i<7;i++){round(s,m);const t=new Uint32Array(16);for(let j=0;j<16;j++)t[j]=m[MSG_PERM[j]];m.set(t);}
    for(let i=0;i<8;i++){s[i]^=s[i+8];s[i+8]^=cv[i];}
    return s;
  }
  function hash(input){
    if(!(input instanceof Uint8Array))input=new Uint8Array(input);
    let cv=new Uint32Array(IV);
    const nBlocks=Math.max(1,Math.ceil(input.length/64));
    for(let i=0;i<nBlocks;i++){
      const start=i*64, block=new Uint8Array(64);
      const end=Math.min(start+64,input.length);
      block.set(input.subarray(start,end));
      let fl=0;
      if(i===0)fl|=CHUNK_START;
      if(i===nBlocks-1)fl|=CHUNK_END|ROOT;
      const out=compress(cv,block,0,end-start,fl);
      if(i===nBlocks-1){
        const r=new Uint8Array(32);
        for(let j=0;j<8;j++){r[j*4]=out[j]&0xFF;r[j*4+1]=(out[j]>>8)&0xFF;r[j*4+2]=(out[j]>>16)&0xFF;r[j*4+3]=(out[j]>>24)&0xFF;}
        return r;
      }
      cv=new Uint32Array(out.subarray(0,8));
    }
  }
  return {hash};
})();

// ── Utilities ──────────────────────────────────────────────────────────────────
function hexEncode(bytes) { return Array.from(bytes).map(b => b.toString(16).padStart(2,'0')).join(''); }
function hexDecode(hex) { const b = new Uint8Array(hex.length/2); for(let i=0;i<b.length;i++) b[i]=parseInt(hex.substr(i*2,2),16); return b; }
function u64LE(n) { const buf = new Uint8Array(8); const big = BigInt(n); for(let i=0;i<8;i++) buf[i]=Number((big>>BigInt(i*8))&0xFFn); return buf; }
function concatBytes(arrays) { let total=0; for(const a of arrays) total+=a.length; const r=new Uint8Array(total); let off=0; for(const a of arrays){r.set(a,off);off+=a.length;} return r; }
function deriveAddress(pk) { return BLAKE3.hash(pk).slice(12,32); }

// ── Wallet ─────────────────────────────────────────────────────────────────────
let walletKeypair = null;
let walletAddress = null;
let walletAddressHex = '';
let accountNonce = 0;
let currentBaseFee = 1000;

function initWallet() {
  if (!WALLET_SEED || WALLET_SEED.length !== 64) {
    console.error('Set PICHAIN_SEED env var to your 64-char hex private key');
    process.exit(1);
  }
  const seed = hexDecode(WALLET_SEED);
  walletKeypair = nacl.sign.keyPair.fromSeed(seed);
  walletAddress = deriveAddress(walletKeypair.publicKey);
  walletAddressHex = hexEncode(walletAddress);
  console.log(`Wallet: Pi314${walletAddressHex}`);
}

async function syncState() {
  try {
    const res = await fetch(`${RPC_URL}/api/v1/account/${walletAddressHex}`);
    const data = await res.json();
    if (data.found) {
      accountNonce = data.nonce;
    }
    const info = await (await fetch(`${RPC_URL}/api/v1/info`)).json();
    currentBaseFee = info.base_fee || 1000;
  } catch(e) {
    console.error('Failed to sync state:', e.message);
  }
}

// ── Transaction Building (must match Rust canonical encoding exactly) ──────────
function buildTxHeader(sender, nonce, gasLimit, maxBaseFee, maxPriorityFee, chainId) {
  return concatBytes([sender, u64LE(nonce), u64LE(gasLimit), u64LE(maxBaseFee), u64LE(maxPriorityFee), u64LE(chainId)]);
}

// Tag 17: Swap { mint_in: [u8;32], mint_out: [u8;32], amount_in: u64, min_amount_out: u64 }
function buildSwapCanonical(sender, nonce, mintIn, mintOut, amountIn, minAmountOut) {
  const h = buildTxHeader(sender, nonce, 50000, currentBaseFee, 500, CHAIN_ID); // Higher priority fee for speed
  return concatBytes([h, new Uint8Array([17]), mintIn, mintOut, u64LE(amountIn), u64LE(minAmountOut)]);
}

function buildSignedTxJson(kindName, kindData, nonce, signature, gasLimit) {
  const signedTx = {
    data: {
      sender: Array.from(walletAddress),
      nonce: nonce,
      kind: { [kindName]: kindData },
      gas_limit: gasLimit,
      max_base_fee: currentBaseFee,
      max_priority_fee: 500, // Higher priority to get included faster
      chain_id: CHAIN_ID
    },
    signature: hexEncode(signature),
    public_key: Array.from(walletKeypair.publicKey)
  };
  const json = JSON.stringify(signedTx, (_, v) => typeof v === 'bigint' ? Number(v) : v);
  return hexEncode(new TextEncoder().encode(json));
}

async function submitTx(hexStr) {
  const res = await fetch(`${RPC_URL}/api/v1/tx/submit`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ signed_tx_hex: hexStr })
  });
  return await res.json();
}

// ── Sniper Logic ───────────────────────────────────────────────────────────────
const watchedLaunches = new Map(); // mint_id -> { state, percent, last_state }
const snipedMints = new Set(); // Already sniped, don't double-buy

async function pollLaunches() {
  try {
    const res = await fetch(`${RPC_URL}/api/v1/launches`);
    const data = await res.json();
    const launches = data.launches || [];

    for (const launch of launches) {
      const prev = watchedLaunches.get(launch.mint_id);
      const pct = launch.percent_complete || 0;

      // Detect graduation: was Active/TargetReached, now Finalized
      if (prev && prev.state !== 'Finalized' && launch.state === 'Finalized') {
        if (!snipedMints.has(launch.mint_id)) {
          console.log(`\n!! GRADUATION DETECTED: $${launch.symbol} (${launch.mint_id.slice(0,16)}...)`);
          console.log(`   Raised: ${(launch.pi_raised / 1e9).toFixed(4)} PI, Target: ${(launch.target_pi / 1e9).toFixed(4)} PI`);
          await executeSnipe(launch);
        }
      }

      // Log approaching launches
      if (launch.state === 'Active' && pct >= GRADUATION_THRESHOLD) {
        if (!prev || prev.percent < GRADUATION_THRESHOLD) {
          console.log(`\n>> WATCHING: $${launch.symbol} at ${pct.toFixed(1)}% of target`);
        }
      }

      watchedLaunches.set(launch.mint_id, {
        state: launch.state,
        percent: pct,
        symbol: launch.symbol,
        decimals: launch.decimals,
      });
    }
  } catch(e) {
    // Silent — keep polling
  }
}

async function executeSnipe(launch) {
  snipedMints.add(launch.mint_id);

  const buyAmountBase = Math.floor(BUY_AMOUNT_PI * 1e9);
  const mintInBytes = hexDecode(NATIVE_PI_MINT);
  const mintOutBytes = hexDecode(launch.mint_id);

  try {
    // Get quote — pool should exist now after graduation
    const qRes = await fetch(`${RPC_URL}/api/v1/swap/quote/${NATIVE_PI_MINT}/${launch.mint_id}/${buyAmountBase}`);
    const quote = await qRes.json();

    if (!quote.found) {
      console.log('   Pool not ready yet, retrying in 1s...');
      await new Promise(r => setTimeout(r, 1000));
      // Retry once
      const retry = await (await fetch(`${RPC_URL}/api/v1/swap/quote/${NATIVE_PI_MINT}/${launch.mint_id}/${buyAmountBase}`)).json();
      if (!retry.found) {
        console.log('   Pool still not found, skipping');
        return;
      }
      Object.assign(quote, retry);
    }

    const minOut = Math.floor(quote.amount_out * (1 - SLIPPAGE_PCT / 100));
    console.log(`   Quote: ${(quote.amount_out / Math.pow(10, launch.decimals || 9)).toFixed(2)} tokens for ${BUY_AMOUNT_PI} PI`);
    console.log(`   Impact: ${(quote.price_impact_bps / 100).toFixed(2)}%`);

    // Sync nonce fresh
    await syncState();

    // Build and sign swap transaction
    const canonical = buildSwapCanonical(walletAddress, accountNonce, mintInBytes, mintOutBytes, buyAmountBase, minOut);
    const sig = nacl.sign.detached(canonical, walletKeypair.secretKey);
    const hex = buildSignedTxJson('Swap', {
      mint_in: Array.from(mintInBytes),
      mint_out: Array.from(mintOutBytes),
      amount_in: buyAmountBase,
      min_amount_out: minOut
    }, accountNonce, sig, 50000);

    const result = await submitTx(hex);
    if (result.status === 'pending') {
      accountNonce++;
      console.log(`   SNIPED! TX: ${result.tx_hash}`);
    } else {
      console.log(`   Failed: ${result.error || 'unknown'}`);
    }
  } catch(e) {
    console.log(`   Error: ${e.message}`);
  }
}

// ── Arbitrage Scanner ──────────────────────────────────────────────────────────
async function scanArbitrage() {
  try {
    const res = await fetch(`${RPC_URL}/api/v1/dex/pairs`);
    const data = await res.json();
    const pairs = data.pairs || [];

    for (const pair of pairs) {
      if (!pair.active || pair.reserve_a === 0 || pair.reserve_b === 0) continue;

      // Check if price has deviated significantly from recent trades
      if (pair.price_change_24h_pct && Math.abs(pair.price_change_24h_pct) > 10) {
        // Potential mean-reversion opportunity
        const dir = pair.price_change_24h_pct > 0 ? 'sell' : 'buy';
        console.log(`   ARB: ${pair.symbol_a}/${pair.symbol_b} ${pair.price_change_24h_pct.toFixed(1)}% 24h → ${dir} signal`);
      }
    }
  } catch(e) {
    // Silent
  }
}

// ── Main Loop ──────────────────────────────────────────────────────────────────
async function main() {
  console.log('PIChain MEV Research Bot — Launch Graduation Sniper');
  console.log('═══════════════════════════════════════════════════');
  console.log(`RPC: ${RPC_URL}`);
  console.log(`Buy amount: ${BUY_AMOUNT_PI} PI per snipe`);
  console.log(`Graduation watch: >${GRADUATION_THRESHOLD}%`);
  console.log(`Slippage: ${SLIPPAGE_PCT}%`);
  console.log(`Poll interval: ${POLL_INTERVAL_MS}ms`);
  console.log('');

  initWallet();
  await syncState();
  console.log(`Nonce: ${accountNonce}, Base fee: ${currentBaseFee}`);
  console.log('');
  console.log('Monitoring launches for graduation events...\n');

  // Main polling loop
  let arbCounter = 0;
  setInterval(async () => {
    await pollLaunches();

    // Scan arbitrage every 10th poll
    if (++arbCounter % 10 === 0) {
      await scanArbitrage();
    }
  }, POLL_INTERVAL_MS);

  // Periodic state sync
  setInterval(syncState, 15000);
}

main().catch(e => { console.error(e); process.exit(1); });
