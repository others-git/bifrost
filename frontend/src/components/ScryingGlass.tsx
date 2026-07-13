// The Scrying Glass — Bifrost's gesture remote. A blank slab of obsidian glass:
// you steer the TV by touch, eyes on the screen, not on the phone.
//
//   flick (any direction, anywhere)  → one arrow press
//   drag and hold past the threshold → auto-repeat (~3/s) while held
//   tap                              → OK / select
//   two-finger tap                   → back
//
// Feedback is built for eyes-elsewhere use: a short haptic tick per fired
// press (where the platform supports it), and for glances — the finger draws
// a fading violet ember trail, the edge tick of the fired direction flares
// gold, and a tap blooms a ripple ring. All ornament here is working feedback.
//
// The slab owns every gesture on it (touchAction: none + data-swipe-ignore +
// stopPropagation), so vertical flicks are arrow presses — never page scrolls —
// and the fly-out's tab swipe can't fight it. Gesture thresholds are absolute
// pixels (finger physics), never scaled: the glass feels identical on a phone
// sheet and a wall kiosk; only its area grows.

import { useEffect, useRef } from "react";
import type { RemoteKey } from "../api";
import { alpha, color, radius } from "../theme";
import { Glyph } from "./glyphs";

const FLICK_PX = 28; // travel to fire the first arrow press
const REPEAT_MS = 300; // auto-repeat cadence while held past the threshold
const TAP_SLOP = 10; // movement allowed for a touch to still be a tap
const TAP_MS = 500; // press longer than this is a hold, not a tap
const EMBER_STEP = 12; // min px between trail embers
const MAX_EMBERS = 48;

type Dir = "up" | "down" | "left" | "right";

function dominantDir(dx: number, dy: number): Dir {
  if (Math.abs(dx) >= Math.abs(dy)) return dx > 0 ? "right" : "left";
  return dy > 0 ? "down" : "up";
}

