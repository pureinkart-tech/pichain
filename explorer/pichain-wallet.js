/**
 * PIChain PQ Wallet Connector
 *
 * Replaces TweetNaCl Ed25519 signing with post-quantum proxy signing.
 * Loaded by all explorer pages. Provides the same wallet API that pages
 * already use (walletKeypair, walletAddress, etc.) but routes signing
 * through the PQ signing proxy on localhost:8315.
 *
 * This file is designed to be a DROP-IN replacement for TweetNaCl.
 * Pages that previously did:
 *   const sig = nacl.sign.detached(canonical, walletKeypair.secretKey);
 *   const hex = buildSignedTxJson(kind, data, nonce, sig, gas);
 *
 * Now do:
 *   const hex = await signAndBuildPQ(kind, data, nonce, gas);
 *
 * The PQ proxy handles canonical encoding, dual signing (ML-DSA + SLH-DSA),
 * and returns the complete SignedTransaction.
 */

(function() {
'use strict';

const PROXY_URL = 'http://127.0.0.1:8315';
const CONNECT_TIMEOUT = 3000;

// ─── Global wallet state (same variable names pages already use) ───

window.pqProxyConnected = false;
window.pqProxyAddress = null;
window.pqWalletProvider = 'none'; // 'proxy' | 'extension' | 'none'

// ─── Connection ───

/**
 * Try connecting to the PQ signing proxy.
 * Returns { connected, address } or throws.
 */
async function connectPQProxy() {
  try {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), CONNECT_TIMEOUT);
    const resp = await fetch(PROXY_URL + '/status', { signal: controller.signal });
    clearTimeout(timeout);
    if (!resp.ok) return { connected: false };
    const data = await resp.json();
    if (data.connected && data.address) {
      window.pqProxyConnected = true;
      window.pqProxyAddress = data.address;
      window.pqWalletProvider = 'proxy';
      console.log('[PQ] Connected to signing proxy:', data.address);
      return { connected: true, address: data.address };
    }
  } catch(e) {
    // Extension fallback
    if (window.__pichain_extension && window.__pichain_extension.isAvailable) {
      try {
        const ext = await window.__pichain_extension.connect();
        window.pqProxyConnected = true;
        window.pqProxyAddress = ext.address;
        window.pqWalletProvider = 'extension';
        console.log('[PQ] Connected to extension:', ext.address);
        return { connected: true, address: ext.address };
      } catch(e2) {}
    }
  }
  window.pqProxyConnected = false;
  window.pqWalletProvider = 'none';
  return { connected: false };
}

/**
 * Sign a transaction via the PQ proxy and return the hex-encoded signed transaction
 * ready for submission to /api/v1/tx/submit.
 *
 * @param {string} kindName - Transaction kind (e.g., 'Swap', 'Transfer', 'Stake')
 * @param {Object} kindData - Transaction kind data
 * @param {number} nonce - Sender nonce
 * @param {number} gasLimit - Gas limit
 * @param {Object} opts - Optional: { maxBaseFee, maxPriorityFee, chainId }
 * @returns {Promise<string>} - Hex-encoded signed transaction for submission
 */
async function signAndBuildPQ(kindName, kindData, nonce, gasLimit, opts) {
  opts = opts || {};
  const chainId = opts.chainId || window.CHAIN_ID || 31415;
  const maxBaseFee = opts.maxBaseFee || window.currentBaseFee || 1000;
  const maxPriorityFee = opts.maxPriorityFee || 100;

  if (!window.pqProxyConnected || !window.pqProxyAddress) {
    throw new Error('PQ wallet not connected. Please run pichain-signer or install the extension.');
  }

  // Build TransactionData matching the Rust struct's serde format
  const txData = {
    sender: window.pqProxyAddress,
    nonce: nonce,
    kind: { [kindName]: kindData },
    gas_limit: gasLimit,
    max_base_fee: maxBaseFee,
    max_priority_fee: maxPriorityFee,
    chain_id: chainId,
  };

  let signedTx;

  if (window.pqWalletProvider === 'proxy') {
    const resp = await fetch(PROXY_URL + '/sign', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ tx_data: txData }),
    });
    const data = await resp.json();
    if (data.error) throw new Error(data.error);
    signedTx = data.signed_tx;
  } else if (window.pqWalletProvider === 'pibot') {
    // Sign via PiBot vault — returns hex directly
    return await signViaPiBot(kindName, kindData, nonce, gasLimit, opts);
  } else if (window.pqWalletProvider === 'extension') {
    signedTx = await window.__pichain_extension.sign(txData);
  } else {
    throw new Error('No PQ signer available');
  }

  // Hex-encode the signed transaction JSON for node submission
  const jsonStr = JSON.stringify(signedTx);
  const bytes = new TextEncoder().encode(jsonStr);
  let hex = '';
  for (let i = 0; i < bytes.length; i++) {
    hex += bytes[i].toString(16).padStart(2, '0');
  }
  return hex;
}

