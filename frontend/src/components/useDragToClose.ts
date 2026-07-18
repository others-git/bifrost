// Drag-to-dismiss — the shared vertical twin of `useSwipeTabs`: a small handle
// at the top of a bottom sheet that a downward drag pulls closed, native-sheet
// style. Live feedback while dragging (the sheet follows the finger), commit on
// either a real distance OR a quick flick, snap back otherwise.
//
// Usage — attach the returned handlers to a small dedicated handle element
// (NOT the whole sheet body, so it never fights inner scrollers or a tab
// swipe), and give the callback a way to move the sheet itself:
//   const drag = useDragToClose(onClose, (px, animate) => {
//     const el = panelRef.current;
//     if (el) { el.style.transition = animate ? "transform 0.22s ease" : "none";
//                el.style.transform = px > 0 ? `translateY(${px}px)` : ""; }
//   });
//   <div {...drag.handlers} style={{ touchAction: "none", ... }}>{grabber}</div>
//
// Touch/pen only — a mouse drag on a handle is just a click.

import { useRef } from "react";

const COMMIT_PX = 90; // downward drag distance that commits a close
const COMMIT_VELOCITY = 0.6; // px/ms — a quick flick commits even short of COMMIT_PX
const MAX_PULL_RATIO = 1; // the sheet follows the finger 1:1 (no damping — it's leaving)

export function useDragToClose(
  onClose: () => void,
  /** Move the sheet by `px` (0 = resting). `animate` requests a spring-back /
   * exit transition; drag-follow frames pass `false` for zero-lag tracking. */
  applyTranslate: (px: number, animate: boolean) => void,
) {
  const start = useRef<{ y: number; t: number; id: number } | null>(null);

  const handlers = {
    onPointerDown: (e: React.PointerEvent) => {
      if (e.pointerType === "mouse") return;
      start.current = { y: e.clientY, t: performance.now(), id: e.pointerId };
      (e.currentTarget as Element).setPointerCapture?.(e.pointerId);
    },
    onPointerMove: (e: React.PointerEvent) => {
      const s = start.current;
      if (!s || e.pointerId !== s.id) return;
      const dy = (e.clientY - s.y) * MAX_PULL_RATIO;
      applyTranslate(Math.max(0, dy), false); // upward drags don't lift the sheet
    },
    onPointerUp: (e: React.PointerEvent) => {
      const s = start.current;
      start.current = null;
      if (!s || e.pointerId !== s.id) return;
      const dy = e.clientY - s.y;
      const dt = Math.max(1, performance.now() - s.t);
      const velocity = dy / dt;
      if (dy > 0 && (dy > COMMIT_PX || velocity > COMMIT_VELOCITY)) {
        applyTranslate(window.innerHeight, true);
        setTimeout(onClose, 180); // let the exit slide play before unmount
      } else {
        applyTranslate(0, true);
      }
    },
    onPointerCancel: () => {
      start.current = null;
      applyTranslate(0, true);
    },
  };

  return { handlers };
}
