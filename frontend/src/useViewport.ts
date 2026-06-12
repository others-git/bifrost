// Responsive breakpoints for the inline-style UI. Inline styles can't express
// media queries, so components read these booleans and branch their styles.
// Per project decision, tablets get the desktop layout — `isMobile` (phones)
// is the only switch most components need.

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
  /** Phone-width: ≤ 640px. The main layout switch. */
  isMobile: boolean;
  /** Tablet: 641–1024px. Treated as desktop, but exposed for fine-tuning. */
  isTablet: boolean;
  /** ≥ 1025px. */
  isDesktop: boolean;
}

export function useViewport(): Viewport {
  const isMobile = useMedia(`(max-width: ${BREAKPOINTS.mobile}px)`);
  const isTablet = useMedia(
    `(min-width: ${BREAKPOINTS.mobile + 1}px) and (max-width: ${BREAKPOINTS.tablet}px)`,
  );
  return { isMobile, isTablet, isDesktop: !isMobile && !isTablet };
}
