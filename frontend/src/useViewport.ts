// Responsive breakpoints for the inline-style UI. Inline styles can't express
// media queries, so components read these booleans and branch their styles.
// `isCompact` (phones + tablets) is the primary layout switch — both get the
// mobile chrome (top title bar + bottom nav, touch sheets, stacked cards) so a
// wall-mounted tablet works as a dedicated control fixture. `isMobile` (phones
// only) is reserved for the few things a phone genuinely can't fit (Floor Plan).

import { useSyncExternalStore } from "react";

// `tablet` covers landscape smart-display fixtures too — a Nest Hub Max is
// 1280×800, so the cap is 1280 (not 1024) or those revert to the desktop layout.
export const BREAKPOINTS = { mobile: 640, tablet: 1280 };
// Touch devices a little wider than the width cap (a landscape Surface, a large
// kiosk tablet) are still fixtures, so a coarse pointer up to this width counts
// as compact even though a fine-pointer laptop at the same width would not.
const COARSE_TABLET_MAX = 1366;

/** Subscribe to an arbitrary media query — for the rare layout that needs a
 * width tier beyond the shared booleans (e.g. masonry column count on very
 * wide monitors). Prefer `useViewport()` for the standard breakpoints. */
export function useMediaQuery(query: string, serverValue = false): boolean {
  return useMedia(query, serverValue);
}

function useMedia(query: string, serverValue = false): boolean {
  return useSyncExternalStore(
    (cb) => {
      const mql = window.matchMedia(query);
      mql.addEventListener("change", cb);
      return () => mql.removeEventListener("change", cb);
    },
    () => window.matchMedia(query).matches,
    () => serverValue,
  );
}

export interface Viewport {
  /** Phone-width: ≤ 640px. Reserved for phone-only behaviour (e.g. hiding the
   * Floor Plan). For general layout, prefer `isCompact`. */
  isMobile: boolean;
  /** Tablet / smart-display fixture: 641–1280px, plus wider touch screens.
   * Gets the compact (mobile) chrome, not desktop. */
  isTablet: boolean;
  /** Above the tablet range with a fine pointer (a real desktop/laptop). */
  isDesktop: boolean;
  /** Phones + tablets/fixtures. The primary layout switch: compact chrome,
   * touch sheets, stacked cards. Tablets are control fixtures, so they share
   * the mobile layout. */
  isCompact: boolean;
}

export function useViewport(): Viewport {
  const isMobile = useMedia(`(max-width: ${BREAKPOINTS.mobile}px)`);
  const isTabletWidth = useMedia(
    `(min-width: ${BREAKPOINTS.mobile + 1}px) and (max-width: ${BREAKPOINTS.tablet}px)`,
  );
  // A coarse (touch) pointer on a fixture-sized screen beyond the width cap.
  const isCoarseTablet = useMedia(
    `(pointer: coarse) and (min-width: ${BREAKPOINTS.tablet + 1}px) and (max-width: ${COARSE_TABLET_MAX}px)`,
  );
  const isTablet = !isMobile && (isTabletWidth || isCoarseTablet);
  return {
    isMobile,
    isTablet,
    isDesktop: !isMobile && !isTablet,
    isCompact: isMobile || isTablet,
  };
}
