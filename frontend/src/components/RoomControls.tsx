// Per-room quick-control configuration: the "Add Control" flow. Each configured
// control is a glyph button that lands on the room's Control card (left of its
// power button). A control has a kind (power/volume/brightness/scene), a glyph,
// an optional label, and either target devices (power/volume/brightness) or a
// scene. Inline panel matching RoomDevicesPanel: edit locally, one Save commits.

import { useState } from "react";
import {
  setRoomControls,
  type MediaDevice,
  type ControlKind,
  type ControlTarget,
  type Light,
  type Scene,
  type PowerDevice,
  type Room,
  type RoomControl,
} from "../api";
import { Button } from "./controls";
import { SelectRow } from "./SelectRow";
import { Select } from "./Select";
import { CONTROL_GLYPH_OPTIONS, Glyph } from "./glyphs";
import { ACCENT } from "../styles";

const KINDS: { value: ControlKind; label: string; hint: string }[] = [
  { value: "power", label: "Power", hint: "Toggle the selected devices on/off" },
  { value: "brightness", label: "Brightness", hint: "Open a brightness control for the selected lights" },
  { value: "volume", label: "Volume", hint: "Open a volume control for the selected speakers" },
  { value: "scene", label: "Scene", hint: "Apply a scene to the room" },
];

const KIND_GLYPH: Record<ControlKind, string> = {
  power: "power",
  brightness: "brightness",
  volume: "volume",
  scene: "scene",
};

const key = (t: ControlTarget) => `${t.domain}:${t.id}`;

/** Flatten the room's members into pickable targets for a given control kind. */
function targetsForKind(
  kind: ControlKind,
  room: Room,
  lights: Light[],
  mediaDevices: MediaDevice[],
  powerDevices: PowerDevice[],
): { domain: ControlTarget["domain"]; id: string; name: string }[] {
  const out: { domain: ControlTarget["domain"]; id: string; name: string }[] = [];
  const memberLights = lights.filter((l) => room.light_ids.includes(l.id));
  const memberAudio = mediaDevices.filter((d) =>
    room.media_devices.some((m) => m.media_device_id === d.id),
  );
  const memberPower = powerDevices.filter((d) => room.power_device_ids.includes(d.id));
  if (kind === "power" || kind === "brightness") {
    for (const l of memberLights) out.push({ domain: "light", id: l.id, name: l.name });
  }
  if (kind === "power" || kind === "volume") {
    for (const d of memberAudio) out.push({ domain: "media", id: d.id, name: d.name });
  }
  if (kind === "power") {
    for (const d of memberPower) out.push({ domain: "power", id: d.id, name: d.name });
  }
  return out;
}

function summary(c: RoomControl, scenes: Scene[]): string {
  if (c.kind === "scene") {
    return `Scene: ${scenes.find((s) => s.id === c.scene_id)?.name ?? "?"}`;
  }
  const n = c.targets.length;
  const kindLabel = KINDS.find((k) => k.value === c.kind)?.label ?? c.kind;
  return `${kindLabel} · ${n} device${n !== 1 ? "s" : ""}`;
}

