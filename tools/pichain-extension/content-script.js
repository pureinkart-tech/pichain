/**
 * PIChain Extension — Content Script (ISOLATED world)
 * Bridges page (MAIN world) ↔ background service worker.
 */
(function() {
  'use strict';

  window.addEventListener('message', async (event) => {
    if (event.source !== window) return;
    if (!event.data || event.data.target !== 'pichain-extension') return;

    const { type, id, payload } = event.data;

    try {
      // Timeout: reject if background doesn't respond in 30 seconds
      const response = await Promise.race([
        chrome.runtime.sendMessage({ type, payload }),
        new Promise((_, reject) =>
          setTimeout(() => reject(new Error('Background worker timeout')), 30000)
        )
      ]);

      window.postMessage({
        target: 'pichain-page',
        id,
        response: response || {}
      }, window.location.origin);
    } catch (e) {
      window.postMessage({
        target: 'pichain-page',
        id,
        error: e.message || 'Extension communication failed'
      }, window.location.origin);
    }
  });
})();
