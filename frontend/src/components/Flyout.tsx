// The shared fly-out shell — one anchored popover / bottom-sheet used by every
// control fly-out (LightEditor, AudioEditor, PowerFlyout). It owns the parts
// those three kept re-implementing: anchor positioning with viewport-flip,
// the compact→bottom-sheet swap, outside-click + Escape close, and the portal to
// <body> (so a dimmed/offline card can't make the fly-out translucent).
//
// `anchor` may be an element (the trigger — clicks on it are ignored so the
// trigger can toggle the fly-out closed) or a screen point ({x, y}, e.g. the
// floor-plan tap location). `width` fixes the desktop popover width; omit it to
// size to content (the color wheel). The compact bottom sheet is always full-width.

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useViewport } from "../useViewport";
import { sheetStyle } from "./sheet";
import { color, radius } from "../theme";

export function Flyout({
  anchor,
  onClose,
  width,
  gap = "0.7rem",
  closeGuard,
  children,
}: {
  anchor: HTMLElement | { x: number; y: number };
  onClose: () => void;
  /** Fixed desktop popover width; omit to size to content. Ignored on compact. */
  width?: number;
  gap?: string;
  /** When it returns true, suppress outside-click / Escape close (e.g. a nested
   * overlay like the remote owns input while open). */
  closeGuard?: () => boolean;
  children: ReactNode;
}) {
  const { isCompact } = useViewport();
  const panelRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    // On compact viewports the fly-out is a bottom sheet — no anchor math.
    if (isCompact) return;
    const panel = panelRef.current;
    if (!panel) return;
    const rect =
      anchor instanceof HTMLElement
        ? anchor.getBoundingClientRect()
        : new DOMRect(anchor.x, anchor.y, 0, 0);
    const w = panel.offsetWidth;
    const h = panel.offsetHeight;
    let left = rect.right + 12;
    if (left + w > window.innerWidth - 8) left = rect.left - 12 - w; // flip to the left
    left = Math.max(8, Math.min(window.innerWidth - w - 8, left));
    let top = rect.top + rect.height / 2 - h / 2;
    top = Math.max(8, Math.min(window.innerHeight - h - 8, top));
    setPos({ left, top });
  }, [anchor, isCompact]);

  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (closeGuard?.()) return;
      const target = e.target as Node;
      if (panelRef.current?.contains(target)) return;
      // Clicks on the trigger are handled by the trigger itself (so it can toggle
      // the fly-out closed); don't also self-close here.
      if (anchor instanceof HTMLElement && anchor.contains(target)) return;
      onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !closeGuard?.()) onClose();
    };
    // Defer so the click that opened the fly-out doesn't immediately close it.
    const t = setTimeout(() => document.addEventListener("pointerdown", onDown), 0);
    window.addEventListener("keydown", onKey);
    return () => {
      clearTimeout(t);
      document.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose, anchor]);

  return createPortal(
    <div
      ref={panelRef}
      style={
        isCompact
          ? sheetStyle
          : {
              position: "fixed",
              left: pos?.left ?? 0,
              top: pos?.top ?? 0,
              visibility: pos ? "visible" : "hidden",
              zIndex: 60,
              width,
              background: color.surface,
              border: `1px solid ${color.hairline}`,
              borderRadius: radius.frame,
              padding: "0.9rem",
              boxShadow: "0 12px 34px rgba(0,0,0,0.7)",
              display: "flex",
              flexDirection: "column",
              gap,
            }
      }
    >
      {children}
    </div>,
    document.body,
  );
}
