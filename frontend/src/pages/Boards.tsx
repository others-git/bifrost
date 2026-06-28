// Boards: user-composable dashboards. A board is a free-form 12-column grid of
// widgets the user drags and resizes (edit mode), saved to the server. Widgets
// reuse the shared controls — a device tile opens the DeviceControl fly-out, a
// "control" widget is the room-control creator's spec (power/brightness/volume/
// scene over any chosen devices, home-wide), plus scene buttons and now-playing.
// View mode renders live, interactive widgets; phones get a stacked read view.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  activateScene,
  createDashboard,
  deleteDashboard,
  getDashboards,
  getGenericDevices,
  getKioskSelf,
  getLights,
  getMediaDevices,
  getPowerDevices,
  getRooms,
  getScenes,
  lightHex,
  mergePatch,
  restoreDefaultHome,
  setLightState,
  setMediaState,
  setPowerState,
  updateDashboard,
  type Dashboard,
  type GenericDevice,
  type Light,
  type LightState,
  type LightStatePatch,
  type MediaDevice,
  type PowerDevice,
  type Room,
  type RoomControl,
  type Scene,
  type Widget,
} from "../api";
import { DeviceControl } from "../components/DeviceControl";
import { LightEditor, type LightControlChange } from "../components/LightEditor";
import {
  aggregateLightState,
  lightOptimistic,
  lightSupports,
  lightWrite,
} from "../components/lightControl";
import { MediaEditor, fanMediaCommand } from "../components/MediaControls";
import { InlineSlider } from "../components/InlineSlider";
import { RoomControlButton, GlyphButton, RestoreHomeButton } from "./Dashboard";
import { CornerFiligree } from "../components/ornament";
import { Button, Segmented } from "../components/controls";
import { Modal, useDialogs } from "../components/dialogs";
import { Select } from "../components/Select";
import { CONTROL_GLYPH_OPTIONS, Glyph, GlyphGrid, weatherGlyph, weatherLabel } from "../components/glyphs";
import { PageHeader } from "../components/PageHeader";
import { useViewport } from "../useViewport";
import { S } from "../styles";
import { alpha, color, glow, labelType, nicheStyle, radius, T } from "../theme";

/** The full-cell "lit niche" plate every Boards widget wears — the same recessed,
 * device-lit surface as the Control room cards' `GlyphButton`s (shared
 * `nicheStyle`), but rectangular and angular (`radius.frame`), never a rounded card. */
const widgetPlate = (accent: string, on: boolean): React.CSSProperties => ({
  position: "relative",
  width: "100%",
  height: "100%",
  borderRadius: radius.frame,
  overflow: "hidden",
  ...nicheStyle(accent, on),
});

/** A group widget's plate: like `widgetPlate`, but a **device box tracks its one
 * light's colour** while a **group box blends ALL its lit members' colours into a
 * gradient** (border, top tint, and glow). Falls back to the single-accent plate
 * when off or given one colour. */
const widgetPlateMulti = (accents: string[], on: boolean): React.CSSProperties => {
  if (!on || accents.length <= 1) {
    return widgetPlate(accents[0] ?? T.accent, on);
  }
  const border = `linear-gradient(120deg, ${accents.map((c) => alpha(c, 0.55)).join(", ")})`;
  const tint = `linear-gradient(120deg, ${accents.map((c) => alpha(c, 0.17)).join(", ")})`;
  return {
    position: "relative",
    width: "100%",
    height: "100%",
    borderRadius: radius.frame,
    overflow: "hidden",
    color: T.text,
    // Gradient border via the transparent-border + dual background-clip trick:
    // the tint+surface fill the interior (padding-box), the colour sweep shows
    // through the 1px transparent border (border-box).
    border: "1px solid transparent",
    background: `${tint} padding-box, ${color.surface} padding-box, ${border} border-box`,
    boxShadow: `${glow(accents[0], 22)}, ${glow(accents[accents.length - 1], 22)}, inset 0 0 16px -9px ${accents[0]}`,
  };
};

// A fixed-ratio grid: 24 columns × 14 rows, both axes scaled to the board's
// actual size. A layout is therefore device-independent — the same board fills a
// phone, a desktop, or a wall tablet proportionally, with every widget scaling to
// the available space rather than living at a fixed pixel size. (≈ square cells on
// a 16:9 screen.) Default widget sizes are in these grid units.
const GAP = 8;
// Fixed row height only for the phone fallback's stacked, view-only list.
const ROW_H = 42;

type WidgetType = "device" | "button" | "group" | "now_playing" | "scene" | "control" | "sensor" | "weather" | "clock" | "label" | "exit";

// The kiosk-exit control is a special built-in widget: present on every board,
// movable (positioned in edit mode) but not user-addable or removable. It renders
// only in edit (to place it) and kiosk (the small ✕ that leaves full-screen).
const EXIT_ID = "__exit__";
const DEFAULT_EXIT: Widget = { id: EXIT_ID, type: "exit", x: 0, y: 9999, w: 4, h: 4, config: {} };
function withExit(ws: Widget[]): Widget[] {
  return ws.some((w) => w.type === "exit") ? ws : [...ws, DEFAULT_EXIT];
}

/** A locally-generated widget id. */
const newId = () => `w_${Math.random().toString(36).slice(2, 10)}`;

/** Served inside the Bifrost kiosk WebView (it appends `BifrostKiosk/<v>` to the
 * UA). A wall fixture auto-launches its assigned board full-screen. */
const IS_KIOSK = /\bBifrostKiosk\//.test(navigator.userAgent);

