import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { useCoalescedWrite, useNudge, useToggleWrite } from "./useWrite";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

/** Let queued microtasks (the awaited `write`) run under fake timers. */
async function settle() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

describe("useCoalescedWrite", () => {
  it("sends nothing until the gesture settles", () => {
    const send = vi.fn();
    const { result } = renderHook(() => useCoalescedWrite(200));

    act(() => result.current.queue(send));
    act(() => void vi.advanceTimersByTime(199));
    expect(send).not.toHaveBeenCalled();

    act(() => void vi.advanceTimersByTime(1));
    expect(send).toHaveBeenCalledOnce();
  });

  it("collapses a whole drag into one write, keeping the last value", () => {
    const send = vi.fn();
    const { result } = renderHook(() => useCoalescedWrite(200));

    for (const v of [10, 20, 30, 40, 50]) {
      act(() => result.current.queue(() => send(v)));
      act(() => void vi.advanceTimersByTime(50)); // faster than the delay
    }
    act(() => void vi.advanceTimersByTime(200));

    expect(send).toHaveBeenCalledOnce();
    expect(send).toHaveBeenCalledWith(50);
  });

  it("is one slot, so a later gesture supersedes an unsent earlier one", () => {
    // A colour drag then an effect pick: sending both would let the server
    // resolve one mode and silently drop the other.
    const colour = vi.fn();
    const effect = vi.fn();
    const { result } = renderHook(() => useCoalescedWrite(200));

    act(() => result.current.queue(colour));
    act(() => void vi.advanceTimersByTime(100));
    act(() => result.current.queue(effect));
    act(() => void vi.advanceTimersByTime(200));

    expect(colour).not.toHaveBeenCalled();
    expect(effect).toHaveBeenCalledOnce();
  });

  it("cancel() drops pending work unsent", () => {
    // The power-toggle hazard: an attribute write carries `on: true`, so one
    // left in the queue when the light is switched off would re-light it.
    const send = vi.fn();
    const { result } = renderHook(() => useCoalescedWrite(200));

    act(() => result.current.queue(send));
    act(() => result.current.cancel());
    act(() => void vi.advanceTimersByTime(400));

    expect(send).not.toHaveBeenCalled();
  });

  it("still lands a pending write after the component unmounts", () => {
    // Drag the brightness slider, then immediately close the fly-out — the
    // light must still get the value.
    const send = vi.fn();
    const { result, unmount } = renderHook(() => useCoalescedWrite(200));

    act(() => result.current.queue(send));
    unmount();
    act(() => void vi.advanceTimersByTime(200));

    expect(send).toHaveBeenCalledOnce();
  });
});

