// The remote pads' volume keys are the one place two control planes meet: a
// TV's volume usually belongs to the media device (which resolves a bound
// receiver server-side), while a bare remote has nothing but the TV's own key
// codes. These pin which one a press takes — the divergence would otherwise
// only show up as "the slider moves the receiver, the +/- keys move the TV".

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { KeysPad, ScryPad } from "./BifrostRemote";

afterEach(cleanup);

/** `press(key)` returns the handler the button calls, so the spy sees the key. */
function pressSpy() {
  const sent = vi.fn();
  return { sent, press: (k: string) => () => sent(k) };
}

describe("the remote pads' volume keys", () => {
  it("nudge the media device by one step when the caller owns one", () => {
    const { sent, press } = pressSpy();
    const onVolume = vi.fn();
    render(<KeysPad press={press as never} onVolume={onVolume} />);

    fireEvent.click(screen.getByLabelText("Volume up"));
    fireEvent.click(screen.getByLabelText("Volume down"));

    expect(onVolume.mock.calls.map((c) => c[0])).toEqual([1, -1]);
    expect(sent).not.toHaveBeenCalled();
  });

  it("fall back to the device's own volume keys with no media plane", () => {
    const { sent, press } = pressSpy();
    render(<KeysPad press={press as never} />);

    fireEvent.click(screen.getByLabelText("Volume up"));
    fireEvent.click(screen.getByLabelText("Volume down"));

    expect(sent.mock.calls.map((c) => c[0])).toEqual(["volume_up", "volume_down"]);
  });

  it("are on the Scrying Glass surface too, routed the same way", () => {
    const { sent, press } = pressSpy();
    const onVolume = vi.fn();
    render(<ScryPad press={press as never} onVolume={onVolume} />);

    fireEvent.click(screen.getByLabelText("Volume down"));

    expect(onVolume).toHaveBeenCalledWith(-1);
    expect(sent).not.toHaveBeenCalled();
  });
});
