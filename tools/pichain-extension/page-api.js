/**
 * PIChain Extension — Page API (runs in MAIN world)
 *
 * This script runs in the page's JavaScript context, NOT the extension's
 * isolated world. It can set window.__pichain_extension directly without
 * CSP issues because Chrome injects it via world: "MAIN".
 *
 * Communication with the extension's isolated content script happens
 * via window.postMessage.
 */

(function() {
  'use strict';

  let _reqId = 0;
  const _pending = new Map();

  // Listen for responses from the isolated content script
  window.addEventListener('message', (event) => {
    if (event.source !== window) return;
    if (!event.data || event.data.target !== 'pichain-page') return;
    const { id, response, error } = event.data;
    const p = _pending.get(id);
    if (p) {
      _pending.delete(id);
      if (error) p.reject(new Error(error));
      else p.resolve(response);
    }
  });

  function sendToExtension(type, payload) {
    return new Promise((resolve, reject) => {
      const id = ++_reqId;
      _pending.set(id, { resolve, reject });
      window.postMessage({
        target: 'pichain-extension',
        type,
        id,
        payload
      }, window.location.origin);
      setTimeout(() => {
        if (_pending.has(id)) {
          _pending.delete(id);
          reject(new Error('Extension request timed out'));
        }
      }, 30000);
    });
  }

  // Expose the extension API to the page
  window.__pichain_extension = {
    isAvailable: true,
    address: null,

    async connect() {
      const resp = await sendToExtension('connect', {});
      if (resp.error) throw new Error(resp.error);
      this.address = resp.address;
      return resp;
    },

    async sign(txData) {
      const resp = await sendToExtension('sign', { tx_data: txData });
      if (resp.error) throw new Error(resp.error);
      return resp.signed_tx;
    },

    async getAddress() {
      const resp = await sendToExtension('get_address', {});
      if (resp.error) throw new Error(resp.error);
      this.address = resp.address;
      return resp.address;
    },

    async unlock(password) {
      const resp = await sendToExtension('unlock', { password });
      if (resp.error) throw new Error(resp.error);
      this.address = resp.address;
      return resp;
    }
  };

  console.log('[PIChain] Extension wallet API injected');
})();
