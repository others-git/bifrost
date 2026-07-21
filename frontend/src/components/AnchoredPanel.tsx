// A dropdown panel portaled to <body> and anchored *below* its trigger button
// (right-aligned, flipping up when it would overflow). Portaling matters: a card
// may be dimmed (`opacity` for offline/disabled), and a child can't escape an
// ancestor's opacity — so an in-card popover would render translucent. On compact
// viewports it's a full-width bottom sheet. Wears the shared `menuSurface` look,
// so its dropdowns match the `Select` dropdowns.
//
// This is the icon-triggered cousin of `Flyout` (which anchors to the side): use
// `AnchoredPanel` for a menu that drops from a small trigger in a dense row.

import { useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useViewport } from "../useViewport";
import { sheetStyle } from "./sheet";
import { menuSurface } from "./Select";

export function AnchoredPanel({
  anchor,
  isCompact,
  width = 200,
  onClose,
  children,
}: {
  anchor: HTMLElement | null;
  isCompact: boolean;
  width?: number;
  onClose: () => void;
  children: ReactNode;
}) {
  const { isMobile } = useViewport();
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    if (isCompact || !anchor || !ref.current) return;
    const rect = anchor.getBoundingClientRect();
    const w = ref.current.offsetWidth;
    const h = ref.current.offsetHeight;
    let left = Math.min(rect.right - w, window.innerWidth - w - 8); // right-aligned
    left = Math.max(8, left);
    let top = rect.bottom + 6;
    if (top + h > window.innerHeight - 8) top = rect.top - 6 - h; // flip up if needed
    top = Math.max(8, top);
    setPos({ left, top });
  }, [anchor, isCompact]);

  // Tablet: a screen-centred modal over a dimming scrim — matching `Flyout`, so
  // the pickers don't read as a different element than the control fly-outs.
  if (isCompact && !isMobile) {
    return createPortal(
      <div
        onClick={onClose}
        style={{
          position: "fixed",
          inset: 0,
          zIndex: 150,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          padding: "1.5rem",
          background: "rgba(0,0,0,0.5)",
          backdropFilter: "blur(3px)",
          WebkitBackdropFilter: "blur(3px)",
        }}
      >
        <div
          ref={ref}
          onClick={(e) => e.stopPropagation()}
          style={{ width: "70%", maxWidth: 360, maxHeight: "70vh", overflowY: "auto", ...menuSurface }}
        >
          {children}
        </div>
      </div>,
      document.body,
    );
  }

  // Phone: full-width bottom sheet. Desktop: anchored dropdown.
  const panelStyle: CSSProperties = isMobile
    ? { ...sheetStyle, zIndex: 150, maxHeight: "60vh" }
    : {
        position: "fixed",
        left: pos?.left ?? -9999,
        top: pos?.top ?? -9999,
        visibility: pos ? "visible" : "hidden",
        zIndex: 150,
        width,
        maxHeight: 300,
        overflowY: "auto",
        ...menuSurface,
      };

  return createPortal(
    <>
      <div onClick={onClose} style={{ position: "fixed", inset: 0, zIndex: 149 }} />
      <div ref={ref} style={panelStyle}>
        {children}
      </div>
    </>,
    document.body,
  );
}