export function BoardsPage() {
  const { isMobile } = useViewport();
  const dialogs = useDialogs();

  const [boards, setBoards] = useState<Dashboard[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [edit, setEdit] = useState(false);
  const [kiosk, setKiosk] = useState(false);
  const [adding, setAdding] = useState(false);
  const [creating, setCreating] = useState(false);
  const [editingBoard, setEditingBoard] = useState(false);
  const [importing, setImporting] = useState(false);
  const [configuring, setConfiguring] = useState<Widget | null>(null);

  // Device fleet + scenes, fetched once and patched optimistically.
  const [lights, setLights] = useState<Light[]>([]);
  const [media, setMedia] = useState<MediaDevice[]>([]);
  const [power, setPower] = useState<PowerDevice[]>([]);
  const [scenes, setScenes] = useState<Scene[]>([]);
  const [generic, setGeneric] = useState<GenericDevice[]>([]);
  const [rooms, setRooms] = useState<Room[]>([]);
  const [flyout, setFlyout] = useState<{ widget: Widget; anchor: HTMLElement } | null>(null);

  const reloadBoards = useCallback(async () => {
    const bs = await getDashboards();
    setBoards(bs);
    setActiveId((cur) => cur ?? bs[0]?.id ?? null);
  }, []);

  const reloadDevices = useCallback(async () => {
    const [l, m, p, s, g, r] = await Promise.all([
      getLights(),
      getMediaDevices(),
      getPowerDevices(),
      getScenes(),
      getGenericDevices(),
      getRooms(),
    ]);
    if (l !== "unauthorized") setLights(l);
    setMedia(m);
    setPower(p);
    setScenes(s);
    setGeneric(g);
    setRooms(r);
  }, []);

  useEffect(() => {
    reloadBoards();
    reloadDevices();
  }, [reloadBoards, reloadDevices]);

  // On a wall-tablet kiosk, auto-launch the board assigned to this device
  // (configured per-kiosk from a main client, resolved via the kiosk's own key
  // cookie) straight into full-screen — no tapping needed on a static fixture.
  useEffect(() => {
    if (!IS_KIOSK) return;
    getKioskSelf().then((self) => {
      if (self?.default_board_id) {
        setActiveId(self.default_board_id);
        setKiosk(true);
      }
    });
  }, []);

  // Live device state via the shared `/api/events` SSE stream (instant push, same
  // as the Control page) — plus a slow poll for generic readouts (sensors), which
  // aren't pushed. Keeps an always-on wall display current without stale lag.
  useEffect(() => {
    const es = new EventSource("/api/events");
    es.addEventListener("light_state", (raw) => {
      const { device_id, patch } = JSON.parse((raw as MessageEvent).data) as {
        device_id: string;
        patch: LightStatePatch;
      };
      setLights((prev) =>
        prev.map((l) => (l.device_id === device_id ? { ...l, last_state: mergePatch(l.last_state, patch) } : l)),
      );
    });
    es.addEventListener("media_state", (raw) => {
      const ev = JSON.parse((raw as MessageEvent).data) as {
        provider_id: string;
        device_id: string;
        state: MediaDevice["state"];
      };
      setMedia((prev) =>
        prev.map((d) => (d.provider_id === ev.provider_id && d.device_id === ev.device_id ? { ...d, state: ev.state } : d)),
      );
    });
    es.addEventListener("power_state", (raw) => {
      const ev = JSON.parse((raw as MessageEvent).data) as {
        provider_id: string;
        device_id: string;
        state: PowerDevice["state"];
      };
      setPower((prev) =>
        prev.map((d) => (d.provider_id === ev.provider_id && d.device_id === ev.device_id ? { ...d, state: ev.state } : d)),
      );
    });
    es.onerror = () => {};
    const t = setInterval(() => { getGenericDevices().then(setGeneric); }, 20000);
    return () => { es.close(); clearInterval(t); };
  }, []);

  const board = boards.find((b) => b.id === activeId) ?? null;
  const widgets = board?.widgets ?? [];

  // Persist the current board's widget layout (debounced by the caller's edits).
  const saveWidgets = useCallback(
    async (next: Widget[]) => {
      if (!board) return;
      setBoards((bs) => bs.map((b) => (b.id === board.id ? { ...b, widgets: next } : b)));
      await updateDashboard(board.id, { widgets: next });
    },
    [board],
  );

  function patchWidget(id: string, next: Widget) {
    if (!board) return;
    // Upsert: moving the built-in exit widget (synthesized, not yet stored) adds
    // it to the layout on first drag so its position persists.
    saveWidgets(
      widgets.some((w) => w.id === id) ? widgets.map((w) => (w.id === id ? next : w)) : [...widgets, next],
    );
  }
  // Batch-update several widgets in one save (a group move) — a series of single
  // patchWidget calls would each start from the same `widgets` snapshot and clobber
  // each other, so the whole set is merged in one pass. Upserts a synthesized
  // widget (the exit) being moved for the first time.
  function patchManyWidgets(updated: Widget[]) {
    if (!board || updated.length === 0) return;
    const m = new Map(updated.map((u) => [u.id, u]));
    const existingIds = new Set(widgets.map((w) => w.id));
    const merged = widgets.map((w) => m.get(w.id) ?? w);
    const added = updated.filter((u) => !existingIds.has(u.id));
    saveWidgets([...merged, ...added]);
  }
  function removeWidget(id: string) {
    if (!board) return;
    saveWidgets(widgets.filter((w) => w.id !== id));
  }
  function addWidget(w: Widget) {
    if (!board) return;
    saveWidgets([...widgets, w]);
  }

  // ── board CRUD ──
  function newBoard() {
    setCreating(true);
  }
  async function createBoard(name: string, aspect: string) {
    setCreating(false);
    const b = await createDashboard(name.trim(), aspect);
    await reloadBoards();
    setActiveId(b.id);
    setEdit(true);
  }
  async function saveBoardEdits(name: string, aspect: string) {
    if (!board) return;
    setEditingBoard(false);
    await updateDashboard(board.id, { name: name.trim(), aspect });
    await reloadBoards();
  }

  // Export the active board to a JSON file (name + aspect + widget layout). Widget
  // device/scene references are ids, so an export re-imports cleanly on the same
  // hub (backup / duplicate); on a different hub the device ids won't resolve.
  function exportBoard() {
    if (!board) return;
    const payload = { bifrost_board: 1, name: board.name, aspect: board.aspect, widgets: board.widgets };
    const blob = new Blob([JSON.stringify(payload, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `${board.name.replace(/[^\w.-]+/g, "_") || "board"}.bifrost-board.json`;
    a.click();
    URL.revokeObjectURL(url);
  }

  // Create a new board from exported JSON (envelope or a bare {name,aspect,widgets}).
  async function importBoard(text: string) {
    let data: { name?: unknown; aspect?: unknown; widgets?: unknown };
    try {
      data = JSON.parse(text);
    } catch {
      await dialogs.alert({ title: "Import failed", message: "That isn't valid JSON." });
      return;
    }
    if (!data || !Array.isArray(data.widgets)) {
      await dialogs.alert({ title: "Import failed", message: "No widget layout found in that file." });
      return;
    }
    setImporting(false);
    const name = (typeof data.name === "string" && data.name.trim()) || "Imported board";
    const aspect = typeof data.aspect === "string" ? data.aspect : "16:9";
    const b = await createDashboard(name, aspect);
    await updateDashboard(b.id, { widgets: data.widgets as Widget[] });
    await reloadBoards();
    setActiveId(b.id);
    setEdit(true);
  }
  async function deleteBoard() {
    if (!board) return;
    const ok = await dialogs.confirm({
      title: `Delete "${board.name}"`,
      message: "Delete this board and its widgets?",
      confirmLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
    await deleteDashboard(board.id);
    setActiveId(null);
    await reloadBoards();
  }

  // ── optimistic device patchers (shared with widgets) ──
  const onLightUpdate = (id: string, st: LightState) =>
    setLights((ls) => ls.map((l) => (l.id === id ? { ...l, last_state: st } : l)));
  const onMediaPatch = (id: string, patch: Partial<MediaDevice["state"]>) =>
    setMedia((ms) => ms.map((m) => (m.id === id ? { ...m, state: { ...m.state, ...patch } } : m)));
  const onPowerToggle = (id: string, next: boolean) => {
    setPower((ps) => ps.map((d) => (d.id === id ? { ...d, state: { ...d.state, on: next } } : d)));
    setPowerState(id, next);
  };

  const renderWidget = (w: Widget) =>
    w.type === "exit" ? (
      // Inert while editing (the box's content is non-interactive then, so it
      // drags); in kiosk the ✕ leaves full-screen.
      <ExitWidget onExit={() => setKiosk(false)} />
    ) : (
    <WidgetContent
      w={w}
      lights={lights}
      media={media}
      power={power}
      scenes={scenes}
      generic={generic}
      edit={edit}
      onLightUpdate={onLightUpdate}
      onMediaPatch={onMediaPatch}
      onPowerToggle={onPowerToggle}
      onChanged={reloadDevices}
      onOpenFlyout={(anchor) => setFlyout({ widget: w, anchor })}
    />
  );

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        flex: 1,
        minHeight: 0,
        width: "100%",
        padding: isMobile ? "1rem 0.85rem" : "1rem 1.4rem",
        gap: "0.5rem",
      }}
    >
      <PageHeader title="Boards" status="Your own dashboards — drag widgets where you want them" />

      {/* Board tabs + actions */}
      <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: "0.5rem", marginBottom: "1rem" }}>
        {boards.map((b) => (
          <button
            key={b.id}
            onClick={() => { setActiveId(b.id); setEdit(false); }}
            style={{
              ...TAB,
              ...(b.id === activeId
                ? { background: alpha(T.accent, 0.16), borderColor: T.accent, color: T.text }
                : {}),
            }}
          >
            {b.name}
          </button>
        ))}
        <button onClick={newBoard} style={{ ...TAB, borderStyle: "dashed", color: T.accent }}>
          + Board
        </button>
        {!isMobile && (
          <button onClick={() => setImporting(true)} style={{ ...TAB, borderStyle: "dashed", color: T.dim }} title="Create a board from an exported JSON file">
            Import
          </button>
        )}
        {board && !isMobile && (
          <div style={{ marginLeft: "auto", display: "flex", gap: "0.4rem" }}>
            {edit && (
              <Button variant="ghost" onClick={() => setAdding(true)}>
                + Widget
              </Button>
            )}
            {!edit && widgets.length > 0 && (
              <Button variant="ghost" onClick={() => setKiosk(true)} title="Full-screen wall display">
                Kiosk
              </Button>
            )}
            <Button
              variant={edit ? "primary" : "ghost"}
              onClick={() => setEdit((v) => !v)}
            >
              {edit ? "Done" : "Edit"}
            </Button>
            {edit && (
              <>
                <Button variant="ghost" onClick={() => setEditingBoard(true)}>Edit board</Button>
                <Button variant="ghost" onClick={exportBoard} title="Download this board as a JSON file">Export</Button>
              </>
            )}
          </div>
        )}
      </div>

      {/* The canvas fills all remaining space (full width + height) — a board is a
          full-screen surface, suited to a wall tablet. */}
      <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
        {!board ? (
          <EmptyState onCreate={newBoard} text="No boards yet. Create one to start composing widgets." />
        ) : widgets.length === 0 ? (
          <EmptyState
            onCreate={() => { setEdit(true); setAdding(true); }}
            text={edit ? "Empty board — add your first widget." : "This board is empty. Tap Edit to add widgets."}
            cta={edit ? "+ Add widget" : "Edit board"}
          />
        ) : isMobile ? (
          // Phones: a simple stacked, view-only render (editing happens on a bigger screen).
          <div style={{ flex: 1, minHeight: 0, overflowY: "auto", display: "flex", flexDirection: "column", gap: GAP }}>
            {[...widgets]
              .filter((w) => w.type !== "exit")
              .sort((a, b) => a.y - b.y || a.x - b.x)
              .map((w) => (
                <div key={w.id} style={{ minHeight: ROW_H }}>
                  {renderWidget(w)}
                </div>
              ))}
          </div>
        ) : (
          <BoardGrid
            widgets={edit ? withExit(widgets) : widgets.filter((w) => w.type !== "exit")}
            aspect={board?.aspect ?? "16:9"}
            edit={edit}
            onChange={patchWidget}
            onChangeMany={patchManyWidgets}
            onConfigure={(w) => setConfiguring(w)}
            onRemove={removeWidget}
            renderWidget={renderWidget}
          />
        )}
      </div>

      {/* Add / configure widget modal */}
      {(adding || configuring) && board && (
        <WidgetEditorModal
          existing={configuring}
          lights={lights}
          media={media}
          power={power}
          scenes={scenes}
          generic={generic}
          rooms={rooms}
          onClose={() => { setAdding(false); setConfiguring(null); }}
          onSave={(spec) => {
            if (configuring) {
              // Keep the widget's existing position/size; only its type + config change
              // (spec carries default w/h that must not clobber a resized widget).
              patchWidget(configuring.id, { ...configuring, type: spec.type, config: spec.config as Widget["config"] });
            } else {
              // Place a new widget at the bottom-left, sized per type but clamped to
              // the board's grid so it can never land off the (non-scrolling) canvas.
              const { cols, rows } = aspectGrid(board.aspect);
              const ww = Math.min(spec.w ?? 12, cols);
              const wh = Math.min(spec.h ?? 8, rows);
              const bottom = widgets.reduce((m, w) => Math.max(m, w.y + w.h), 0);
              const y = Math.max(0, Math.min(rows - wh, bottom));
              addWidget({ id: newId(), x: 0, y, ...spec, w: ww, h: wh });
            }
            setAdding(false);
            setConfiguring(null);
          }}
        />
      )}

      {/* New board: name + aspect ratio */}
      {creating && (
        <BoardModal title="New board" confirmLabel="Create" onClose={() => setCreating(false)} onSubmit={createBoard} />
      )}
      {editingBoard && board && (
        <BoardModal
          title="Edit board"
          confirmLabel="Save"
          initialName={board.name}
          initialAspect={board.aspect}
          onClose={() => setEditingBoard(false)}
          onSubmit={saveBoardEdits}
          onDelete={deleteBoard}
        />
      )}
      {importing && <ImportBoardModal onClose={() => setImporting(false)} onImport={importBoard} />}

      {/* Device / media fly-out for tile widgets */}
      {flyout &&
        (() => {
          const cfg = (flyout.widget.config ?? {}) as { domain?: string; id?: string };
          const onClose = () => setFlyout(null);
          if (cfg.domain === "light") {
            const l = lights.find((d) => d.id === cfg.id);
            return l ? (
              <DeviceControl domain="light" light={l} anchor={flyout.anchor} onLocalPatch={onLightUpdate} onClose={onClose} />
            ) : null;
          }
          if (cfg.domain === "power") {
            const d = power.find((x) => x.id === cfg.id);
            return d ? (
              <DeviceControl domain="power" device={d} anchor={flyout.anchor} onToggle={(n) => onPowerToggle(d.id, n)} onClose={onClose} />
            ) : null;
          }
          const m = media.find((x) => x.id === cfg.id);
          return m ? (
            <DeviceControl domain="media" device={m} anchor={flyout.anchor} onLocalPatch={onMediaPatch} onClose={onClose} />
          ) : null;
        })()}

      {/* Kiosk: the board fills the whole screen edge-to-edge — over the bottom nav
          (z 40) and top bar — view-only, for a wall tablet. Sits below fly-outs
          (z 60) so widget controls still open above it. The built-in exit widget
          (a small ✕, positioned in edit mode) leaves full-screen. */}
      {kiosk && board && (
        <div style={{ position: "fixed", inset: 0, zIndex: 50, background: color.void, display: "flex", flexDirection: "column" }}>
          <BoardGrid
            widgets={withExit(widgets)}
            aspect={board?.aspect ?? "16:9"}
            edit={false}
            onChange={() => {}}
            onChangeMany={() => {}}
            onConfigure={() => {}}
            onRemove={() => {}}
            renderWidget={renderWidget}
          />
        </div>
      )}

      {dialogs.element}
    </div>
  );
}

// ── The drag-resize grid ──────────────────────────────────────────────────────

// Parse an "<w>:<h>" aspect into a grid: `BASE` cells on the longer axis, the
// shorter axis scaled to keep cells ≈ square. Falls back to 16:9 for junk.
const GRID_BASE = 48;
// Group a device list by its room (direct `room_id`, else the inherited
// provider-group room), so the picker reads room-by-room. Rooms come first in
// name order; devices with no room fall into a trailing "No room" bucket.
type RoomedDevice = { id: string; name: string; room_id?: string | null; inherited_room_id?: string | null };
function groupByRoom<T extends RoomedDevice>(devices: T[], rooms: Room[]): { room: string; devices: T[] }[] {
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

// Clamp a widget's box into the current grid, so a widget can never render off
// the (non-scrolling) canvas — e.g. after the board's aspect ratio changes the
// row/column count, or a board is opened on a different grid than it was built on.
function clampWidget(w: Widget, cols: number, rows: number): Widget {
  const ww = Math.max(1, Math.min(w.w, cols));
  const wh = Math.max(1, Math.min(w.h, rows));
  return {
    ...w,
    w: ww,
    h: wh,
    x: Math.max(0, Math.min(cols - ww, w.x)),
    y: Math.max(0, Math.min(rows - wh, w.y)),
  };
}

// Room-grouped options for a device `Select` — same room ordering as the
// checkbox picker, so every device chooser reads room-by-room (with the Select's
// built-in search box on top).
function deviceSelectOptions<T extends RoomedDevice>(devices: T[], rooms: Room[]) {
  return groupByRoom(devices, rooms).flatMap(({ room, devices }) =>
    devices.map((d) => ({ value: d.id, label: d.name, group: room })),
  );
}

function aspectGrid(aspect: string): { ar: number; cols: number; rows: number } {
  const [aw, ah] = aspect.split(":").map((n) => Number(n));
  const w = aw > 0 ? aw : 16;
  const h = ah > 0 ? ah : 9;
  const ar = w / h;
  const cols = ar >= 1 ? GRID_BASE : Math.max(1, Math.round(GRID_BASE * ar));
  const rows = ar >= 1 ? Math.max(1, Math.round(GRID_BASE / ar)) : GRID_BASE;
  return { ar, cols, rows };
}

function BoardGrid({
  widgets,
  aspect,
  edit,
  onChange,
  onChangeMany,
  onConfigure,
  onRemove,
  renderWidget,
}: {
  widgets: Widget[];
  aspect: string;
  edit: boolean;
  onChange: (id: string, next: Widget) => void;
  onChangeMany: (updated: Widget[]) => void;
  onConfigure: (w: Widget) => void;
  onRemove: (id: string) => void;
  renderWidget: (w: Widget) => React.ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 1200, h: 600 });
  // Multi-select: drag on the empty canvas to marquee-select; then dragging any
  // selected widget moves the whole group.
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [marquee, setMarquee] = useState<{ x0: number; y0: number; x1: number; y1: number } | null>(null);
  const [groupNudge, setGroupNudge] = useState<{ dx: number; dy: number } | null>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    setSize({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, []);
  // Selection only makes sense while editing; drop it when leaving edit mode.
  useEffect(() => {
    if (!edit) setSelected(new Set());
  }, [edit]);

  // The canvas is a fixed-aspect box letterboxed inside the available space: its
  // size depends only on the container and the board's aspect ratio — never on the
  // widgets — so there's no measure→grow→re-measure feedback loop. The grid is
  // COLS×ROWS derived from the aspect, and a widget is a fraction of the canvas, so
  // a board looks identical (just scaled) on a phone, desktop, or wall tablet.
  const { ar, cols, rows } = aspectGrid(aspect);
  let canvasW = size.w;
  let canvasH = canvasW / ar;
  if (canvasH > size.h) {
    canvasH = size.h;
    canvasW = canvasH * ar;
  }
  const cellW = canvasW / cols;
  const cellH = canvasH / rows;

  // ── marquee select (drag on empty canvas) ──
  const marqStart = useRef<{ x: number; y: number } | null>(null);
  function canvasPoint(e: React.PointerEvent) {
    const r = canvasRef.current!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }
  function marqueeDown(e: React.PointerEvent) {
    // Only a press that lands on the canvas itself (widgets stop propagation).
    if (!edit || e.target !== e.currentTarget) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
    const p = canvasPoint(e);
    marqStart.current = p;
    setMarquee({ x0: p.x, y0: p.y, x1: p.x, y1: p.y });
  }
  function marqueeMove(e: React.PointerEvent) {
    if (!marqStart.current) return;
    const p = canvasPoint(e);
    setMarquee({ x0: marqStart.current.x, y0: marqStart.current.y, x1: p.x, y1: p.y });
  }
  function marqueeUp() {
    if (!marqStart.current || !marquee) {
      marqStart.current = null;
      return;
    }
    const l = Math.min(marquee.x0, marquee.x1), r = Math.max(marquee.x0, marquee.x1);
    const t = Math.min(marquee.y0, marquee.y1), b = Math.max(marquee.y0, marquee.y1);
    if (r - l < 4 && b - t < 4) {
      setSelected(new Set()); // a click on empty canvas clears the selection
    } else {
      const hit = new Set<string>();
      for (const raw of widgets) {
        const w = clampWidget(raw, cols, rows);
        const wl = w.x * cellW, wt = w.y * cellH, wr = (w.x + w.w) * cellW, wb = (w.y + w.h) * cellH;
        if (!(r < wl || l > wr || b < wt || t > wb)) hit.add(w.id);
      }
      setSelected(hit);
    }
    marqStart.current = null;
    setMarquee(null);
  }

  // Commit a group move: shift every selected widget by the same cell delta,
  // clamping the delta so the whole selection stays on the canvas.
  function commitGroup(dCols: number, dRows: number) {
    const sel = widgets.filter((w) => selected.has(w.id)).map((w) => clampWidget(w, cols, rows));
    if (sel.length === 0) { setGroupNudge(null); return; }
    const lowDx = Math.max(...sel.map((w) => -w.x));
    const highDx = Math.min(...sel.map((w) => cols - w.w - w.x));
    const lowDy = Math.max(...sel.map((w) => -w.y));
    const highDy = Math.min(...sel.map((w) => rows - w.h - w.y));
    const ddc = Math.max(lowDx, Math.min(highDx, dCols));
    const ddr = Math.max(lowDy, Math.min(highDy, dRows));
    onChangeMany(sel.map((w) => ({ ...w, x: w.x + ddc, y: w.y + ddr })));
    setGroupNudge(null);
  }

  // Three tiers of edit guides, painted top→bottom: a thick center cross, medium
  // quarter lines (¼/½/¾), and the faint fine cell grid. The center & quarters give
  // orientation points; the cells give fine snap targets.
  const fine = T.border;
  const quart = alpha(T.accent, 0.28);
  const cen = alpha(T.accent, 0.6);
  const cenV = `linear-gradient(90deg, transparent calc(50% - 1px), ${cen} calc(50% - 1px), ${cen} calc(50% + 1px), transparent calc(50% + 1px))`;
  const cenH = `linear-gradient(0deg, transparent calc(50% - 1px), ${cen} calc(50% - 1px), ${cen} calc(50% + 1px), transparent calc(50% + 1px))`;
  const quartV = `repeating-linear-gradient(90deg, ${quart} 0, ${quart} 1px, transparent 1px, transparent 25%)`;
  const quartH = `repeating-linear-gradient(0deg, ${quart} 0, ${quart} 1px, transparent 1px, transparent 25%)`;
  const fineV = `linear-gradient(90deg, ${fine} 1px, transparent 1px)`;
  const fineH = `linear-gradient(0deg, ${fine} 1px, transparent 1px)`;
  const guides = edit
    ? {
        backgroundImage: `${cenV}, ${cenH}, ${quartV}, ${quartH}, ${fineV}, ${fineH}`,
        backgroundSize: `100% 100%, 100% 100%, 100% 100%, 100% 100%, ${cellW}px 100%, 100% ${cellH}px`,
      }
    : {};

  return (
    <div
      ref={ref}
      style={{
        flex: 1,
        minHeight: 0,
        width: "100%",
        overflow: "hidden",
        position: "relative",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
      }}
    >
      <div
        ref={canvasRef}
        onPointerDown={marqueeDown}
        onPointerMove={marqueeMove}
        onPointerUp={marqueeUp}
        onPointerCancel={marqueeUp}
        style={{
          position: "relative",
          width: canvasW,
          height: canvasH,
          borderRadius: radius.lg,
          touchAction: edit ? "none" : undefined,
          // A faint frame + tiered grid guides while editing, so the canvas bounds
          // and its center/quarters read at a glance.
          outline: edit ? `1px solid ${T.hairline}` : undefined,
          ...guides,
        }}
      >
        {widgets.map((w) => (
          <WidgetBox
            key={w.id}
            w={clampWidget(w, cols, rows)}
            cols={cols}
            rows={rows}
            cellW={cellW}
            cellH={cellH}
            edit={edit}
            selected={selected.has(w.id)}
            groupMove={selected.has(w.id) && selected.size > 1}
            nudge={groupNudge}
            onGroupMove={(dx, dy) => setGroupNudge({ dx, dy })}
            onGroupCommit={commitGroup}
            onSingleStart={() => selected.size > 0 && setSelected(new Set())}
            onChange={(next) => onChange(w.id, next)}
            onConfigure={() => onConfigure(w)}
            onRemove={() => onRemove(w.id)}
          >
            {renderWidget(w)}
          </WidgetBox>
        ))}
        {marquee && (
          <div
            style={{
              position: "absolute",
              left: Math.min(marquee.x0, marquee.x1),
              top: Math.min(marquee.y0, marquee.y1),
              width: Math.abs(marquee.x1 - marquee.x0),
              height: Math.abs(marquee.y1 - marquee.y0),
              border: `1px solid ${T.accent}`,
              background: alpha(T.accent, 0.12),
              borderRadius: 3,
              pointerEvents: "none",
              zIndex: 20,
            }}
          />
        )}
      </div>
    </div>
  );
}

function WidgetBox({
  w,
  cols,
  rows,
  cellW,
  cellH,
  edit,
  selected = false,
  groupMove = false,
  nudge = null,
  onGroupMove,
  onGroupCommit,
  onSingleStart,
  onChange,
  onConfigure,
  onRemove,
  children,
}: {
  w: Widget;
  cols: number;
  rows: number;
  cellW: number;
  cellH: number;
  edit: boolean;
  /** Part of the current multi-selection (highlighted). */
  selected?: boolean;
  /** Dragging this widget moves the whole selection (selected + >1 chosen). */
  groupMove?: boolean;
  /** Live group translate (px) applied to selected widgets that aren't the one
   * being dragged. */
  nudge?: { dx: number; dy: number } | null;
  onGroupMove?: (dx: number, dy: number) => void;
  onGroupCommit?: (dCols: number, dRows: number) => void;
  /** Starting a plain (non-group) move clears any selection. */
  onSingleStart?: () => void;
  onChange: (next: Widget) => void;
  onConfigure: () => void;
  onRemove: () => void;
  children: React.ReactNode;
}) {
  const [drag, setDrag] = useState<{ mode: "move" | "resize"; sx: number; sy: number; dx: number; dy: number } | null>(
    null,
  );

  function down(e: React.PointerEvent, mode: "move" | "resize") {
    if (!edit) return;
    e.preventDefault();
    e.stopPropagation();
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
    if (mode === "move" && !groupMove) onSingleStart?.();
    setDrag({ mode, sx: e.clientX, sy: e.clientY, dx: 0, dy: 0 });
  }
  function move(e: React.PointerEvent) {
    if (!drag) return;
    const dx = e.clientX - drag.sx, dy = e.clientY - drag.sy;
    setDrag({ ...drag, dx, dy });
    if (drag.mode === "move" && groupMove) onGroupMove?.(dx, dy);
  }
  function up() {
    if (!drag) return;
    const dCols = Math.round(drag.dx / cellW);
    const dRows = Math.round(drag.dy / cellH);
    if (drag.mode === "move" && groupMove) {
      onGroupCommit?.(dCols, dRows); // moves the whole selection at once
    } else if (drag.mode === "move") {
      onChange({
        ...w,
        x: Math.max(0, Math.min(cols - w.w, w.x + dCols)),
        y: Math.max(0, Math.min(rows - w.h, w.y + dRows)),
      });
    } else {
      onChange({
        ...w,
        w: Math.max(1, Math.min(cols - w.x, w.w + dCols)),
        h: Math.max(1, Math.min(rows - w.y, w.h + dRows)),
      });
    }
    setDrag(null);
  }

  const movePx = drag?.mode === "move" ? drag : null;
  const sizePx = drag?.mode === "resize" ? drag : null;
  // The widget being dragged uses its own delta; other selected widgets follow the
  // live group nudge.
  const offDx = movePx?.dx ?? (selected && !drag && nudge ? nudge.dx : 0);
  const offDy = movePx?.dy ?? (selected && !drag && nudge ? nudge.dy : 0);
  return (
    <div
      onPointerMove={move}
      onPointerUp={up}
      onPointerCancel={up}
      onPointerDown={edit ? (e) => down(e, "move") : undefined}
      style={{
        position: "absolute",
        left: w.x * cellW + offDx,
        top: w.y * cellH + offDy,
        width: w.w * cellW - GAP + (sizePx?.dx ?? 0),
        height: w.h * cellH - GAP + (sizePx?.dy ?? 0),
        touchAction: edit ? "none" : undefined,
        cursor: edit ? (drag?.mode === "move" ? "grabbing" : "grab") : undefined,
        zIndex: drag || (selected && nudge) ? 10 : 1,
        borderRadius: radius.frame,
        // A label/heading is a passive overlay — in view/kiosk it's click-through so
        // a button placed under it stays usable. (Still draggable in edit mode.)
        ...(!edit && w.type === "label" ? { pointerEvents: "none" as const } : {}),
        ...(edit
          ? {
              outline: selected ? `2px solid ${T.accent}` : `1px dashed ${T.hairline}`,
              boxShadow: drag ? "0 8px 24px -8px #000" : undefined,
            }
          : {}),
      }}
    >
      {/* Content — non-interactive while editing so drags don't trigger controls.
          A size container so readout widgets can scale their type to the box. */}
      <div
        style={{
          width: "100%",
          height: "100%",
          overflow: "hidden",
          // None while editing (so the box drags). A label is also click-through in
          // view mode — both the outer box and this content must be `none`, or a
          // child's `auto` re-captures clicks meant for a button under it.
          pointerEvents: edit || w.type === "label" ? "none" : "auto",
          containerType: "size",
        }}
      >
        {children}
      </div>

      {edit && (
        <>
          {/* The built-in exit control has no config and can't be removed. */}
          {w.type !== "exit" && (
            <>
              <button onClick={onConfigure} onPointerDown={(e) => e.stopPropagation()} title="Configure" style={CORNER_BTN(4, 28)}>
                <Glyph name="gear" size={13} />
              </button>
              <button onClick={onRemove} onPointerDown={(e) => e.stopPropagation()} title="Remove" style={{ ...CORNER_BTN(4, 4), color: "#c77" }}>
                ×
              </button>
            </>
          )}
          <div
            onPointerDown={(e) => down(e, "resize")}
            title="Resize"
            style={{
              position: "absolute",
              right: 0,
              bottom: 0,
              width: 18,
              height: 18,
              cursor: "nwse-resize",
              background: `linear-gradient(135deg, transparent 50%, ${T.accent} 50%)`,
              borderBottomRightRadius: radius.md,
            }}
          />
        </>
      )}
    </div>
  );
}

// ── Widget content (view mode) ────────────────────────────────────────────────

function WidgetContent({
  w,
  lights,
  media,
  power,
  scenes,
  generic,
  edit,
  onLightUpdate,
  onMediaPatch,
  onPowerToggle,
  onChanged,
  onOpenFlyout,
}: {
  w: Widget;
  lights: Light[];
  media: MediaDevice[];
  power: PowerDevice[];
  scenes: Scene[];
  generic: GenericDevice[];
  edit: boolean;
  onLightUpdate: (id: string, st: LightState) => void;
  onMediaPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
  onPowerToggle: (id: string, next: boolean) => void;
  onChanged: () => void;
  onOpenFlyout: (anchor: HTMLElement) => void;
}) {
  const cfg = (w.config ?? {}) as Record<string, unknown>;

  if (w.type === "sensor") return <SensorWidget cfg={cfg} generic={generic} />;
  if (w.type === "weather") return <WeatherWidget cfg={cfg} generic={generic} />;

  if (w.type === "group" || w.type === "button") {
    return (
      <GroupWidget
        cfg={cfg as { domain?: string; ids?: string[]; label?: string }}
        lights={lights}
        media={media}
        power={power}
        edit={edit}
        compact={w.type === "button"}
        onLightUpdate={onLightUpdate}
        onMediaPatch={onMediaPatch}
        onPowerToggle={onPowerToggle}
      />
    );
  }

  if (w.type === "control") {
    const control = cfg as unknown as RoomControl;
    return (
      <div style={CENTER}>
        <RoomControlButton
          control={control}
          lights={lights}
          power={power}
          audio={media}
          onLightUpdate={onLightUpdate}
          onPowerToggle={onPowerToggle}
          onMediaPatch={onMediaPatch}
          onChanged={onChanged}
          size={44}
        />
        {((cfg.name as string) || control.label) && (
          <span style={TILE_LABEL}>{(cfg.name as string) || control.label}</span>
        )}
      </div>
    );
  }

  if (w.type === "scene") {
    const isRestore = !!cfg.restore_home;
    // Restore Home reuses the Control page's gilded two-tap seal (arm → confirm
    // within 5s), so it looks and behaves identically wherever it appears.
    if (isRestore) {
      const defaultHome = scenes.find((s) => s.is_default && !s.room_id);
      return (
        <div style={{ ...CENTER, pointerEvents: edit ? "none" : "auto" }}>
          <RestoreHomeButton
            name={(cfg.name as string) || defaultHome?.name || "Home"}
            onRestore={() => { restoreDefaultHome(); }}
          />
        </div>
      );
    }
    const scene = scenes.find((s) => s.id === cfg.scene_id);
    const label = (cfg.name as string) || (scene?.name ?? "Scene");
    return (
      <button
        disabled={edit}
        onClick={async () => { if (scene) await activateScene(scene.id); }}
        title={label}
        style={{
          ...widgetPlate(color.gold, true),
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          gap: "0.45rem",
          cursor: edit ? "default" : "pointer",
        }}
      >
        <CornerFiligree colors={[color.gold]} />
        <Glyph name="scene" size={24} />
        <span style={{ ...labelType, position: "relative", fontSize: "0.74rem", color: color.textAccent, ...ELLIPSIS, maxWidth: "100%" }}>
          {label}
        </span>
      </button>
    );
  }

  if (w.type === "clock") return <ClockWidget cfg={cfg} />;
  if (w.type === "label") return <LabelWidget cfg={cfg} />;

  // device / now_playing → an inline instrument tile.
  return (
    <DeviceTile
      cfg={cfg}
      isNowPlaying={w.type === "now_playing"}
      lights={lights}
      media={media}
      power={power}
      edit={edit}
      onLightUpdate={onLightUpdate}
      onMediaPatch={onMediaPatch}
      onPowerToggle={onPowerToggle}
      onOpenFlyout={onOpenFlyout}
    />
  );
}

/** A device tile: glyph + name (tap → fly-out for colour/transport/more), an
 * aggregate power toggle, and an inline brightness/volume bar — so the common
 * adjustment happens on the tile, not in a fly-out. */
function DeviceTile({
  cfg,
  isNowPlaying,
  lights,
  media,
  power,
  edit,
  onLightUpdate,
  onMediaPatch,
  onPowerToggle,
  onOpenFlyout,
}: {
  cfg: Record<string, unknown>;
  isNowPlaying: boolean;
  lights: Light[];
  media: MediaDevice[];
  power: PowerDevice[];
  edit: boolean;
  onLightUpdate: (id: string, st: LightState) => void;
  onMediaPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
  onPowerToggle: (id: string, next: boolean) => void;
  onOpenFlyout: (anchor: HTMLElement) => void;
}) {
  const domain = (cfg.domain as string) ?? "media";
  const dev =
    domain === "light"
      ? lights.find((d) => d.id === cfg.id)
      : domain === "power"
        ? power.find((d) => d.id === cfg.id)
        : media.find((d) => d.id === cfg.id);
  const commitTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  if (!dev) return <div style={{ ...CENTER, color: T.faint, fontSize: "0.75rem" }}>Device removed</div>;

  const name = (cfg.name as string) || (dev as { name: string }).name;
  const light = domain === "light" ? (dev as Light) : undefined;
  const mediaDev = domain === "media" ? (dev as MediaDevice) : undefined;
  const powerDev = domain === "power" ? (dev as PowerDevice) : undefined;
  const on = !!(light?.last_state?.on ?? mediaDev?.state.power ?? powerDev?.state.on);
  const reachable =
    (light?.last_state?.reachable ?? mediaDev?.state.reachable ?? powerDev?.state.reachable) !== false;
  const accent = light ? lightHex(light) : domain === "power" ? color.gold : color.violet;
  const np = isNowPlaying ? mediaDev?.state.now_playing : undefined;
  const glyph = (dev as { glyph?: string | null }).glyph ?? (light ? "bulb" : powerDev ? "power" : "speaker");

  function togglePower() {
    // Each optimistic flip reverts if the device rejects the write (offline).
    if (light) {
      onLightUpdate(light.id, { ...(light.last_state ?? { on: false }), on: !on }); // power only — preserves the light's mode
      setLightState(light.id, { on: !on }).then((err) => {
        if (err) onLightUpdate(light.id, { ...(light.last_state ?? { on: false }), on });
      });
    } else if (mediaDev) {
      onMediaPatch(mediaDev.id, { power: !on });
      setMediaState(mediaDev.id, { power: !on }).then((err) => {
        if (err) onMediaPatch(mediaDev.id, { power: on });
      });
    } else if (powerDev) {
      onPowerToggle(powerDev.id, !on);
    }
  }

  const dimmable = !!light?.capabilities.dimmable;
  const showSlider = dimmable || !!mediaDev;
  // A light that's off reads 0% (not its stored brightness); media volume is shown
  // regardless of power.
  const sliderVal = Math.round(
    (light ? (on ? (light.last_state?.brightness ?? 0) : 0) : (mediaDev?.state.volume ?? 0)) as number,
  );
  const onSlide = (v: number) => {
    if (light) onLightUpdate(light.id, lightOptimistic(light.last_state, { field: "brightness", brightness: v }));
    else if (mediaDev) onMediaPatch(mediaDev.id, { volume: v });
  };
  // Write only ~300ms after the drag stops (on release) — never spam mid-drag.
  const onCommit = (v: number) => {
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => {
      if (light) setLightState(light.id, { on: true, brightness: v });
      else if (mediaDev) setMediaState(mediaDev.id, { volume: v });
    }, 300);
  };

  // A power device is strictly on/off — its tile is just a big centered power
  // button (like its fly-out), with the name as a caption that opens the fly-out.
  if (powerDev) {
    return (
      <div
        style={{
          ...widgetPlate(accent, on),
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          gap: "0.5rem",
          padding: "0.6rem",
          opacity: reachable ? 1 : 0.45,
        }}
      >
        <CornerFiligree colors={on ? [accent] : undefined} />
        <button
          disabled={edit}
          onClick={(e) => onOpenFlyout(e.currentTarget)}
          title={name}
          style={{ position: "relative", display: "flex", alignItems: "center", gap: "0.4rem", border: "none", background: "none", color: on ? accent : T.dim, cursor: edit ? "default" : "pointer", padding: 0, maxWidth: "100%" }}
        >
          <Glyph name={glyph} size={15} />
          <span style={{ ...TILE_LABEL, color: T.text }}>{name}</span>
        </button>
        <GlyphButton on={on} accent={accent} title={on ? "Turn off" : "Turn on"} active={false} buttonRef={null} onClick={togglePower} size={54}>
          <Glyph name="power" size={24} />
        </GlyphButton>
        <span style={{ position: "relative", fontSize: "0.7rem", color: on ? accent : T.faint }}>
          {reachable ? (on ? "On" : "Off") : "Offline"}
        </span>
      </div>
    );
  }

  return (
    <div
      style={{
        ...widgetPlate(accent, on),
        display: "flex",
        flexDirection: "column",
        padding: "0.5rem 0.6rem",
        gap: "0.4rem",
        opacity: reachable ? 1 : 0.45,
      }}
    >
      <CornerFiligree colors={on ? [accent] : undefined} />
      <div style={{ position: "relative", display: "flex", alignItems: "center", gap: "0.45rem" }}>
        <button
          disabled={edit}
          onClick={(e) => onOpenFlyout(e.currentTarget)}
          title={name}
          style={{ flex: 1, minWidth: 0, display: "flex", alignItems: "center", gap: "0.5rem", border: "none", background: "none", color: on ? accent : T.dim, cursor: edit ? "default" : "pointer", padding: 0, textAlign: "left" }}
        >
          <Glyph name={glyph} size={20} />
          <span style={{ ...TILE_LABEL, color: T.text }}>{name}</span>
        </button>
        <GlyphButton on={on} accent={accent} title={on ? "Turn off" : "Turn on"} active={false} buttonRef={null} onClick={togglePower} size={30}>
          <Glyph name="power" size={14} />
        </GlyphButton>
      </div>
      {np?.title && (
        <div style={{ position: "relative", minWidth: 0 }}>
          <div style={{ ...ELLIPSIS, fontSize: "0.78rem", color: T.text }}>{np.title}</div>
          {np.artist && <div style={{ ...ELLIPSIS, fontSize: "0.7rem", color: T.dim }}>{np.artist}</div>}
        </div>
      )}
      <div style={{ position: "relative", flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
        {/* The empty area opens the fly-out too — the whole tile (bar aside) is the control. */}
        <div
          onClick={(e) => { if (!edit) onOpenFlyout(e.currentTarget); }}
          style={{ flex: 1, minHeight: 0, display: "flex", alignItems: "center", justifyContent: "center", cursor: edit ? "default" : "pointer" }}
        >
          {!showSlider && (
            <span style={{ fontSize: "0.72rem", color: on ? accent : T.faint }}>
              {reachable ? (on ? "On" : "Off") : "Offline"}
            </span>
          )}
        </div>
        {showSlider && (
          <div style={{ flex: "0 0 60%", minHeight: 33, display: "flex" }}>
            <InlineSlider fill value={sliderVal} accent={accent} unit={mediaDev ? "" : "%"} onChange={onSlide} onCommit={onCommit} />
          </div>
        )}
      </div>
    </div>
  );
}

/** The built-in kiosk-exit control: a small ✕ that leaves full-screen. Movable in
 * edit mode (positioned like any widget) but never removed. Scales to its box. */
function ExitWidget({ onExit }: { onExit: () => void }) {
  return (
    <button
      onClick={onExit}
      title="Exit full-screen"
      style={{
        width: "100%",
        height: "100%",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        border: `1px solid ${T.hairline}`,
        borderRadius: radius.frame,
        background: alpha(color.void, 0.5),
        color: T.dim,
        cursor: "pointer",
        fontSize: "clamp(0.9rem, 42cqmin, 2.6rem)",
        lineHeight: 1,
      }}
    >
      ✕
    </button>
  );
}

/** A live clock tile. `cfg.format` = "24h" | "12h". */
function ClockWidget({ cfg }: { cfg: Record<string, unknown> }) {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(t);
  }, []);
  const h24 = cfg.format !== "12h";
  // A size multiplier on the auto-scaling type (1 = default); still box-relative.
  const scale = typeof cfg.scale === "number" && cfg.scale > 0 ? cfg.scale : 1;
  const time = now.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", hour12: !h24 });
  const date = now.toLocaleDateString(undefined, { weekday: "long", month: "short", day: "numeric" });
  return (
    <div style={{ ...widgetPlate(color.gold, false), display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: "0.15rem" }}>
      <CornerFiligree />
      <div style={{ fontSize: `clamp(1rem, ${26 * scale}cqmin, ${5 * scale}rem)`, fontWeight: 600, color: T.text, fontVariantNumeric: "tabular-nums", letterSpacing: "0.02em", lineHeight: 1.05 }}>{time}</div>
      <div style={{ fontSize: `clamp(0.65rem, ${8 * scale}cqmin, ${1.4 * scale}rem)`, color: T.dim }}>{date}</div>
    </div>
  );
}

/** A text/heading tile to title or section a board. `cfg.text`, `cfg.heading`. */
function LabelWidget({ cfg }: { cfg: Record<string, unknown> }) {
  const text = (cfg.text as string) || (cfg.name as string) || "Label";
  const heading = cfg.heading !== false;
  return (
    <div style={{ width: "100%", height: "100%", display: "flex", alignItems: "center", padding: "0.4rem 0.2rem" }}>
      <span
        style={
          heading
            ? { ...labelType, fontSize: "1rem", color: color.textAccent, ...ELLIPSIS, maxWidth: "100%" }
            : { fontSize: "0.9rem", color: T.dim, ...ELLIPSIS, maxWidth: "100%" }
        }
      >
        {text}
      </span>
    </div>
  );
}

/** A "device group" widget — domain control over an ad-hoc set of devices, like
 * a room card but for any chosen devices: an aggregate power toggle plus (for
 * lights/media) the full shared editor cascaded to every member. */
function GroupWidget({
  cfg,
  lights,
  media,
  power,
  edit,
  compact = false,
  onLightUpdate,
  onMediaPatch,
  onPowerToggle,
}: {
  cfg: { domain?: string; ids?: string[]; label?: string; name?: string; glyph?: string };
  lights: Light[];
  media: MediaDevice[];
  power: PowerDevice[];
  edit: boolean;
  /** Bare glyph-button form (the "button" widget): the whole box is one device-lit
   * GlyphButton — tap opens the controls, hold toggles power. Bound to a single
   * device or a group, just like the full card. */
  compact?: boolean;
  onLightUpdate: (id: string, st: LightState) => void;
  onMediaPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
  onPowerToggle: (id: string, next: boolean) => void;
}) {
  const domain = cfg.domain ?? "light";
  const ids = cfg.ids ?? [];
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const commitTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  // `all` is a mixed power group: every selected device of any type, controlled by
  // the one action shared across all domains — power on/off. The typed domains keep
  // their richer controls (brightness/volume), so they stay single-type.
  const all = domain === "all";
  const tLights = domain === "light" || all ? lights.filter((l) => ids.includes(l.id)) : [];
  const tMedia = domain === "media" || all ? media.filter((m) => ids.includes(m.id)) : [];
  const tPower = domain === "power" || all ? power.filter((p) => ids.includes(p.id)) : [];
  const total = tLights.length + tMedia.length + tPower.length;
  const onCount =
    tLights.filter((l) => l.last_state?.on).length +
    tMedia.filter((m) => m.state.power).length +
    tPower.filter((p) => p.state.on).length;
  const anyOn = onCount > 0;
  const accent = domain === "light" ? T.accent : domain === "media" ? T.media : color.gold;
  // Only the single-type light/media groups have a richer editor; power & mixed
  // ("all") groups are on/off only.
  const hasEditor = domain === "light" || domain === "media";

  function togglePower() {
    const next = !anyOn;
    // Each optimistic flip reverts if its device rejects the write (offline).
    for (const l of tLights) {
      onLightUpdate(l.id, { ...(l.last_state ?? { on: false }), on: next }); // power only — preserves each light's mode
      setLightState(l.id, { on: next }).then((err) => {
        if (err) onLightUpdate(l.id, { ...(l.last_state ?? { on: false }), on: !next });
      });
    }
    for (const m of tMedia) {
      onMediaPatch(m.id, { power: next });
      setMediaState(m.id, { power: next }).then((err) => {
        if (err) onMediaPatch(m.id, { power: !next });
      });
    }
    for (const p of tPower) onPowerToggle(p.id, next);
  }

  // Per-light cascade (mirrors the room-header cascade, fanned per device) via the
  // shared `lightControl` rule: send only the moved dimension to each capable
  // member — an effect goes only to members whose catalog has it (never with a
  // colour alongside, which would drop the effect), a brightness change carries
  // just brightness, etc. Optimistic state keeps each light's full look.
  function cascade(change: LightControlChange) {
    const ids = tLights.filter((l) => lightSupports(change, l.capabilities)).map((l) => l.id);
    for (const l of tLights) {
      if (ids.includes(l.id)) onLightUpdate(l.id, lightOptimistic(l.last_state, change));
      else if (change.field !== "effect")
        onLightUpdate(l.id, { ...(l.last_state ?? { on: true }), on: true });
    }
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => {
      for (const id of ids) setLightState(id, lightWrite(change));
    }, 200);
  }

  // Media cascade: the editor commits its own `device`; fan the rest.
  function fanMedia(id: string, patch: Partial<MediaDevice["state"]>) {
    fanMediaCommand(tMedia, id, patch, { onPatch: onMediaPatch, commit: setMediaState });
  }

  // All editor readouts (hex / 0%-when-off brightness / mirek / effect
  // intersection) come from the one shared aggregator. `lit` stays local (typed
  // `Light[]`) for the filigree hexes below.
  const lit = tLights.filter((l) => l.last_state?.on);
  const agg = aggregateLightState(tLights);
  const initHex = agg.hex;
  const initBrightness = agg.brightness;
  const initMirek = agg.mirek;
  const groupEffects = agg.effects;
  const groupEffect = agg.commonEffect;

  const kindWord = domain === "light" ? "light" : domain === "media" ? "speaker" : domain === "power" ? "switch" : "device";
  const label = cfg.name || cfg.label || `${total} ${kindWord}${total !== 1 ? "s" : ""}`;

  // Inline master controls — brightness for a dimmable light group, volume for a
  // media group; both fan to every member (cascade handles its own debounce).
  const groupDimmable = tLights.some((l) => l.capabilities.dimmable);
  const groupVolume = tMedia.length ? Math.round(tMedia.reduce((s, m) => s + (m.state.volume ?? 0), 0) / tMedia.length) : 0;
  const showSlider = (domain === "light" && groupDimmable && total > 0) || (domain === "media" && tMedia.length > 0);
  // The inline bars update local state live but only write to the devices ~300ms
  // after the drag stops (on release), so a drag doesn't spam the providers.
  const slideBrightnessChange = (v: number) => {
    for (const l of tLights) {
      if (l.capabilities.dimmable)
        onLightUpdate(l.id, lightOptimistic(l.last_state, { field: "brightness", brightness: v }));
    }
  };
  const slideBrightnessCommit = (v: number) => {
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => {
      // Brightness only — never re-send a member's colour/effect (shared rule).
      for (const l of tLights) {
        if (l.capabilities.dimmable) setLightState(l.id, { on: true, brightness: v });
      }
    }, 300);
  };
  const slideVolumeChange = (v: number) => {
    for (const m of tMedia) onMediaPatch(m.id, { volume: v, power: true });
  };
  const slideVolumeCommit = (v: number) => {
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => {
      for (const m of tMedia) setMediaState(m.id, { volume: v });
    }, 300);
  };

  const litHexes = anyOn ? (domain === "light" && lit.length ? lit.map(lightHex) : [accent]) : undefined;
  const dotColor = litHexes?.[0] ?? accent;
  const buttonAccent = domain === "light" ? dotColor : accent;

  // The shared light/media editor popovers, anchored on `ref` — reused by both the
  // full card and the compact button.
  const editors = (
    <>
      {open && domain === "light" && ref.current && (
        <LightEditor
          anchor={ref.current}
          title={label}
          initialHex={initHex}
          initialBrightness={initBrightness}
          initialMirek={initMirek}
          showColor={tLights.some((l) => l.capabilities.color_rgb)}
          showWhite={tLights.some((l) => l.capabilities.color_temperature)}
          showBrightness={tLights.some((l) => l.capabilities.dimmable)}
          effects={groupEffects.length > 0 ? groupEffects : undefined}
          initialEffect={groupEffect}
          on={anyOn}
          onToggle={togglePower}
          onChange={cascade}
          onClose={() => setOpen(false)}
        />
      )}
      {open && domain === "media" && tMedia[0] && ref.current && (
        <MediaEditor device={tMedia[0]} anchor={ref.current} onLocalPatch={fanMedia} onClose={() => setOpen(false)} />
      )}
    </>
  );

  // Compact "button" form: the whole box is one device-lit GlyphButton — like a
  // room card's per-device glyph button. Tap opens the controls (or toggles a
  // power group, which has none); hold toggles power.
  if (compact) {
    // A single bound device shows its own glyph; a group shows the domain glyph.
    const single = total === 1 ? (tLights[0] ?? tMedia[0] ?? tPower[0]) : undefined;
    const compactGlyph =
      cfg.glyph || // explicit override wins
      (single as { glyph?: string | null } | undefined)?.glyph ||
      (domain === "light" ? "bulb" : domain === "media" ? "speaker" : "power");
    return (
      <>
        <GlyphButton
          on={anyOn}
          accent={buttonAccent}
          title={label}
          active={open}
          buttonRef={ref}
          onClick={() => (hasEditor ? setOpen(true) : togglePower())}
          onLongPress={togglePower}
          size="100%"
        >
          <span style={{ fontSize: "clamp(16px, 44cqmin, 80px)", lineHeight: 0 }}>
            <Glyph name={compactGlyph} size="1em" />
          </span>
        </GlyphButton>
        {editors}
      </>
    );
  }

  return (
    <div
      style={{
        ...widgetPlateMulti(litHexes ?? [accent], anyOn),
        display: "flex",
        flexDirection: "column",
        padding: "0.5rem 0.6rem",
        gap: "0.35rem",
      }}
    >
      <CornerFiligree colors={litHexes} />
      <div style={{ position: "relative", display: "flex", alignItems: "center", gap: "0.5rem" }}>
        <span
          aria-hidden
          style={{
            width: 14,
            height: 14,
            flexShrink: 0,
            borderRadius: "50%",
            border: "1px solid rgba(255,255,255,0.22)",
            background: anyOn ? `radial-gradient(circle at 35% 30%, #ffffff44, transparent 45%), ${dotColor}` : "#3a372e",
            boxShadow: anyOn ? `0 0 10px -2px ${dotColor}` : "none",
          }}
        />
        <button
          ref={ref}
          disabled={edit || !hasEditor || total === 0}
          onClick={() => setOpen((v) => !v)}
          title={hasEditor ? "More controls" : undefined}
          style={{ flex: 1, minWidth: 0, display: "flex", alignItems: "center", gap: "0.3rem", border: "none", background: "none", padding: 0, cursor: edit || !hasEditor || total === 0 ? "default" : "pointer", textAlign: "left" }}
        >
          <span style={{ ...labelType, fontSize: "0.78rem", color: "#d8cfba", ...ELLIPSIS }}>{label}</span>
          {hasEditor && total > 0 && <Glyph name="chevron" size={13} />}
        </button>
        <GlyphButton on={anyOn} accent={buttonAccent} title="Toggle all" active={false} buttonRef={null} onClick={togglePower} size={32}>
          <Glyph name="power" size={15} />
        </GlyphButton>
      </div>
      <div style={{ position: "relative", flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
        {/* The empty area above the bar opens the full editor — the whole widget
            (bar aside) is the control. */}
        <div
          onClick={() => { if (hasEditor) setOpen(true); }}
          title={hasEditor ? "More controls" : undefined}
          style={{ flex: 1, minHeight: 0, display: "flex", alignItems: "center", justifyContent: "center", cursor: hasEditor && !edit ? "pointer" : "default" }}
        >
          {!showSlider && (
            <span style={{ fontSize: "0.78rem", color: anyOn ? buttonAccent : T.faint }}>
              {total === 0 ? "No devices" : `${onCount} of ${total} on`}
            </span>
          )}
        </div>
        {showSlider && (
          <div style={{ flex: "0 0 60%", minHeight: 39, display: "flex" }}>
            {domain === "light" ? (
              // The brightness bar takes the bulb colour (like the dot), not the theme accent.
              <InlineSlider fill value={initBrightness} accent={dotColor} unit="%" onChange={slideBrightnessChange} onCommit={slideBrightnessCommit} />
            ) : (
              <InlineSlider fill value={groupVolume} accent={accent} unit="" onChange={slideVolumeChange} onCommit={slideVolumeCommit} />
            )}
          </div>
        )}
      </div>
      {editors}
    </div>
  );
}

/** A sensor readout tile — a live value (temperature, humidity, …) from a generic
 * device's control. `cfg.provider_id`/`cfg.device_id`/`cfg.key`. */
function SensorWidget({ cfg, generic }: { cfg: Record<string, unknown>; generic: GenericDevice[] }) {
  const dev = generic.find((d) => d.provider_id === cfg.provider_id && d.device_id === cfg.device_id);
  const ctrl = dev?.controls.find((c) => c.key === cfg.key);
  const label = (cfg.name as string) || ctrl?.label || dev?.name || "Sensor";
  const v = ctrl?.value;
  const display = v === undefined || v === null ? "—" : typeof v === "boolean" ? (v ? "On" : "Off") : String(v);
  return (
    <div style={{ ...widgetPlate(color.gold, false), display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: "0.1rem", padding: "0.5rem" }}>
      <CornerFiligree />
      <div style={{ fontSize: "clamp(1.3rem, 24cqmin, 4rem)", fontWeight: 600, color: T.text, fontVariantNumeric: "tabular-nums", lineHeight: 1.1 }}>
        {display}
        {ctrl?.unit && <span style={{ fontSize: "0.5em", color: T.dim, marginLeft: 2 }}>{ctrl.unit}</span>}
      </div>
      <div style={{ ...ELLIPSIS, maxWidth: "100%", fontSize: "clamp(0.68rem, 7cqmin, 1.2rem)", color: T.dim }}>{label}</div>
    </div>
  );
}

// A weather widget reads a HA `weather.*` entity (surfaced through the generic
// passthrough domain): the entity state is the condition (→ icon + label) and
// the `temperature`/`humidity` readouts are drawn beside it. Glanceable, no
// external API or key — it reuses whatever weather integration HA already has.
function WeatherWidget({ cfg, generic }: { cfg: Record<string, unknown>; generic: GenericDevice[] }) {
  const dev = generic.find(
    (d) => d.provider_id === cfg.provider_id && d.device_id === cfg.device_id,
  );
  const readout = (key: string) => dev?.controls.find((c) => c.key === key);
  const condition = readout("condition")?.value as string | undefined;
  const temp = readout("temperature");
  const humidity = readout("humidity");
  const place = (cfg.name as string) || dev?.name || "Weather";
  return (
    <div
      style={{
        ...widgetPlate(color.gold, true),
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        gap: "0.7rem",
        padding: "0.5rem 0.7rem",
      }}
    >
      <CornerFiligree colors={[color.gold]} />
      <div style={{ position: "relative", color: color.textAccent, flexShrink: 0, fontSize: "clamp(34px, 30cqmin, 96px)", lineHeight: 0 }}>
        <Glyph name={weatherGlyph(condition)} size="1em" />
      </div>
      <div style={{ position: "relative", display: "flex", flexDirection: "column", minWidth: 0 }}>
        <div style={{ fontSize: "clamp(1.4rem, 24cqmin, 4rem)", fontWeight: 600, color: T.text, fontVariantNumeric: "tabular-nums", lineHeight: 1.05 }}>
          {temp ? String(temp.value) : "—"}
          {temp?.unit && <span style={{ fontSize: "0.5em", color: T.dim, marginLeft: 2 }}>{temp.unit}</span>}
        </div>
        <div style={{ ...ELLIPSIS, maxWidth: "100%", fontSize: "clamp(0.72rem, 8cqmin, 1.3rem)", color: T.dim }}>
          {weatherLabel(condition)}
        </div>
        <div style={{ ...ELLIPSIS, maxWidth: "100%", fontSize: "clamp(0.64rem, 6cqmin, 1rem)", color: T.dim, opacity: 0.8 }}>
          {place}
          {humidity ? ` · ${String(humidity.value)}${humidity.unit ?? "%"}` : ""}
        </div>
      </div>
    </div>
  );
}

// ── Board create / edit: name + aspect ratio ──────────────────────────────────

const ASPECT_PRESETS = ["16:9", "16:10", "18.5:9", "4:3", "3:2", "21:9", "1:1", "9:16"];

function BoardModal({
  title,
  confirmLabel,
  initialName = "",
  initialAspect = "16:9",
  onClose,
  onSubmit,
  onDelete,
}: {
  title: string;
  confirmLabel: string;
  initialName?: string;
  initialAspect?: string;
  onClose: () => void;
  onSubmit: (name: string, aspect: string) => void;
  /** When present (editing an existing board), a left-aligned "Delete board"
   * action lives here instead of the edit toolbar, so a stray click can't nuke
   * the board — deleting is now Edit → Edit board → Delete → confirm. */
  onDelete?: () => void;
}) {
  const [name, setName] = useState(initialName);
  const [aspect, setAspect] = useState(initialAspect);
  const isPreset = ASPECT_PRESETS.includes(aspect);

  function submit() {
    if (!name.trim()) return;
    onSubmit(name, aspect.trim() || "16:9");
  }

  return (
    <Modal title={title} onClose={onClose}>
      <Field label="Name">
        <input
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && submit()}
          placeholder="e.g. Living Room"
          style={INPUT}
        />
      </Field>
      <Field label="Aspect ratio">
        <div style={{ display: "flex", flexWrap: "wrap", gap: "0.4rem" }}>
          {ASPECT_PRESETS.map((p) => (
            <button key={p} onClick={() => setAspect(p)} style={{ ...CHIP, ...(aspect === p ? CHIP_ON : {}) }}>
              {p}
            </button>
          ))}
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", marginTop: "0.5rem" }}>
          <span style={{ fontSize: "0.72rem", color: T.dim }}>Custom</span>
          <input
            value={isPreset ? "" : aspect}
            onChange={(e) => setAspect(e.target.value)}
            placeholder="e.g. 18.5:9"
            style={{ ...INPUT, maxWidth: 150 }}
          />
        </div>
        <div style={{ fontSize: "0.66rem", color: T.dim, marginTop: "0.4rem" }}>
          The board is shaped to this ratio and scales to fit any screen (match your tablet — e.g. a
          Galaxy A9 is 18.5:9). Changing it rescales the existing widgets.
        </div>
      </Field>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: "0.5rem", marginTop: "1rem" }}>
        {onDelete ? (
          <Button variant="ghost" onClick={onDelete} style={{ color: "#c77", borderColor: "#5a3636" }}>
            Delete board
          </Button>
        ) : (
          <span />
        )}
        <div style={{ display: "flex", gap: "0.5rem" }}>
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button variant="primary" onClick={submit}>{confirmLabel}</Button>
        </div>
      </div>
    </Modal>
  );
}

