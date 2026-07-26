// Self-hosted variable font (no CDN — works offline). "Inter Variable" has a
// large x-height that stays legible at the small UI sizes used throughout.
import "@fontsource-variable/inter";
// Engraved-Roman gothic display face for headings / uppercase labels — the
// victorian-gothic note (see theme.ts `font.display`). 600 = the label weight.
import "@fontsource/cinzel/400.css";
import "@fontsource/cinzel/600.css";
import "@fontsource/cinzel/700.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { initTheme } from "./theme";

// Paint the saved (or default) theme's CSS variables onto <html> before the
// first render, so every var()-based token resolves on the first paint.
initTheme();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>
);

// PWA service worker (production only — avoids caching dev/HMR).
//
// NOT on a kiosk. A wall fixture must never be served anything stale: the
// kiosk app has its own offline overlay + retry, so the SW's offline shell
// buys nothing there and once pinned an old shell silently outlives every
// deploy (posters that "never load" because the bundle predates the fix).
// Kiosks actively unregister any previously-installed worker and drop its
// caches, so an already-pinned fixture self-heals on its next real load.
const IS_KIOSK_CLIENT = /\bBifrostKiosk\//.test(navigator.userAgent);
if (import.meta.env.PROD && "serviceWorker" in navigator) {
  if (IS_KIOSK_CLIENT) {
    navigator.serviceWorker
      .getRegistrations()
      .then((regs) => Promise.all(regs.map((r) => r.unregister())))
      .catch(() => {});
    if ("caches" in window) {
      caches
        .keys()
        .then((keys) => Promise.all(keys.map((k) => caches.delete(k))))
        .catch(() => {});
    }
  } else {
    window.addEventListener("load", () => {
      navigator.serviceWorker.register("/sw.js").catch(() => {});
    });
  }
}
