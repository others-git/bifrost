// Self-update for the kiosk. A wall-mounted tablet can't be poked with adb once
// it's in use, so the frontend watches for a redeployed server and reloads
// itself — picking up a pushed update on its own within ~a minute.
//
// Signal: the server's **per-process instance id** (`GET /api/instance`), minted
// fresh on every start. A changed id means the server was restarted/redeployed,
// so a new build is live. This is strictly more reliable than watching the
// hashed frontend bundle: the frontend is embedded in the binary, so a
// backend-only redeploy leaves the bundle name unchanged but still bumps the
// instance id — those updates were previously missed.

import { useEffect } from "react";
import { getInstance } from "./api";

const POLL_MS = 60_000;

/** Reload the page when the server's instance id changes (a redeploy). */
export function useAutoReloadOnNewBuild(intervalMs = POLL_MS) {
  useEffect(() => {
    let baseline: string | null = null;
    let alive = true;
    const check = async () => {
      const info = await getInstance(); // null on offline/transient — skip this tick
      if (!alive || !info?.instance_id) return;
      if (baseline === null) {
        baseline = info.instance_id; // the server we connected to
      } else if (info.instance_id !== baseline) {
        // Don't yank the page mid-typing (e.g. the login field) — catch it on
        // the next tick instead.
        const el = document.activeElement;
        if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA")) return;
        window.location.reload();
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
