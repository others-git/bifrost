import { useCallback, useEffect, useRef, useState } from "react";
import {
  activateScene,
  createPlan,
  getGroups,
  getPlan,
  getPlans,
  getScenes,
  mergePatch,
  putPlanLayout,
  putPlanLights,
  putPlanRooms,
  removePlan,
  setGroupState,
  setLightState,
  xyToRgb,
  type Group,
  type Light,
  type LightState,
  type LightStatePatch,
  type Mount,
  type Placement,
  type PlanDetail,
  type PlanSummary,
  type Scene,
} from "../api";
import { S } from "../styles";

type Tool = "view" | "floor" | "wall" | "erase" | "place" | "room";

const TOOLS: { id: Tool; label: string; hint: string }[] = [
  { id: "view", label: "View", hint: "Click a light to toggle it. Drag to pan, scroll to zoom." },
  { id: "floor", label: "Floor", hint: "Click-drag to paint floor tiles." },
  { id: "wall", label: "Wall", hint: "Click-drag along tile boundaries to draw walls. Leave gaps for doors." },
  { id: "erase", label: "Erase", hint: "Click-drag to remove tiles, walls, and room assignments." },
  { id: "place", label: "Lights", hint: "Pick a light from the palette, then click a tile — near an edge wall-mounts it, the middle ceiling-mounts it. Click a placed light to remove it." },
  { id: "room", label: "Rooms", hint: "Pick or create a room on the right, then click-drag tiles to paint it. A tile belongs to one room." },
];

const ROOM_COLORS = ["#8b5cf6", "#3b82f6", "#22d3ee", "#4ade80", "#facc15", "#fb923c", "#f43f5e"];

const tileKey = (x: number, y: number) => `${x},${y}`;
const wallKey = (x: number, y: number, dir: "h" | "v") => `${x},${y},${dir}`;

interface EditRoom {
  id: string;
  name: string;
  tiles: Set<string>;
}

interface Popover {
  px: number;
  py: number;
  placements: Placement[];
}

