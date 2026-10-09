// Installed-app (PWA) detection and the decisions that hang off it.

import { useMediaQuery } from "./useViewport";

/** An installed PWA's display modes. `minimal-ui` is deliberately absent — it
 * keeps browser chrome, so it reads as a browser tab rather than an app. */
export const INSTALLED_QUERY =
  "(display-mode: standalone), (display-mode: fullscreen), (display-mode: window-controls-overlay)";

/** True when running as an installed app (home-screen PWA on iOS/Android, an
 * installed Edge/Chrome app on Windows) rather than in a browser tab. iOS
 * Safari only gained the `display-mode` query in 16.4 and still exposes the
 * legacy `navigator.standalone`, so either one counts. The kiosk WebView is
 * neither — it is identified by its user-agent (`IS_KIOSK`), not by this. */
export function useInstalled(): boolean {
  const byQuery = useMediaQuery(INSTALLED_QUERY);
  return byQuery || (navigator as Navigator & { standalone?: boolean }).standalone === true;
}

/** A phone in either orientation: narrow, or a touch screen whose SHORT side
 * is phone-sized. `isMobile` alone is width-only, so a landscape phone (~850px
 * wide) would read as a tablet. Tablets' short side is ≥ ~740px. */
export const PHONE_QUERY = "(max-width: 640px), (pointer: coarse) and (max-height: 640px)";

export function usePhone(): boolean {
  return useMediaQuery(PHONE_QUERY);
}

/** Whether the Boards page is offered. Boards are a tablet's reason to exist
 * (and composed on a desktop); only an installed PHONE app drops them — there
 * a board is unusable and it is one more tab in a crowded bottom bar. Tablets,
 * an installed desktop app (Windows), the kiosk WebView and any browser tab
 * keep them. */
export function boardsAvailable(installed: boolean, isPhone: boolean, isKiosk: boolean): boolean {
  return isKiosk || !(installed && isPhone);
}
