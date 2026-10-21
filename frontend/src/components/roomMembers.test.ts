// A receiver bound to a room's TV folds into the TV's control only while it is
// actually serving the TV. On any other input (net radio, a turntable) the TV's
// control can't reach its input or now-playing, so it must show on its own —
// on every surface, since Control and Boards both read this one rule.

import { describe, expect, it } from "vitest";
import type { MediaDevice, Room } from "../api";
import { bindingInputs, receiverOnBoundInput, roomMembers } from "./RoomCard";

function media(id: string, extra: Partial<MediaDevice> = {}, source?: string): MediaDevice {
  return {
    id,
    name: id,
    enabled: true,
    receiver_id: null,
    receiver_source: null,
    state: { power: true, volume: 20, mute: false, reachable: true, source },
    ...extra,
  } as unknown as MediaDevice;
}

function room(ids: string[]): Room {
  return {
    id: "r",
    name: "Living Room",
    light_ids: [],
    power_device_ids: [],
    media_devices: ids.map((id) => ({ media_device_id: id })),
  } as unknown as Room;
}

const audioIds = (devices: MediaDevice[]) =>
  roomMembers(room(devices.map((d) => d.id)), [], [], devices).audio.map((d) => d.id);

describe("roomMembers' bound-receiver fold", () => {
  it("folds the receiver while it sits on the TV's input", () => {
    const tv = media("tv", { receiver_id: "avr", receiver_source: "tv" });
    const avr = media("avr", {}, "TV");
    expect(audioIds([tv, avr])).toEqual(["tv"]);
  });

  it("shows the receiver when it is on another input", () => {
    const tv = media("tv", { receiver_id: "avr", receiver_source: "tv" });
    const avr = media("avr", {}, "net");
    expect(audioIds([tv, avr])).toEqual(["tv", "avr"]);
  });

  it("shows the receiver when the binding names no input", () => {
    const tv = media("tv", { receiver_id: "avr", receiver_source: null });
    const avr = media("avr", {}, "tv");
    expect(audioIds([tv, avr])).toEqual(["tv", "avr"]);
  });

  it("shows the receiver when its own input is unknown", () => {
    const tv = media("tv", { receiver_id: "avr", receiver_source: "tv" });
    expect(receiverOnBoundInput(media("avr"), [tv])).toBe(false);
  });

  it("folds when any one of several bound sources owns the current input", () => {
    const tv = media("tv", { receiver_id: "avr", receiver_source: "tv" });
    const game = media("console", { receiver_id: "avr", receiver_source: "game" });
    const avr = media("avr", {}, "game");
    expect(audioIds([tv, game, avr])).toEqual(["tv", "console"]);
  });

  it("folds on any input in the binding's match list", () => {
    const tv = media("tv", { receiver_id: "avr", receiver_source: "hdmi1", receiver_match_inputs: ["hdmi1", "TV/ARC"] });
    expect(audioIds([tv, media("avr", {}, "tv/arc")])).toEqual(["tv"]);
    expect(audioIds([tv, media("avr", {}, "net")])).toEqual(["tv", "avr"]);
  });

  it("uses the match list instead of the power-on input when both are set", () => {
    const tv = media("tv", { receiver_id: "avr", receiver_source: "hdmi1", receiver_match_inputs: ["TV/ARC"] });
    expect(audioIds([tv, media("avr", {}, "hdmi1")])).toEqual(["tv", "avr"]);
  });

  it("never folds a receiver whose source is not in the room", () => {
    const avr = media("avr", {}, "tv");
    expect(audioIds([avr])).toEqual(["avr"]);
  });
});

describe("bindingInputs", () => {
  it("prefers the match list, falls back to the power-on input, else nothing", () => {
    expect(bindingInputs(media("a", { receiver_source: "x", receiver_match_inputs: ["y", "z"] }))).toEqual(["y", "z"]);
    expect(bindingInputs(media("b", { receiver_source: "x", receiver_match_inputs: [] }))).toEqual(["x"]);
    expect(bindingInputs(media("c"))).toEqual([]);
  });
});
