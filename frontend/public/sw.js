// Minimal service worker for an installable, fast-loading PWA shell.
//
// Caching contract (the WebView-safe rules — see also `cache_control_for` on
// the server, which is the HTTP-layer half of the same policy):
// - API and SSE (/api/*): never touched — device control must be live.
// - Document loads: network-first; the cached shell is an OFFLINE fallback
//   only. Detection covers `mode === "navigate"` AND `destination ===
//   "document"` — a WebView's app-driven main-frame load doesn't always
//   present as `navigate`, and the old mode-only check dropped those loads
//   into the generic cache-first branch below, serving a STALE SHELL forever
//   (old hashed bundle pinned, reloads useless). That one-line gap is how a
//   wall kiosk kept running a weeks-old build.
// - Hashed assets (/assets/*): cache-first — content-addressed, so staleness
//   is impossible by construction.
// - Everything else (manifest, icons, favicon): passthrough — the server's
//   own no-store headers rule.
//
// CACHE is versioned: bumping it makes `activate` drop every entry the old
// worker ever pinned, and the update itself always reaches clients — the
// browser refetches sw.js bypassing both this worker and the HTTP cache
// (server serves it no-store), even on a page that was being served a stale
// shell. After claiming, controlled pages are re-navigated once so a client
// pinned by the old worker recovers without anyone touching it.

const CACHE = "bifrost-shell-v2";

self.addEventListener("install", () => self.skipWaiting());

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim())
      .then(() => self.clients.matchAll({ type: "window" }))
      .then((clients) =>
        Promise.all(
          clients.map((c) => {
            // Force pages the OLD worker served (possibly a stale shell) onto
            // the fresh one. One-shot per activation; same-origin only.
            try {
              return c.navigate(c.url).catch(() => {});
            } catch {
              return Promise.resolve();
            }
          }),
        ),
      ),
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  if (req.method !== "GET") return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;
  if (url.pathname.startsWith("/api")) return; // never cache API / SSE

  const isDocument = req.mode === "navigate" || req.destination === "document";
  if (isDocument) {
    event.respondWith(
      fetch(req)
        .then((res) => {
          const copy = res.clone();
          caches.open(CACHE).then((c) => c.put("/", copy));
          return res;
        })
        .catch(() => caches.match("/").then((r) => r || caches.match(req))),
    );
    return;
  }

  // Only content-addressed assets are cache-first; anything else follows the
  // server's headers untouched.
  if (!url.pathname.startsWith("/assets/")) return;

  event.respondWith(
    caches.match(req).then(
      (cached) =>
        cached ||
        fetch(req).then((res) => {
          if (res.ok) {
            const copy = res.clone();
            caches.open(CACHE).then((c) => c.put(req, copy));
          }
          return res;
        }),
    ),
  );
});
