// Scenes page: a global library of palette scenes (Hue-style presets of
// brightness + colors). Scenes are not tied to a room — create them here, then
// apply any scene to any room (from a room's controls on the Lights page or
// Floor Plan, or via the per-scene "Apply to" menu here). A room's current
// colors can also be captured as a new scene from the Lights page.

import { useEffect, useState } from "react";
import {
  applySceneToRoom,
  createPaletteScene,
  deletePaletteScene,
  getPaletteScenes,
  getRooms,
  type PaletteScene,
  type Room,
} from "../api";
import { SceneEditor, SceneSwatch } from "../components/scenes";
import { useDialogs } from "../components/dialogs";
import { S } from "../styles";

export function ScenesPage() {
  const [scenes, setScenes] = useState<PaletteScene[]>([]);
  const [rooms, setRooms] = useState<Room[]>([]);
  const [loading, setLoading] = useState(true);
  const [creating, setCreating] = useState(false);
  const dialogs = useDialogs();

  async function load() {
    const [s, r] = await Promise.all([getPaletteScenes(), getRooms()]);
    setScenes(s);
    setRooms(r);
    setLoading(false);
  }
  useEffect(() => {
    load();
  }, []);

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
    <div style={{ padding: "1.5rem 2rem", maxWidth: 1100, margin: "0 auto" }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: "0.75rem", marginBottom: "1rem" }}>
        <h1 style={{ margin: 0, fontSize: "1.4rem" }}>Scenes</h1>
        <span style={{ color: "#666", fontSize: "0.85rem" }}>
          Reusable color &amp; brightness presets you can apply to any room
        </span>
      </div>

      {loading ? (
        <p style={{ color: "#666" }}>Loading…</p>
      ) : (
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
              <button
                onClick={() => setCreating(true)}
                style={{
                  ...S.buttonGhost,
                  width: "100%",
                  padding: "1.2rem",
                  borderStyle: "dashed",
                  color: "#888",
                }}
              >
                + New scene
              </button>
            )}
            {scenes.length === 0 && !creating && (
              <span style={{ color: "#666", fontSize: "0.8rem" }}>
                No scenes yet — create one, or save a room's current look from the Lights page.
              </span>
            )}
          </div>
        </div>
      )}

      {dialogs.element}
    </div>
  );
}

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
          <span style={{ color: "#666", fontSize: "0.75rem" }}>{Math.round(scene.brightness)}%</span>
        )}
      </div>
      <div style={{ display: "flex", gap: "0.4rem", alignItems: "center" }}>
        <select
          defaultValue=""
          disabled={rooms.length === 0 || busy}
          onChange={(e) => {
            applyTo(e.target.value);
            e.target.value = "";
          }}
          title="Apply this scene to a room"
          style={{
            ...S.buttonGhost,
            flex: 1,
            padding: "0.35rem 0.5rem",
            fontSize: "0.8rem",
            cursor: "pointer",
          }}
        >
          <option value="" disabled>
            {busy ? "Applying…" : rooms.length === 0 ? "No rooms" : "Apply to room…"}
          </option>
          {rooms.map((r) => (
            <option key={r.id} value={r.id}>
              {r.name}
            </option>
          ))}
        </select>
        <button
          onClick={onDelete}
          title="Delete scene"
          style={{ ...S.buttonGhost, padding: "0 0.55rem", color: "#866" }}
        >
          ×
        </button>
      </div>
    </div>
  );
}
