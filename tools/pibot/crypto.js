/**
 * PIChain crypto utilities — PQ vault-based signing.
 *
 * All private key operations go through the pichain-signer vault.
 * No key material ever exists in this Node.js process.
 *
 * Vault API (localhost:8315):
 *   POST /vault/create  { user_id }          → { address }
 *   POST /vault/address { user_id }          → { address }
 *   POST /vault/sign    { user_id, tx_data } → { signed_tx, tx_hash }
 *   POST /vault/export  { user_id }          → { wallet }
 */

const VAULT_URL = process.env.VAULT_URL || 'http://127.0.0.1:8315';

// ── BLAKE3 (pure JS, matches Rust blake3::hash exactly) ────────────────────
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
    if(!(input instanceof Uint8Array)) input=new Uint8Array(input);
    let cv=new Uint32Array(IV);
    const nBlocks=Math.max(1,Math.ceil(input.length/64));
    for(let i=0;i<nBlocks;i++){
      const start=i*64, block=new Uint8Array(64);
      const end=Math.min(start+64,input.length);
      block.set(input.subarray(start,end));
      let fl=0;
      if(i===0) fl|=CHUNK_START;
      if(i===nBlocks-1) fl|=CHUNK_END|ROOT;
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

// ── Hex encoding/decoding ──────────────────────────────────────────────────
function hexEncode(bytes) { return Array.from(bytes).map(b=>b.toString(16).padStart(2,'0')).join(''); }
function hexDecode(hex) { const b=new Uint8Array(hex.length/2); for(let i=0;i<b.length;i++) b[i]=parseInt(hex.substr(i*2,2),16); return b; }

// ── Byte utilities ─────────────────────────────────────────────────────────
function u64LE(n) { const buf=new Uint8Array(8); const big=BigInt(n); for(let i=0;i<8;i++) buf[i]=Number((big>>BigInt(i*8))&0xFFn); return buf; }
function u32LE(n) { const buf=new Uint8Array(4); buf[0]=n&0xFF;buf[1]=(n>>8)&0xFF;buf[2]=(n>>16)&0xFF;buf[3]=(n>>24)&0xFF; return buf; }
function concatBytes(arrays) { let total=0; for(const a of arrays) total+=a.length; const r=new Uint8Array(total); let off=0; for(const a of arrays){r.set(a,off);off+=a.length;} return r; }

const NATIVE_PI_MINT = '0'.repeat(64);

// ── Vault API helpers ──────────────────────────────────────────────────────

async function vaultRequest(path, body) {
  const resp = await fetch(VAULT_URL + path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(15000), // 15 second timeout
  });
  const data = await resp.json();
  if (data.error) throw new Error(data.error);
  return data;
}

/**
 * Create a new PQ wallet for a user via the vault.
 * @param {string} userId - Unique user identifier (e.g., Telegram user ID)
 * @returns {Promise<{address: string}>}
 */
async function createWallet(userId) {
  return vaultRequest('/vault/create', { user_id: String(userId) });
}

/**
 * Get a user's wallet address from the vault.
 * @param {string} userId
 * @returns {Promise<{address: string}>}
 */
async function getAddress(userId) {
  return vaultRequest('/vault/address', { user_id: String(userId) });
}

/**
 * Check if the vault is available.
 * @returns {Promise<boolean>}
 */
async function isVaultAvailable() {
  try {
    const resp = await fetch(VAULT_URL + '/status', { signal: AbortSignal.timeout(2000) });
    return resp.ok;
  } catch { return false; }
}

/**
 * Sign a transaction via the vault. NO key material touches Node.js.
 *
 * @param {string} userId - User whose wallet signs
 * @param {string} kindName - Transaction kind (e.g., 'Transfer', 'Swap')
 * @param {Object} kindData - Transaction kind data
 * @param {number} nonce - Sender nonce
 * @param {number} gasLimit - Gas limit
 * @param {number} baseFee - Base fee
 * @param {number} chainId - Chain ID
 * @returns {Promise<{signedTxHex: string, txHash: string}>}
 */
async function signTransaction(userId, address, kindName, kindData, nonce, gasLimit, baseFee, chainId) {
  const txData = {
    sender: address,
    nonce,
    kind: { [kindName]: kindData },
    gas_limit: gasLimit,
    max_base_fee: baseFee,
    max_priority_fee: 100,
    chain_id: chainId,
  };

  const result = await vaultRequest('/vault/sign', {
    user_id: String(userId),
    tx_data: txData,
  });

  // Hex-encode the signed transaction JSON for node submission
  const jsonStr = JSON.stringify(result.signed_tx, (_, v) =>
    typeof v === 'bigint' ? '__BI_'+v.toString()+'__' : v
  ).replace(/"__BI_(\d+)__"/g, '$1');
  const signedTxHex = Buffer.from(jsonStr, 'utf8').toString('hex');

  return { signedTxHex, txHash: result.tx_hash };
}

/**
 * Export a user's wallet for backup (from vault).
 * @param {string} userId
 * @returns {Promise<Object>} - PQ wallet export JSON
 */
async function exportWallet(userId) {
  const result = await vaultRequest('/vault/export', { user_id: String(userId) });
  return result.wallet;
}

/**
 * Import a PQ wallet into the vault for a user.
 * @param {string} userId - Unique user identifier
 * @param {Object} walletExport - PQ wallet export (from CLI miner or desktop app)
 * @returns {Promise<{address: string}>}
 */
async function importWallet(userId, walletExport) {
  return vaultRequest('/vault/import', { user_id: String(userId), wallet: walletExport });
}

// ── PoW solver for wallet activation ───────────────────────────────────────
async function solveActivationPoW(challengeBytes, diffBits) {
  const { createHash } = require('crypto');
  const fullBytes = (diffBits / 8) | 0;
  const remBits = diffBits % 8;
  const mask = remBits > 0 ? (0xFF << (8 - remBits)) & 0xFF : 0;
  for (let nonce = 0; nonce < 100_000_000; nonce++) {
    const buf = Buffer.alloc(challengeBytes.length + 8);
    buf.set(challengeBytes);
    let n = nonce;
    for (let j = 0; j < 8; j++) { buf[challengeBytes.length + j] = n & 0xFF; n = Math.floor(n / 256); }
    const hash = createHash('sha256').update(buf).digest();
    let ok = true;
    for (let i = 0; i < fullBytes; i++) { if (hash[i] !== 0) { ok = false; break; } }
    if (ok && remBits > 0 && (hash[fullBytes] & mask) !== 0) ok = false;
    if (ok) return nonce;
  }
  throw new Error('PoW solution not found');
}

// ── Legacy compatibility shim ──────────────────────────────────────────────
// These exist so existing bot code doesn't break immediately during migration.
// They wrap vault calls and return objects matching the old Ed25519 wallet format.

function walletFromSeed(_seedHex) {
  throw new Error('Ed25519 wallets are no longer supported. Use createWallet(userId) with the vault API.');
}

function buildSignedTxJson() {
  throw new Error('Ed25519 signing removed. Use signTransaction() with the vault API.');
}

module.exports = {
  BLAKE3, hexEncode, hexDecode, u64LE, u32LE, concatBytes,
  NATIVE_PI_MINT,
  // Vault API (PQ-native)
  createWallet, getAddress, signTransaction, exportWallet, importWallet,
  isVaultAvailable, solveActivationPoW,
  // Legacy (throws errors)
  walletFromSeed, buildSignedTxJson,
};
