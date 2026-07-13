// The shared fly-out shell — one anchored popover / bottom-sheet used by every
// control fly-out (LightEditor, MediaEditor, PowerFlyout). It owns the parts
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
import { color, radius, alpha, gildedRule, hitHalo, labelType } from "../theme";

export function Flyout({
  anchor,
  onClose,
  width,
  gap = "0.7rem",
  closeGuard,
  ambientColor,
  ambientStrength = 0.22,
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
  /** Soft colour wash behind the whole fly-out — the light "casting" its colour
   * onto the panel. Applies under every tab. Omit for no cast. */
  ambientColor?: string;
  /** 0–1 opacity of the ambient cast (callers scale it by brightness). */
  ambientStrength?: number;
  children: ReactNode;
}) {
  const { isCompact, isMobile } = useViewport();
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
      // A Select (or other popover) opened from inside the fly-out portals its
      // menu to <body>, so an option lives outside panelRef. Without this, a
      // pointerdown on an option would read as an outside-click and close the
      // fly-out before the option's onClick fires — the selection is lost and the
      // fly-out vanishes. Treat any portaled menu as "inside".
      if (target instanceof Element && target.closest("[data-bf-menu]")) return;
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

  // The ambient cast is a radial colour layer composited over the panel's base
  // background, so it sits behind every tab's content and doesn't scroll or need
  // extra DOM. It fades to transparent well inside the panel, so the rounded
  // corners stay clean.
  const ambient = ambientColor
    ? `radial-gradient(135% 88% at 50% 0%, ${alpha(ambientColor, ambientStrength)} 0%, transparent 56%)`
    : null;

  // Phones keep the full-width bottom sheet (thumb-reachable, screen-wide);
  // tablets get a centred modal over a scrim; desktop is an anchored popover.
  if (isCompact && isMobile) {
    return createPortal(
      <div
        ref={panelRef}
        style={{
          ...sheetStyle,
          ...(ambient ? { background: `${ambient}, ${sheetStyle.background}` } : {}),
        }}
      >
        {children}
      </div>,
      document.body,
    );
  }

  if (isCompact) {
    // Tablet: a screen-centred modal with a dimming scrim.
    return createPortal(
      <div
        style={{
          position: "fixed",
          inset: 0,
          zIndex: 60,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          padding: "1.5rem",
          background: "rgba(0,0,0,0.55)",
          backdropFilter: "blur(3px)",
          WebkitBackdropFilter: "blur(3px)",
        }}
      >
        <div
          ref={panelRef}
          style={{
            width: "70%",
            maxWidth: 560,
            maxHeight: "85vh",
            overflowY: "auto",
            background: ambient ? `${ambient}, ${color.surface}` : color.surface,
            border: `1px solid ${color.hairline}`,
            borderRadius: radius.frame,
            padding: "0.9rem",
            boxShadow: "0 20px 60px rgba(0,0,0,0.75)",
            display: "flex",
            flexDirection: "column",
            gap,
          }}
        >
          {children}
        </div>
      </div>,
      document.body,
    );
  }

  return createPortal(
    <div
      ref={panelRef}
      style={{
        position: "fixed",
        left: pos?.left ?? 0,
        top: pos?.top ?? 0,
        visibility: pos ? "visible" : "hidden",
        zIndex: 60,
        width,
        background: ambient ? `${ambient}, ${color.surface}` : color.surface,
        border: `1px solid ${color.hairline}`,
        borderRadius: radius.frame,
        padding: "0.9rem",
        boxShadow: "0 12px 34px rgba(0,0,0,0.7)",
        display: "flex",
        flexDirection: "column",
        gap,
      }}
    >
      {children}
    </div>,
    document.body,
  );
}

/** The shared fly-out header — engraved title with a domain-tinted glow, an
 * optional leading icon + subtitle, optional inline actions, a circular close,
 * and the gilded rule beneath. Every control fly-out (light/audio/power) uses
 * this so the chrome is identical and only the accent + body differ. */
export function FlyoutHeader({
  title,
  subtitle,
  icon,
  leading,
  accent = color.gold,
  actions,
  onClose,
}: {
  title: string;
  subtitle?: ReactNode;
  icon?: ReactNode;
  /** Element pinned at the far left, before the title — the power button lives
   * here so every device type powers on/off in the same spot. */
  leading?: ReactNode;
  /** Domain tint for the title glow: cyan = light, violet = audio, gold = power. */
  accent?: string;
  /** Controls rendered left of the close button. */
  actions?: ReactNode;
  onClose: () => void;
}) {
  return (
    <>
      <div style={{ display: "flex", alignItems: "center", gap: "0.7rem" }}>
        {leading}
        {icon && (
          <span style={{ color: accent, flexShrink: 0, display: "grid", placeItems: "center" }}>
            {icon}
          </span>
        )}
        <div style={{ minWidth: 0, flex: 1 }}>
          <div
            style={{
              ...labelType,
              fontSize: "0.8rem",
              color: color.text,
              textShadow: `0 0 14px ${alpha(accent, 0.35)}`,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {title}
          </div>
          {subtitle != null && (
            <div
              style={{
                fontSize: "0.7rem",
                color: color.faint,
                marginTop: 2,
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
            >
              {subtitle}
            </div>
          )}
        </div>
        {actions}
        {/* The small ring is the visual; the halo makes it a full-size target. */}
        <button
          onClick={onClose}
          aria-label="Close"
          style={{
            ...hitHalo(24, 24),
            display: "grid",
            placeItems: "center",
            flexShrink: 0,
            background: "none",
            border: "none",
            cursor: "pointer",
          }}
        >
          <span
            style={{
              display: "grid",
              placeItems: "center",
              width: 24,
              height: 24,
              borderRadius: "50%",
              border: `1px solid ${color.hairline}`,
              color: color.faint,
              fontSize: "1rem",
              lineHeight: 1,
              boxSizing: "border-box",
            }}
          >
            ×
          </span>
        </button>
      </div>
      <div aria-hidden style={{ height: 1, background: gildedRule, opacity: 0.7 }} />
    </>
  );
}

/** A labeled section inside a fly-out: an engraved (Cinzel) uppercase label over a
 * gold filigree divider. `inline` lays the label and content on one row (e.g. a
 * Power toggle); otherwise the content stacks beneath the label. Shared so every
 * fly-out's sections read identically. */
export function FlyoutSection({
  label,
  inline = false,
  children,
}: {
  label: string;
  inline?: boolean;
  children: ReactNode;
}) {
  const labelStyle = { ...labelType, fontSize: "0.62rem", color: color.dim } as const;
  return (
    <div
      style={{
        borderTop: `1px solid ${color.hairline}`,
        paddingTop: "0.7rem",
        ...(inline
          ? { display: "flex", justifyContent: "space-between", alignItems: "center" }
          : {}),
      }}
    >
      {inline ? (
        <span style={labelStyle}>{label}</span>
      ) : (
        <div style={{ ...labelStyle, marginBottom: "0.5rem" }}>{label}</div>
      )}
      {children}
    </div>
  );
}
