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

// Register the PWA service worker (production only — avoids caching dev/HMR).
if (import.meta.env.PROD && "serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/sw.js").catch(() => {});
  });
}