export function FloorPlanPage({ lights }: { lights: Light[] }) {
  const [plans, setPlans] = useState<PlanSummary[]>([]);
  const [planId, setPlanId] = useState<string>("");
  const [plan, setPlan] = useState<PlanDetail | null>(null);

  // Editable copies of the plan layout.
  const [tiles, setTiles] = useState<Set<string>>(new Set());
  const [walls, setWalls] = useState<Set<string>>(new Set());
  const [placements, setPlacements] = useState<Placement[]>([]);
  const [rooms, setRooms] = useState<EditRoom[]>([]);
  const [dirty, setDirty] = useState(false);

  const [tool, setTool] = useState<Tool>("view");
  const [selectedLight, setSelectedLight] = useState<string>("");
  const [selectedRoom, setSelectedRoom] = useState<string>("");
  const [popover, setPopover] = useState<Popover | null>(null);
  const [toast, setToast] = useState("");

  const [groups, setGroups] = useState<Group[]>([]);
  const [scenes, setScenes] = useState<Scene[]>([]);

  // Live light states: start from the lights prop, patched by SSE + optimistic toggles.
  const [statesById, setStatesById] = useState<Map<string, LightState>>(new Map());

  useEffect(() => {
    setStatesById((prev) => {
      const next = new Map(prev);
      for (const l of lights) if (l.last_state && !next.has(l.id)) next.set(l.id, l.last_state);
      return next;
    });
  }, [lights]);

  useEffect(() => {
    const es = new EventSource("/api/events");
    es.addEventListener("light_state", (raw) => {
      const { device_id, patch } = JSON.parse((raw as MessageEvent).data) as {
        device_id: string;
        patch: LightStatePatch;
      };
      const light = lights.find((l) => l.device_id === device_id);
      if (!light) return;
      setStatesById((prev) => {
        const next = new Map(prev);
        next.set(light.id, mergePatch(next.get(light.id) ?? light.last_state, patch));
        return next;
      });
    });
    es.onerror = () => {};
    return () => es.close();
  }, [lights]);

  async function loadPlans() {
    const list = await getPlans();
    setPlans(list);
    if (list.length > 0 && !list.some((p) => p.id === planId)) setPlanId(list[0].id);
  }

  async function loadPlan(id: string) {
    const p = await getPlan(id);
    setPlan(p);
    setTiles(new Set(p.tiles.map(([x, y]) => tileKey(x, y))));
    setWalls(new Set(p.walls.map((w) => wallKey(w.x, w.y, w.dir))));
    setPlacements(p.lights);
    setRooms(
      p.rooms.map((r) => ({
        id: r.id,
        name: r.name,
        tiles: new Set(r.tiles.map(([x, y]) => tileKey(x, y))),
      })),
    );
    setDirty(false);
    setPopover(null);
  }

  useEffect(() => { loadPlans(); }, []); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    getGroups().then(setGroups);
    getScenes().then(setScenes);
  }, []);

  useEffect(() => {
    if (!planId) { setPlan(null); return; }
    loadPlan(planId);
  }, [planId]); // eslint-disable-line react-hooks/exhaustive-deps

  function showToast(msg: string) {
    setToast(msg);
    setTimeout(() => setToast(""), 3000);
  }

  async function handleCreate() {
    const name = window.prompt("Plan name (e.g. Ground Floor):");
    if (!name?.trim()) return;
    const dims = window.prompt("Size in feet, width x height (max 128):", "50x40");
    const m = dims?.match(/^\s*(\d+)\s*[x×]\s*(\d+)\s*$/);
    if (!m) return;
    const { id } = await createPlan(name.trim(), Number(m[1]), Number(m[2]));
    await loadPlans();
    setPlanId(id);
  }

  async function handleDelete() {
    if (!plan) return;
    if (!window.confirm(`Delete plan "${plan.name}"?`)) return;
    await removePlan(plan.id);
    setPlanId("");
    await loadPlans();
  }

  async function handleSave() {
    if (!plan) return;
    const tileArr = [...tiles].map((k) => k.split(",").map(Number) as [number, number]);
    const wallArr = [...walls].map((k) => {
      const [x, y, dir] = k.split(",");
      return { x: Number(x), y: Number(y), dir: dir as "h" | "v" };
    });
    const roomArr = rooms.map((r) => ({
      id: r.id,
      name: r.name,
      tiles: [...r.tiles].map((k) => k.split(",").map(Number) as [number, number]),
    }));
    try {
      await putPlanLayout(plan.id, tileArr, wallArr);
      await putPlanLights(plan.id, placements);
      await putPlanRooms(plan.id, roomArr);
      // Reload to pick up server-assigned room group IDs.
      await loadPlan(plan.id);
      setGroups(await getGroups());
      showToast("Saved.");
    } catch (e) {
      showToast(`Save failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  async function toggleLight(lightId: string) {
    const current = statesById.get(lightId) ?? { on: false };
    const next = { ...current, on: !current.on };
    setStatesById((prev) => new Map(prev).set(lightId, next)); // optimistic
    await setLightState(lightId, next);
  }

  /** "[Office] Right Lamp" — prefix a light's name with its first group. */
  function lightLabel(l: Light): string {
    const g = groups.find((g) => g.light_ids.includes(l.id));
    return g ? `[${g.name}] ${l.name}` : l.name;
  }

  const placedIds = new Set(placements.map((p) => p.light_id));

  return (
    <div style={{ padding: "1.5rem 2rem" }}>
      {/* Plan switcher row */}
      <div style={{ display: "flex", alignItems: "center", gap: "0.6rem", marginBottom: "0.9rem", flexWrap: "wrap" }}>
        {plans.map((p) => (
          <button
            key={p.id}
            onClick={() => setPlanId(p.id)}
            style={{ ...S.buttonGhost, ...(p.id === planId ? { borderColor: "#f90", color: "#f90" } : {}) }}
          >
            {p.name}
          </button>
        ))}
        <button onClick={handleCreate} style={S.buttonGhost}>+ New plan</button>
        {plan && (
          <>
            <span style={{ flex: 1 }} />
            <button onClick={handleSave} disabled={!dirty} style={dirty ? S.button : S.buttonGhost}>
              {dirty ? "Save changes" : "Saved"}
            </button>
            <button onClick={handleDelete} style={S.buttonDanger}>Delete</button>
          </>
        )}
      </div>

      {toast && (
        <div style={{ background: "#1e3a1e", border: "1px solid #2a5a2a", borderRadius: 8, padding: "0.5rem 1rem", marginBottom: "0.75rem", color: "#8f8", fontSize: "0.875rem" }}>
          {toast}
        </div>
      )}

      {!plan ? (
        <p style={{ color: "#666" }}>
          No floor plans yet. Create one, paint your layout, then place your lights on it.
        </p>
      ) : (
        <>
          {/* Toolbar */}
          <div style={{ display: "flex", gap: "0.4rem", marginBottom: "0.5rem", alignItems: "center", flexWrap: "wrap" }}>
            {TOOLS.map((t) => (
              <button
                key={t.id}
                onClick={() => { setTool(t.id); setPopover(null); }}
                style={{ ...S.buttonGhost, ...(tool === t.id ? { borderColor: "#f90", color: "#f90" } : {}) }}
              >
                {t.label}
              </button>
            ))}
            <span style={{ color: "#666", fontSize: "0.78rem", marginLeft: "0.5rem" }}>
              {TOOLS.find((t) => t.id === tool)?.hint}
            </span>
          </div>

          <div style={{ display: "flex", gap: "1rem", alignItems: "flex-start" }}>
            {tool === "view" && plan.rooms.length > 0 && (
              <RoomController
                plan={plan}
                scenes={scenes}
                onSetRoom={async (room, on) => {
                  if (!room.group_id) return;
                  const memberIds = roomMemberIds(room, placements);
                  setStatesById((prev) => {
                    const next = new Map(prev);
                    for (const id of memberIds) {
                      next.set(id, { ...(next.get(id) ?? { on: false }), on });
                    }
                    return next;
                  });
                  await setGroupState(room.group_id, { on });
                }}
                onApplyScene={async (room, sceneId) => {
                  const memberIds = roomMemberIds(room, placements);
                  if (memberIds.length === 0) return;
                  await activateScene(sceneId, memberIds);
                }}
                memberCount={(room) => roomMemberIds(room, placements).length}
              />
            )}

            <PlanCanvas
              plan={plan}
              tiles={tiles}
              walls={walls}
              placements={placements}
              rooms={rooms}
              selectedRoom={selectedRoom}
              statesById={statesById}
              tool={tool}
              selectedLight={selectedLight}
              onMutate={(fn) => { fn(); setDirty(true); setPopover(null); }}
              setTiles={setTiles}
              setWalls={setWalls}
              setPlacements={setPlacements}
              setRooms={setRooms}
              onLightClick={(pls, px, py) => {
                if (pls.length === 1) toggleLight(pls[0].light_id);
                else setPopover({ px, py, placements: pls });
              }}
            />

            {tool === "place" && (
              <div style={{ width: 220, flexShrink: 0 }}>
                <h3 style={{ margin: "0 0 0.5rem", fontSize: "0.9rem", color: "#aaa" }}>Lights</h3>
                <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem" }}>
                  {lights.map((l) => (
                    <button
                      key={l.id}
                      onClick={() => setSelectedLight(l.id === selectedLight ? "" : l.id)}
                      style={{
                        ...S.buttonGhost,
                        textAlign: "left",
                        fontSize: "0.8rem",
                        ...(l.id === selectedLight ? { borderColor: "#f90", color: "#f90" } : {}),
                        ...(placedIds.has(l.id) ? { opacity: 0.55 } : {}),
                      }}
                    >
                      {placedIds.has(l.id) ? "✓ " : ""}{lightLabel(l)}
                    </button>
                  ))}
                  {lights.length === 0 && (
                    <span style={{ color: "#666", fontSize: "0.8rem" }}>No lights discovered yet.</span>
                  )}
                </div>
              </div>
            )}

            {tool === "room" && (
              <RoomEditorPanel
                rooms={rooms}
                selectedRoom={selectedRoom}
                onSelect={setSelectedRoom}
                onCreate={() => {
                  const name = window.prompt("Room name:");
                  if (!name?.trim()) return;
                  const room: EditRoom = { id: "", name: name.trim(), tiles: new Set() };
                  // Local placeholder id so selection works before save.
                  room.id = `new-${Date.now()}`;
                  setRooms((prev) => [...prev, room]);
                  setSelectedRoom(room.id);
                  setDirty(true);
                }}
                onRename={(id) => {
                  const room = rooms.find((r) => r.id === id);
                  const name = window.prompt("Room name:", room?.name ?? "");
                  if (!name?.trim()) return;
                  setRooms((prev) => prev.map((r) => (r.id === id ? { ...r, name: name.trim() } : r)));
                  setDirty(true);
                }}
                onDelete={(id) => {
                  if (!window.confirm("Remove this room? Its auto-group is deleted on save.")) return;
                  setRooms((prev) => prev.filter((r) => r.id !== id));
                  if (selectedRoom === id) setSelectedRoom("");
                  setDirty(true);
                }}
              />
            )}
          </div>

          {popover && (
            <div
              style={{
                position: "fixed",
                left: popover.px,
                top: popover.py,
                background: "#1c1c1c",
                border: "1px solid #333",
                borderRadius: 8,
                padding: "0.5rem",
                zIndex: 10,
                display: "flex",
                flexDirection: "column",
                gap: "0.3rem",
                boxShadow: "0 4px 16px rgba(0,0,0,0.5)",
              }}
            >
              {popover.placements.map((p) => {
                const light = lights.find((l) => l.id === p.light_id);
                const on = statesById.get(p.light_id)?.on ?? false;
                return (
                  <button
                    key={p.light_id}
                    onClick={() => toggleLight(p.light_id)}
                    style={{ ...S.buttonGhost, fontSize: "0.8rem", textAlign: "left", color: on ? "#f90" : "#888" }}
                  >
                    {on ? "● " : "○ "}{light ? lightLabel(light) : p.light_id}
                  </button>
                );
              })}
              <button onClick={() => setPopover(null)} style={{ ...S.buttonGhost, fontSize: "0.75rem", color: "#666" }}>
                Close
              </button>
            </div>
          )}
        </>
      )}
    </div>
  );
}

/** Light IDs currently placed on a (saved) room's tiles. */
function roomMemberIds(room: { tiles: [number, number][] }, placements: Placement[]): string[] {
  const tileSet = new Set(room.tiles.map(([x, y]) => tileKey(x, y)));
  return placements.filter((p) => tileSet.has(tileKey(p.x, p.y))).map((p) => p.light_id);
}

// ── Room controller (left of canvas, view mode) ─────────────────────────────

function RoomController({
  plan,
  scenes,
  onSetRoom,
  onApplyScene,
  memberCount,
}: {
  plan: PlanDetail;
  scenes: Scene[];
  onSetRoom: (room: PlanDetail["rooms"][number], on: boolean) => Promise<void>;
  onApplyScene: (room: PlanDetail["rooms"][number], sceneId: string) => Promise<void>;
  memberCount: (room: PlanDetail["rooms"][number]) => number;
}) {
  const [sceneId, setSceneId] = useState(scenes[0]?.id ?? "");
  const [busy, setBusy] = useState("");

  useEffect(() => {
    if (scenes.length > 0 && !scenes.some((s) => s.id === sceneId)) setSceneId(scenes[0].id);
  }, [scenes, sceneId]);

  return (
    <div style={{ width: 230, flexShrink: 0, display: "flex", flexDirection: "column", gap: "0.6rem" }}>
      <h3 style={{ margin: 0, fontSize: "0.9rem", color: "#aaa" }}>Rooms</h3>

      {scenes.length > 0 && (
        <label style={{ display: "flex", flexDirection: "column", gap: "0.25rem", fontSize: "0.78rem", color: "#888" }}>
          Scene
          <select value={sceneId} onChange={(e) => setSceneId(e.target.value)} style={{ ...S.input, cursor: "pointer" }}>
            {scenes.map((s) => (
              <option key={s.id} value={s.id}>{s.name}</option>
            ))}
          </select>
        </label>
      )}

      {plan.rooms.map((room, i) => {
        const color = ROOM_COLORS[i % ROOM_COLORS.length];
        const count = memberCount(room);
        return (
          <div key={room.id} style={{ ...S.card, gap: "0.5rem", borderLeft: `3px solid ${color}` }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline" }}>
              <span style={{ fontWeight: 600, fontSize: "0.9rem" }}>{room.name}</span>
              <span style={{ color: "#666", fontSize: "0.75rem" }}>
                {count} light{count !== 1 ? "s" : ""}
              </span>
            </div>
            <div style={{ display: "flex", gap: "0.4rem", flexWrap: "wrap" }}>
              <button
                onClick={async () => { setBusy(room.id); try { await onSetRoom(room, true); } finally { setBusy(""); } }}
                disabled={busy === room.id || !room.group_id || count === 0}
                style={{ ...S.buttonGhost, padding: "0.3rem 0.6rem", fontSize: "0.78rem" }}
              >
                On
              </button>
              <button
                onClick={async () => { setBusy(room.id); try { await onSetRoom(room, false); } finally { setBusy(""); } }}
                disabled={busy === room.id || !room.group_id || count === 0}
                style={{ ...S.buttonGhost, padding: "0.3rem 0.6rem", fontSize: "0.78rem" }}
              >
                Off
              </button>
              {scenes.length > 0 && (
                <button
                  onClick={async () => { setBusy(room.id); try { await onApplyScene(room, sceneId); } finally { setBusy(""); } }}
                  disabled={busy === room.id || !sceneId || count === 0}
                  title="Apply the selected scene to this room only"
                  style={{ ...S.buttonGhost, padding: "0.3rem 0.6rem", fontSize: "0.78rem" }}
                >
                  Apply scene
                </button>
              )}
            </div>
          </div>
        );
      })}

      <span style={{ color: "#555", fontSize: "0.72rem" }}>
        Rooms are painted with the Rooms tool. Each room keeps a group in sync
        with the lights placed inside it.
      </span>
    </div>
  );
}

// ── Room editor panel (right of canvas, room tool) ──────────────────────────

function RoomEditorPanel({
  rooms,
  selectedRoom,
  onSelect,
  onCreate,
  onRename,
  onDelete,
}: {
  rooms: EditRoom[];
  selectedRoom: string;
  onSelect: (id: string) => void;
  onCreate: () => void;
  onRename: (id: string) => void;
  onDelete: (id: string) => void;
}) {
  return (
    <div style={{ width: 220, flexShrink: 0, display: "flex", flexDirection: "column", gap: "0.4rem" }}>
      <h3 style={{ margin: "0 0 0.2rem", fontSize: "0.9rem", color: "#aaa" }}>Rooms</h3>
      {rooms.map((r, i) => (
        <div key={r.id} style={{ display: "flex", gap: "0.3rem", alignItems: "center" }}>
          <button
            onClick={() => onSelect(r.id === selectedRoom ? "" : r.id)}
            style={{
              ...S.buttonGhost,
              flex: 1,
              textAlign: "left",
              fontSize: "0.8rem",
              borderLeft: `3px solid ${ROOM_COLORS[i % ROOM_COLORS.length]}`,
              ...(r.id === selectedRoom ? { borderColor: "#f90", color: "#f90" } : {}),
            }}
          >
            {r.name}
            <span style={{ color: "#666", marginLeft: "0.4rem" }}>{r.tiles.size}</span>
          </button>
          <button onClick={() => onRename(r.id)} title="Rename" style={{ ...S.buttonGhost, padding: "0.3rem 0.5rem" }}>✎</button>
          <button onClick={() => onDelete(r.id)} title="Delete" style={{ ...S.buttonGhost, padding: "0.3rem 0.5rem", color: "#866" }}>×</button>
        </div>
      ))}
      <button onClick={onCreate} style={S.buttonGhost}>+ New room</button>
      {rooms.length > 0 && !selectedRoom && (
        <span style={{ color: "#666", fontSize: "0.75rem" }}>Select a room, then paint its tiles.</span>
      )}
    </div>
  );
}

// ── Canvas ───────────────────────────────────────────────────────────────────

function PlanCanvas({
  plan,
  tiles,
  walls,
  placements,
  rooms,
  selectedRoom,
  statesById,
  tool,
  selectedLight,
  onMutate,
  setTiles,
  setWalls,
  setPlacements,
  setRooms,
  onLightClick,
}: {
  plan: PlanDetail;
  tiles: Set<string>;
  walls: Set<string>;
  placements: Placement[];
  rooms: EditRoom[];
  selectedRoom: string;
  statesById: Map<string, LightState>;
  tool: Tool;
  selectedLight: string;
  onMutate: (fn: () => void) => void;
  setTiles: React.Dispatch<React.SetStateAction<Set<string>>>;
  setWalls: React.Dispatch<React.SetStateAction<Set<string>>>;
  setPlacements: React.Dispatch<React.SetStateAction<Placement[]>>;
  setRooms: React.Dispatch<React.SetStateAction<EditRoom[]>>;
  onLightClick: (placements: Placement[], px: number, py: number) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [view, setView] = useState({ cell: 0, ox: 0, oy: 0 }); // cell=0 → fit on first draw
  const drag = useRef<{ px: number; py: number; moved: boolean; panning: boolean } | null>(null);

  // Mount-point position within a tile, in tile units.
  const mountOffset = (m: Mount): [number, number] =>
    m === "n" ? [0.5, 0.08] : m === "s" ? [0.5, 0.92] : m === "e" ? [0.92, 0.5] : m === "w" ? [0.08, 0.5] : [0.5, 0.5];

  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const dpr = window.devicePixelRatio || 1;
    const cssW = canvas.clientWidth;
    const cssH = canvas.clientHeight;
    // Round before comparing: with fractional DPR an int-vs-float comparison
    // is always unequal, and rewriting the width attribute every frame fed a
    // flexbox min-width:auto growth loop.
    const targetW = Math.round(cssW * dpr);
    const targetH = Math.round(cssH * dpr);
    if (canvas.width !== targetW || canvas.height !== targetH) {
      canvas.width = targetW;
      canvas.height = targetH;
    }

    let { cell, ox, oy } = view;
    if (cell === 0) {
      cell = Math.max(8, Math.min(48, Math.floor(Math.min(cssW / plan.width, cssH / plan.height))));
      ox = (cssW - plan.width * cell) / 2;
      oy = (cssH - plan.height * cell) / 2;
      setView({ cell, ox, oy });
      return;
    }

    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssW, cssH);
    ctx.fillStyle = "#0d0d0f";
    ctx.fillRect(0, 0, cssW, cssH);

    // Floor tiles
    ctx.fillStyle = "#1d2129";
    for (const k of tiles) {
      const [x, y] = k.split(",").map(Number);
      ctx.fillRect(ox + x * cell, oy + y * cell, cell, cell);
    }

    // Room tints
    rooms.forEach((room, i) => {
      const color = ROOM_COLORS[i % ROOM_COLORS.length];
      const emphasized = tool === "room" && room.id === selectedRoom;
      ctx.fillStyle = color;
      ctx.globalAlpha = emphasized ? 0.32 : 0.15;
      for (const k of room.tiles) {
        const [x, y] = k.split(",").map(Number);
        ctx.fillRect(ox + x * cell, oy + y * cell, cell, cell);
      }
      ctx.globalAlpha = 1;
    });

    // Grid
    ctx.strokeStyle = "rgba(255,255,255,0.05)";
    ctx.lineWidth = 1;
    for (let x = 0; x <= plan.width; x++) {
      ctx.beginPath();
      ctx.moveTo(ox + x * cell, oy);
      ctx.lineTo(ox + x * cell, oy + plan.height * cell);
      ctx.stroke();
    }
    for (let y = 0; y <= plan.height; y++) {
      ctx.beginPath();
      ctx.moveTo(ox, oy + y * cell);
      ctx.lineTo(ox + plan.width * cell, oy + y * cell);
      ctx.stroke();
    }

    // Walls
    ctx.strokeStyle = "#d9deeb";
    ctx.lineWidth = Math.max(2, cell * 0.14);
    ctx.lineCap = "square";
    for (const k of walls) {
      const [xs, ys, dir] = k.split(",");
      const x = Number(xs), y = Number(ys);
      ctx.beginPath();
      if (dir === "h") {
        ctx.moveTo(ox + x * cell, oy + y * cell);
        ctx.lineTo(ox + (x + 1) * cell, oy + y * cell);
      } else {
        ctx.moveTo(ox + x * cell, oy + y * cell);
        ctx.lineTo(ox + x * cell, oy + (y + 1) * cell);
      }
      ctx.stroke();
    }

    // Room labels (above tints and walls, below lights)
    if (cell >= 9) {
      rooms.forEach((room) => {
        if (room.tiles.size === 0) return;
        let sx = 0, sy = 0;
        for (const k of room.tiles) {
          const [x, y] = k.split(",").map(Number);
          sx += x + 0.5;
          sy += y + 0.5;
        }
        const cx = ox + (sx / room.tiles.size) * cell;
        const cy = oy + (sy / room.tiles.size) * cell;
        ctx.font = `600 ${Math.max(10, cell * 0.45)}px system-ui`;
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillStyle = "rgba(255,255,255,0.55)";
        ctx.fillText(room.name, cx, cy);
      });
    }

    // Lights, clustered by (x, y, mount)
    const clusters = new Map<string, Placement[]>();
    for (const p of placements) {
      const k = `${p.x},${p.y},${p.mount}`;
      clusters.set(k, [...(clusters.get(k) ?? []), p]);
    }
    for (const group of clusters.values()) {
      const p = group[0];
      const [mx, my] = mountOffset(p.mount);
      const cx = ox + (p.x + mx) * cell;
      const cy = oy + (p.y + my) * cell;
      const r = Math.max(3.5, cell * 0.18);

      const states = group.map((g) => statesById.get(g.light_id));
      const lit = states.find((s) => s?.on);
      let fill = "#3a3d45";
      if (lit) {
        if (lit.color) {
          const [rr, gg, bb] = xyToRgb(lit.color.x, lit.color.y, Math.max(lit.color.brightness, 0.25));
          fill = `rgb(${rr},${gg},${bb})`;
        } else {
          fill = "#ffd9a0";
        }
        const glow = ctx.createRadialGradient(cx, cy, r * 0.5, cx, cy, r * 3);
        glow.addColorStop(0, fill);
        glow.addColorStop(1, "rgba(0,0,0,0)");
        ctx.globalAlpha = 0.35 * Math.min(1, ((lit.brightness ?? 100) / 100) + 0.3);
        ctx.fillStyle = glow;
        ctx.beginPath();
        ctx.arc(cx, cy, r * 3, 0, Math.PI * 2);
        ctx.fill();
        ctx.globalAlpha = 1;
      }
      ctx.fillStyle = fill;
      ctx.beginPath();
      ctx.arc(cx, cy, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.strokeStyle = "rgba(0,0,0,0.6)";
      ctx.lineWidth = 1;
      ctx.stroke();

      if (group.length > 1) {
        ctx.fillStyle = "#fff";
        ctx.font = `${Math.max(9, cell * 0.3)}px system-ui`;
        ctx.textAlign = "left";
        ctx.textBaseline = "middle";
        ctx.fillText(`×${group.length}`, cx + r + 2, cy);
      }
    }
  }, [view, plan, tiles, walls, placements, rooms, selectedRoom, tool, statesById]);

  useEffect(() => { draw(); }, [draw]);
  useEffect(() => {
    // Redraw whenever the canvas box changes (window resize, palette
    // appearing/disappearing) — ResizeObserver catches both.
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ro = new ResizeObserver(() => draw());
    ro.observe(canvas);
    return () => ro.disconnect();
  }, [draw]);

  // Reset the fit when switching plans.
  useEffect(() => { setView({ cell: 0, ox: 0, oy: 0 }); }, [plan.id]);

  function toGrid(e: React.PointerEvent): { gx: number; gy: number } {
    const rect = canvasRef.current!.getBoundingClientRect();
    return {
      gx: (e.clientX - rect.left - view.ox) / view.cell,
      gy: (e.clientY - rect.top - view.oy) / view.cell,
    };
  }

  function nearestEdge(gx: number, gy: number): { x: number; y: number; dir: "h" | "v" } | null {
    const tx = Math.floor(gx), ty = Math.floor(gy);
    const fx = gx - tx, fy = gy - ty;
    const candidates: { d: number; x: number; y: number; dir: "h" | "v" }[] = [
      { d: fy, x: tx, y: ty, dir: "h" },
      { d: 1 - fy, x: tx, y: ty + 1, dir: "h" },
      { d: fx, x: tx, y: ty, dir: "v" },
      { d: 1 - fx, x: tx + 1, y: ty, dir: "v" },
    ];
    candidates.sort((a, b) => a.d - b.d);
    const best = candidates[0];
    // Bounds: 'h' walls x<width, y<=height; 'v' walls x<=width, y<height.
    const xMax = best.dir === "v" ? plan.width : plan.width - 1;
    const yMax = best.dir === "h" ? plan.height : plan.height - 1;
    if (best.x < 0 || best.x > xMax || best.y < 0 || best.y > yMax) return null;
    return best;
  }

  function applyTool(gx: number, gy: number, e: React.PointerEvent) {
    const tx = Math.floor(gx), ty = Math.floor(gy);
    const inBounds = tx >= 0 && tx < plan.width && ty >= 0 && ty < plan.height;

    if (tool === "floor" && inBounds) {
      onMutate(() => setTiles((prev) => new Set(prev).add(tileKey(tx, ty))));
    } else if (tool === "wall") {
      const edge = nearestEdge(gx, gy);
      if (edge) onMutate(() => setWalls((prev) => new Set(prev).add(wallKey(edge.x, edge.y, edge.dir))));
    } else if (tool === "room" && inBounds && selectedRoom) {
      const k = tileKey(tx, ty);
      onMutate(() =>
        setRooms((prev) =>
          prev.map((r) => {
            const tiles = new Set(r.tiles);
            // A tile belongs to exactly one room.
            if (r.id === selectedRoom) tiles.add(k);
            else tiles.delete(k);
            return { ...r, tiles };
          }),
        ),
      );
    } else if (tool === "erase") {
      const edge = nearestEdge(gx, gy);
      const nearWall = edge && Math.min(gx - Math.floor(gx), 1 - (gx - Math.floor(gx)), gy - Math.floor(gy), 1 - (gy - Math.floor(gy))) < 0.2;
      const k = tileKey(tx, ty);
      onMutate(() => {
        if (nearWall && edge) {
          setWalls((prev) => { const n = new Set(prev); n.delete(wallKey(edge.x, edge.y, edge.dir)); return n; });
        } else if (inBounds) {
          setTiles((prev) => { const n = new Set(prev); n.delete(k); return n; });
          setRooms((prev) =>
            prev.map((r) => {
              if (!r.tiles.has(k)) return r;
              const tiles = new Set(r.tiles);
              tiles.delete(k);
              return { ...r, tiles };
            }),
          );
        }
      });
    } else if (tool === "place" && e.type === "pointerdown" && inBounds) {
      // Hit an existing placement? Remove it.
      const hit = hitPlacement(gx, gy);
      if (hit) {
        onMutate(() => setPlacements((prev) => prev.filter((p) => p.light_id !== hit.light_id)));
        return;
      }
      if (!selectedLight) return;
      const fx = gx - tx, fy = gy - ty;
      const m = Math.min(fx, 1 - fx, fy, 1 - fy);
      let mount: Mount = "c";
      if (m < 0.28) {
        mount = fy === m ? "n" : 1 - fy === m ? "s" : fx === m ? "w" : "e";
      }
      onMutate(() =>
        setPlacements((prev) => [
          ...prev.filter((p) => p.light_id !== selectedLight),
          { light_id: selectedLight, x: tx, y: ty, mount },
        ]),
      );
    }
  }

  function hitPlacement(gx: number, gy: number): Placement | null {
    for (const p of placements) {
      const [mx, my] = mountOffset(p.mount);
      const dx = gx - (p.x + mx), dy = gy - (p.y + my);
      if (Math.hypot(dx, dy) < 0.3) return p;
    }
    return null;
  }

  function handlePointerDown(e: React.PointerEvent) {
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    const panning = e.button === 1 || e.button === 2 || tool === "view";
    drag.current = { px: e.clientX, py: e.clientY, moved: false, panning };
    if (!panning) {
      const { gx, gy } = toGrid(e);
      applyTool(gx, gy, e);
    }
  }

  function handlePointerMove(e: React.PointerEvent) {
    if (!drag.current) return;
    const dx = e.clientX - drag.current.px;
    const dy = e.clientY - drag.current.py;
    if (Math.hypot(dx, dy) > 4) drag.current.moved = true;

    if (drag.current.panning && drag.current.moved) {
      setView((v) => ({ ...v, ox: v.ox + dx, oy: v.oy + dy }));
      drag.current.px = e.clientX;
      drag.current.py = e.clientY;
    } else if (!drag.current.panning && (tool === "floor" || tool === "wall" || tool === "erase" || tool === "room")) {
      const { gx, gy } = toGrid(e);
      applyTool(gx, gy, e);
    }
  }

  function handlePointerUp(e: React.PointerEvent) {
    const wasClick = drag.current && !drag.current.moved;
    const wasPanning = drag.current?.panning;
    drag.current = null;
    if (wasClick && wasPanning && tool === "view" && e.button === 0) {
      const { gx, gy } = toGrid(e);
      const hit = hitPlacement(gx, gy);
      if (hit) {
        const k = `${hit.x},${hit.y},${hit.mount}`;
        const cluster = placements.filter((p) => `${p.x},${p.y},${p.mount}` === k);
        onLightClick(cluster, e.clientX, e.clientY);
      }
    }
  }

  function handleWheel(e: React.WheelEvent) {
    const rect = canvasRef.current!.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    setView((v) => {
      const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15;
      const cell = Math.max(4, Math.min(80, v.cell * factor));
      const scale = cell / v.cell;
      return { cell, ox: mx - (mx - v.ox) * scale, oy: my - (my - v.oy) * scale };
    });
  }

  return (
    // minWidth: 0 stops the canvas's intrinsic width (set for DPR sharpness)
    // from widening this flex item — without it the layout grows every frame.
    <div style={{ flex: 1, minWidth: 0 }}>
      <canvas
        ref={canvasRef}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onWheel={handleWheel}
        onContextMenu={(e) => e.preventDefault()}
        style={{
          width: "100%",
          height: "calc(100vh - 250px)",
          minHeight: 360,
          borderRadius: 10,
          border: "1px solid #262626",
          touchAction: "none",
          cursor: tool === "view" ? "grab" : "crosshair",
          display: "block",
        }}
      />
    </div>
  );
}
