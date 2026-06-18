// Scenes page: the library of saved full-state snapshots. A scene captures each
// light's color/temperature/effect + each power device's on/off, restored in one
// tap. Two scopes: Home scenes (whole-home; one can be the "Restore Home"
// default) and Room scenes (a single room's members). Capture a room's current
// state here or from the room's controls on the Control page / Floor Plan.

import { useEffect, useState } from "react";
import {
  activateScene,
  createScene,
  getRooms,
  getScenes,
  removeScene,
  setDefaultScene,
  type Room,
  type Scene,
} from "../api";
import { Glyph } from "../components/glyphs";
import { useDialogs } from "../components/dialogs";
import { PageHeader } from "../components/PageHeader";
import { useViewport } from "../useViewport";
import { S } from "../styles";
import { alpha, color, font, glow, labelType, radius } from "../theme";
import { Button } from "../components/controls";

export function ScenesPage() {
  const { isMobile } = useViewport();
  const [scenes, setScenes] = useState<Scene[]>([]);
  const [rooms, setRooms] = useState<Room[]>([]);
  const [loading, setLoading] = useState(true);
  const dialogs = useDialogs();

  async function load() {
    const [s, r] = await Promise.all([getScenes(), getRooms()]);
    setScenes(s);
    setRooms(r);
    setLoading(false);
  }
  useEffect(() => {
    load();
  }, []);

  const homeScenes = scenes.filter((s) => !s.room_id);

  async function apply(scene: Scene) {
    try {
      await activateScene(scene.id);
    } catch (e) {
      await dialogs.alert({ title: "Couldn't apply", message: e instanceof Error ? e.message : String(e) });
    }
  }

  async function toggleHomeDefault(scene: Scene) {
    await setDefaultScene(scene.id, !scene.is_default);
    await load();
  }

  async function remove(scene: Scene, scope: string) {
    const ok = await dialogs.confirm({
      title: `Delete ${scope} scene`,
      message: `Delete "${scene.name}"?`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
    await removeScene(scene.id);
    await load();
  }

  async function capture(roomId?: string) {
    const name = await dialogs.prompt({
      title: roomId ? "Save room scene" : "Save home scene",
      message: roomId
        ? "Snapshots this room's current lights (color/temperature/effect) and switches, to restore in one tap."
        : "Snapshots every light and switch's current state, so you can restore it in one tap (e.g. after a power outage resets them).",
      placeholder: roomId ? "e.g. Movie Night" : "e.g. Default",
      confirmLabel: "Save",
    });
    if (!name?.trim()) return;
    await createScene(name.trim(), roomId);
    await load();
  }

  return (
    <div style={{ padding: isMobile ? "1rem 0.85rem" : "1.5rem 2rem", maxWidth: 1100, margin: "0 auto" }}>
      <PageHeader title="Scenes" status="Saved full-state snapshots — whole-home or per-room" />

      {loading ? (
        <p style={{ color: "var(--bf-faint)" }}>Loading…</p>
      ) : (
        <>
          {/* ── Home scenes ── */}
          <section style={{ marginBottom: "1.75rem" }}>
            <h3 style={SECTION_HEADING}><Glyph name="power" size={15} /> Home scenes</h3>
            <p style={{ color: "var(--bf-dim)", fontSize: "0.82rem", margin: "0 0 0.7rem", maxWidth: 620 }}>
              Whole-home snapshots — every light and switch — for one-tap restore (e.g. after a power
              outage resets them to factory state). Mark one as the default for the{" "}
              <strong>Restore Home</strong> button on the dashboard.
            </p>
            <div style={{ display: "flex", flexDirection: "column", gap: "0.5rem", maxWidth: 620 }}>
              {homeScenes.map((s) => (
                <div
                  key={s.id}
                  style={{
                    ...S.card,
                    flexDirection: "row",
                    alignItems: "center",
                    gap: "0.6rem",
                    padding: "0.6rem 0.8rem",
                    ...(s.is_default
                      ? { border: `1px solid ${alpha(color.gold, 0.45)}`, boxShadow: `inset 0 0 24px -14px ${color.gold}, ${glow(color.gold, 12)}` }
                      : {}),
                  }}
                >
                  <div style={{ minWidth: 0, flex: 1 }}>
                    <div style={{ fontWeight: 600, display: "flex", alignItems: "center", gap: "0.5rem" }}>
                      {s.name}
                      {s.is_default && <span style={DEFAULT_BADGE}>default</span>}
                    </div>
                    <div style={{ color: "var(--bf-faint)", fontSize: "0.75rem" }}>
                      {s.lights} light{s.lights !== 1 ? "s" : ""} · {s.power} switch{s.power !== 1 ? "es" : ""}
                    </div>
                  </div>
                  <Button variant="ghost" onClick={() => apply(s)}>Apply</Button>
                  <Button variant="ghost" onClick={() => toggleHomeDefault(s)}>
                    {s.is_default ? "Unset" : "Set default"}
                  </Button>
                  <Button
                    variant="ghost"
                    onClick={() => remove(s, "home")}
                    title="Delete home scene"
                    style={{ color: "#c77", borderColor: "#5a3636", padding: "0 0.6rem" }}
                  >
                    ×
                  </Button>
                </div>
              ))}
              <Button
                variant="ghost"
                onClick={() => capture()}
                style={{ borderStyle: "dashed", borderColor: alpha(color.gold, 0.4), color: color.gold, alignSelf: "flex-start" }}
              >
                + Capture current state
              </Button>
            </div>
          </section>

          {/* ── Room scenes (grouped by room) ── */}
          <h3 style={SECTION_HEADING}><Glyph name="scene" size={15} /> Room scenes</h3>
          <p style={{ color: "var(--bf-dim)", fontSize: "0.82rem", margin: "0 0 0.9rem", maxWidth: 620 }}>
            Per-room snapshots — exact colors, temperatures, and effects of that room's lights.
          </p>
          {rooms.length === 0 ? (
            <p style={{ color: "var(--bf-faint)", fontSize: "0.85rem" }}>No rooms yet.</p>
          ) : (
            <div
              style={{
                display: "grid",
                gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))",
                gap: "1rem",
                alignItems: "start",
              }}
            >
              {rooms.map((room) => (
                <RoomScenesCard
                  key={room.id}
                  room={room}
                  scenes={scenes.filter((s) => s.room_id === room.id)}
                  onApply={apply}
                  onDelete={(s) => remove(s, "room")}
                  onCapture={() => capture(room.id)}
                />
              ))}
            </div>
          )}
        </>
      )}

      {dialogs.element}
    </div>
  );
}

