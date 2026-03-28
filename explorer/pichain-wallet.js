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
  localStorage.setItem('pichain_disconnected', '1');
  window.pqProxyConnected = false;
  window.pqProxyAddress = null;
  window.pqWalletProvider = 'none';
}

function getPersistedAddress() {
  if (localStorage.getItem('pichain_disconnected') === '1') return null;
  return localStorage.getItem('pichain_connected_address');
}

// ─── Expose globally ───

window.connectPQProxy = connectPQProxy;
window.signAndBuildPQ = signAndBuildPQ;
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

  // Fall back to persisted address (view-only, no signing)
  const saved = getPersistedAddress();
  if (saved) {
    window.pqProxyAddress = saved;
    // Don't set pqProxyConnected = true since signer isn't available for signing
    // Pages can still show balance and read-only data
  }
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', autoConnect);
} else {
  autoConnect();
}

})();