/**
 * Submit a hex-encoded signed transaction to the node.
 */
async function submitPQTx(hexStr) {
  const API = window.API || window.location.origin;
  const res = await fetch(API + '/api/v1/tx/submit', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ signed_tx_hex: hexStr }),
  });
  return await res.json();
}

/**
 * Combined: sign and submit in one call.
 */
async function signAndSubmitPQ(kindName, kindData, nonce, gasLimit, opts) {
  const hex = await signAndBuildPQ(kindName, kindData, nonce, gasLimit, opts);
  return await submitPQTx(hex);
}

// ─── Wallet UI Integration ───

/**
 * Check if the PQ proxy is connected and update any wallet UI elements.
 * Call this from initWallet() in each page.
 */
function updatePQBadge() {
  if (!window.pqProxyConnected) return;

  // Add PQ badge to wallet display
  const bars = document.querySelectorAll('.wallet-bar, .wallet-info, .wallet-addr-row, #walletDisplay');
  bars.forEach(bar => {
    if (bar.querySelector('.pq-badge')) return;
    const badge = document.createElement('span');
    badge.className = 'pq-badge';
    badge.style.cssText = 'display:inline-flex;align-items:center;gap:3px;padding:1px 6px;border-radius:3px;font-size:10px;font-weight:700;margin-left:6px;background:rgba(34,197,94,0.15);color:#22c55e;';
    badge.textContent = window.pqWalletProvider === 'extension' ? 'PQ EXT' : 'PQ';
    bar.appendChild(badge);
  });
}

/**
 * Persist wallet connection across all pages.
 * Saves address to localStorage so user doesn't reconnect on every page.
 * SECURITY: Only the address is stored — NEVER private keys.
 * The signer proxy holds keys locally on the user's machine.
 */
function persistWalletConnection(address) {
  if (address) {
    localStorage.setItem('pichain_connected_address', address);
    localStorage.removeItem('pichain_disconnected');
  }
}

function clearWalletConnection() {
  localStorage.removeItem('pichain_connected_address');
  localStorage.removeItem('pichain_pibot_tid');
  localStorage.setItem('pichain_disconnected', '1');
  window.pqProxyConnected = false;
  window.pqProxyAddress = null;
  window.pqWalletProvider = 'none';
}

/**
 * Connect via PiBot: generate a link code, user verifies on Telegram,
 * website polls until linked. Returns { connected, address }.
 */
async function connectViaPiBot() {
  const code = Math.random().toString(36).slice(2, 8).toUpperCase();
  const API = window.API || window.location.origin;

  // Open PiBot with the link code
  window.open('https://t.me/PiChainTradeBot?start=link_' + code, '_blank');

  // Poll for verification (every 2s for 2 minutes)
  for (let i = 0; i < 60; i++) {
    await new Promise(r => setTimeout(r, 2000));
    try {
      const resp = await fetch(API + '/api/v1/session/check?code=' + code, { signal: AbortSignal.timeout(3000) });
      if (resp.ok) {
        const data = await resp.json();
        if (data.linked && data.address) {
          window.pqProxyConnected = true;
          window.pqProxyAddress = data.address;
          window.pqWalletProvider = 'pibot';
          localStorage.setItem('pichain_pibot_tid', data.telegram_id);
          persistWalletConnection(data.address);
          return { connected: true, address: data.address };
        }
      }
    } catch {}
  }
  return { connected: false };
}

/**
 * Sign via PiBot vault (for users connected through Telegram linking).
 */
async function signViaPiBot(kindName, kindData, nonce, gasLimit, opts) {
  opts = opts || {};
  const tid = localStorage.getItem('pichain_pibot_tid');
  if (!tid) throw new Error('Not connected via PiBot');

  const API = window.API || window.location.origin;
  const chainId = opts.chainId || window.CHAIN_ID || 31415;
  const maxBaseFee = opts.maxBaseFee || window.currentBaseFee || 1000;

  const txData = {
    sender: window.pqProxyAddress,
    nonce: nonce,
    kind: { [kindName]: kindData },
    gas_limit: gasLimit,
    max_base_fee: maxBaseFee,
    max_priority_fee: opts.maxPriorityFee || 100,
    chain_id: chainId,
  };

  const resp = await fetch(API + '/api/v1/session/sign', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ telegram_id: tid, tx_data: txData }),
  });
  const data = await resp.json();
  if (data.error) throw new Error(data.error);

  // Return hex-encoded signed tx for submission
  const jsonStr = JSON.stringify(data.signed_tx);
  const bytes = new TextEncoder().encode(jsonStr);
  let hex = '';
  for (let i = 0; i < bytes.length; i++) hex += bytes[i].toString(16).padStart(2, '0');
  return hex;
}

