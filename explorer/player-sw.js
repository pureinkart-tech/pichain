// PIChain Player — Service Worker for offline playback
const CACHE_NAME = 'pichain-player-v1';
const STATIC_ASSETS = ['/', '/index.html'];

// Cache the app shell on install
self.addEventListener('install', e => {
  e.waitUntil(
    caches.open(CACHE_NAME).then(cache => cache.addAll(STATIC_ASSETS))
  );
  self.skipWaiting();
});

// Clean old caches
self.addEventListener('activate', e => {
  e.waitUntil(
    caches.keys().then(keys =>
      Promise.all(keys.filter(k => k !== CACHE_NAME).map(k => caches.delete(k)))
    )
  );
  self.clients.claim();
});

// Serve from cache first (offline-first), fall back to network
self.addEventListener('fetch', e => {
  // Don't cache API calls or audio downloads
  if (e.request.url.includes('/api/') || e.request.url.includes('/music-api/')) return;

  e.respondWith(
    caches.match(e.request).then(cached => cached || fetch(e.request))
  );
});
