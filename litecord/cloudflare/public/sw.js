// Lets LiteCord install as its own app and open its window even before the network is back.
// It only keeps a copy of the app's own files; chat never goes through here.
const CACHE = "litecord-v1";
const FILES = ["/", "/manifest.webmanifest", "/icon.svg", "/icon-192.png", "/icon-512.png"];

self.addEventListener("install", e => {
  e.waitUntil(caches.open(CACHE).then(c => c.addAll(FILES)).then(() => self.skipWaiting()));
});
self.addEventListener("activate", e => {
  e.waitUntil(caches.keys().then(keys => Promise.all(keys.filter(k => k !== CACHE).map(k => caches.delete(k))))
    .then(() => self.clients.claim()));
});
// Always try the network first, so updates show up right away.
self.addEventListener("fetch", e => {
  const url = new URL(e.request.url);
  if (e.request.method !== "GET" || url.origin !== location.origin || url.pathname === "/relay") return;
  e.respondWith(fetch(e.request).then(res => {
    if (res.ok && FILES.includes(url.pathname)) {
      const copy = res.clone();
      caches.open(CACHE).then(c => c.put(url.pathname, copy));
    }
    return res;
  }).catch(() => caches.match(url.pathname === "/" || url.pathname === "/index.html" ? "/" : url.pathname)));
});