/** Import a board from an exported JSON file (upload or paste) → a new board. */
function ImportBoardModal({ onClose, onImport }: { onClose: () => void; onImport: (text: string) => void }) {
  const [text, setText] = useState("");
  function pickFile(e: React.ChangeEvent<HTMLInputElement>) {
    const f = e.target.files?.[0];
    if (f) f.text().then(setText);
  }
  return (
    <Modal title="Import board" onClose={onClose}>
      <Field label="Board file (.json)">
        <input type="file" accept="application/json,.json" onChange={pickFile} style={{ color: T.dim, fontSize: "0.8rem" }} />
      </Field>
      <Field label="…or paste the JSON">
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          rows={8}
          placeholder='{ "bifrost_board": 1, "name": "…", "aspect": "16:9", "widgets": [ … ] }'
          style={{ ...INPUT, fontFamily: "monospace", fontSize: "0.74rem", resize: "vertical", width: "100%" }}
        />
      </Field>
      <div style={{ fontSize: "0.66rem", color: T.dim, marginTop: "0.2rem" }}>
        Creates a new board. Device and scene references are by id, so importing onto a different hub
        may leave some widgets unresolved.
      </div>
      <div style={{ display: "flex", justifyContent: "flex-end", gap: "0.5rem", marginTop: "1rem" }}>
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
        <Button variant="primary" onClick={() => onImport(text)} disabled={!text.trim()}>Import</Button>
      </div>
    </Modal>
  );
}

