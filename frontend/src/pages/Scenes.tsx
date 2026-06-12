// Scenes page: a first-class home for room palette scenes, one section per
// room. Scenes render as gradient tiles you click to apply; the inline editor
// and "Save current" mirror the Floor Plan's room controller.

import { useEffect, useState } from "react";
import {
  applyRoomScene,
  createRoomScene,
  deleteRoomScene,
  getRoomScenes,
  getRooms,
  rgbToHex,
  xyToRgb,
  type Light,
  type LightState,
  type Room,
  type RoomScene,
} from "../api";
import { SceneEditor, SceneSwatch } from "../components/scenes";
import { useDialogs, type Dialogs } from "../components/dialogs";
import { S } from "../styles";

export function ScenesPage({ lights }: { lights: Light[] }) {
  const [rooms, setRooms] = useState<Room[]>([]);
  const [loading, setLoading] = useState(true);
  const dialogs = useDialogs();

  // Live states keyed by light id, for "Save current".
  const statesById = new Map<string, LightState>();
  for (const l of lights) if (l.last_state) statesById.set(l.id, l.last_state);

  useEffect(() => {
    getRooms().then((r) => {
      setRooms(r);
      setLoading(false);
    });
  }, []);

  return (
    <div style={{ padding: "1.5rem 2rem", maxWidth: 1100, margin: "0 auto" }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: "0.75rem", marginBottom: "1rem" }}>
        <h1 style={{ margin: 0, fontSize: "1.4rem" }}>Scenes</h1>
        <span style={{ color: "#666", fontSize: "0.85rem" }}>
          Saved colors &amp; brightness you can apply to a whole room
        </span>
      </div>

      {loading ? (
        <p style={{ color: "#666" }}>Loading…</p>
      ) : rooms.length === 0 ? (
        <div style={{ ...S.card, color: "#888", fontSize: "0.9rem", maxWidth: 460 }}>
          No rooms yet. Create rooms on the <strong>Floor Plan</strong> page, then come back here to
          build scenes for them.
        </div>
      ) : (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(auto-fill, minmax(320px, 1fr))",
            gap: "1rem",
            alignItems: "start",
          }}
        >
          {rooms.map((room) => (
            <RoomSceneCard key={room.id} room={room} statesById={statesById} dialogs={dialogs} />
          ))}
        </div>
      )}

      {dialogs.element}
    </div>
  );
}

function RoomSceneCard({
  room,
  statesById,
  dialogs,
}: {
  room: Room;
  statesById: Map<string, LightState>;
  dialogs: Dialogs;
}) {
  const roomId = room.id;
  const count = room.light_ids.length;
  const [scenes, setScenes] = useState<RoomScene[]>([]);
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState("");

  async function load() {
    setScenes(await getRoomScenes(roomId));
  }
  useEffect(() => {
    load();
  }, [roomId]); // eslint-disable-line react-hooks/exhaustive-deps

  async function apply(sceneId: string) {
    setBusy(sceneId);
    try {
      await applyRoomScene(roomId, sceneId);
    } finally {
      setBusy("");
    }
  }

  async function remove(scene: RoomScene) {
    const ok = await dialogs.confirm({
      title: "Delete scene",
      message: `Delete scene "${scene.name}"?`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
    await deleteRoomScene(roomId, scene.id);
    await load();
  }

  /** Snapshot the room's currently-lit lights into a new scene. */
  async function saveCurrent() {
    const palette: string[] = [];
    let brightnessSum = 0;
    let litCount = 0;
    for (const id of room.light_ids) {
      const st = statesById.get(id);
      if (!st?.on) continue;
      litCount++;
      brightnessSum += st.brightness ?? 100;
      if (st.color) palette.push(rgbToHex(...xyToRgb(st.color.x, st.color.y, st.color.brightness)));
    }
    if (litCount === 0) {
      await dialogs.alert({
        title: "Nothing to save",
        message: "No lights in this room are on — turn some on first.",
      });
      return;
    }
    const name = await dialogs.prompt({
      title: "Save current as scene",
      message: "Saves the room's current colors and brightness.",
      placeholder: "Scene name",
      confirmLabel: "Save",
    });
    if (!name?.trim()) return;
    await createRoomScene(roomId, {
      name: name.trim(),
      brightness: Math.round(brightnessSum / litCount),
      palette,
    });
    await load();
  }

  return (
    <div style={{ ...S.card, gap: "0.7rem" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline" }}>
        <span style={{ fontWeight: 600 }}>{room.name}</span>
        <span style={{ color: "#666", fontSize: "0.75rem" }}>
          {count} light{count !== 1 ? "s" : ""}
        </span>
      </div>

      {scenes.length === 0 && !editing && (
        <span style={{ color: "#666", fontSize: "0.8rem" }}>No scenes yet.</span>
      )}

      <div style={{ display: "flex", flexDirection: "column", gap: "0.4rem" }}>
        {scenes.map((s) => (
          <div key={s.id} style={{ display: "flex", alignItems: "stretch", gap: "0.4rem" }}>
            <button
              onClick={() => apply(s.id)}
              disabled={count === 0 || busy === s.id}
              title={s.palette.length > 0 ? s.palette.join(" ") : "brightness only"}
              style={{
                ...S.buttonGhost,
                flex: 1,
                display: "flex",
                alignItems: "center",
                gap: "0.6rem",
                padding: "0.4rem 0.5rem",
                textAlign: "left",
                opacity: count === 0 ? 0.5 : 1,
              }}
            >
              <SceneSwatch palette={s.palette} width={44} height={22} radius={5} />
              <span style={{ flex: 1, fontSize: "0.85rem" }}>{busy === s.id ? "Applying…" : s.name}</span>
              {s.brightness != null && (
                <span style={{ color: "#666", fontSize: "0.72rem" }}>{Math.round(s.brightness)}%</span>
              )}
            </button>
            <button
              onClick={() => remove(s)}
              title="Delete scene"
              style={{ ...S.buttonGhost, padding: "0 0.5rem", color: "#866" }}
            >
              ×
            </button>
          </div>
        ))}
      </div>

      <div style={{ display: "flex", gap: "0.4rem" }}>
        <button
          onClick={() => setEditing((v) => !v)}
          style={{ ...S.buttonGhost, padding: "0.3rem 0.6rem", fontSize: "0.78rem" }}
        >
          {editing ? "Close" : "+ New scene"}
        </button>
        <button
          onClick={saveCurrent}
          disabled={count === 0}
          title="Save the room's current light colors and brightness as a scene"
          style={{ ...S.buttonGhost, padding: "0.3rem 0.6rem", fontSize: "0.78rem" }}
        >
          Save current
        </button>
      </div>

      {editing && (
        <SceneEditor
          onSave={async (scene) => {
            await createRoomScene(roomId, scene);
            setEditing(false);
            await load();
          }}
          onCancel={() => setEditing(false)}
        />
      )}
    </div>
  );
}