function getPersistedAddress() {
  if (localStorage.getItem('pichain_disconnected') === '1') return null;
  return localStorage.getItem('pichain_connected_address');
}

// ─── Expose globally ───

/**
 * UI helper: connect with PiBot from wallet modal.
 * Shows waiting state, opens Telegram, polls for verification.
 */
async function connectWithPiBot() {
  // Generate link code and open Telegram IMMEDIATELY (must be in direct click handler)
  const code = Math.random().toString(36).slice(2, 8).toUpperCase();
  window.open('https://t.me/PiChainTradeBot?start=link_' + code, '_blank');

  const step1 = document.getElementById('walletStep1') || document.getElementById('wmStep1');
  const step3 = document.getElementById('walletStep3') || document.getElementById('wmStep3');
  const status = document.getElementById('captchaStatus') || document.getElementById('wmStatus');
  const spinner = document.getElementById('captchaSpinner') || document.getElementById('wmSpinner');

  if (step1) step1.style.display = 'none';
  if (step3) {
    step3.style.display = 'block';
    if (status) { status.textContent = 'Waiting for PiBot verification... Click Start in Telegram.'; status.style.color = 'var(--muted)'; }
    if (spinner) spinner.style.display = 'inline-block';
  }

  // Poll for verification
  const API = window.API || window.location.origin;
  let result = { connected: false };
  for (let i = 0; i < 60; i++) {
    await new Promise(r => setTimeout(r, 2000));
    try {
      const resp = await fetch(API + '/api/v1/session/check?code=' + code, { signal: AbortSignal.timeout(3000) });
      if (resp.ok) {
        const data = await resp.json();
        if (data.linked && data.address) {
          window.pqProxyConnected = true;
          window.pqProxyAddress = data.address;
          window.pqWalletProvider = 'pibot';
          localStorage.setItem('pichain_pibot_tid', data.telegram_id);
          window.persistWalletConnection(data.address);
          result = { connected: true, address: data.address };
          break;
        }
      }
    } catch {}
  }
  if (result.connected) {
    const modal = document.getElementById('walletModal');
    if (modal) modal.classList.remove('show');
    if (typeof showToast === 'function') showToast('Connected via PiBot: ' + result.address.slice(0, 16) + '...', 'success');
    if (typeof updateWalletUI === 'function') updateWalletUI();
    if (typeof onWalletConnected === 'function') onWalletConnected();
    // Set compat vars for pages that check walletKeypair
    window.walletAddressHex = result.address.replace(/^Pi314/, '').toLowerCase();
    window.walletAddress = window.walletAddressHex ? new Uint8Array(window.walletAddressHex.match(/.{2}/g).map(b => parseInt(b, 16))) : null;
    window.walletKeypair = window.walletAddress ? { publicKey: window.walletAddress } : null;
  } else {
    if (status) { status.textContent = 'Connection timed out. Try again.'; status.style.color = 'var(--rose)'; }
    if (spinner) spinner.style.display = 'none';
  }
}

window.connectWithPiBot = connectWithPiBot;
window.connectPQProxy = connectPQProxy;
window.connectViaPiBot = connectViaPiBot;
window.signAndBuildPQ = signAndBuildPQ;
window.signViaPiBot = signViaPiBot;
window.submitPQTx = submitPQTx;
window.signAndSubmitPQ = signAndSubmitPQ;
window.updatePQBadge = updatePQBadge;
window.persistWalletConnection = persistWalletConnection;
window.clearWalletConnection = clearWalletConnection;
window.getPersistedAddress = getPersistedAddress;

// Auto-connect on load: try signer proxy first, then persisted address
async function autoConnect() {
  // Don't auto-connect if user explicitly disconnected
  if (localStorage.getItem('pichain_disconnected') === '1') return;

  // Try live signer proxy first
  try {
    const result = await connectPQProxy();
    if (result.connected) {
      persistWalletConnection(result.address);
      updatePQBadge();
      return;
    }
  } catch {}

  // Check for PiBot session (can sign via vault)
  const pibotTid = localStorage.getItem('pichain_pibot_tid');
  const savedAddr = getPersistedAddress();
  if (pibotTid && savedAddr) {
    window.pqProxyConnected = true;
    window.pqProxyAddress = savedAddr;
    window.pqWalletProvider = 'pibot';
    return;
  }

  // Fall back to persisted address (view-only, no signing)
  if (savedAddr) {
    window.pqProxyAddress = savedAddr;
  }
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', autoConnect);
} else {
  autoConnect();
}

})();
