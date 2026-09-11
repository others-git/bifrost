// The one place a control gesture becomes a device write.
//
// Every surface used to hand-roll `clearTimeout(t); t = setTimeout(send, N)`,
// which is how the app ended up with five different N for the same capability
// (light attributes at 200/250/300ms depending on whether you were on the room
// card, the floor plan or a board) and with power writes that coalesced in the
// device fly-out and nowhere else. These hooks are that pattern, named and
// shared, so a cadence is a property of the *capability* rather than of whichever
// page you happen to be standing on.
//
// Two write modes, because continuous and discrete controls want opposite edges,
// plus the stepper's accumulator that feeds either:
//
//   useCoalescedWrite — sliders (brightness, colour, volume, segments). Trailing
//     only: nothing is sent until the gesture settles, so dragging a slider
//     across its track costs one write instead of thirty.
//
//   useNudge — stepper buttons (volume ±1). Not a write slot of its own: it is
//     the accumulator in front of one, so a burst of taps steps from the last
//     value asked for instead of from a prop React has not re-rendered yet.
//
//   useToggleWrite — buttons (power, mute, enable). Leading *and* trailing: the
//     first press goes out immediately (a light must not wait 200ms to come on),
//     presses inside the window are absorbed, and the window closes by sending
//     the final intent if it differs from what actually went out. So a burst
//     costs at most one write per window and always converges on what the user
//     last asked for — `off → on` still sends both (the `on` just lands at the
//     window's end, so power-cycling works), while `off → on → off` drops the
//     middle `on`, because the device is already where it needs to be.
//
// Neither write hook cancels a pending write on unmount, deliberately: dragging a
// brightness slider and immediately closing the fly-out must still reach the
// light. The pending work holds its own closure and no React state, so it is
// safe to land after the component is gone.

import { useRef } from "react";

/** Light colour/brightness/temp/effect. Long enough to swallow a drag's frames,
 *  short enough that the light tracks your finger. */
export const ATTR_DELAY = 200;
/** Volume. Slightly longer — a volume drag is coarser and the receivers behind
 *  it (Sonos, Onkyo, a TV) are the slowest writers we have. */
export const VOLUME_DELAY = 250;
/** Per-segment strip painting. Deliberately the tightest: a segment editor that
 *  lags behind the finger stops reading as direct manipulation. */
export const SEGMENT_DELAY = 150;
/** Power/mute/enable. The burst window, not a delay — the first press is
 *  immediate, so this only bounds how fast repeated presses reach the device. */
export const TOGGLE_WINDOW = 400;

/**
 * A trailing-coalesced write slot.
 *
 * `queue(work)` replaces any still-pending work and runs the latest one
 * `delayMs` after the last call. One hook instance is one slot, so call sites
 * that must supersede each other (a colour drag and an effect pick are mutually
 * exclusive light modes — sending both would let the server resolve one and
 * silently drop the other) share a single instance on purpose.
 *
 * `cancel()` drops the pending work unsent. A power toggle needs this: an
 * attribute write carries `on: true` (adjusting a light implies lighting it), so
 * a brightness write still in the queue when you switch the light off would
 * arrive a moment later and turn it back on.
 */
export function useCoalescedWrite(delayMs: number): {
  queue: (work: () => void) => void;
  cancel: () => void;
} {
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const cancel = () => {
    clearTimeout(timer.current);
    timer.current = undefined;
  };
  return {
    queue: (work: () => void) => {
      cancel();
      timer.current = setTimeout(work, delayMs);
    },
    cancel,
  };
}

/**
 * A leading + trailing coalesced write for a discrete control.
 *
 * `write` receives the value to send and reports failure the way this app's API
 * layer does — a truthy result is an error message. `onOptimistic` paints the
 * UI before the network round-trip; `onRevert` puts it back when the write
 * reports failure, and lives here rather than at each call site so no surface
 * can forget it (a Boards tile used to keep claiming a device was on after the
 * write to it failed, while the identically-named Dashboard handler reverted).
 */
export function useToggleWrite<T>({
  write,
  onOptimistic,
  onRevert,
  windowMs = TOGGLE_WINDOW,
  same = Object.is,
}: {
  write: (value: T) => unknown | Promise<unknown>;
  onOptimistic?: (value: T) => void;
  onRevert?: (value: T) => void;
  windowMs?: number;
  /** Value equality, for a target that isn't a primitive. */
  same?: (a: T, b: T) => boolean;
}): (value: T) => void {
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const sent = useRef<{ value: T } | undefined>(undefined);
  const pending = useRef<{ value: T } | undefined>(undefined);

  async function send(value: T) {
    sent.current = { value };
    const failed = await write(value);
    if (failed) onRevert?.(value);
  }

  function closeWindow() {
    timer.current = undefined;
    const next = pending.current;
    pending.current = undefined;
    // Nothing new, or the device is already where the user left it.
    if (!next || (sent.current && same(next.value, sent.current.value))) return;
    void send(next.value);
    openWindow();
  }

  function openWindow() {
    timer.current = setTimeout(closeWindow, windowMs);
  }

  return (value: T) => {
    onOptimistic?.(value);
    if (timer.current !== undefined) {
      pending.current = { value };
      return;
    }
    void send(value);
    openWindow();
  };
}

/**
 * The accumulator for a stepping control (volume ±1), sitting in front of a
 * write slot rather than owning one.
 *
 * `nudge(delta)` steps from the value the **last tap asked for**, not from
 * `current`: taps arrive faster than a round-trip and can outrun a re-render, so
 * reading the prop each time would make three taps on +1 move the level by one.
 * Any change to `current` that isn't ours — a slider drag, another surface, a
 * push from the device — drops the accumulator, so the next tap steps from where
 * the device actually is rather than from a level the user has since abandoned.
 *
 * The step is clamped and rounded here so no call site has to, and `commit` gets
 * a value that is already a legal level.
 */
export function useNudge(
  current: number,
  commit: (value: number) => void,
  { min = 0, max = 100 }: { min?: number; max?: number } = {},
): (delta: number) => void {
  const pending = useRef<number | undefined>(undefined);
  const seen = useRef(current);
  // Derived-from-props state: cheaper and more correct than an effect, which
  // would clear the accumulator a render too late (after the next tap read it).
  if (seen.current !== current) {
    seen.current = current;
    pending.current = undefined;
  }
  return (delta: number) => {
    const next = Math.max(min, Math.min(max, Math.round((pending.current ?? current) + delta)));
    pending.current = next;
    commit(next);
  };
}