// ── Add / configure widget modal ──────────────────────────────────────────────

type WidgetSpec = Partial<Pick<Widget, "type" | "config" | "w" | "h">> & { type: WidgetType; config: unknown };

function WidgetEditorModal({
  existing,
  lights,
  media,
  power,
  scenes,
  generic,
  rooms,
  onClose,
  onSave,
}: {
  existing: Widget | null;
  lights: Light[];
  media: MediaDevice[];
  power: PowerDevice[];
  scenes: Scene[];
  generic: GenericDevice[];
  rooms: Room[];
  onClose: () => void;
  onSave: (spec: WidgetSpec) => void;
}) {
  const [type, setType] = useState<WidgetType>((existing?.type as WidgetType) ?? "device");
  const cfg = (existing?.config ?? {}) as Record<string, unknown>;

  // A custom display name overriding the widget's default caption (any type).
  const [name, setName] = useState<string>((cfg.name as string) ?? "");

  // device / now_playing
  const [domain, setDomain] = useState<string>((cfg.domain as string) ?? "light");
  const [deviceId, setDeviceId] = useState<string>((cfg.id as string) ?? "");

  // scene
  const [restoreHome, setRestoreHome] = useState<boolean>(!!cfg.restore_home);
  const [sceneId, setSceneId] = useState<string>((cfg.scene_id as string) ?? "");

  // group
  const [groupDomain, setGroupDomain] = useState<string>((cfg.domain as string) ?? "light");
  const [groupIds, setGroupIds] = useState<string[]>((cfg.ids as string[]) ?? []);
  // Optional glyph override for the Button widget ("" = auto from the device/domain).
  const [buttonGlyph, setButtonGlyph] = useState<string>((cfg.glyph as string) ?? "");
  // Filter text for the group/button device checkbox list.
  const [groupQuery, setGroupQuery] = useState("");

  // clock / label
  const [clockFormat, setClockFormat] = useState<string>((cfg.format as string) ?? "24h");
  const [clockScale, setClockScale] = useState<string>(String((cfg.scale as number) ?? 1));
  const [labelText, setLabelText] = useState<string>((cfg.text as string) ?? "");
  const [labelHeading, setLabelHeading] = useState<boolean>(cfg.heading !== false);

  // sensor — "<provider_id>|<device_id>" composite + the control key.
  const [sensorDev, setSensorDev] = useState<string>(
    cfg.provider_id && cfg.device_id ? `${cfg.provider_id}|${cfg.device_id}` : "",
  );
  const [sensorKey, setSensorKey] = useState<string>((cfg.key as string) ?? "");

  // control
  const [control, setControl] = useState<RoomControl>(
    existing?.type === "control"
      ? (existing.config as RoomControl)
      : { kind: "power", glyph: "power", label: "", targets: [], scene_id: null },
  );

  // Only offer devices that can actually be controlled: skip disabled devices and
  // de-dup/composite "merged-in" copies — a shadowed duplicate or a media companion
  // is hidden from control everywhere else, so it must not be pickable here either.
  // (The widget renderers still receive the full lists so a widget can resolve any
  // device it already references.)
  const selLights = lights.filter((l) => l.enabled !== false && !l.shadowed_by);
  const selMedia = media.filter((m) => m.enabled !== false && !m.shadowed_by && !m.companion_of);
  const selPower = power.filter((p) => p.enabled !== false && !p.shadowed_by);
  const devicesFor = (d: string): { id: string; name: string }[] =>
    d === "light" ? selLights : d === "power" ? selPower : selMedia;

  function save() {
    const nm = name.trim() || undefined;
    if (type === "device" || type === "now_playing") {
      const list = type === "now_playing" ? selMedia : devicesFor(domain);
      const id = deviceId || list[0]?.id;
      if (!id) return;
      onSave({
        type,
        config: { domain: type === "now_playing" ? "media" : domain, id, name: nm },
        w: type === "now_playing" ? 16 : 12,
        h: 8,
      });
    } else if (type === "group") {
      if (groupIds.length === 0) return;
      onSave({ type, config: { domain: groupDomain, ids: groupIds, name: nm }, w: 16, h: 8 });
    } else if (type === "button") {
      if (groupIds.length === 0) return;
      onSave({ type, config: { domain: groupDomain, ids: groupIds, name: nm, glyph: buttonGlyph || undefined }, w: 6, h: 6 });
    } else if (type === "scene") {
      onSave({
        type,
        config: { ...(restoreHome ? { restore_home: true } : { scene_id: sceneId }), name: nm },
        w: 12,
        h: 4,
      });
    } else if (type === "sensor") {
      const [pid, did] = sensorDev.split("|");
      if (!pid || !did || !sensorKey) return;
      onSave({ type, config: { provider_id: pid, device_id: did, key: sensorKey, name: nm }, w: 8, h: 8 });
    } else if (type === "weather") {
      const [pid, did] = sensorDev.split("|");
      if (!pid || !did) return;
      onSave({ type, config: { provider_id: pid, device_id: did, name: nm }, w: 12, h: 8 });
    } else if (type === "clock") {
      onSave({ type, config: { format: clockFormat, scale: Number(clockScale) || 1, name: nm }, w: 12, h: 8 });
    } else if (type === "label") {
      onSave({ type, config: { text: labelText.trim() || nm || "Label", heading: labelHeading }, w: 16, h: 4 });
    } else {
      onSave({ type, config: { ...control, name: nm }, w: 8, h: 8 });
    }
  }

  return (
    <Modal title={existing ? "Configure widget" : "Add widget"} onClose={onClose}>
      {!existing && (
        <Field label="Type">
          <div style={{ display: "flex", flexWrap: "wrap", gap: "0.4rem" }}>
            {(["device", "button", "group", "now_playing", "scene", "control", "sensor", "weather", "clock", "label"] as WidgetType[]).map((t) => (
              <button key={t} onClick={() => setType(t)} style={{ ...CHIP, ...(type === t ? CHIP_ON : {}) }}>
                {WIDGET_LABELS[t]}
              </button>
            ))}
          </div>
        </Field>
      )}

      <Field label="Name (optional)">
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Custom widget name" style={INPUT} />
      </Field>

      {(type === "device" || type === "now_playing") && (
        <>
          {type === "device" && (
            <Field label="Domain">
              <Select value={domain} onChange={setDomain} options={[{ value: "light", label: "Light" }, { value: "media", label: "Media / TV" }, { value: "power", label: "Switch / Plug" }]} />
            </Field>
          )}
          <Field label="Device">
            <Select
              value={deviceId}
              onChange={setDeviceId}
              options={deviceSelectOptions((type === "now_playing" ? selMedia : devicesFor(domain)) as RoomedDevice[], rooms)}
              placeholder="Choose a device"
            />
          </Field>
        </>
      )}

      {(type === "group" || type === "button") && (
        <>
          <Field label="Controls">
            <Select
              value={groupDomain}
              onChange={(d) => { setGroupDomain(d); setGroupIds([]); }}
              options={[
                { value: "light", label: "Lights — brightness / colour" },
                { value: "media", label: "Media / speakers — volume" },
                { value: "power", label: "Switches / plugs — on/off" },
                { value: "all", label: "Any device — power on/off" },
              ]}
            />
          </Field>
          <Field label={type === "button" ? "Device or group (pick one for a single device)" : "Devices"}>
            <input
              value={groupQuery}
              onChange={(e) => setGroupQuery(e.target.value)}
              placeholder="Search…"
              style={{ ...INPUT, marginBottom: "0.4rem", fontSize: "0.82rem" }}
            />
            <div style={{ maxHeight: 240, overflowY: "auto", display: "flex", flexDirection: "column", gap: "0.1rem" }}>
              {groupByRoom(
                (groupDomain === "light"
                  ? selLights
                  : groupDomain === "media"
                    ? selMedia
                    : groupDomain === "power"
                      ? selPower
                      : [...selLights, ...selMedia, ...selPower]) as RoomedDevice[],
                rooms,
              )
                .map(({ room, devices }) => ({
                  room,
                  devices: groupQuery.trim()
                    ? devices.filter((d) => d.name.toLowerCase().includes(groupQuery.trim().toLowerCase()))
                    : devices,
                }))
                .filter(({ devices }) => devices.length > 0)
                .map(({ room, devices }) => (
                <div key={room}>
                  <div style={ROOM_HEADER}>{room}</div>
                  {devices.map((d) => (
                    <label key={d.id} style={CHECK_ROW}>
                      <input
                        type="checkbox"
                        checked={groupIds.includes(d.id)}
                        onChange={() =>
                          setGroupIds((cur) => (cur.includes(d.id) ? cur.filter((x) => x !== d.id) : [...cur, d.id]))
                        }
                      />
                      <span style={ELLIPSIS}>{d.name}</span>
                    </label>
                  ))}
                </div>
              ))}
            </div>
          </Field>
          {type === "button" && (
            <Field label="Icon">
              <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem", maxHeight: 120, overflowY: "auto" }}>
                <button
                  title="Auto (from the device)"
                  onClick={() => setButtonGlyph("")}
                  style={{ ...GLYPH_OPT, ...(buttonGlyph === "" ? CHIP_ON : {}), width: "auto", padding: "0 0.5rem", fontSize: "0.72rem" }}
                >
                  Auto
                </button>
                <GlyphGrid options={CONTROL_GLYPH_OPTIONS} value={buttonGlyph} onPick={setButtonGlyph} size={34} />
              </div>
            </Field>
          )}
        </>
      )}

      {type === "scene" && (
        <>
          <Field label="Action">
            <Segmented
              value={restoreHome ? "restore" : "scene"}
              onChange={(v) => setRestoreHome(v === "restore")}
              options={[{ value: "scene", label: "Apply a scene" }, { value: "restore", label: "Restore Home" }]}
            />
          </Field>
          {!restoreHome && (
            <Field label="Scene">
              <Select
                value={sceneId}
                onChange={setSceneId}
                placeholder="Choose a scene"
                options={[...scenes]
                  .sort((a, b) => {
                    const ga = a.room_name ?? "", gb = b.room_name ?? "";
                    if (ga !== gb) return ga === "" ? -1 : gb === "" ? 1 : ga.localeCompare(gb);
                    return a.name.localeCompare(b.name);
                  })
                  .map((s) => ({ value: s.id, label: s.name, group: s.room_name ?? "Home" }))}
              />
            </Field>
          )}
        </>
      )}

      {type === "control" && (
        <ControlEditor control={control} onChange={setControl} lights={lights} media={media} power={power} scenes={scenes} />
      )}

      {type === "sensor" && (
        <>
          <Field label="Device">
            <Select
              value={sensorDev}
              onChange={(v) => { setSensorDev(v); setSensorKey(""); }}
              options={generic.map((d) => ({ value: `${d.provider_id}|${d.device_id}`, label: d.name }))}
              placeholder={generic.length ? "Choose a device" : "No generic devices found"}
            />
          </Field>
          <Field label="Reading">
            <Select
              value={sensorKey}
              onChange={setSensorKey}
              options={
                generic
                  .find((d) => `${d.provider_id}|${d.device_id}` === sensorDev)
                  ?.controls.filter((c) => ["readout", "number", "toggle", "enum"].includes(c.type))
                  .map((c) => ({ value: c.key, label: c.label })) ?? []
              }
              placeholder="Choose a reading"
            />
          </Field>
        </>
      )}

      {type === "weather" && (
        <Field label="Weather entity">
          <Select
            value={sensorDev}
            onChange={setSensorDev}
            options={generic
              .filter((d) => d.kind === "weather")
              .map((d) => ({ value: `${d.provider_id}|${d.device_id}`, label: d.name }))}
            placeholder={
              generic.some((d) => d.kind === "weather")
                ? "Choose a weather entity"
                : "No weather entity found (add one in Home Assistant)"
            }
          />
        </Field>
      )}

      {type === "clock" && (
        <>
          <Field label="Format">
            <Segmented
              value={clockFormat}
              onChange={setClockFormat}
              options={[{ value: "24h", label: "24-hour" }, { value: "12h", label: "12-hour" }]}
            />
          </Field>
          <Field label="Size">
            <Segmented
              value={clockScale}
              onChange={setClockScale}
              options={[
                { value: "0.6", label: "S" },
                { value: "1", label: "M" },
                { value: "1.5", label: "L" },
                { value: "2.2", label: "XL" },
              ]}
            />
          </Field>
        </>
      )}

      {type === "label" && (
        <>
          <Field label="Text">
            <input value={labelText} onChange={(e) => setLabelText(e.target.value)} placeholder="e.g. Upstairs" style={INPUT} />
          </Field>
          <Field label="Style">
            <Segmented
              value={labelHeading ? "heading" : "plain"}
              onChange={(v) => setLabelHeading(v === "heading")}
              options={[{ value: "heading", label: "Heading" }, { value: "plain", label: "Plain" }]}
            />
          </Field>
        </>
      )}

      <div style={{ display: "flex", justifyContent: "flex-end", gap: "0.5rem", marginTop: "1rem" }}>
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
        <Button variant="primary" onClick={save}>{existing ? "Save" : "Add"}</Button>
      </div>
    </Modal>
  );
}