describe("useToggleWrite", () => {
  const setup = (write: (v: boolean) => unknown = () => null) => {
    const onOptimistic = vi.fn();
    const onRevert = vi.fn();
    const { result } = renderHook(() =>
      useToggleWrite<boolean>({ write, onOptimistic, onRevert, windowMs: 400 }),
    );
    return { toggle: result.current, onOptimistic, onRevert };
  };

  it("sends the first press immediately — a light never waits on a debounce", async () => {
    const write = vi.fn((_v: boolean) => null);
    const { toggle } = setup(write);

    act(() => toggle(true));
    await settle();

    expect(write).toHaveBeenCalledOnce();
    expect(write).toHaveBeenCalledWith(true);
  });

  it("paints the UI optimistically on every press, even absorbed ones", () => {
    const { toggle, onOptimistic } = setup();

    act(() => toggle(false));
    act(() => toggle(true));
    act(() => toggle(false));

    expect(onOptimistic.mock.calls.map((c) => c[0])).toEqual([false, true, false]);
  });

  it("drops an intermediate flip the user bounced through", async () => {
    // The observed live pattern: off → on → off inside 1.5s while fighting a
    // control that had lost sync. The device is already off; the middle `on`
    // was never a state it needed to reach.
    const write = vi.fn((_v: boolean) => null);
    const { toggle } = setup(write);

    act(() => toggle(false));
    await settle();
    act(() => void vi.advanceTimersByTime(150));
    act(() => toggle(true));
    act(() => void vi.advanceTimersByTime(150));
    act(() => toggle(false));
    act(() => void vi.advanceTimersByTime(400));
    await settle();

    expect(write.mock.calls.map((c) => c[0])).toEqual([false]);
  });

  it("still power-cycles: off then on sends both", async () => {
    const write = vi.fn((_v: boolean) => null);
    const { toggle } = setup(write);

    act(() => toggle(false));
    await settle();
    act(() => toggle(true));
    act(() => void vi.advanceTimersByTime(400));
    await settle();

    expect(write.mock.calls.map((c) => c[0])).toEqual([false, true]);
  });

  it("never loses the final intent, however long the burst", async () => {
    const write = vi.fn((_v: boolean) => null);
    const { toggle } = setup(write);

    for (const v of [true, false, true, false, true]) {
      act(() => toggle(v));
      act(() => void vi.advanceTimersByTime(50));
    }
    act(() => void vi.advanceTimersByTime(400));
    await settle();
    act(() => void vi.advanceTimersByTime(400));
    await settle();

    const calls = write.mock.calls.map((c) => c[0]);
    expect(calls[calls.length - 1]).toBe(true); // where the user left it
    expect(calls.length).toBeLessThan(5); // …without replaying every press
  });

  it("paces a sustained burst to one write per window", async () => {
    const write = vi.fn((_v: boolean) => null);
    const { toggle } = setup(write);

    // 10 alternating presses, 100ms apart, across ~1s.
    for (let i = 0; i < 10; i++) {
      act(() => toggle(i % 2 === 0));
      act(() => void vi.advanceTimersByTime(100));
      await settle();
    }
    expect(write.mock.calls.length).toBeLessThanOrEqual(4);
  });

  it("reverts the optimistic paint when the write reports failure", async () => {
    const { toggle, onRevert } = setup(() => "light unreachable");

    act(() => toggle(true));
    await settle();

    expect(onRevert).toHaveBeenCalledWith(true);
  });

  it("leaves the paint alone when the write succeeds", async () => {
    const { toggle, onRevert } = setup(() => null);

    act(() => toggle(true));
    await settle();

    expect(onRevert).not.toHaveBeenCalled();
  });
});

describe("useNudge", () => {
  it("steps from the last value asked for, not from a prop that hasn't re-rendered", () => {
    const commit = vi.fn();
    // `current` deliberately never changes: this is the burst case, where taps
    // outrun the re-render that would carry the new level back down.
    const { result } = renderHook(() => useNudge(50, commit));

    act(() => result.current(1));
    act(() => result.current(1));
    act(() => result.current(1));

    expect(commit.mock.calls.map((c) => c[0])).toEqual([51, 52, 53]);
  });

  it("drops the accumulator when the value changes under it", () => {
    const commit = vi.fn();
    const { result, rerender } = renderHook(({ v }) => useNudge(v, commit), {
      initialProps: { v: 50 },
    });

    act(() => result.current(1)); // 51
    rerender({ v: 20 }); // someone dragged the slider (or the device pushed)
    act(() => result.current(1));

    expect(commit).toHaveBeenLastCalledWith(21);
  });

  it("clamps to the range instead of walking past it", () => {
    const commit = vi.fn();
    const { result } = renderHook(() => useNudge(99, commit));

    act(() => result.current(1));
    act(() => result.current(1));
    act(() => result.current(1));

    expect(commit.mock.calls.map((c) => c[0])).toEqual([100, 100, 100]);
  });

  it("honours a non-default range", () => {
    const commit = vi.fn();
    const { result } = renderHook(() => useNudge(0, commit, { min: -10, max: 10 }));

    act(() => result.current(-5));
    act(() => result.current(-5));
    act(() => result.current(-5));

    expect(commit.mock.calls.map((c) => c[0])).toEqual([-5, -10, -10]);
  });

  it("rounds a fractional level to a whole step", () => {
    const commit = vi.fn();
    const { result } = renderHook(() => useNudge(41.6, commit));

    act(() => result.current(1));
    act(() => result.current(1));

    expect(commit.mock.calls.map((c) => c[0])).toEqual([43, 44]);
  });
});