export function RoomControlsPanel({
  room,
  lights,
  mediaDevices,
  powerDevices,
  scenes,
  onSaved,
  onCancel,
}: {
  room: Room;
  lights: Light[];
  mediaDevices: MediaDevice[];
  powerDevices: PowerDevice[];
  scenes: Scene[];
  onSaved: () => void;
  onCancel: () => void;
}) {
  const [controls, setControls] = useState<RoomControl[]>(() => room.controls.map((c) => ({ ...c })));
  const [editing, setEditing] = useState<number | null>(null); // index, or -1 for new
  const [saving, setSaving] = useState(false);

  function commit(control: RoomControl, index: number | null) {
    setControls((prev) => {
      if (index === null || index < 0) return [...prev, control];
      const next = [...prev];
      next[index] = control;
      return next;
    });
    setEditing(null);
  }

  async function save() {
    setSaving(true);
    try {
      await setRoomControls(room.id, controls);
      onSaved();
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      style={{
        borderTop: "1px solid var(--bf-surfaceHi)",
        paddingTop: "0.7rem",
        marginTop: "0.2rem",
        display: "flex",
        flexDirection: "column",
        gap: "0.7rem",
      }}
    >
      <span style={{ fontSize: "0.8rem", color: "var(--bf-dim)", fontWeight: 600 }}>
        Quick controls
        <span style={{ color: "var(--bf-faint)", fontWeight: 400 }}> (shown on the Control card)</span>
      </span>

      {controls.length === 0 && editing === null && (
        <span style={{ fontSize: "0.78rem", color: "var(--bf-faint)" }}>No quick controls yet.</span>
      )}

      {/* Existing controls. */}
      {controls.map((c, i) =>
        editing === i ? (
          <ControlEditor
            key={i}
            room={room}
            lights={lights}
            mediaDevices={mediaDevices}
            powerDevices={powerDevices}
            scenes={scenes}
            initial={c}
            onDone={(ctrl) => commit(ctrl, i)}
            onCancel={() => setEditing(null)}
          />
        ) : (
          <div
            key={i}
            style={{
              display: "flex",
              alignItems: "center",
              gap: "0.6rem",
              padding: "0.4rem 0.6rem",
              borderRadius: 8,
              background: "rgba(255,255,255,0.03)",
            }}
          >
            <span style={{ color: ACCENT, display: "grid", placeItems: "center" }}>
              <Glyph name={c.glyph} size={20} />
            </span>
            <div style={{ minWidth: 0, flex: 1 }}>
              <div style={{ fontSize: "0.85rem" }}>{c.label || summary(c, scenes)}</div>
              {c.label && (
                <div style={{ fontSize: "0.72rem", color: "var(--bf-faint)" }}>{summary(c, scenes)}</div>
              )}
            </div>
            <Button variant="ghost" onClick={() => setEditing(i)}>Edit</Button>
            <Button variant="danger" onClick={() => setControls((p) => p.filter((_, j) => j !== i))}>
              Remove
            </Button>
          </div>
        ),
      )}

      {/* New-control editor / add button. */}
      {editing === -1 ? (
        <ControlEditor
          room={room}
          lights={lights}
          mediaDevices={mediaDevices}
          powerDevices={powerDevices}
          scenes={scenes}
          onDone={(ctrl) => commit(ctrl, -1)}
          onCancel={() => setEditing(null)}
        />
      ) : (
        editing === null && (
          <div>
            <Button variant="ghost" onClick={() => setEditing(-1)} style={{ borderColor: ACCENT, color: ACCENT }}>
              + Add control
            </Button>
          </div>
        )
      )}

      <div style={{ display: "flex", gap: "0.5rem", justifyContent: "flex-end", marginTop: "0.2rem" }}>
        <Button variant="ghost" onClick={onCancel} disabled={saving}>Cancel</Button>
        <Button onClick={save} disabled={saving || editing !== null}>
          {saving ? "Saving…" : "Save controls"}
        </Button>
      </div>
    </div>
  );
}

function ControlEditor({
  room,
  lights,
  mediaDevices,
  powerDevices,
  scenes,
  initial,
  onDone,
  onCancel,
}: {
  room: Room;
  lights: Light[];
  mediaDevices: MediaDevice[];
  powerDevices: PowerDevice[];
  scenes: Scene[];
  initial?: RoomControl;
  onDone: (c: RoomControl) => void;
  onCancel: () => void;
}) {
  const [kind, setKind] = useState<ControlKind>(initial?.kind ?? "power");
  const [glyph, setGlyph] = useState<string>(initial?.glyph ?? KIND_GLYPH.power);
  const [label, setLabel] = useState<string>(initial?.label ?? "");
  const [sceneId, setSceneId] = useState<string | undefined>(initial?.scene_id ?? undefined);
  const [targets, setTargets] = useState<Set<string>>(
    () => new Set((initial?.targets ?? []).map(key)),
  );
  // Track whether the user picked a glyph by hand; if not, follow the kind.
  const [glyphPinned, setGlyphPinned] = useState<boolean>(!!initial);

  function changeKind(next: ControlKind) {
    setKind(next);
    if (!glyphPinned) setGlyph(KIND_GLYPH[next]);
    setTargets(new Set()); // available targets change with kind
  }

  const available = targetsForKind(kind, room, lights, mediaDevices, powerDevices);
  // A scene quick-control applies one of this room's saved Room Scenes.
  const roomScenes = scenes.filter((s) => s.room_id === room.id);

  const valid =
    glyph.trim().length > 0 &&
    (kind === "scene" ? !!sceneId : targets.size > 0);

  function done() {
    const chosen: ControlTarget[] =
      kind === "scene"
        ? []
        : available
            .filter((t) => targets.has(`${t.domain}:${t.id}`))
            .map((t) => ({ domain: t.domain, id: t.id }));
    onDone({
      id: initial?.id,
      kind,
      glyph,
      label: label.trim() || null,
      targets: chosen,
      scene_id: kind === "scene" ? sceneId : null,
    });
  }

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: "0.6rem",
        padding: "0.6rem",
        borderRadius: 8,
        border: `1px solid ${ACCENT}`,
        background: "rgba(255,255,255,0.03)",
      }}
    >
      {/* Kind. */}
      <label style={{ display: "flex", flexDirection: "column", gap: "0.25rem" }}>
        <span style={{ fontSize: "0.76rem", color: "var(--bf-dim)" }}>Control type</span>
        <Select
          value={kind}
          onChange={(v) => changeKind(v as ControlKind)}
          options={KINDS.map((k) => ({ value: k.value, label: k.label }))}
        />
        <span style={{ fontSize: "0.72rem", color: "var(--bf-faint)" }}>
          {KINDS.find((k) => k.value === kind)?.hint}
        </span>
      </label>

      {/* Glyph picker. */}
      <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem" }}>
        <span style={{ fontSize: "0.76rem", color: "var(--bf-dim)" }}>Glyph</span>
        <div style={{ display: "flex", flexWrap: "wrap", gap: "0.35rem" }}>
          {CONTROL_GLYPH_OPTIONS.map((g) => {
            const on = glyph === g.name;
            return (
              <button
                key={g.name}
                type="button"
                title={g.label}
                aria-label={g.label}
                onClick={() => { setGlyph(g.name); setGlyphPinned(true); }}
                style={{
                  width: 42,
                  height: 42,
                  display: "grid",
                  placeItems: "center",
                  borderRadius: 8,
                  cursor: "pointer",
                  color: on ? ACCENT : "var(--bf-dim)",
                  background: on ? `${ACCENT}1f` : "rgba(255,255,255,0.03)",
                  border: `1px solid ${on ? ACCENT : "var(--bf-border)"}`,
                }}
              >
                <Glyph name={g.name} size={20} />
              </button>
            );
          })}
        </div>
      </div>

      {/* Targets (devices) or scene. */}
      {kind === "scene" ? (
        <label style={{ display: "flex", flexDirection: "column", gap: "0.25rem" }}>
          <span style={{ fontSize: "0.76rem", color: "var(--bf-dim)" }}>Scene</span>
          {roomScenes.length === 0 ? (
            <span style={{ fontSize: "0.76rem", color: "var(--bf-faint)" }}>
              No scenes for this room yet — save one from the room's controls.
            </span>
          ) : (
            <Select
              value={sceneId}
              onChange={(v) => setSceneId(v)}
              placeholder="Pick a scene…"
              options={roomScenes.map((s) => ({ value: s.id, label: s.name }))}
            />
          )}
        </label>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem" }}>
          <span style={{ fontSize: "0.76rem", color: "var(--bf-dim)" }}>Devices</span>
          {available.length === 0 ? (
            <span style={{ fontSize: "0.76rem", color: "var(--bf-faint)" }}>
              No matching devices in this room.
            </span>
          ) : (
            <div style={{ display: "flex", flexDirection: "column", gap: "0.25rem", maxHeight: 200, overflowY: "auto" }}>
              {available.map((t) => {
                const k = `${t.domain}:${t.id}`;
                return (
                  <SelectRow
                    key={k}
                    checked={targets.has(k)}
                    onToggle={() =>
                      setTargets((s) => {
                        const next = new Set(s);
                        if (next.has(k)) next.delete(k);
                        else next.add(k);
                        return next;
                      })
                    }
                  >
                    {t.name}
                    <span style={{ fontSize: "0.72rem", color: "var(--bf-faint)" }}>{t.domain}</span>
                  </SelectRow>
                );
              })}
            </div>
          )}
        </div>
      )}

      {/* Optional label. */}
      <label style={{ display: "flex", flexDirection: "column", gap: "0.25rem" }}>
        <span style={{ fontSize: "0.76rem", color: "var(--bf-dim)" }}>Label (optional)</span>
        <input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          placeholder="e.g. All on"
          style={{
            background: "var(--bf-surface)",
            border: "1px solid var(--bf-border)",
            borderRadius: 6,
            color: "var(--bf-text)",
            padding: "0.4rem 0.55rem",
            fontSize: "0.85rem",
          }}
        />
      </label>

      <div style={{ display: "flex", gap: "0.5rem", justifyContent: "flex-end" }}>
        <Button variant="ghost" onClick={onCancel}>Cancel</Button>
        <Button onClick={done} disabled={!valid}>{initial ? "Update" : "Add"}</Button>
      </div>
    </div>
  );
}