/** The reused room-control creator, home-wide: pick an action, a glyph, and the
 * devices (or a scene) it acts on. Produces a RoomControl used by the widget. */
function ControlEditor({
  control,
  onChange,
  lights,
  media,
  power,
  scenes,
}: {
  control: RoomControl;
  onChange: (c: RoomControl) => void;
  lights: Light[];
  media: MediaDevice[];
  power: PowerDevice[];
  scenes: Scene[];
}) {
  const kind = control.kind;
  // Only target controllable devices: skip disabled and de-dup/composite "merged-in"
  // copies (a shadowed duplicate or a media companion), hidden from control elsewhere.
  const selLights = lights.filter((l) => l.enabled !== false && !l.shadowed_by);
  const selMedia = media.filter((m) => m.enabled !== false && !m.shadowed_by && !m.companion_of);
  const selPower = power.filter((p) => p.enabled !== false && !p.shadowed_by);
  // Which domains a kind can target.
  const pool: { domain: "light" | "media" | "power"; list: { id: string; name: string }[] }[] =
    kind === "brightness"
      ? [{ domain: "light", list: selLights }]
      : kind === "volume"
        ? [{ domain: "media", list: selMedia }]
        : [
            { domain: "light", list: selLights },
            { domain: "media", list: selMedia },
            { domain: "power", list: selPower },
          ];
  const has = (domain: string, id: string) => control.targets.some((t) => t.domain === domain && t.id === id);
  const toggleTarget = (domain: "light" | "media" | "power", id: string) =>
    onChange({
      ...control,
      targets: has(domain, id)
        ? control.targets.filter((t) => !(t.domain === domain && t.id === id))
        : [...control.targets, { domain, id }],
    });

  return (
    <>
      <Field label="Action">
        <Select
          value={kind}
          onChange={(k) => onChange({ ...control, kind: k as RoomControl["kind"], targets: [] })}
          options={[{ value: "power", label: "Power toggle" }, { value: "brightness", label: "Brightness" }, { value: "volume", label: "Volume" }, { value: "scene", label: "Apply scene" }]}
        />
      </Field>
      <Field label="Glyph">
        <div style={{ maxHeight: 120, overflowY: "auto" }}>
          <GlyphGrid
            options={CONTROL_GLYPH_OPTIONS}
            value={control.glyph ?? null}
            onPick={(n) => onChange({ ...control, glyph: n })}
            size={34}
          />
        </div>
      </Field>
      <Field label="Label (optional)">
        <input
          value={control.label ?? ""}
          onChange={(e) => onChange({ ...control, label: e.target.value })}
          placeholder="e.g. Movie time"
          style={INPUT}
        />
      </Field>
      {kind === "scene" ? (
        <Field label="Scene">
          <Select
            value={control.scene_id ?? ""}
            onChange={(id) => onChange({ ...control, scene_id: id })}
            options={scenes.map((s) => ({ value: s.id, label: s.name }))}
            placeholder="Choose a scene"
          />
        </Field>
      ) : (
        <Field label="Devices">
          <div style={{ maxHeight: 180, overflowY: "auto", display: "flex", flexDirection: "column", gap: "0.25rem" }}>
            {pool.flatMap(({ domain, list }) =>
              list.map((d) => (
                <label key={`${domain}:${d.id}`} style={CHECK_ROW}>
                  <input type="checkbox" checked={has(domain, d.id)} onChange={() => toggleTarget(domain, d.id)} />
                  <span style={ELLIPSIS}>{d.name}</span>
                </label>
              )),
            )}
          </div>
        </Field>
      )}
    </>
  );
}

