// Swipe-to-switch-tabs — THE shared touch gesture for tabbed panels, with live
// feedback: while the finger drags, the panel content follows it (damped, with
// hard resistance past the first/last tab), and on commit the incoming tab
// slides in from the swipe direction (`bifrost-tab-enter-*`, index.html).
//
// Usage — the hook returns two attachment points:
//   const swipe = useSwipeTabs(tab, ORDER, setTab);
//   <div {...swipe.handlers} style={{ touchAction: "pan-y", … }}>   // gesture surface
//     <TabStrip … />
//     <div key={swipe.key} className={swipe.className} style={{ …, ...swipe.style }}>
//       {content}                                                   // sliding content
//     </div>
//   </div>
// The gesture surface MUST set `touchAction: "pan-y"` — without it the browser
// claims horizontal pans for itself and fires pointercancel mid-swipe, so the
// gesture never completes (vertical scrolling stays native either way). The
// strip may live on the surface but outside the sliding wrapper, so it stays
// put while the content moves. `key` remounts the wrapper when the tab
// changes, which is what replays the entrance animation.
//
// Ground rules (why this is a hook and not per-page code):
// - Touch/pen only — a mouse drag means text selection, not navigation.
// - A swipe is a deliberate flick: mostly horizontal (≥2:1), a real distance
//   (≥56px), and quick (≤600ms). Anything else is a scroll or a tap.
// - Once a drag arms (≥12px, mostly horizontal) the pointer is captured, so a
//   swipe that started on a button never also presses it.
// - Drag-owning controls win: anything marked `data-swipe-ignore`, plus
//   canvases (color wheels), inputs, and selects, never start a swipe. Shared
//   sliders already stop pointerdown propagation.
// - No wrap-around: swiping past the last tab rubber-bands (matches the strip).

import { useRef, useState, type CSSProperties } from "react";

const MIN_DX = 56; // px to commit a tab change
const MAX_MS = 600; // a swipe is quick; slower = a scroll/hesitation
const ARM_DX = 10; // px before the drag visual engages
const MAX_PULL = 72; // px the content will follow at most
const SNAP = "transform 0.18s ease, opacity 0.18s ease";

export function useSwipeTabs<T>(
  value: T,
  order: readonly T[],
  onChange: (v: T) => void,
): {
  handlers: {
    onPointerDown: (e: React.PointerEvent) => void;
    onPointerMove: (e: React.PointerEvent) => void;
    onPointerUp: (e: React.PointerEvent) => void;
    onPointerCancel: () => void;
  };
  /** Attach to the sliding content wrapper — the drag is written straight to
   * the DOM node (no re-render per move, so a heavy panel can't jank it). */
  contentRef: (el: HTMLElement | null) => void;
  /** Merge onto the sliding content wrapper (snap-back transition). */
  style: CSSProperties;
  /** Set on the wrapper — plays the slide-in after a committed swipe. */
  className: string;
  /** Set as the wrapper's `key` so a tab change remounts it (replays entry). */
  key: string;
} {
  const start = useRef<{ x: number; y: number; t: number; id: number } | null>(null);
  const armed = useRef(false);
  const content = useRef<HTMLElement | null>(null);
  const [enter, setEnter] = useState<"" | "left" | "right">("");
  const enterTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  function pull(px: number) {
    const el = content.current;
    if (!el) return;
    el.style.transition = "none";
    el.style.transform = `translateX(${px}px)`;
    el.style.opacity = String(1 - (Math.abs(px) / MAX_PULL) * 0.25);
  }

  function reset() {
    start.current = null;
    armed.current = false;
    const el = content.current;
    if (el) {
      el.style.transition = SNAP;
      el.style.transform = "";
      el.style.opacity = "";
    }
  }

  const handlers = {
    onPointerDown: (e: React.PointerEvent) => {
      if (e.pointerType === "mouse" || !e.isPrimary) return;
      const el = e.target as Element;
      if (el.closest?.("[data-swipe-ignore], input, textarea, canvas, select")) return;
      armed.current = false;
      start.current = { x: e.clientX, y: e.clientY, t: performance.now(), id: e.pointerId };
    },
    onPointerMove: (e: React.PointerEvent) => {
      const s = start.current;
      if (!s || e.pointerId !== s.id) return;
      const dx = e.clientX - s.x;
      const dy = e.clientY - s.y;
      if (!armed.current) {
        // Engage only for a clearly horizontal drag; until then, taps and
        // vertical scrolls proceed untouched. The threshold is tight because
        // inner scrollers (the effects grid) race us for diagonal gestures —
        // the sooner we arm and capture, the more swipes survive.
        if (Math.abs(dx) < ARM_DX || Math.abs(dx) < Math.abs(dy) * 1.2) return;
        armed.current = true;
        // From here it's a swipe: capture so the button it started on never
        // also receives the pointer-up (no accidental press).
        (e.currentTarget as Element).setPointerCapture?.(e.pointerId);
      }
      const i = order.indexOf(value);
      const hasTarget = dx < 0 ? i < order.length - 1 : i > 0;
      // Content follows the finger, damped; a much stiffer rubber-band at the
      // ends says "nothing further this way". Written straight to the node —
      // no React state, so a 60-tile effects grid never re-renders mid-drag.
      const damp = hasTarget ? 0.35 : 0.12;
      pull(Math.max(-MAX_PULL, Math.min(MAX_PULL, dx * damp)));
    },
    onPointerUp: (e: React.PointerEvent) => {
      const s = start.current;
      if (!s || e.pointerId !== s.id) return;
      const dx = e.clientX - s.x;
      const dy = e.clientY - s.y;
      const dt = performance.now() - s.t;
      reset(); // snap-back animates via the style's transition
      if (dt > MAX_MS || Math.abs(dx) < MIN_DX || Math.abs(dx) < 2 * Math.abs(dy)) return;
      const i = order.indexOf(value);
      const next = dx < 0 ? i + 1 : i - 1; // swipe left → the tab to the right
      if (i < 0 || next < 0 || next >= order.length) return;
      setEnter(dx < 0 ? "right" : "left");
      clearTimeout(enterTimer.current);
      enterTimer.current = setTimeout(() => setEnter(""), 300);
      onChange(order[next]);
    },
    onPointerCancel: reset,
  };

  return {
    handlers,
    contentRef: (el: HTMLElement | null) => {
      content.current = el;
    },
    style: { transition: SNAP },
    className: enter ? `bifrost-tab-enter-${enter}` : "",
    key: String(value),
  };
}
