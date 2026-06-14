// Responsive breakpoints for the inline-style UI. Inline styles can't express
// media queries, so components read these booleans and branch their styles.
// `isCompact` (phones + tablets) is the primary layout switch — both get the
// mobile chrome (top title bar + bottom nav, touch sheets, stacked cards) so a
// wall-mounted tablet works as a dedicated control fixture. `isMobile` (phones
// only) is reserved for the few things a phone genuinely can't fit (Floor Plan).

import { useSyncExternalStore } from "react";

export const BREAKPOINTS = { mobile: 640, tablet: 1024 };

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
  /** Tablet: 641–1024px. Gets the compact (mobile) chrome, not desktop. */
  isTablet: boolean;
  /** ≥ 1025px. */
  isDesktop: boolean;
  /** Phones + tablets (≤ 1024px). The primary layout switch: compact chrome,
   * touch sheets, stacked cards. Tablets are control fixtures, so they share
   * the mobile layout. */
  isCompact: boolean;
}

export function useViewport(): Viewport {
  const isMobile = useMedia(`(max-width: ${BREAKPOINTS.mobile}px)`);
  const isTablet = useMedia(
    `(min-width: ${BREAKPOINTS.mobile + 1}px) and (max-width: ${BREAKPOINTS.tablet}px)`,
  );
  return {
    isMobile,
    isTablet,
    isDesktop: !isMobile && !isTablet,
    isCompact: isMobile || isTablet,
  };
}
