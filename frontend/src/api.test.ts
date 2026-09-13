// The kiosk session-recovery path in the api layer.
//
// A dashboard session is a 7-day absolute expiry that nothing renews, while a
// wall tablet's WebView stays loaded for weeks — so a kiosk's session lapses
// under a running page as a matter of course. Until this existed the exchange
// happened only in the app's boot path, so the page could not re-authenticate
// without being reloaded: the board froze, its buttons 401'd, and a reload was
// the only cure. These pin the recovery so that can't regress into a silent
// months-long staleness bug again.
//
// `IS_KIOSK` is read from the user-agent at module load, so each test stubs the
// UA and re-imports the module.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const REAL_UA = navigator.userAgent;

function setUserAgent(ua: string) {
  Object.defineProperty(navigator, "userAgent", { value: ua, configurable: true });
}

/** A fetch stub that 401s every session-gated call until the kiosk exchange
 * runs, exactly as the hub does once a session row expires. */
function hubWithExpiredSession() {
  let authed = false;
  const calls: string[] = [];
  const fetchMock = vi.fn(async (input: string) => {
    calls.push(input);
    if (input === "/api/auth/kiosk") {
      authed = true;
      return new Response(null, { status: 204 });
    }
    if (!authed) return new Response("unauthorized", { status: 401 });
    return new Response(JSON.stringify([{ id: "l1" }]), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  });
  return { calls, fetchMock, authed: () => authed };
}

beforeEach(() => vi.resetModules());
afterEach(() => {
  setUserAgent(REAL_UA);
  vi.unstubAllGlobals();
});

describe("a kiosk whose session has lapsed", () => {
  it("exchanges its key for a session and retries the call", async () => {
    setUserAgent("Mozilla/5.0 (Linux; Android 15; SM-X218U) BifrostKiosk/1.4");
    const hub = hubWithExpiredSession();
    vi.stubGlobal("fetch", hub.fetchMock);
    const api = await import("./api");

    expect(api.IS_KIOSK).toBe(true);
    await expect(api.getLights()).resolves.toEqual([{ id: "l1" }]);
    expect(hub.calls).toEqual(["/api/lights", "/api/auth/kiosk", "/api/lights"]);
  });

  it("mints one session however many calls 401 together", async () => {
    // A waking board fires several reads at once. Each minting its own session
    // would leave a pile of rows behind for a single lapse.
    setUserAgent("BifrostKiosk/1.4");
    const hub = hubWithExpiredSession();
    vi.stubGlobal("fetch", hub.fetchMock);
    const api = await import("./api");

    await Promise.all([api.getLights(), api.getLights(), api.getLights()]);

    expect(hub.calls.filter((c) => c === "/api/auth/kiosk")).toHaveLength(1);
  });

  it("gives up after one exchange when the key is no longer valid", async () => {
    // A deauthorized kiosk must surface the 401 to the caller (which sends the
    // app to the login screen) rather than loop against a hub refusing it: one
    // exchange attempt, no retry of the original call.
    setUserAgent("BifrostKiosk/1.4");
    const calls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string) => {
        calls.push(input);
        return new Response("unauthorized", { status: 401 });
      }),
    );
    const api = await import("./api");

    await expect(api.getLights()).resolves.toBe("unauthorized");
    expect(calls).toEqual(["/api/lights", "/api/auth/kiosk"]);
  });
});

describe("an ordinary browser", () => {
  it("does not try the kiosk exchange on a 401", async () => {
    // Its 401 is the pre-login state, and the exchange would fail anyway — the
    // login screen is the right answer.
    setUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/152.0.0.0");
    const hub = hubWithExpiredSession();
    vi.stubGlobal("fetch", hub.fetchMock);
    const api = await import("./api");

    expect(api.IS_KIOSK).toBe(false);
    await expect(api.getLights()).resolves.toBe("unauthorized");
    expect(hub.calls).toEqual(["/api/lights"]);
  });
});
