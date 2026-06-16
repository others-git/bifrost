// Scenes page: a global library of palette scenes (Hue-style presets of
// brightness + colors). Scenes are not tied to a room — create them here, then
// apply any scene to any room (from a room's controls on the Lights page or
// Floor Plan, or via the per-scene "Apply to" menu here). A room's current
// colors can also be captured as a new scene from the Lights page.

import { useEffect, useState } from "react";
import {
  activateScene,
  applySceneToRoom,
  createPaletteScene,
  createScene,
  deletePaletteScene,
  getPaletteScenes,
  getRooms,
  getScenes,
  removeScene,
  setDefaultScene,
  type PaletteScene,
  type Room,
  type Scene,
} from "../api";
import { SceneEditor, SceneSwatch } from "../components/scenes";
import { Glyph } from "../components/glyphs";
import { useDialogs } from "../components/dialogs";
import { PageHeader } from "../components/PageHeader";
import { Select } from "../components/Select";
import { useViewport } from "../useViewport";
import { S } from "../styles";
import { alpha, color, font, glow, labelType, radius } from "../theme";
import { Button } from "../components/controls";

export function ScenesPage() {
  const { isMobile } = useViewport();
  const [scenes, setScenes] = useState<PaletteScene[]>([]);
  const [homeScenes, setHomeScenes] = useState<Scene[]>([]);
  const [rooms, setRooms] = useState<Room[]>([]);
  const [loading, setLoading] = useState(true);
  const [creating, setCreating] = useState(false);
  const dialogs = useDialogs();

  async function load() {
    const [s, h, r] = await Promise.all([getPaletteScenes(), getScenes(), getRooms()]);
    setScenes(s);
    setHomeScenes(h);
    setRooms(r);
    setLoading(false);
  }
  useEffect(() => {
    load();
  }, []);

  // ── Home scenes (whole-home snapshot/restore) ──────────────────────────────
  async function captureHome() {
    const name = await dialogs.prompt({
      title: "Save home scene",
      message: "Snapshots every light and switch's current on/off + color state, so you can restore it in one tap (e.g. after a power outage resets them).",
      placeholder: "e.g. Default",
      confirmLabel: "Save",
    });
    if (!name?.trim()) return;
    await createScene(name.trim());
    await load();
  }

  async function applyHome(scene: Scene) {
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

  async function removeHome(scene: Scene) {
    const ok = await dialogs.confirm({
      title: "Delete home scene",
      message: `Delete home scene "${scene.name}"?`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
    await removeScene(scene.id);
    await load();
  }

  async function remove(scene: PaletteScene) {
    const ok = await dialogs.confirm({
      title: "Delete scene",
      message: `Delete scene "${scene.name}"? It will no longer be available to any room.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
    await deletePaletteScene(scene.id);
    await load();
  }

  return (
    <div style={{ padding: isMobile ? "1rem 0.85rem" : "1.5rem 2rem", maxWidth: 1100, margin: "0 auto" }}>
      <PageHeader title="Scenes" status="Whole-home restore presets + reusable color/brightness looks" />

      {loading ? (
        <p style={{ color: "var(--bf-faint)" }}>Loading…</p>
      ) : (
        <>
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
                    // The default preset wears a gilded edge + inner glow.
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
                  <Button variant="ghost" onClick={() => applyHome(s)}>Apply</Button>
                  <Button variant="ghost" onClick={() => toggleHomeDefault(s)}>
                    {s.is_default ? "Unset" : "Set default"}
                  </Button>
                  <Button
                    variant="ghost"
                    onClick={() => removeHome(s)}
                    title="Delete home scene"
                    style={{ color: "#c77", borderColor: "#5a3636", padding: "0 0.6rem" }}
                  >
                    ×
                  </Button>
                </div>
              ))}
              <Button
                variant="ghost"
                onClick={captureHome}
                style={{ borderStyle: "dashed", borderColor: alpha(color.gold, 0.4), color: color.gold, alignSelf: "flex-start" }}
              >
                + Capture current state
              </Button>
            </div>
          </section>

          <h3 style={SECTION_HEADING}><Glyph name="scene" size={15} /> Palette scenes</h3>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))",
              gap: "1rem",
              alignItems: "start",
            }}
          >
            {scenes.map((scene) => (
            <SceneCard
              key={scene.id}
              scene={scene}
              rooms={rooms}
              onDelete={() => remove(scene)}
            />
          ))}

          <div style={{ ...S.card, gap: "0.6rem" }}>
            {creating ? (
              <SceneEditor
                onSave={async (scene) => {
                  await createPaletteScene(scene);
                  setCreating(false);
                  await load();
                }}
                onCancel={() => setCreating(false)}
              />
            ) : (
              <Button variant="ghost"
                onClick={() => setCreating(true)} style={{ width: "100%",
                  padding: "1.2rem",
                  borderStyle: "dashed",
                  color: "var(--bf-dim)" }}
              >
                + New scene
              </Button>
            )}
            {scenes.length === 0 && !creating && (
              <span style={{ color: "var(--bf-faint)", fontSize: "0.8rem" }}>
                No scenes yet — create one, or save a room's current look from the Lights page.
              </span>
            )}
          </div>
          </div>
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
  color: color.gold,
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

function SceneCard({
  scene,
  rooms,
  onDelete,
}: {
  scene: PaletteScene;
  rooms: Room[];
  onDelete: () => void;
}) {
  const [busy, setBusy] = useState(false);

  async function applyTo(roomId: string) {
    if (!roomId) return;
    setBusy(true);
    try {
      await applySceneToRoom(roomId, scene.id);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div style={{ ...S.card, gap: "0.6rem" }}>
      <SceneSwatch palette={scene.palette} width={248} height={40} radius={8} />
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", gap: "0.5rem" }}>
        <span style={{ fontWeight: 600 }}>{scene.name}</span>
        {scene.brightness != null && (
          <span style={{ color: "var(--bf-faint)", fontSize: "0.75rem" }}>{Math.round(scene.brightness)}%</span>
        )}
      </div>
      <div style={{ display: "flex", gap: "0.4rem", alignItems: "center" }}>
        <Select
          value={undefined}
          disabled={rooms.length === 0 || busy}
          onChange={applyTo}
          placeholder={busy ? "Applying…" : rooms.length === 0 ? "No rooms yet" : "Apply to room…"}
          options={rooms.map((r) => ({ value: r.id, label: r.name }))}
          style={{ flex: 1 }}
        />
        <Button variant="ghost"
          onClick={onDelete}
          title="Delete scene" style={{ padding: "0 0.6rem", color: "#c77", borderColor: "#5a3636" }}
        >
          ×
        </Button>
      </div>
    </div>
  );
}