// ── small UI primitives ───────────────────────────────────────────────────────

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div style={{ marginBottom: "0.7rem" }}>
      <div style={{ fontSize: "0.72rem", color: T.dim, marginBottom: "0.3rem" }}>{label}</div>
      {children}
    </div>
  );
}

function EmptyState({ onCreate, text, cta = "+ Create board" }: { onCreate: () => void; text: string; cta?: string }) {
  return (
    <div style={{ ...S.card, alignItems: "center", textAlign: "center", padding: "3rem 1.5rem", gap: "1rem" }}>
      <div style={{ color: T.dim }}>{text}</div>
      <Button variant="primary" onClick={onCreate}>{cta}</Button>
    </div>
  );
}

const WIDGET_LABELS: Record<WidgetType, string> = {
  device: "Device tile",
  group: "Device group",
  now_playing: "Now playing",
  scene: "Scene button",
  control: "Custom control",
  button: "Button",
  sensor: "Sensor",
  weather: "Weather",
  clock: "Clock",
  label: "Label / heading",
  exit: "Exit",
};

const TAB: React.CSSProperties = {
  padding: "0.4rem 0.85rem",
  borderRadius: radius.md,
  border: `1px solid ${T.border}`,
  background: T.surface,
  color: T.dim,
  cursor: "pointer",
  fontSize: "0.85rem",
};
const CENTER: React.CSSProperties = {
  width: "100%",
  height: "100%",
  display: "flex",
  flexDirection: "column",
  alignItems: "center",
  justifyContent: "center",
  gap: "0.4rem",
};
const TILE_LABEL: React.CSSProperties = { fontSize: "0.82rem", fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", maxWidth: "100%" };
const ELLIPSIS: React.CSSProperties = { overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" };
const CORNER_BTN = (top: number, right: number): React.CSSProperties => ({
  position: "absolute",
  top,
  right,
  width: 22,
  height: 22,
  display: "grid",
  placeItems: "center",
  borderRadius: 6,
  border: `1px solid ${T.border}`,
  background: T.panel,
  color: T.dim,
  cursor: "pointer",
  fontSize: "0.95rem",
  lineHeight: 1,
  zIndex: 2,
});
const INPUT: React.CSSProperties = {
  width: "100%",
  padding: "0.45rem 0.55rem",
  borderRadius: radius.md,
  border: `1px solid ${T.border}`,
  background: T.surface,
  color: T.text,
  fontSize: "0.85rem",
};
const CHIP: React.CSSProperties = {
  padding: "0.35rem 0.7rem",
  borderRadius: radius.md,
  border: `1px solid ${T.border}`,
  background: T.surface,
  color: T.dim,
  cursor: "pointer",
  fontSize: "0.8rem",
};
const CHIP_ON: React.CSSProperties = { background: alpha(T.accent, 0.16), borderColor: T.accent, color: T.text };
const GLYPH_OPT: React.CSSProperties = {
  width: 34,
  height: 34,
  display: "grid",
  placeItems: "center",
  borderRadius: radius.md,
  border: `1px solid ${T.border}`,
  background: T.surface,
  color: T.dim,
  cursor: "pointer",
};
const CHECK_ROW: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: "0.5rem",
  fontSize: "0.82rem",
  color: T.text,
  padding: "0.2rem 0.1rem 0.2rem 0.6rem",
  cursor: "pointer",
};
const ROOM_HEADER: React.CSSProperties = {
  fontSize: "0.64rem",
  textTransform: "uppercase",
  letterSpacing: "0.06em",
  color: T.dim,
  margin: "0.5rem 0 0.15rem",
  fontWeight: 600,
};
