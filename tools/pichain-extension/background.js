/**
 * PIChain Extension — Background Service Worker
 *
 * Routes signing requests to the native messaging host (pichain-signer)
 * or falls back to the HTTP signing proxy on localhost:8315.
 *
 * Handles Chrome's MV3 service worker lifecycle — workers can be terminated
 * after 30s of inactivity, so native ports are reconnected on demand.
 */

const NATIVE_HOST = 'net.pichain.signer';
const PROXY_URL = 'http://127.0.0.1:8315';
const CONNECT_TIMEOUT = 3000;

let nativePort = null;
let walletAddress = null;
let useHttpFallback = false;

// ─── Native Messaging ───

function connectNative() {
  if (nativePort) return true;
  try {
    nativePort = chrome.runtime.connectNative(NATIVE_HOST);

    nativePort.onMessage.addListener((msg) => {
      if (msg.id && pendingNative.has(msg.id)) {
        const p = pendingNative.get(msg.id);
        pendingNative.delete(msg.id);
        if (msg.error) p.reject(new Error(msg.error));
        else p.resolve(msg);
      }
    });

    nativePort.onDisconnect.addListener(() => {
      const err = chrome.runtime.lastError?.message || 'disconnected';
      console.log('[PQ] Native host disconnected:', err);
      nativePort = null;
      // If it was never connected (host not found), use HTTP fallback
      if (err.includes('not found') || err.includes('not installed')) {
        useHttpFallback = true;
      }
    });

    return true;
  } catch (e) {
    console.log('[PQ] Native messaging unavailable:', e.message);
    nativePort = null;
    useHttpFallback = true;
    // Notify user via badge that HTTP fallback is in use
    chrome.action?.setBadgeText({ text: 'HTTP' });
    chrome.action?.setBadgeBackgroundColor({ color: '#f59e0b' }); // amber warning
    return false;
  }
}

const pendingNative = new Map();
let nativeReqId = 0;

function sendToNative(msg) {
  return new Promise((resolve, reject) => {
    // Reconnect if worker was restarted by Chrome
    if (!nativePort) {
      if (!connectNative() || !nativePort) {
        reject(new Error('native_unavailable'));
        return;
      }
    }
    const id = ++nativeReqId;
    msg.id = id;
    pendingNative.set(id, { resolve, reject });
    try {
      nativePort.postMessage(msg);
    } catch (e) {
      pendingNative.delete(id);
      nativePort = null;
      reject(new Error('native_unavailable'));
      return;
    }
    setTimeout(() => {
      if (pendingNative.has(id)) {
        pendingNative.delete(id);
        reject(new Error('Native host timeout'));
      }
    }, 30000);
  });
}

// ─── HTTP Proxy Fallback ───

async function httpRequest(path, body) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), CONNECT_TIMEOUT);
  try {
    const opts = body
      ? { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal: controller.signal }
      : { signal: controller.signal };
    const resp = await fetch(PROXY_URL + path, opts);
    clearTimeout(timeout);
    const data = await resp.json();
    if (data.error) throw new Error(data.error);
    return data;
  } catch (e) {
    clearTimeout(timeout);
    throw e;
  }
}

// Try native first, fall back to HTTP
async function tryNativeOrHttp(nativeMsg, httpPath, httpBody) {
  if (!useHttpFallback) {
    try {
      return await sendToNative(nativeMsg);
    } catch (e) {
      if (e.message === 'native_unavailable') {
        useHttpFallback = true;
      } else {
        throw e;
      }
    }
  }
  return await httpRequest(httpPath, httpBody);
}

// ─── Message Handler ───

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  handleMessage(message)
    .then(resp => sendResponse(resp))
    .catch(e => sendResponse({ error: e.message }));
  return true; // async
});

async function handleMessage(message) {
  switch (message.type) {
    case 'connect': {
      const resp = await tryNativeOrHttp(
        { type: 'status' },
        '/status',
        null
      );
      walletAddress = resp.address;
      // Save to storage for popup
      chrome.storage.local.set({
        walletAddress: resp.address,
        connected: true,
        provider: useHttpFallback ? 'proxy' : 'native'
      });
      return {
        address: resp.address,
        connected: true,
        provider: useHttpFallback ? 'proxy' : 'native'
      };
    }

    case 'get_address': {
      if (walletAddress) return { address: walletAddress };
      const resp = await tryNativeOrHttp({ type: 'status' }, '/status', null);
      walletAddress = resp.address;
      return { address: walletAddress };
    }

    case 'sign': {
      const resp = await tryNativeOrHttp(
        { type: 'sign', tx_data: message.payload.tx_data },
        '/sign',
        { tx_data: message.payload.tx_data }
      );
      return { signed_tx: resp.signed_tx, tx_hash: resp.tx_hash };
    }

    case 'unlock': {
      if (useHttpFallback) {
        const status = await httpRequest('/status', null);
        walletAddress = status.address;
        chrome.storage.local.set({ walletAddress: status.address, connected: true });
        return { ok: true, address: status.address };
      }
      const resp = await sendToNative({
        type: 'unlock',
        password: message.payload.password,
      });
      walletAddress = resp.address;
      chrome.storage.local.set({ walletAddress: resp.address, connected: true });
      return { ok: resp.ok, address: resp.address };
    }

    case 'get_state': {
      // For popup to read current state
      const data = await chrome.storage.local.get(['walletAddress', 'connected', 'provider']);
      return data;
    }

    default:
      throw new Error(`Unknown message type: ${message.type}`);
  }
}

// Keep-alive: Chrome MV3 workers terminate after 30s inactivity.
// The alarm API keeps the worker alive when a wallet is connected.
chrome.alarms?.create('keepalive', { periodInMinutes: 0.4 });
chrome.alarms?.onAlarm.addListener((alarm) => {
  if (alarm.name === 'keepalive' && walletAddress) {
    // Lightweight ping to keep native port alive
    if (nativePort) {
      try { nativePort.postMessage({ type: 'ping' }); } catch(e) { nativePort = null; }
    }
  }
});

// Try connecting on startup
connectNative();
