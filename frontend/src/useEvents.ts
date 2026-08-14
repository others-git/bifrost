// One shared `/api/events` SSE connection for the whole app, instead of every
// page opening its own. Two problems this fixes at once:
//
// 1. Pool pressure: the server is plain HTTP/1.1, so Chromium/WebView caps
//    concurrent connections per origin at 6. App.tsx keeps one connection open
//    for the whole session; every page used to open a SECOND one on top of
//    that. A kiosk sitting on one page permanently pinned two slots.
// 2. Zombie connections: none of the old per-page EventSources ever detected
//    or recovered from a connection that looks OPEN but is actually dead — the
//    classic outcome of a WebView's networking/timers being suspended and
//    resumed around a screen-off/on cycle (exactly what the kiosk's own
//    display scheduler does). Nothing ever reopened it short of a full
//    component remount, which is why "close the board and reopen it" was the
//    only fix.
//
// This hook collapses every consumer onto one ref-counted EventSource and
// actively reconnects: on a detected error (backoff), whenever the page becomes
// visible or the network comes back online, and — the backstop that needs no
// guess about *why* a stream died — whenever the server's heartbeat stops
// arriving. A wall tablet holds one connection open for days, so "it looks
// open" is never enough; silence is the only reliable liveness signal.

import { useEffect, useRef } from "react";

export type BifrostEventName = "light_state" | "media_state" | "power_state" | "sensor_state" | "inventory";

type Handler = (raw: MessageEvent) => void;

const EVENT_NAMES: BifrostEventName[] = ["light_state", "media_state", "power_state", "sensor_state", "inventory"];

/** The server's liveness beat (`api::events::HEARTBEAT`, every 20s). Not a
 * subscribable event — nothing but the watchdog below cares — but it is a real
 * named event on purpose: `EventSource` never surfaces the SSE keep-alive
 * comment, so without it a silently-dead stream is indistinguishable from a
 * quiet house. */
const HEARTBEAT_EVENT = "hb";

const listeners = new Map<BifrostEventName, Set<Handler>>(EVENT_NAMES.map((name) => [name, new Set()]));

let es: EventSource | null = null;
let refCount = 0;
let backoffMs = 1000;
const MAX_BACKOFF_MS = 30_000;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

// ── Liveness watchdog ───────────────────────────────────────────────────────
// A connection can be OPEN and dead: a suspended WebView's socket, a stream the
// server stopped feeding. The server beats every 20s, so silence well past that
// means the stream is gone whether or not the browser noticed — reconnect on
// our own authority instead of waiting for an `onerror` that may never come.
const SILENCE_LIMIT_MS = 70_000;
const WATCHDOG_INTERVAL_MS = 15_000;
let lastSeenAt = 0;
let watchdog: ReturnType<typeof setInterval> | null = null;

function dispatch(name: BifrostEventName, raw: MessageEvent) {
  listeners.get(name)?.forEach((fn) => fn(raw));
}

function clearReconnectTimer() {
  if (reconnectTimer !== null) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
}

function connect() {
  if (es) return;
  const conn = new EventSource("/api/events");
  lastSeenAt = Date.now();
  EVENT_NAMES.forEach((name) => {
    conn.addEventListener(name, (raw) => {
      backoffMs = 1000;
      lastSeenAt = Date.now();
      dispatch(name, raw as MessageEvent);
    });
  });
  conn.addEventListener(HEARTBEAT_EVENT, () => {
    backoffMs = 1000;
    lastSeenAt = Date.now();
  });
  conn.onopen = () => {
    lastSeenAt = Date.now();
  };
  conn.onerror = () => {
    conn.close();
    if (es === conn) es = null;
    scheduleReconnect();
  };
  es = conn;
}

function scheduleReconnect() {
  if (refCount <= 0 || reconnectTimer !== null) return;
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    if (refCount > 0) connect();
  }, backoffMs);
  backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS);
}

/** Force a fresh connection now — used when we have a specific reason to
 * believe the current one may be a silent zombie (visibility/online), rather
 * than waiting for the browser to notice on its own. */
function reconnectNow() {
  if (refCount <= 0) return;
  clearReconnectTimer();
  es?.close();
  es = null;
  backoffMs = 1000;
  connect();
}

function teardown() {
  clearReconnectTimer();
  stopWatchdog();
  es?.close();
  es = null;
}

function startWatchdog() {
  if (watchdog !== null) return;
  watchdog = setInterval(() => {
    // Only meaningful while we believe we're connected: a stream waiting on its
    // backoff has no beat to miss.
    if (refCount <= 0 || !es) return;
    if (Date.now() - lastSeenAt > SILENCE_LIMIT_MS) reconnectNow();
  }, WATCHDOG_INTERVAL_MS);
}

function stopWatchdog() {
  if (watchdog !== null) {
    clearInterval(watchdog);
    watchdog = null;
  }
}

// Only force a reconnect on a visibility flip if the page was actually hidden
// long enough to plausibly be the WebView-suspended-networking case (a
// screen-off/on cycle, minutes+) — not every ordinary tab-switch. A blind
// reconnect on every focus would tear down a perfectly healthy connection for
// desktop users alt-tabbing every few seconds, adding churn and a brief
// missed-event window for no reason.
const VISIBILITY_RECONNECT_THRESHOLD_MS = 10_000;
let hiddenAt: number | null =
  typeof document !== "undefined" && document.visibilityState === "hidden" ? Date.now() : null;

if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") {
      hiddenAt = Date.now();
      return;
    }
    const wasHiddenMs = hiddenAt === null ? 0 : Date.now() - hiddenAt;
    hiddenAt = null;
    if (wasHiddenMs >= VISIBILITY_RECONNECT_THRESHOLD_MS) reconnectNow();
  });
}
// Going offline→online is a strong signal the connection actually dropped
// (unlike a mere visibility flip), so this one always reconnects.
if (typeof window !== "undefined") {
  window.addEventListener("online", reconnectNow);
}

/**
 * Subscribe to the shared `/api/events` stream. `handlers` is read via a ref
 * on every dispatch, so the latest closures (and the latest set of handler
 * keys) are always in effect, even though the underlying subscription is
 * only set up once per mount/`enabled` change. Pass `enabled: false` to skip
 * subscribing (and holding open the shared connection) entirely, e.g. while
 * not yet logged in.
 */
export function useEvents(handlers: Partial<Record<BifrostEventName, Handler>>, opts?: { enabled?: boolean }) {
  const enabled = opts?.enabled ?? true;
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;

  useEffect(() => {
    if (!enabled) return;
    refCount++;
    connect();
    startWatchdog();

    // Subscribe to every event name, not just the ones present in `handlers`
    // right now — the wrapper checks handlersRef.current at DISPATCH time, so
    // a caller whose handler set changes across renders (e.g. a conditional
    // key) stays correctly subscribed without needing the effect to re-run.
    const wrapped = new Map<BifrostEventName, Handler>();
    EVENT_NAMES.forEach((name) => {
      const fn: Handler = (raw) => handlersRef.current[name]?.(raw);
      wrapped.set(name, fn);
      listeners.get(name)!.add(fn);
    });

    return () => {
      wrapped.forEach((fn, name) => listeners.get(name)!.delete(fn));
      refCount--;
      if (refCount <= 0) teardown();
    };
    // Subscribes once per mount/enable; handlersRef always holds the latest closures.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled]);
}
