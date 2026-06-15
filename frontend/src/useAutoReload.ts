// Self-update for the kiosk. A wall-mounted tablet can't be poked with adb once
// it's in use, so the frontend watches for a newly-deployed build and reloads
// itself — picking up a pushed update on its own within ~a minute.
//
// Signal: the **hashed entry bundle** the served `index.html` references
// (e.g. `/assets/index-C0n379Pz.js`). Vite rehashes it on every build, so a
// changed name means a new frontend is live — more reliable than a version
// string (works even if `Cargo.toml`'s version wasn't bumped).

import { useEffect } from "react";

const POLL_MS = 60_000;

/** The hashed entry bundle `index.html` loads, or `null` if not found. */
function entryBundle(html: string): string | null {
  return html.match(/\/assets\/index-[\w-]+\.js/)?.[0] ?? null;
}

/** Reload the page when the deployed frontend build changes. */
export function useAutoReloadOnNewBuild(intervalMs = POLL_MS) {
  useEffect(() => {
    let baseline: string | null = null;
    let alive = true;
    const check = async () => {
      try {
        // Cache-bust + no-store so we always see the freshly-served shell.
        const res = await fetch(`/?_=${Date.now()}`, { cache: "no-store" });
        if (!res.ok) return;
        const bundle = entryBundle(await res.text());
        if (!alive || !bundle) return;
        if (baseline === null) {
          baseline = bundle; // the build we're currently running
        } else if (bundle !== baseline) {
          // Don't yank the page mid-typing (e.g. the login field) — catch it
          // on the next tick instead.
          const el = document.activeElement;
          if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA")) return;
          window.location.reload();
        }
      } catch {
        // Offline / transient — try again next tick.
      }
    };
    check();
    const t = setInterval(check, intervalMs);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [intervalMs]);
}
