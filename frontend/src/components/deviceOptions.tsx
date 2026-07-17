// Room-grouped option building for device `Select`s — THE device-chooser
// pattern (Boards widget config, the automation editor's pickers): devices
// grouped under their room's header (direct room, else the inherited
// provider-group room), rooms in name order, a trailing "No room" bucket,
// names sorted within each room, with the Select's built-in search on top.

import type { Room } from "../api";

export type RoomedDevice = {
  id: string;
  name: string;
  room_id?: string | null;
  inherited_room_id?: string | null;
};

/** Group a device list by its effective room. */
export function groupByRoom<T extends RoomedDevice>(
  devices: T[],
  rooms: Room[],
): { room: string; devices: T[] }[] {
  const nameOf = (id?: string | null) => (id ? rooms.find((r) => r.id === id)?.name : undefined);
  const buckets = new Map<string, T[]>();
  for (const d of devices) {
    const key = nameOf(d.room_id) ?? nameOf(d.inherited_room_id) ?? "";
    (buckets.get(key) ?? buckets.set(key, []).get(key)!).push(d);
  }
  return [...buckets.entries()]
    .sort((a, b) => {
      if (a[0] === "") return 1; // "No room" last
      if (b[0] === "") return -1;
      return a[0].localeCompare(b[0]);
    })
    .map(([room, devs]) => ({
      room: room || "No room",
      devices: devs.slice().sort((x, y) => x.name.localeCompare(y.name)),
    }));
}

/** Room-grouped options for a device `Select`. The option value is the device
 * id — pre-map ids (e.g. `light:${id}`) when one Select mixes domains. */
export function deviceSelectOptions<T extends RoomedDevice>(
  devices: T[],
  rooms: Room[],
): { value: string; label: string; group: string }[] {
  return groupByRoom(devices, rooms).flatMap(({ room, devices }) =>
    devices.map((d) => ({ value: d.id, label: d.name, group: room })),
  );
}

// ── Multi-select checklist ────────────────────────────────────────────────────

import { useState } from "react";
import { S } from "../styles";
import { T } from "../theme";
import { SelectRow } from "./SelectRow";

const GROUP_HEADER: React.CSSProperties = {
  fontSize: "0.64rem",
  textTransform: "uppercase",
  letterSpacing: "0.06em",
  color: T.dim,
  margin: "0.5rem 0 0.15rem",
  gridColumn: "1 / -1", // spans every column of the grid below it
  fontWeight: 600,
};
const ITEM_LABEL: React.CSSProperties = { overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" };

/** The multi-select twin of a grouped `Select`: a searchable checklist over
 * the same `{value, label, group}` option arrays (`deviceSelectOptions`).
 * Used wherever several devices are picked at once — Boards group widgets,
 * the automation editor's action targets. Each item is a `SelectRow` — the
 * same "selected = lit box" primitive as the Rooms/Media device pickers and
 * the room Presence panel, so a multi-select reads the same everywhere in the
 * app instead of a bare checkbox row. Items lay out as a responsive grid
 * (auto-fills 2+ columns once the container is wide enough), so a long device
 * list needs far less scrolling; a group header spans the full width. */
export function OptionCheckList({
  options,
  selected,
  onToggle,
  maxHeight = 240,
}: {
  options: { value: string; label: string; group?: string }[];
  selected: string[];
  onToggle: (value: string) => void;
  maxHeight?: number;
}) {
  const [query, setQuery] = useState("");
  const q = query.trim().toLowerCase();
  const visible = q ? options.filter((o) => o.label.toLowerCase().includes(q)) : options;

  // Preserve first-appearance group order (rooms arrive name-sorted already).
  const groups: { group: string; items: typeof options }[] = [];
  for (const o of visible) {
    const g = o.group ?? "";
    const bucket = groups.find((x) => x.group === g);
    if (bucket) bucket.items.push(o);
    else groups.push({ group: g, items: [o] });
  }

  return (
    <div>
      <input
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder="Search…"
        style={{ ...S.input, marginBottom: "0.4rem", fontSize: "0.82rem", width: "100%", boxSizing: "border-box" }}
      />
      <div
        style={{
          maxHeight,
          overflowY: "auto",
          display: "grid",
          gridTemplateColumns: "repeat(auto-fill, minmax(150px, 1fr))",
          gridAutoRows: "min-content",
          gap: "0.3rem",
          alignItems: "start",
        }}
      >
        {groups.map(({ group, items }) => (
          <div key={group || "·"} style={{ display: "contents" }}>
            {group && <div style={GROUP_HEADER}>{group}</div>}
            {items.map((o) => (
              <SelectRow key={o.value} checked={selected.includes(o.value)} onToggle={() => onToggle(o.value)}>
                <span style={ITEM_LABEL}>{o.label}</span>
              </SelectRow>
            ))}
          </div>
        ))}
        {visible.length === 0 && (
          <span style={{ color: T.faint, fontSize: "0.8rem", padding: "0.3rem 0", gridColumn: "1 / -1" }}>
            No matches.
          </span>
        )}
      </div>
    </div>
  );
}
