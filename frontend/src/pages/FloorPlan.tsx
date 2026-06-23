import { useCallback, useEffect, useRef, useState } from "react";
import {
  activateScene,
  createScene,
  createPlan,
  getMediaDevices,
  getScenes,
  getPlan,
  getPlans,
  getRooms,
  mergePatch,
  putPlanAudio,
  putPlanLayout,
  putPlanLights,
  putPlanRooms,
  putPlanSize,
  removePlan,
  removeScene,
  rgbToHex,
  rgbToXy,
  setLightState,
  setRoomState,
  xyToRgb,
  type MediaDevice,
  type MediaPlacement,
  type Light,
  type LightState,
  type LightStatePatch,
  type Mount,
  type Placement,
  type PlanDetail,
  type PlanSummary,
  type Room,
  type Scene,
} from "../api";
import { ColorWheel, hexToHs, hsvToRgb, LightEditor, type LightControlChange } from "../components/LightEditor";
import {
  aggregateLightState,
  lightOptimistic,
  lightSupports,
  lightWrite,
  type LightLike,
} from "../components/lightControl";
import { DeviceControl } from "../components/DeviceControl";
import { RoomVolumeStrip } from "../components/RoomMedia";
import { SceneButton, SceneModal } from "../components/scenes";
import { Modal, useDialogs } from "../components/dialogs";
import { ACCENT, S } from "../styles";
import { alpha, color, font, glassCard, glow, labelType, radius } from "../theme";
import { Glyph } from "../components/glyphs";
import { Button, Switch as SharedSwitch } from "../components/controls";

type Tool = "view" | "floor" | "wall" | "erase" | "place" | "room" | "paint";

const TOOLS: { id: Tool; label: string; hint: string }[] = [
  { id: "view", label: "View", hint: "Click a light to open its editor. Drag to pan, scroll to zoom." },
  { id: "floor", label: "Floor", hint: "Click-drag to paint floor tiles." },
  { id: "wall", label: "Wall", hint: "Click-drag along tile boundaries to draw walls. Leave gaps for doors." },
  { id: "erase", label: "Erase", hint: "Click-drag to remove tiles, walls, and room assignments." },
  { id: "place", label: "Devices", hint: "Pick Lights or Speakers, choose a device, then click a tile to place it. Lights near an edge wall-mount; drag across tiles to lay an LED strip. Click a placed device to remove it." },
  { id: "room", label: "Rooms", hint: "Pick or create a room on the right, then click-drag tiles to paint it. A tile belongs to one room." },
  { id: "paint", label: "Brush", hint: "Pick a color and brightness on the right, then click or brush across lights to color them live." },
];

/** Glyph per tool for the icon palette. */
const TOOL_GLYPH: Record<Tool, string> = {
  view: "cursor",
  floor: "floor",
  wall: "wall",
  erase: "erase",
  place: "place",
  room: "room",
  paint: "brush",
};

/** Device category chosen under the Devices tool. */
type PlaceCategory = "light" | "media";

const ROOM_COLORS = ["#8b5cf6", "#3b82f6", "#22d3ee", "#4ade80", "#facc15", "#fb923c", "#f43f5e"];

const tileKey = (x: number, y: number) => `${x},${y}`;
const wallKey = (x: number, y: number, dir: "h" | "v") => `${x},${y},${dir}`;

/** How far (in tile units) an edge-mounted strip sits from a tile's centerline.
 *  0.42 puts it ~0.08 from the wall, matching the old fixed edge mount. */
const STRIP_INSET = 0.42;

/**
 * The on-canvas polyline for an LED strip, in tile coordinates.
 *
 * The naive approach — offset every vertex by one fixed mount point — makes the
 * strip bend through the middle of corner tiles and keep the wrong offset after
 * a turn (so a strip that should follow trim around a corner "jumps through the
 * wall"). Instead we treat the run as a centerline and offset it to one wall
 * side, mitering the 90° corners so the line bends at the true tile corner and
 * each leg hugs its own wall — like trim running around the inside of a hallway.
 *
 * The hand (which side) is taken from the start tile's mount; `c` (ceiling)
 * keeps the strip on the centerline. The path's turns are always right angles
 * (collinear moves are merged while drawing), which makes the miter exact.
 */
function stripPath(p: Placement): [number, number][] {
  const verts: [number, number][] = [[p.x, p.y], ...(p.points ?? [])];
  const centers = verts.map(([x, y]) => [x + 0.5, y + 0.5] as [number, number]);
  if (p.mount === "c" || verts.length < 2) return centers;

  const dir = (a: [number, number], b: [number, number]): [number, number] => [
    Math.sign(b[0] - a[0]),
    Math.sign(b[1] - a[1]),
  ];
  // Right-hand normal of a unit direction.
  const rightNormal = (d: [number, number]): [number, number] => [d[1], -d[0]];
  const mountDir: Record<string, [number, number]> = {
    n: [0, -1],
    s: [0, 1],
    e: [1, 0],
    w: [-1, 0],
    c: [0, 0],
  };

  // Pick the hand so the first leg hugs the mounted edge.
  const rn0 = rightNormal(dir(verts[0], verts[1]));
  const md = mountDir[p.mount] ?? [0, 0];
  const hand = md[0] * rn0[0] + md[1] * rn0[1] < 0 ? -1 : 1;
  const sideOf = (d: [number, number]): [number, number] => {
    const rn = rightNormal(d);
    return [rn[0] * hand, rn[1] * hand];
  };

  // Per-leg side normals; vertex i sits between leg i-1 and leg i.
  const sides: [number, number][] = [];
  for (let i = 1; i < verts.length; i++) sides.push(sideOf(dir(verts[i - 1], verts[i])));

  return centers.map(([cx, cy], i) => {
    const sIn = i > 0 ? sides[i - 1] : null;
    const sOut = i < sides.length ? sides[i] : null;
    // Endpoints offset by their one leg; corners by the sum of both legs, which
    // for perpendicular legs lands exactly on the mitered corner point.
    const dx = (sIn?.[0] ?? 0) + (sOut?.[0] ?? 0);
    const dy = (sIn?.[1] ?? 0) + (sOut?.[1] ?? 0);
    return [cx + STRIP_INSET * dx, cy + STRIP_INSET * dy] as [number, number];
  });
}

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

/** What the shared editor fly-out is currently pointed at. */
type EditorTarget =
  | { kind: "light"; id: string; anchor: HTMLElement | { x: number; y: number } }
  | { kind: "media"; id: string; anchor: HTMLElement | { x: number; y: number } }
  | { kind: "room"; roomId: string; anchor: HTMLElement };

