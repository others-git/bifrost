import { afterEach, describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import { boardsAvailable, INSTALLED_QUERY, PHONE_QUERY, useInstalled, usePhone } from "./pwa";

function stubMatchMedia(matching: string | null) {
  vi.spyOn(window, "matchMedia").mockImplementation(
    (query: string) =>
      ({
        matches: query === matching,
        media: query,
        onchange: null,
        addEventListener: () => {},
        removeEventListener: () => {},
        addListener: () => {},
        removeListener: () => {},
        dispatchEvent: () => false,
      }) as MediaQueryList,
  );
}

afterEach(() => {
  vi.restoreAllMocks();
  delete (navigator as Navigator & { standalone?: boolean }).standalone;
});

describe("useInstalled", () => {
  it("is false in a browser tab", () => {
    stubMatchMedia(null);
    expect(renderHook(() => useInstalled()).result.current).toBe(false);
  });

  it("is true when an installed display mode matches (Android, Windows)", () => {
    stubMatchMedia(INSTALLED_QUERY);
    expect(renderHook(() => useInstalled()).result.current).toBe(true);
  });

  it("honours iOS's legacy navigator.standalone", () => {
    stubMatchMedia(null);
    Object.defineProperty(navigator, "standalone", { value: true, configurable: true });
    expect(renderHook(() => useInstalled()).result.current).toBe(true);
  });
});

describe("usePhone", () => {
  it("is true when the phone query matches", () => {
    stubMatchMedia(PHONE_QUERY);
    expect(renderHook(() => usePhone()).result.current).toBe(true);
  });

  it("is false otherwise (tablet, desktop)", () => {
    stubMatchMedia(null);
    expect(renderHook(() => usePhone()).result.current).toBe(false);
  });
});

describe("boardsAvailable", () => {
  it("hides Boards on an installed phone app", () => {
    expect(boardsAvailable(true, true, false)).toBe(false);
  });

  it("keeps Boards on an installed tablet or desktop app", () => {
    expect(boardsAvailable(true, false, false)).toBe(true);
  });

  it("keeps Boards in a phone browser tab", () => {
    expect(boardsAvailable(false, true, false)).toBe(true);
  });

  it("never hides Boards from a kiosk, whose assigned board is its home page", () => {
    expect(boardsAvailable(true, true, true)).toBe(true);
  });
});
