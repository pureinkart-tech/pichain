/**
 * PIChain Wallet Extension — Popup Controller
 */

const $ = id => document.getElementById(id);

function showSection(name) {
  ['connect', 'wallet', 'error'].forEach(id => {
    const el = $(id + '-section');
    if (el) el.style.display = 'none';
  });
  const target = $(name + '-section');
  if (target) target.style.display = 'block';
}

function showError(msg) {
  showSection('error');
  const el = $('error-msg');
  if (el) el.textContent = msg;
}

async function sendMsg(type, payload = {}) {
  return new Promise((resolve, reject) => {
    chrome.runtime.sendMessage({ type, payload }, (response) => {
      if (chrome.runtime.lastError) {
        reject(new Error(chrome.runtime.lastError.message));
      } else {
        resolve(response || {});
      }
    });
  });
}

function showConnected(address, provider) {
  $('wallet-address').textContent = address;
  $('provider-name').textContent = provider === 'native' ? 'Native Signer' : 'HTTP Proxy';
  showSection('wallet');
  // Update badge
  chrome.action?.setBadgeText({ text: 'PQ' });
  chrome.action?.setBadgeBackgroundColor({ color: '#22c55e' });
}

// ─── Connect ───

$('btn-connect').addEventListener('click', async () => {
  const status = $('conn-status');
  status.textContent = 'Connecting...';
  status.className = 'status-val';

  try {
    const resp = await sendMsg('connect');
    if (resp.error) throw new Error(resp.error);
    if (resp.address) {
      showConnected(resp.address, resp.provider);
    } else {
      throw new Error('No wallet address returned');
    }
  } catch (e) {
    status.textContent = 'Failed';
    status.className = 'status-val disconnected';
    showError(
      e.message.includes('native') || e.message.includes('proxy') || e.message.includes('fetch')
        ? 'Cannot connect to PIChain Signer.\n\n1. Download pichain-signer from pichain.net/download\n2. Run: pichain-signer --wallet wallet.json --generate\n3. Click Connect again'
        : e.message
    );
  }
});

// ─── Copy Address ───

$('btn-copy').addEventListener('click', () => {
  const addr = $('wallet-address').textContent;
  navigator.clipboard.writeText(addr).then(() => {
    $('btn-copy').textContent = 'Copied!';
    setTimeout(() => $('btn-copy').textContent = 'Copy Address', 2000);
  });
});

// ─── Disconnect ───

$('btn-disconnect').addEventListener('click', () => {
  chrome.storage.local.remove(['walletAddress', 'connected', 'provider']);
  chrome.action?.setBadgeText({ text: '' });
  // Clear sensitive data from DOM
  $('wallet-address').textContent = '';
  $('provider-name').textContent = '';
  showSection('connect');
  $('conn-status').textContent = 'Not Connected';
  $('conn-status').className = 'status-val disconnected';
});

// ─── Back from error ───

$('btn-back')?.addEventListener('click', () => {
  showSection('connect');
});

// ─── Auto-check on popup open ───

(async () => {
  try {
    // Check saved state first
    const state = await sendMsg('get_state');
    if (state.connected && state.walletAddress) {
      // Verify still actually connected
      try {
        const resp = await sendMsg('connect');
        if (resp.address) {
          showConnected(resp.address, resp.provider || state.provider);
          return;
        }
      } catch(e) { console.log('[PQ] Reconnect failed:', e.message); }
    }
    // Try auto-connect
    const resp = await sendMsg('connect');
    if (resp && resp.address && !resp.error) {
      showConnected(resp.address, resp.provider);
    }
  } catch (e) {
    // Not connected — show connect button (default state)
  }
})();