export function FloorPlanPage({ lights }: { lights: Light[] }) {
  const dialogs = useDialogs();
  const [plans, setPlans] = useState<PlanSummary[]>([]);
  const [planId, setPlanId] = useState<string>("");
  const [plan, setPlan] = useState<PlanDetail | null>(null);

  // Editable copies of the plan layout.
  const [tiles, setTiles] = useState<Set<string>>(new Set());
  const [walls, setWalls] = useState<Set<string>>(new Set());
  const [placements, setPlacements] = useState<Placement[]>([]);
  const [mediaPlacements, setMediaPlacements] = useState<MediaPlacement[]>([]);
  const [rooms, setRooms] = useState<EditRoom[]>([]);
  const [dirty, setDirty] = useState(false);

  const [tool, setTool] = useState<Tool>("view");
  // Which device + category the Devices tool will place.
  const [placeCategory, setPlaceCategory] = useState<PlaceCategory>("light");
  const [selected, setSelected] = useState<{ kind: PlaceCategory; id: string } | null>(null);
  const [selectedRoom, setSelectedRoom] = useState<string>("");
  // Audio devices (for the Speakers palette + on-plan fly-out), keyed by id;
  // patched optimistically by the fly-out.
  const [mediaById, setMediaById] = useState<Map<string, MediaDevice>>(new Map());
  const [paintColor, setPaintColor] = useState("#ff9900");
  const [paintBrightness, setPaintBrightness] = useState(100);
  const [popover, setPopover] = useState<Popover | null>(null);
  const [editor, setEditor] = useState<EditorTarget | null>(null);
  const [showNewPlan, setShowNewPlan] = useState(false);
  const editTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const [toast, setToast] = useState("");

  const [allRooms, setAllRooms] = useState<Room[]>([]);
  const [scenes, setScenes] = useState<Scene[]>([]);
  // The room whose scene modal is open (opened from its color editor).
  const [scenesRoom, setScenesRoom] = useState<Room | null>(null);
  const [sceneBusy, setSceneBusy] = useState(false);

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
    setMediaPlacements(p.media);
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

  // Audio devices for the Speakers palette + on-plan fly-out.
  async function loadMediaDevices() {
    const list = await getMediaDevices();
    setMediaById(new Map(list.map((d) => [d.id, d])));
  }

  function patchAudio(id: string, patch: Partial<MediaDevice["state"]>) {
    setMediaById((prev) => {
      const dev = prev.get(id);
      if (!dev) return prev;
      return new Map(prev).set(id, { ...dev, state: { ...dev.state, ...patch } });
    });
  }

  useEffect(() => { loadPlans(); }, []); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    getRooms().then(setAllRooms);
  }, []);
  useEffect(() => {
    getScenes().then(setScenes);
  }, []);
  useEffect(() => {
    loadMediaDevices();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!planId) { setPlan(null); return; }
    loadPlan(planId);
  }, [planId]); // eslint-disable-line react-hooks/exhaustive-deps

  function showToast(msg: string) {
    setToast(msg);
    setTimeout(() => setToast(""), 3000);
  }

  async function handleCreate(name: string, width: number, height: number) {
    const { id } = await createPlan(name, width, height);
    setShowNewPlan(false);
    await loadPlans();
    setPlanId(id);
  }

  async function handleDelete() {
    if (!plan) return;
    const ok = await dialogs.confirm({
      title: "Delete floor plan",
      message: `Delete floor plan "${plan.name}"?`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
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
      await putPlanAudio(plan.id, mediaPlacements);
      await putPlanRooms(plan.id, roomArr);
      // Reload to pick up server-assigned Room bindings and memberships.
      await loadPlan(plan.id);
      setAllRooms(await getRooms());
      showToast("Saved.");
    } catch (e) {
      showToast(`Save failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  async function handleResize(w: number, h: number) {
    if (!plan) return;
    const lostTiles = [...tiles].filter((k) => {
      const [x, y] = k.split(",").map(Number);
      return x >= w || y >= h;
    }).length;
    const lostLights = placements.filter(
      (p) => p.x >= w || p.y >= h || p.points?.some(([vx, vy]) => vx >= w || vy >= h),
    ).length;
    if (lostTiles > 0 || lostLights > 0) {
      const ok = await dialogs.confirm({
        title: "Shrink plan",
        message: `Shrinking to ${w}×${h} removes ${lostTiles} tile(s) and ${lostLights} placed light(s) outside the new bounds. Continue?`,
        confirmLabel: "Shrink",
        danger: true,
      });
      if (!ok) return;
    }
    try {
      // Commit pending edits first — resizing reloads the plan from the server.
      if (dirty) await handleSave();
      await putPlanSize(plan.id, w, h);
      await loadPlan(plan.id);
      await loadPlans();
      showToast(`Resized to ${w}×${h}.`);
    } catch (e) {
      showToast(`Resize failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  async function setRoom(room: Room, on: boolean) {
    setStatesById((prev) => {
      const next = new Map(prev);
      for (const id of room.light_ids) {
        next.set(id, { ...(next.get(id) ?? { on: false }), on });
      }
      return next;
    });
    await setRoomState(room.id, { on });
  }

  async function applyScene(_room: Room, sceneId: string) {
    setSceneBusy(true);
    try {
      await activateScene(sceneId);
    } finally {
      setSceneBusy(false);
    }
  }

  /** Capture the room's current full state (colours/temps/effects + power) as a
   * new Room Scene. */
  async function saveRoomLook(room: Room, name: string) {
    if (!name.trim()) return;
    setSceneBusy(true);
    try {
      await createScene(name.trim(), room.id);
      setScenes(await getScenes());
    } finally {
      setSceneBusy(false);
    }
  }

  async function deleteScene(sceneId: string) {
    setSceneBusy(true);
    try {
      await removeScene(sceneId);
      setScenes(await getScenes());
    } finally {
      setSceneBusy(false);
    }
  }

  /** Live edits from the shared editor — optimistic locally, debounced to the API.
   * Per-device light edits go through the shared `DeviceControl`/`LightFlyout`;
   * this is the room cascade. Uses the shared `lightControl` rule: send only the
   * dimension that moved. An effect fans out per light (only to members whose
   * catalog has it), carrying just the effect; colour/brightness/temp drive the
   * whole room with one minimal PUT and let the backend gate by capability. */
  function editorChange(target: EditorTarget, change: LightControlChange) {
    if (target.kind !== "room") return;
    const room = allRooms.find((r) => r.id === target.roomId);
    if (!room) return;
    const capsOf = (id: string) => lights.find((l) => l.id === id)?.capabilities;
    // For an effect, only members that support it take part.
    const effectIds =
      change.field === "effect"
        ? room.light_ids.filter((id) => {
            const c = capsOf(id);
            return c ? lightSupports(change, c) : false;
          })
        : room.light_ids;

    setStatesById((prev) => {
      const next = new Map(prev);
      for (const id of effectIds) next.set(id, lightOptimistic(prev.get(id), change));
      return next;
    });
    clearTimeout(editTimer.current);
    editTimer.current = setTimeout(() => {
      if (change.field === "effect") {
        for (const id of effectIds) setLightState(id, lightWrite(change));
      } else {
        setRoomState(room.id, lightWrite(change));
      }
    }, 250);
  }

  async function paintLight(lightId: string) {
    const r = parseInt(paintColor.slice(1, 3), 16);
    const g = parseInt(paintColor.slice(3, 5), 16);
    const b = parseInt(paintColor.slice(5, 7), 16);
    // Paint deliberately sets colour + brightness together (a two-dimension
    // action); send exactly that, no stale effect/temp riding along.
    const next: LightState = { on: true, color: rgbToXy(r, g, b), brightness: paintBrightness };
    setStatesById((prev) => new Map(prev).set(lightId, { ...(prev.get(lightId) ?? {}), ...next }));
    await setLightState(lightId, next);
  }

  /** "[Office] Right Lamp" — prefix a light's name with its room. */
  function lightLabel(l: Light): string {
    const r = allRooms.find((r) => r.light_ids.includes(l.id));
    return r ? `[${r.name}] ${l.name}` : l.name;
  }

  const placedIds = new Set(placements.map((p) => p.light_id));
  const placedMediaIds = new Set(mediaPlacements.map((p) => p.media_device_id));

  const toolButton = (id: Tool) => {
    const t = TOOLS.find((x) => x.id === id)!;
    const active = tool === id;
    return (
      <button
        key={id}
        onClick={() => { setTool(id); setPopover(null); setEditor(null); }}
        title={`${t.label} — ${t.hint}`}
        aria-label={t.label}
        style={{
          width: 42,
          height: 42,
          display: "grid",
          placeItems: "center",
          borderRadius: radius.md,
          cursor: "pointer",
          color: active ? ACCENT : color.dim,
          background: active
            ? `radial-gradient(130% 130% at 50% 0%, ${alpha(ACCENT, 0.16)}, transparent 62%), ${color.surface}`
            : "transparent",
          border: `1px solid ${active ? alpha(ACCENT, 0.5) : "transparent"}`,
          boxShadow: active ? glow(ACCENT, 14) : "none",
          transition: "color .15s, background .15s, border-color .15s, box-shadow .15s",
        }}
      >
        <Glyph name={TOOL_GLYPH[id]} size={20} />
      </button>
    );
  };

  return (
    <div style={{ padding: "1.5rem 2rem" }}>
      {/* Plan tabs */}
      <div style={{ display: "flex", alignItems: "center", gap: "0.1rem", marginBottom: "0.9rem", borderBottom: "1px solid #221c24", flexWrap: "wrap" }}>
        {plans.map((p) => {
          const active = p.id === planId;
          return (
            <button
              key={p.id}
              onClick={() => setPlanId(p.id)}
              style={{
                background: active ? "#17151b" : "transparent",
                border: "none",
                borderTopLeftRadius: 6,
                borderTopRightRadius: 6,
                borderBottom: `2px solid ${active ? ACCENT : "transparent"}`,
                color: active ? ACCENT : "var(--bf-dim)",
                cursor: "pointer",
                padding: "0.45rem 0.9rem",
                fontSize: "0.85rem",
                fontWeight: active ? 600 : 400,
              }}
            >
              {p.name}
            </button>
          );
        })}
        <button
          onClick={() => setShowNewPlan(true)}
          title="New floor plan"
          style={{ background: "transparent", border: "none", color: "var(--bf-faint)", cursor: "pointer", padding: "0.45rem 0.8rem", fontSize: "0.85rem" }}
        >
          + New
        </button>
        {plan && (
          <>
            <span style={{ flex: 1 }} />
            {dirty && (
              <span style={{ color: "#a86", fontSize: "0.72rem", maxWidth: 280, textAlign: "right" }}>
                Saving adds devices placed inside a room to that room (never removes).
              </span>
            )}
            <Button variant={dirty ? "primary" : "ghost"} onClick={handleSave} disabled={!dirty} style={{ marginLeft: "0.5rem" }}>
              {dirty ? "Save changes" : "Saved"}
            </Button>
            <Button variant="danger" onClick={handleDelete} style={{ marginLeft: "0.4rem" }}>Delete</Button>
          </>
        )}
      </div>

      {toast && (
        <div style={{ background: "#1e3a1e", border: "1px solid #2a5a2a", borderRadius: 8, padding: "0.5rem 1rem", marginBottom: "0.75rem", color: "#8f8", fontSize: "0.875rem" }}>
          {toast}
        </div>
      )}

      {!plan ? (
        <p style={{ color: "var(--bf-faint)" }}>
          No floor plans yet. Create one, paint your layout, then place your lights on it.
        </p>
      ) : (
        <>
          {/* Contextual options bar — the active tool + its settings + a hint,
              the way a drafting tool surfaces tool options above the canvas. */}
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: "0.8rem",
              padding: "0.4rem 0.75rem",
              marginBottom: "0.7rem",
              ...glassCard,
              borderRadius: radius.md,
            }}
          >
            <span style={{ display: "flex", alignItems: "center", gap: "0.45rem", color: ACCENT, fontFamily: font.display, textTransform: "uppercase", letterSpacing: "0.12em", fontSize: "0.78rem", fontWeight: 600 }}>
              <Glyph name={TOOL_GLYPH[tool]} size={16} />
              {TOOLS.find((t) => t.id === tool)?.label}
            </span>
            {tool === "place" && (
              <div style={{ display: "inline-flex", borderRadius: radius.sm, border: `1px solid ${color.border}`, overflow: "hidden" }}>
                {(["light", "media"] as PlaceCategory[]).map((cat) => {
                  const on = placeCategory === cat;
                  return (
                    <button
                      key={cat}
                      onClick={() => { setPlaceCategory(cat); setSelected(null); }}
                      style={{
                        padding: "0.3rem 0.75rem",
                        border: "none",
                        cursor: "pointer",
                        fontSize: "0.78rem",
                        background: on ? alpha(ACCENT, 0.14) : "transparent",
                        color: on ? ACCENT : color.dim,
                      }}
                    >
                      {cat === "light" ? "Lights" : "Speakers"}
                    </button>
                  );
                })}
              </div>
            )}
            <span style={{ flex: 1 }} />
            <span style={{ color: color.faint, fontSize: "0.75rem", maxWidth: 540, textAlign: "right", lineHeight: 1.3 }}>
              {TOOLS.find((t) => t.id === tool)?.hint}
            </span>
          </div>

          <div style={{ display: "flex", gap: "1rem", alignItems: "flex-start" }}>
            {/* Left tool palette — icon-driven, grouped like a drafting tool. */}
            <div
              style={{
                flexShrink: 0,
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                gap: "0.2rem",
                padding: "0.35rem",
                alignSelf: "flex-start",
                ...glassCard,
                borderRadius: radius.md,
              }}
            >
              {toolButton("view")}
              <ToolDivider />
              {toolButton("floor")}
              {toolButton("wall")}
              {toolButton("erase")}
              {toolButton("room")}
              <ToolDivider />
              {toolButton("place")}
              {toolButton("paint")}
            </div>

            <PlanCanvas
              plan={plan}
              tiles={tiles}
              walls={walls}
              placements={placements}
              mediaPlacements={mediaPlacements}
              mediaById={mediaById}
              rooms={rooms}
              selectedRoom={selectedRoom}
              statesById={statesById}
              tool={tool}
              selected={selected}
              onMutate={(fn) => { fn(); setDirty(true); setPopover(null); }}
              setTiles={setTiles}
              setWalls={setWalls}
              setPlacements={setPlacements}
              setMediaPlacements={setMediaPlacements}
              setRooms={setRooms}
              onLightClick={(pls, px, py) => {
                if (pls.length === 1) {
                  setEditor({ kind: "light", id: pls[0].light_id, anchor: { x: px, y: py } });
                } else {
                  setPopover({ px, py, placements: pls });
                }
              }}
              onMediaClick={(deviceId, px, py) =>
                setEditor({ kind: "media", id: deviceId, anchor: { x: px, y: py } })
              }
              onResize={handleResize}
              onPaint={paintLight}
            />

            {tool === "view" && plan.rooms.length > 0 && (
              <RoomController
                plan={plan}
                rooms={allRooms}
                statesById={statesById}
                onSetRoom={setRoom}
                onEditRoom={(room, anchor) => setEditor({ kind: "room", roomId: room.id, anchor })}
              />
            )}

            {tool === "place" && placeCategory === "light" && (
              <InspectorPanel title="Lights">
                <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem", maxHeight: "calc(100vh - 230px)", overflowY: "auto" }}>
                  {lights.map((l) => {
                    const sel = selected?.kind === "light" && selected.id === l.id;
                    return (
                      <Button variant="ghost"
                        key={l.id}
                        onClick={() => setSelected(sel ? null : { kind: "light", id: l.id })} style={{ textAlign: "left",
                          fontSize: "0.8rem",
                          ...(sel ? { borderColor: ACCENT, color: ACCENT } : {}),
                          ...(placedIds.has(l.id) ? { opacity: 0.55 } : {}) }}
                      >
                        {placedIds.has(l.id) ? "✓ " : ""}{lightLabel(l)}
                      </Button>
                    );
                  })}
                  {lights.length === 0 && (
                    <span style={{ color: "var(--bf-faint)", fontSize: "0.8rem" }}>No lights discovered yet.</span>
                  )}
                </div>
              </InspectorPanel>
            )}

            {tool === "place" && placeCategory === "media" && (
              <InspectorPanel title="Speakers">
                <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem", maxHeight: "calc(100vh - 230px)", overflowY: "auto" }}>
                  {[...mediaById.values()].map((d) => {
                    const sel = selected?.kind === "media" && selected.id === d.id;
                    return (
                      <Button variant="ghost"
                        key={d.id}
                        onClick={() => setSelected(sel ? null : { kind: "media", id: d.id })} style={{ textAlign: "left",
                          fontSize: "0.8rem",
                          ...(sel ? { borderColor: ACCENT, color: ACCENT } : {}),
                          ...(placedMediaIds.has(d.id) ? { opacity: 0.55 } : {}) }}
                      >
                        {placedMediaIds.has(d.id) ? "✓ " : ""}{d.name}
                      </Button>
                    );
                  })}
                  {mediaById.size === 0 && (
                    <span style={{ color: "var(--bf-faint)", fontSize: "0.8rem" }}>No audio devices discovered yet.</span>
                  )}
                </div>
              </InspectorPanel>
            )}

            {tool === "paint" && (
              <InspectorPanel title="Brush">
                {(() => {
                  const [bh, bs] = hexToHs(paintColor);
                  return (
                    <ColorWheel
                      size={180}
                      hue={bh}
                      sat={bs}
                      onPick={(h, s) => setPaintColor(rgbToHex(...hsvToRgb(h, s, 1)))}
                    />
                  );
                })()}
                <label style={{ display: "flex", alignItems: "center", gap: "0.5rem", fontSize: "0.75rem", color: "var(--bf-dim)" }}>
                  <input
                    type="range"
                    min={1}
                    max={100}
                    value={paintBrightness}
                    onChange={(e) => setPaintBrightness(Number(e.target.value))}
                    style={{ flex: 1, accentColor: ACCENT }}
                  />
                  <span style={{ width: 36, textAlign: "right" }}>{paintBrightness}%</span>
                </label>
                <span style={{ color: "var(--bf-faint)", fontSize: "0.75rem" }}>
                  Brush across lights on the plan to apply. Save the result as a
                  room scene with “Save current” in View mode.
                </span>
              </InspectorPanel>
            )}

            {tool === "room" && (
              <RoomEditorPanel
                rooms={rooms}
                allRooms={allRooms}
                selectedRoom={selectedRoom}
                onSelect={setSelectedRoom}
                onCreate={async () => {
                  const name = await dialogs.prompt({
                    title: "New room",
                    placeholder: "Room name",
                    confirmLabel: "Create",
                  });
                  if (!name?.trim()) return;
                  // Local placeholder id so selection works before save.
                  const room: EditRoom = { id: `new-${Date.now()}`, name: name.trim(), tiles: new Set() };
                  setRooms((prev) => [...prev, room]);
                  setSelectedRoom(room.id);
                  setDirty(true);
                }}
                onAddExisting={(name) => {
                  // Add an existing Bifrost Room as a region to paint; the backend
                  // binds it to the same-named Room on save (no duplicate).
                  const room: EditRoom = { id: `new-${Date.now()}`, name, tiles: new Set() };
                  setRooms((prev) => [...prev, room]);
                  setSelectedRoom(room.id);
                  setDirty(true);
                }}
                onRename={async (id) => {
                  const room = rooms.find((r) => r.id === id);
                  const name = await dialogs.prompt({
                    title: "Rename room",
                    initial: room?.name ?? "",
                    placeholder: "Room name",
                    confirmLabel: "Rename",
                  });
                  if (!name?.trim()) return;
                  setRooms((prev) => prev.map((r) => (r.id === id ? { ...r, name: name.trim() } : r)));
                  setDirty(true);
                }}
                onDelete={async (id) => {
                  const ok = await dialogs.confirm({
                    title: "Remove room",
                    message: "Remove this room? Its auto-group is deleted on save.",
                    confirmLabel: "Remove",
                    danger: true,
                  });
                  if (!ok) return;
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
                background: "#19171d",
                border: "1px solid #2a2030",
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
                  <Button variant="ghost"
                    key={p.light_id}
                    onClick={() => {
                      setEditor({ kind: "light", id: p.light_id, anchor: { x: popover.px, y: popover.py } });
                      setPopover(null);
                    }} style={{ fontSize: "0.8rem", textAlign: "left", color: on ? ACCENT : "var(--bf-dim)" }}
                  >
                    {on ? "● " : "○ "}{light ? lightLabel(light) : p.light_id}
                  </Button>
                );
              })}
              <Button variant="ghost" onClick={() => setPopover(null)} style={{ fontSize: "0.75rem", color: "var(--bf-faint)" }}>
                Close
              </Button>
            </div>
          )}
        </>
      )}

      {editor && (() => {
        if (editor.kind === "light") {
          const light = lights.find((l) => l.id === editor.id);
          if (!light) return null;
          // Feed the editor the live state (more current than the stored
          // last_state); optimistic edits land back in `statesById`.
          const merged = { ...light, last_state: statesById.get(editor.id) ?? light.last_state };
          return (
            <DeviceControl
              domain="light"
              light={merged}
              anchor={editor.anchor}
              onLocalPatch={(id, next) => setStatesById((m) => new Map(m).set(id, next))}
              onClose={() => setEditor(null)}
            />
          );
        }
        if (editor.kind === "media") {
          const device = mediaById.get(editor.id);
          if (!device) return null;
          return (
            <DeviceControl
              domain="media"
              device={device}
              anchor={editor.anchor}
              onLocalPatch={patchAudio}
              onClose={() => setEditor(null)}
              receiverName={
                device.receiver_id ? mediaById.get(device.receiver_id)?.name : undefined
              }
            />
          );
        }
        const room = allRooms.find((r) => r.id === editor.roomId);
        if (!room) return null;
        const anyOn = room.light_ids.some((id) => statesById.get(id)?.on);
        // Feed each member's *live* plan state (`statesById`, not `last_state`)
        // into the one shared aggregator, so hex / brightness / mirek / effect
        // intersection / running-effect are computed the same way as everywhere.
        const members: LightLike[] = room.light_ids.flatMap((id) => {
          const l = lights.find((x) => x.id === id);
          if (!l) return [];
          const st = statesById.get(id);
          return [st ? { capabilities: l.capabilities, last_state: st } : { capabilities: l.capabilities }];
        });
        const agg = aggregateLightState(members);
        return (
          <LightEditor
            anchor={editor.anchor}
            title={room.name}
            initialHex={agg.hex}
            initialBrightness={agg.brightness}
            initialMirek={agg.mirek}
            showWhite={agg.showWhite}
            effects={agg.effects.length > 0 ? agg.effects : undefined}
            initialEffect={agg.commonEffect}
            on={anyOn}
            onToggle={() => setRoom(room, !anyOn)}
            onChange={(ch) => editorChange(editor, ch)}
            onClose={() => setEditor(null)}
          >
            <SceneButton
              onClick={() => {
                setEditor(null);
                setScenesRoom(room);
              }}
            />
          </LightEditor>
        );
      })()}

      {scenesRoom && (
        <SceneModal
          roomName={scenesRoom.name}
          scenes={scenes.filter((s) => s.room_id === scenesRoom.id)}
          busy={sceneBusy}
          onApply={async (id) => {
            await applyScene(scenesRoom, id);
            setScenesRoom(null);
          }}
          onSave={(name) => saveRoomLook(scenesRoom, name)}
          onDelete={deleteScene}
          onClose={() => setScenesRoom(null)}
        />
      )}

      {showNewPlan && <NewPlanDialog onCreate={handleCreate} onClose={() => setShowNewPlan(false)} />}
      {dialogs.element}
    </div>
  );
}