const SECTION_HEADING: React.CSSProperties = {
  ...labelType,
  display: "flex",
  alignItems: "center",
  gap: "0.45rem",
  fontSize: "0.82rem",
  // Themeable engraved-label colour (defaults to gold; a theme can recolour it).
  color: color.textAccent,
  margin: "0 0 0.6rem",
};

const DEFAULT_BADGE: React.CSSProperties = {
  fontFamily: font.display,
  fontSize: "0.6rem",
  textTransform: "uppercase",
  letterSpacing: "0.12em",
  color: color.gold,
  border: `1px solid ${alpha(color.gold, 0.5)}`,
  background: alpha(color.gold, 0.08),
  boxShadow: glow(color.gold, 9),
  borderRadius: radius.pill,
  padding: "0.08rem 0.5rem",
};

function RoomScenesCard({
  room,
  scenes,
  onApply,
  onDelete,
  onCapture,
}: {
  room: Room;
  scenes: Scene[];
  onApply: (s: Scene) => void;
  onDelete: (s: Scene) => void;
  onCapture: () => void;
}) {
  return (
    <div style={{ ...S.card, gap: "0.55rem" }}>
      <div style={{ fontWeight: 600 }}>{room.name}</div>
      {scenes.length === 0 ? (
        <span style={{ color: "var(--bf-faint)", fontSize: "0.78rem" }}>No scenes for this room yet.</span>
      ) : (
        scenes.map((s) => (
          <div key={s.id} style={{ display: "flex", alignItems: "center", gap: "0.4rem" }}>
            <div style={{ minWidth: 0, flex: 1 }}>
              <div style={{ fontSize: "0.86rem", fontWeight: 600 }}>{s.name}</div>
              <div style={{ color: "var(--bf-faint)", fontSize: "0.72rem" }}>
                {s.lights} light{s.lights !== 1 ? "s" : ""}
                {s.power > 0 ? ` · ${s.power} power` : ""}
              </div>
            </div>
            <Button variant="ghost" onClick={() => onApply(s)} style={{ padding: "0.2rem 0.6rem" }}>
              Apply
            </Button>
            <Button
              variant="ghost"
              onClick={() => onDelete(s)}
              title="Delete scene"
              style={{ padding: "0 0.55rem", color: "#c77", borderColor: "#5a3636" }}
            >
              ×
            </Button>
          </div>
        ))
      )}
      <Button
        variant="ghost"
        onClick={onCapture}
        style={{ borderStyle: "dashed", color: "var(--bf-dim)", alignSelf: "flex-start" }}
      >
        + Capture current state
      </Button>
    </div>
  );
}