export function ScryingGlass({
  onKey,
  height,
}: {
  onKey: (k: RemoteKey) => void;
  /** CSS height of the slab; the caller sizes it to the form factor. */
  height: string | number;
}) {
  const slab = useRef<HTMLDivElement>(null);
  const fx = useRef<HTMLDivElement>(null); // ember/ripple layer
  const ticks = useRef<Partial<Record<Dir, HTMLSpanElement | null>>>({});
  const start = useRef<{ x: number; y: number; t: number; id: number } | null>(null);
  const offset = useRef<{ dx: number; dy: number }>({ dx: 0, dy: 0 });
  const fired = useRef(false);
  const twoFinger = useRef(false);
  const pointers = useRef<Set<number>>(new Set());
  const repeat = useRef<ReturnType<typeof setInterval>>(undefined);
  const lastEmber = useRef<{ x: number; y: number }>({ x: 0, y: 0 });
  const calm = useRef(false); // prefers-reduced-motion: skip embers/ripples

  useEffect(() => {
    calm.current = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    return () => clearInterval(repeat.current);
  }, []);

  // Haptic signatures — tuned so each gesture feels distinct, and above the
  // ~10ms floor some motors can't reproduce. Scoped to the glass only: the
  // discrete keys stay silent (they have visible press feedback; the glass is
  // used eyes-off, so it earns the buzz). Safe no-op where unsupported (iOS).
  function haptic(pattern: number | number[]) {
    navigator.vibrate?.(pattern);
  }

  function flare(dir: Dir) {
    const el = ticks.current[dir];
    if (!el) return;
    el.classList.remove("bifrost-scry-flare");
    // Force a reflow so re-adding the class replays the animation.
    void el.offsetWidth;
    el.classList.add("bifrost-scry-flare");
    setTimeout(() => el.classList.remove("bifrost-scry-flare"), 360);
  }

  function spawn(x: number, y: number, cls: string, size: number, ttl: number) {
    const layer = fx.current;
    if (!layer || calm.current) return;
    const dot = document.createElement("div");
    dot.className = cls;
    dot.style.cssText = `position:absolute;left:${x}px;top:${y}px;width:${size}px;height:${size}px;pointer-events:none;`;
    layer.appendChild(dot);
    while (layer.childElementCount > MAX_EMBERS) layer.firstElementChild?.remove();
    setTimeout(() => dot.remove(), ttl);
  }

  function slabXY(e: React.PointerEvent): { x: number; y: number } {
    const r = slab.current!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  function fire(dir: Dir, repeat = false) {
    onKey(dir);
    haptic(repeat ? 10 : 18);
    flare(dir);
  }

  function reset() {
    clearInterval(repeat.current);
    start.current = null;
    fired.current = false;
    slab.current?.classList.remove("bifrost-scry-awake");
  }

  return (
    <div
      ref={slab}
      data-swipe-ignore
      role="application"
      aria-label="Gesture pad — flick to move, tap for OK, two-finger tap for back"
      onPointerDown={(e) => {
        e.stopPropagation();
        pointers.current.add(e.pointerId);
        if (pointers.current.size > 1) {
          twoFinger.current = true;
          return;
        }
        twoFinger.current = false;
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        start.current = { x: e.clientX, y: e.clientY, t: performance.now(), id: e.pointerId };
        offset.current = { dx: 0, dy: 0 };
        fired.current = false;
        const p = slabXY(e);
        lastEmber.current = p;
        slab.current?.classList.add("bifrost-scry-awake");
        // Re-fire while held past the threshold; direction follows the finger.
        clearInterval(repeat.current);
        repeat.current = setInterval(() => {
          const { dx, dy } = offset.current;
          if (fired.current && Math.hypot(dx, dy) >= FLICK_PX) fire(dominantDir(dx, dy), true);
        }, REPEAT_MS);
      }}
      onPointerMove={(e) => {
        const s = start.current;
        if (!s || e.pointerId !== s.id) return;
        const dx = e.clientX - s.x;
        const dy = e.clientY - s.y;
        offset.current = { dx, dy };
        const p = slabXY(e);
        if (Math.hypot(p.x - lastEmber.current.x, p.y - lastEmber.current.y) >= EMBER_STEP) {
          lastEmber.current = p;
          spawn(p.x, p.y, "bifrost-scry-ember", 10, 700);
        }
        if (!fired.current && Math.hypot(dx, dy) >= FLICK_PX) {
          fired.current = true;
          fire(dominantDir(dx, dy));
        }
      }}
      onPointerUp={(e) => {
        pointers.current.delete(e.pointerId);
        const s = start.current;
        if (!s || e.pointerId !== s.id) {
          // The second finger of a two-finger tap lifting.
          if (twoFinger.current && pointers.current.size === 0) {
            twoFinger.current = false;
            onKey("back");
            haptic([16, 60, 16]);
          }
          return;
        }
        const dist = Math.hypot(e.clientX - s.x, e.clientY - s.y);
        const dt = performance.now() - s.t;
        const wasFired = fired.current;
        const wasTwo = twoFinger.current;
        reset();
        if (pointers.current.size > 0) return; // other finger still down
        if (wasTwo) {
          twoFinger.current = false;
          onKey("back");
          haptic([16, 60, 16]);
          return;
        }
        if (!wasFired && dist < TAP_SLOP && dt < TAP_MS) {
          onKey("select");
          haptic(24);
          const p = slabXY(e);
          spawn(p.x, p.y, "bifrost-scry-ripple", 18, 520);
        }
      }}
      onPointerCancel={(e) => {
        pointers.current.delete(e.pointerId);
        reset();
      }}
      style={{
        position: "relative",
        height,
        borderRadius: radius.frame,
        border: `1px solid ${color.hairline}`,
        // Obsidian: near-black glass with a dormant violet sheen that wakes on
        // touch (the .bifrost-scry-awake class deepens the inner glow).
        background: `radial-gradient(120% 90% at 50% 18%, ${alpha(color.violet, 0.07)}, transparent 60%), linear-gradient(165deg, #0b0a10 0%, #060509 100%)`,
        boxShadow: `inset 0 2px 16px rgba(0,0,0,0.85), inset 0 0 70px -34px ${alpha(color.violet, 0.5)}`,
        touchAction: "none",
        cursor: "crosshair",
        overflow: "hidden",
        transition: "box-shadow 0.4s ease",
      }}
    >
      {/* Edge ticks — the compass hints; each flares gold when its direction fires. */}
      {(["up", "right", "down", "left"] as const).map((dir) => {
        const rot = { up: 180, down: 0, left: 90, right: -90 }[dir];
        const pos: React.CSSProperties =
          dir === "up"
            ? { top: 7, left: "50%", transform: "translateX(-50%)" }
            : dir === "down"
              ? { bottom: 7, left: "50%", transform: "translateX(-50%)" }
              : dir === "left"
                ? { left: 7, top: "50%", transform: "translateY(-50%)" }
                : { right: 7, top: "50%", transform: "translateY(-50%)" };
        return (
          <span
            key={dir}
            ref={(el) => {
              ticks.current[dir] = el;
            }}
            aria-hidden
            style={{
              position: "absolute",
              ...pos,
              color: alpha(color.gold, 0.35),
              display: "grid",
              pointerEvents: "none",
            }}
          >
            <span style={{ display: "grid", transform: `rotate(${rot}deg)` }}>
              <Glyph name="chevron" size={14} />
            </span>
          </span>
        );
      })}
      {/* Whisper of instruction — fades once you've clearly used it. */}
      <span
        aria-hidden
        style={{
          position: "absolute",
          inset: 0,
          display: "grid",
          placeItems: "center",
          pointerEvents: "none",
          color: alpha(color.text, 0.22),
          fontSize: "0.68rem",
          letterSpacing: "0.14em",
          textTransform: "uppercase",
        }}
      >
        flick to move · tap for ok
      </span>
      {/* Ember / ripple layer. */}
      <div ref={fx} aria-hidden style={{ position: "absolute", inset: 0, pointerEvents: "none" }} />
    </div>
  );
}