/** Name + dimensions form for a new floor plan. */
function NewPlanDialog({
  onCreate,
  onClose,
}: {
  onCreate: (name: string, width: number, height: number) => Promise<void>;
  onClose: () => void;
}) {
  const [name, setName] = useState("");
  const [width, setWidth] = useState(50);
  const [height, setHeight] = useState(40);
  const [busy, setBusy] = useState(false);
  const valid =
    name.trim().length > 0 && width >= 1 && width <= 128 && height >= 1 && height <= 128;

  async function submit() {
    if (!valid || busy) return;
    setBusy(true);
    try {
      await onCreate(name.trim(), width, height);
    } finally {
      setBusy(false);
    }
  }

  const dimInput = (value: number, set: (v: number) => void) => (
    <input
      type="number"
      min={1}
      max={128}
      value={value}
      onChange={(e) => set(Number(e.target.value))}
      style={{ ...S.input, width: 80 }}
    />
  );

  return (
    <Modal title="New floor plan" onClose={onClose}>
      <input
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => { if (e.key === "Enter") submit(); }}
        placeholder="Plan name (e.g. Ground Floor)"
        autoFocus
        style={S.input}
      />
      <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", fontSize: "0.875rem", color: "var(--bf-dim)" }}>
        {dimInput(width, setWidth)}
        ×
        {dimInput(height, setHeight)}
        <span style={{ color: "var(--bf-faint)", fontSize: "0.78rem" }}>feet — one tile per foot, max 128</span>
      </div>
      <div style={{ display: "flex", gap: "0.5rem", justifyContent: "flex-end", marginTop: "0.25rem" }}>
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
        <Button onClick={submit} disabled={!valid || busy}>
          {busy ? "Creating…" : "Create"}
        </Button>
      </div>
    </Modal>
  );
}

