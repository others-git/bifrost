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

  const panelStyle: CSSProperties = isCompact
    ? { ...sheetStyle, zIndex: 61, maxHeight: "60vh" }
    : {
        position: "fixed",
        left: pos?.left ?? -9999,
        top: pos?.top ?? -9999,
        visibility: pos ? "visible" : "hidden",
        zIndex: 61,
        width,
        maxHeight: 300,
        overflowY: "auto",
        ...menuSurface,
      };

  return createPortal(
    <>
      <div onClick={onClose} style={{ position: "fixed", inset: 0, zIndex: 60 }} />
      <div ref={ref} style={panelStyle}>
        {children}
      </div>
    </>,
    document.body,
  );
}