/** A hairline separator between tool groups in the icon palette. */
function ToolDivider() {
  return <div style={{ width: "70%", height: 1, background: color.hairline, margin: "0.25rem 0" }} />;
}

/** A docked right-side inspector panel — glass surface + engraved header, the
 * counterpart to the left tool palette. */
function InspectorPanel({ title, width = 240, children }: { title: string; width?: number; children: React.ReactNode }) {
  return (
    <div
      style={{
        width,
        flexShrink: 0,
        alignSelf: "flex-start",
        display: "flex",
        flexDirection: "column",
        gap: "0.5rem",
        padding: "0.7rem 0.75rem",
        ...glassCard,
        borderRadius: radius.md,
      }}
    >
      <h3 style={{ ...labelType, margin: 0, fontSize: "0.76rem", color: color.dim }}>{title}</h3>
      {children}
    </div>
  );
}

// ── Room controller (right of canvas, view mode) ────────────────────────────

function RoomController({
  plan,
  rooms,
  statesById,
  onSetRoom,
  onEditRoom,
}: {
  plan: PlanDetail;
  rooms: Room[];
  statesById: Map<string, LightState>;
  onSetRoom: (room: Room, on: boolean) => Promise<void>;
  onEditRoom: (room: Room, anchor: HTMLElement) => void;
}) {
  const [busy, setBusy] = useState("");
  const [mediaDevices, setMediaDevices] = useState<MediaDevice[]>([]);

  useEffect(() => {
    getMediaDevices().then(setMediaDevices);
  }, []);

  // Plan regions bound to a Room, joined with the live Room data.
  const bound = plan.rooms
    .map((region, i) => ({
      region,
      room: rooms.find((r) => r.id === region.room_id),
      color: ROOM_COLORS[i % ROOM_COLORS.length],
    }))
    // Disabled rooms are hidden from the planner's room controller.
    .filter((b): b is { region: typeof b.region; room: Room; color: string } => !!b.room && b.room.enabled);

  if (bound.length === 0) return null;

  return (
    <div style={{ width: 250, flexShrink: 0, display: "flex", flexDirection: "column", gap: "0.5rem" }}>
      <h3 style={{ margin: 0, fontSize: "0.9rem", color: "var(--bf-dim)" }}>Rooms</h3>
      <span style={{ color: "var(--bf-faint)", fontSize: "0.72rem", marginTop: "-0.2rem" }}>
        Click a room to set its color, brightness &amp; scenes.
      </span>

      {bound.map(({ room, color }) => {
        const count = room.light_ids.length;
        const anyOn = room.light_ids.some((id) => statesById.get(id)?.on);
        const litColor = room.light_ids
          .map((id) => statesById.get(id))
          .find((st) => st?.on && st.color)?.color;
        const dotHex = litColor
          ? rgbToHex(...xyToRgb(litColor.x, litColor.y, litColor.brightness))
          : color;
        const tunable = count > 0;
        return (
          <div
            key={room.id}
            style={{ ...S.card, padding: 0, overflow: "hidden", borderLeft: `3px solid ${color}` }}
          >
            {/* The whole header opens the room's color/brightness/scene editor. */}
            <div
              onClick={(e) => { if (tunable) onEditRoom(room, e.currentTarget); }}
              title={tunable ? "Set this room's color, brightness, and scenes" : undefined}
              style={{
                display: "flex",
                alignItems: "center",
                gap: "0.55rem",
                padding: "0.6rem 0.7rem",
                cursor: tunable ? "pointer" : "default",
              }}
            >
              <span
                aria-hidden
                style={{
                  width: 16,
                  height: 16,
                  flexShrink: 0,
                  borderRadius: "50%",
                  border: "1px solid rgba(255,255,255,0.22)",
                  background: anyOn
                    ? `radial-gradient(circle at 35% 30%, #ffffff44, transparent 45%), ${dotHex}`
                    : "#3a372e",
                  boxShadow: anyOn ? `0 0 10px -3px ${dotHex}` : "none",
                }}
              />
              <span style={{ fontWeight: 600, fontSize: "0.88rem", flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                {room.name}
              </span>
              <span style={{ color: "var(--bf-faint)", fontSize: "0.72rem", whiteSpace: "nowrap" }}>
                {count} light{count !== 1 ? "s" : ""}
              </span>
              <Switch
                on={anyOn}
                disabled={busy === room.id || count === 0}
                onToggle={async () => {
                  setBusy(room.id);
                  try { await onSetRoom(room, !anyOn); } finally { setBusy(""); }
                }}
              />
            </div>

            {room.media_devices.length > 0 && (
              <div style={{ borderTop: "1px solid #26241f", padding: "0.4rem 0.7rem 0.5rem" }}>
                <RoomVolumeStrip room={room} devices={mediaDevices} />
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

/** Compact on/off switch — a thin wrapper over the shared control. */
function Switch({ on, onToggle, disabled }: { on: boolean; onToggle: () => void; disabled?: boolean }) {
  return <SharedSwitch on={on} onChange={() => onToggle()} disabled={disabled} />;
}

// ── Room editor panel (right of canvas, room tool) ──────────────────────────

function RoomEditorPanel({
  rooms,
  allRooms,
  selectedRoom,
  onSelect,
  onCreate,
  onAddExisting,
  onRename,
  onDelete,
}: {
  rooms: EditRoom[];
  allRooms: Room[];
  selectedRoom: string;
  onSelect: (id: string) => void;
  onCreate: () => void;
  onAddExisting: (name: string) => void;
  onRename: (id: string) => void;
  onDelete: (id: string) => void;
}) {
  // Existing Bifrost Rooms not yet painted as a region here (name-matched, as
  // the backend binds a region to a same-named Room). Offer them as one-tap adds
  // so the user picks "Bedroom" rather than re-typing it exactly.
  const painted = new Set(rooms.map((r) => r.name.toLowerCase()));
  const unpainted = allRooms.filter((r) => r.enabled && !painted.has(r.name.toLowerCase()));

  return (
    <div style={{ width: 240, flexShrink: 0, alignSelf: "flex-start", display: "flex", flexDirection: "column", gap: "0.4rem", padding: "0.7rem 0.75rem", ...glassCard, borderRadius: radius.md }}>
      <h3 style={{ ...labelType, margin: 0, fontSize: "0.76rem", color: color.dim }}>Rooms</h3>
      {rooms.map((r, i) => (
        <div key={r.id} style={{ display: "flex", gap: "0.3rem", alignItems: "center" }}>
          <Button variant="ghost"
            onClick={() => onSelect(r.id === selectedRoom ? "" : r.id)} style={{ flex: 1,
              textAlign: "left",
              fontSize: "0.8rem",
              borderLeft: `3px solid ${ROOM_COLORS[i % ROOM_COLORS.length]}`,
              ...(r.id === selectedRoom ? { borderColor: ACCENT, color: ACCENT } : {}) }}
          >
            {r.name}
            <span style={{ color: "var(--bf-faint)", marginLeft: "0.4rem" }}>{r.tiles.size}</span>
          </Button>
          <Button variant="ghost" onClick={() => onRename(r.id)} title="Rename" style={{ padding: "0.3rem 0.5rem" }}>✎</Button>
          <Button variant="ghost" onClick={() => onDelete(r.id)} title="Delete" style={{ padding: "0.3rem 0.5rem", color: "#866" }}>×</Button>
        </div>
      ))}

      {unpainted.length > 0 && (
        <>
          <span style={{ color: "var(--bf-faint)", fontSize: "0.72rem", marginTop: "0.2rem" }}>
            Your rooms — tap to add &amp; paint:
          </span>
          <div style={{ display: "flex", flexWrap: "wrap", gap: "0.3rem" }}>
            {unpainted.map((r) => (
              <Button
                key={r.id}
                variant="ghost"
                onClick={() => onAddExisting(r.name)}
                style={{ fontSize: "0.78rem", padding: "0.25rem 0.55rem" }}
              >
                + {r.name}
              </Button>
            ))}
          </div>
        </>
      )}

      <Button variant="ghost" onClick={onCreate}>+ New room</Button>
      {rooms.length > 0 && !selectedRoom && (
        <span style={{ color: "var(--bf-faint)", fontSize: "0.75rem" }}>Select a room, then paint its tiles.</span>
      )}
    </div>
  );
}

// ── Canvas ───────────────────────────────────────────────────────────────────

/** Distance from point (px, py) to the segment (x1, y1)–(x2, y2), in tile units. */
function distToSegment(px: number, py: number, x1: number, y1: number, x2: number, y2: number): number {
  const dx = x2 - x1, dy = y2 - y1;
  const lenSq = dx * dx + dy * dy;
  const t = lenSq === 0 ? 0 : Math.max(0, Math.min(1, ((px - x1) * dx + (py - y1) * dy) / lenSq));
  return Math.hypot(px - (x1 + t * dx), py - (y1 + t * dy));
}

// ── Device glyphs (simple vector icons drawn on the canvas) ──────────────────

const AUDIO_HEX = "#a78bfa";

/** Resolve a theme CSS variable to a concrete colour for canvas drawing. The 2D
 * context can't read `var(--bf-*)`, so we look the value up off <html> at draw
 * time (falling back to a literal if the variable isn't set). This is what lets
 * the canvas re-theme along with the rest of the app. */
function cssHex(name: string, fallback: string): string {
  const val = getComputedStyle(document.documentElement).getPropertyValue(`--bf-${name}`).trim();
  return val || fallback;
}

function roundRectPath(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  ctx.beginPath();
  if (typeof ctx.roundRect === "function") ctx.roundRect(x, y, w, h, r);
  else ctx.rect(x, y, w, h);
}

/** Light bulb: a screw base under the (colored) glass. The glass circle itself
 * is drawn by the caller so it can carry the live light color + glow; this adds
 * the base that makes it read as a bulb. */
function drawBulbBase(ctx: CanvasRenderingContext2D, cx: number, cy: number, r: number) {
  const bw = r * 0.95;
  const bh = r * 0.7;
  const bx = cx - bw / 2;
  const by = cy + r * 0.72;
  ctx.fillStyle = "#23202a";
  ctx.strokeStyle = "rgba(0,0,0,0.55)";
  ctx.lineWidth = 1;
  roundRectPath(ctx, bx, by, bw, bh, Math.max(1, r * 0.18));
  ctx.fill();
  ctx.stroke();
  // a couple of thread lines
  ctx.strokeStyle = "rgba(255,255,255,0.18)";
  ctx.beginPath();
  ctx.moveTo(bx, by + bh * 0.4);
  ctx.lineTo(bx + bw, by + bh * 0.4);
  ctx.moveTo(bx, by + bh * 0.72);
  ctx.lineTo(bx + bw, by + bh * 0.72);
  ctx.stroke();
}

/** Speaker: a tall cabinet with a tweeter + woofer. */
function drawSpeakerIcon(ctx: CanvasRenderingContext2D, cx: number, cy: number, s: number) {
  ctx.strokeStyle = cssHex("violet", AUDIO_HEX);
  ctx.fillStyle = "rgba(167,139,250,0.16)";
  ctx.lineWidth = Math.max(1, s * 0.07);
  const w = s * 0.66, h = s;
  const x = cx - w / 2, y = cy - h / 2;
  roundRectPath(ctx, x, y, w, h, s * 0.12);
  ctx.fill();
  ctx.stroke();
  ctx.beginPath();
  ctx.arc(cx, y + h * 0.28, Math.max(1, s * 0.08), 0, Math.PI * 2);
  ctx.stroke();
  ctx.beginPath();
  ctx.arc(cx, y + h * 0.64, Math.max(1.5, s * 0.17), 0, Math.PI * 2);
  ctx.stroke();
}

/** Receiver: a wide flat chassis with a display slot and two knobs. */
function drawReceiverIcon(ctx: CanvasRenderingContext2D, cx: number, cy: number, s: number) {
  ctx.strokeStyle = cssHex("violet", AUDIO_HEX);
  ctx.fillStyle = "rgba(167,139,250,0.16)";
  ctx.lineWidth = Math.max(1, s * 0.07);
  const w = s * 1.15, h = s * 0.62;
  const x = cx - w / 2, y = cy - h / 2;
  roundRectPath(ctx, x, y, w, h, s * 0.1);
  ctx.fill();
  ctx.stroke();
  // display on the left
  ctx.strokeRect(x + w * 0.1, cy - h * 0.18, w * 0.34, h * 0.36);
  // two knobs on the right
  ctx.beginPath();
  ctx.arc(x + w * 0.72, cy, Math.max(1, s * 0.08), 0, Math.PI * 2);
  ctx.stroke();
  ctx.beginPath();
  ctx.arc(x + w * 0.9, cy, Math.max(1, s * 0.08), 0, Math.PI * 2);
  ctx.stroke();
}

function PlanCanvas({
  plan,
  tiles,
  walls,
  placements,
  mediaPlacements,
  mediaById,
  rooms,
  selectedRoom,
  statesById,
  tool,
  selected,
  onMutate,
  setTiles,
  setWalls,
  setPlacements,
  setMediaPlacements,
  setRooms,
  onLightClick,
  onMediaClick,
  onResize,
  onPaint,
}: {
  plan: PlanDetail;
  tiles: Set<string>;
  walls: Set<string>;
  placements: Placement[];
  mediaPlacements: MediaPlacement[];
  mediaById: Map<string, MediaDevice>;
  rooms: EditRoom[];
  selectedRoom: string;
  statesById: Map<string, LightState>;
  tool: Tool;
  selected: { kind: PlaceCategory; id: string } | null;
  onMutate: (fn: () => void) => void;
  setTiles: React.Dispatch<React.SetStateAction<Set<string>>>;
  setWalls: React.Dispatch<React.SetStateAction<Set<string>>>;
  setPlacements: React.Dispatch<React.SetStateAction<Placement[]>>;
  setMediaPlacements: React.Dispatch<React.SetStateAction<MediaPlacement[]>>;
  setRooms: React.Dispatch<React.SetStateAction<EditRoom[]>>;
  onLightClick: (placements: Placement[], px: number, py: number) => void;
  onMediaClick: (deviceId: string, px: number, py: number) => void;
  onResize: (width: number, height: number) => void;
  onPaint: (lightId: string) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [view, setView] = useState({ cell: 0, ox: 0, oy: 0 }); // cell=0 → fit on first draw
  const drag = useRef<{
    px: number;
    py: number;
    moved: boolean;
    panning: boolean;
    resizing?: boolean;
    /** Light id being placed — dragging extends it into a strip. */
    placing?: string;
    /** Strip vertices laid so far (including the start tile); collinear
     * moves extend the last segment, direction changes add a corner. */
    path?: [number, number][];
    /** Lights already painted during this brush stroke. */
    painted?: Set<string>;
  } | null>(null);
  // Live grid size while the corner handle is being dragged.
  const [pendingSize, setPendingSize] = useState<{ w: number; h: number } | null>(null);

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
    ctx.fillStyle = "#0b0a0e";
    ctx.fillRect(0, 0, cssW, cssH);

    // Floor tiles
    ctx.fillStyle = "#15131a";
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

    // Grid (drawn at the pending size while the resize handle is dragged)
    const gridW = pendingSize?.w ?? plan.width;
    const gridH = pendingSize?.h ?? plan.height;
    ctx.strokeStyle = "rgba(255,255,255,0.05)";
    ctx.lineWidth = 1;
    for (let x = 0; x <= gridW; x++) {
      ctx.beginPath();
      ctx.moveTo(ox + x * cell, oy);
      ctx.lineTo(ox + x * cell, oy + gridH * cell);
      ctx.stroke();
    }
    for (let y = 0; y <= gridH; y++) {
      ctx.beginPath();
      ctx.moveTo(ox, oy + y * cell);
      ctx.lineTo(ox + gridW * cell, oy + y * cell);
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
        ctx.font = `600 ${Math.max(10, cell * 0.45)}px "Inter Variable", system-ui`;
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillStyle = "rgba(255,255,255,0.55)";
        ctx.fillText(room.name, cx, cy);
      });
    }

    // LED strips: a wall-hugging polyline (see stripPath) through the run.
    for (const p of placements) {
      if (!p.points || p.points.length === 0) continue;
      const pts = stripPath(p);
      const run = new Path2D();
      run.moveTo(ox + pts[0][0] * cell, oy + pts[0][1] * cell);
      for (let i = 1; i < pts.length; i++) {
        run.lineTo(ox + pts[i][0] * cell, oy + pts[i][1] * cell);
      }
      const st = statesById.get(p.light_id);
      let color = "#2a2433";
      if (st?.on) {
        if (st.color) {
          const [rr, gg, bb] = xyToRgb(st.color.x, st.color.y, Math.max(st.color.brightness, 0.25));
          color = `rgb(${rr},${gg},${bb})`;
        } else {
          color = "#ffd9a0";
        }
        ctx.strokeStyle = color;
        ctx.lineCap = "round";
        ctx.lineJoin = "round";
        ctx.globalAlpha = 0.3 * Math.min(1, ((st.brightness ?? 100) / 100) + 0.3);
        ctx.lineWidth = Math.max(5, cell * 0.34);
        ctx.stroke(run);
        ctx.globalAlpha = 1;
      }
      ctx.strokeStyle = color;
      ctx.lineCap = "round";
      ctx.lineJoin = "round";
      ctx.lineWidth = Math.max(1.5, cell * 0.07);
      ctx.stroke(run);
    }

    // Point lights, clustered by (x, y, mount)
    const clusters = new Map<string, Placement[]>();
    for (const p of placements) {
      if (p.points && p.points.length > 0) continue;
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
      let fill = "#2a2433";
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
      // Bulb: screw base under the colored glass (skipped when too small).
      if (cell >= 14) drawBulbBase(ctx, cx, cy, r);
      ctx.fillStyle = fill;
      ctx.beginPath();
      ctx.arc(cx, cy, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.strokeStyle = "rgba(0,0,0,0.6)";
      ctx.lineWidth = 1;
      ctx.stroke();

      if (group.length > 1) {
        ctx.fillStyle = "#fff";
        ctx.font = `${Math.max(9, cell * 0.3)}px "Inter Variable", system-ui`;
        ctx.textAlign = "left";
        ctx.textBaseline = "middle";
        ctx.fillText(`×${group.length}`, cx + r + 2, cy);
      }
    }

    // Audio devices — a speaker or receiver glyph, by the device's kind.
    for (const p of mediaPlacements) {
      const [mx, my] = mountOffset(p.mount);
      const cx = ox + (p.x + mx) * cell;
      const cy = oy + (p.y + my) * cell;
      const sz = Math.max(11, cell * 0.5);
      ctx.lineJoin = "round";
      ctx.lineCap = "round";
      if (mediaById.get(p.media_device_id)?.kind === "receiver") {
        drawReceiverIcon(ctx, cx, cy, sz);
      } else {
        drawSpeakerIcon(ctx, cx, cy, sz);
      }
    }

    // Resize handle on the bottom-right grid corner; outline + size readout
    // while dragging.
    const hx = ox + gridW * cell;
    const hy = oy + gridH * cell;
    if (pendingSize) {
      ctx.strokeStyle = cssHex("cyan", "#38bdf8");
      ctx.lineWidth = 1.5;
      ctx.setLineDash([5, 4]);
      ctx.strokeRect(ox, oy, gridW * cell, gridH * cell);
      ctx.setLineDash([]);
      ctx.font = "600 12px 'Inter Variable', system-ui";
      ctx.textAlign = "right";
      ctx.textBaseline = "bottom";
      ctx.fillStyle = cssHex("cyan", "#38bdf8");
      ctx.fillText(`${gridW} × ${gridH}`, hx - 12, hy - 6);
    }
    ctx.fillStyle = pendingSize ? cssHex("cyan", "#38bdf8") : cssHex("faint", "#6f6772");
    ctx.strokeStyle = "#0b0a0e";
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.arc(hx, hy, 7, 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = "#0b0a0e";
    ctx.font = "700 9px 'Inter Variable', system-ui";
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText("⤡", hx, hy + 0.5);
  }, [view, plan, tiles, walls, placements, mediaPlacements, mediaById, rooms, selectedRoom, tool, statesById, pendingSize]);

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
      // Hit an existing placement (light or speaker)? Remove it.
      const hit = hitPlacement(gx, gy);
      if (hit) {
        onMutate(() => setPlacements((prev) => prev.filter((p) => p.light_id !== hit.light_id)));
        return;
      }
      const mediaHit = hitAudio(gx, gy);
      if (mediaHit) {
        onMutate(() =>
          setMediaPlacements((prev) =>
            prev.filter((p) => p.media_device_id !== mediaHit.media_device_id),
          ),
        );
        return;
      }
      if (!selected) return;
      const fx = gx - tx, fy = gy - ty;
      const m = Math.min(fx, 1 - fx, fy, 1 - fy);
      let mount: Mount = "c";
      if (m < 0.28) {
        mount = fy === m ? "n" : 1 - fy === m ? "s" : fx === m ? "w" : "e";
      }
      if (selected.kind === "media") {
        // Speakers are point placements — no strip drag.
        const deviceId = selected.id;
        onMutate(() =>
          setMediaPlacements((prev) => [
            ...prev.filter((p) => p.media_device_id !== deviceId),
            { media_device_id: deviceId, x: tx, y: ty, mount },
          ]),
        );
        return;
      }
      const lightId = selected.id;
      onMutate(() =>
        setPlacements((prev) => [
          ...prev.filter((p) => p.light_id !== lightId),
          { light_id: lightId, x: tx, y: ty, mount },
        ]),
      );
      if (drag.current) {
        drag.current.placing = lightId;
        drag.current.path = [[tx, ty]];
      }
    } else if (tool === "paint") {
      const hit = hitPlacement(gx, gy);
      // Paint each light once per brush stroke, not per pointer event.
      if (hit && drag.current && !drag.current.painted?.has(hit.light_id)) {
        (drag.current.painted ??= new Set()).add(hit.light_id);
        onPaint(hit.light_id);
      }
    }
  }

  function hitPlacement(gx: number, gy: number): Placement | null {
    for (const p of placements) {
      if (p.points && p.points.length > 0) {
        const pts = stripPath(p);
        for (let i = 1; i < pts.length; i++) {
          const [ax, ay] = pts[i - 1];
          const [bx, by] = pts[i];
          if (distToSegment(gx, gy, ax, ay, bx, by) < 0.3) return p;
        }
      } else {
        const [mx, my] = mountOffset(p.mount);
        const dx = gx - (p.x + mx), dy = gy - (p.y + my);
        if (Math.hypot(dx, dy) < 0.3) return p;
      }
    }
    return null;
  }

  function hitAudio(gx: number, gy: number): MediaPlacement | null {
    for (const p of mediaPlacements) {
      const [mx, my] = mountOffset(p.mount);
      if (Math.hypot(gx - (p.x + mx), gy - (p.y + my)) < 0.35) return p;
    }
    return null;
  }

  function handlePointerDown(e: React.PointerEvent) {
    (e.target as HTMLElement).setPointerCapture(e.pointerId);

    // The corner resize handle wins over any tool.
    const rect = canvasRef.current!.getBoundingClientRect();
    const hx = view.ox + plan.width * view.cell;
    const hy = view.oy + plan.height * view.cell;
    if (Math.hypot(e.clientX - rect.left - hx, e.clientY - rect.top - hy) < 12) {
      drag.current = { px: e.clientX, py: e.clientY, moved: false, panning: false, resizing: true };
      setPendingSize({ w: plan.width, h: plan.height });
      return;
    }

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

    if (drag.current.resizing) {
      const { gx, gy } = toGrid(e);
      setPendingSize({
        w: Math.max(1, Math.min(128, Math.round(gx))),
        h: Math.max(1, Math.min(128, Math.round(gy))),
      });
      return;
    }

    // Dragging a just-placed light extends it into a strip. Collinear motion
    // stretches the current segment; changing direction starts a corner;
    // backing onto the previous vertex undoes one.
    if (drag.current.placing && drag.current.path && tool === "place") {
      const { gx, gy } = toGrid(e);
      const ex = Math.max(0, Math.min(plan.width - 1, Math.floor(gx)));
      const ey = Math.max(0, Math.min(plan.height - 1, Math.floor(gy)));
      const path = drag.current.path;
      const last = path[path.length - 1];
      if (ex !== last[0] || ey !== last[1]) {
        const prev = path.length >= 2 ? path[path.length - 2] : null;
        if (prev && ex === prev[0] && ey === prev[1]) {
          path.pop();
        } else if (
          prev &&
          (last[0] - prev[0]) * (ey - last[1]) === (last[1] - prev[1]) * (ex - last[0])
        ) {
          path[path.length - 1] = [ex, ey];
        } else {
          path.push([ex, ey]);
        }
        const lightId = drag.current.placing;
        const points = path.length > 1 ? path.slice(1) : undefined;
        onMutate(() =>
          setPlacements((prev2) =>
            prev2.map((p) => (p.light_id === lightId ? { ...p, points } : p)),
          ),
        );
      }
      return;
    }

    if (drag.current.panning && drag.current.moved) {
      setView((v) => ({ ...v, ox: v.ox + dx, oy: v.oy + dy }));
      drag.current.px = e.clientX;
      drag.current.py = e.clientY;
    } else if (!drag.current.panning && (tool === "floor" || tool === "wall" || tool === "erase" || tool === "room" || tool === "paint")) {
      const { gx, gy } = toGrid(e);
      applyTool(gx, gy, e);
    }
  }

  function handlePointerUp(e: React.PointerEvent) {
    if (drag.current?.resizing) {
      drag.current = null;
      const next = pendingSize;
      setPendingSize(null);
      if (next && (next.w !== plan.width || next.h !== plan.height)) {
        onResize(next.w, next.h);
      }
      return;
    }
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
        return;
      }
      const mediaHit = hitAudio(gx, gy);
      if (mediaHit) {
        onMediaClick(mediaHit.media_device_id, e.clientX, e.clientY);
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
          border: "1px solid #221c24",
          touchAction: "none",
          cursor: tool === "view" ? "grab" : "crosshair",
          display: "block",
        }}
      />
    </div>
  );
}
