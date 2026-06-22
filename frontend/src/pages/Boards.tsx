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
  getLights,
  getMediaDevices,
  getPowerDevices,
  getScenes,
  lightHex,
  mergePatch,
  restoreDefaultHome,
  rgbToXy,
  setLightState,
  setMediaState,
  setPowerState,
  updateDashboard,
  type Dashboard,
  type GenericDevice,
  type Light,
  type LightState,
  type LightStatePatch,
  type MediaCommand,
  type MediaDevice,
  type PowerDevice,
  type RoomControl,
  type Scene,
  type Widget,
} from "../api";
import { DeviceControl } from "../components/DeviceControl";
import { LightEditor, hexToRgb, type LightControlChange } from "../components/LightEditor";
import { MediaEditor } from "../components/MediaControls";
import { InlineSlider } from "../components/InlineSlider";
import { RoomControlButton, GlyphButton } from "./Dashboard";
import { CornerFiligree } from "../components/ornament";
import { Button, Segmented } from "../components/controls";
import { Modal, useDialogs } from "../components/dialogs";
import { Select } from "../components/Select";
import { CONTROL_GLYPH_OPTIONS, Glyph, weatherGlyph, weatherLabel } from "../components/glyphs";
import { PageHeader } from "../components/PageHeader";
import { useViewport } from "../useViewport";
import { S } from "../styles";
import { alpha, color, labelType, nicheStyle, radius, T } from "../theme";

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

// A fixed-ratio grid: 24 columns × 14 rows, both axes scaled to the board's
// actual size. A layout is therefore device-independent — the same board fills a
// phone, a desktop, or a wall tablet proportionally, with every widget scaling to
// the available space rather than living at a fixed pixel size. (≈ square cells on
// a 16:9 screen.) Default widget sizes are in these grid units.
const COLS = 24;
const ROWS = 14;
const GAP = 8;
// Fixed row height only for the phone fallback's stacked, view-only list.
const ROW_H = 42;

type WidgetType = "device" | "group" | "now_playing" | "scene" | "control" | "sensor" | "weather" | "clock" | "label";

/** A locally-generated widget id. */
const newId = () => `w_${Math.random().toString(36).slice(2, 10)}`;

export function BoardsPage() {
  const { isMobile } = useViewport();
  const dialogs = useDialogs();

  const [boards, setBoards] = useState<Dashboard[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [edit, setEdit] = useState(false);
  const [kiosk, setKiosk] = useState(false);
  const [adding, setAdding] = useState(false);
  const [configuring, setConfiguring] = useState<Widget | null>(null);

  // Device fleet + scenes, fetched once and patched optimistically.
  const [lights, setLights] = useState<Light[]>([]);
  const [media, setMedia] = useState<MediaDevice[]>([]);
  const [power, setPower] = useState<PowerDevice[]>([]);
  const [scenes, setScenes] = useState<Scene[]>([]);
  const [generic, setGeneric] = useState<GenericDevice[]>([]);
  const [flyout, setFlyout] = useState<{ widget: Widget; anchor: HTMLElement } | null>(null);

  const reloadBoards = useCallback(async () => {
    const bs = await getDashboards();
    setBoards(bs);
    setActiveId((cur) => cur ?? bs[0]?.id ?? null);
  }, []);

  const reloadDevices = useCallback(async () => {
    const [l, m, p, s, g] = await Promise.all([
      getLights(),
      getMediaDevices(),
      getPowerDevices(),
      getScenes(),
      getGenericDevices(),
    ]);
    if (l !== "unauthorized") setLights(l);
    setMedia(m);
    setPower(p);
    setScenes(s);
    setGeneric(g);
  }, []);

  useEffect(() => {
    reloadBoards();
    reloadDevices();
  }, [reloadBoards, reloadDevices]);

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
    saveWidgets(widgets.map((w) => (w.id === id ? next : w)));
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
  async function newBoard() {
    const name = await dialogs.prompt({
      title: "New board",
      message: "Name this dashboard.",
      placeholder: "e.g. Living Room",
      confirmLabel: "Create",
    });
    if (!name?.trim()) return;
    const b = await createDashboard(name.trim());
    await reloadBoards();
    setActiveId(b.id);
    setEdit(true);
  }
  async function renameBoard() {
    if (!board) return;
    const name = await dialogs.prompt({
      title: "Rename board",
      message: "New name.",
      placeholder: board.name,
      confirmLabel: "Save",
    });
    if (!name?.trim()) return;
    await updateDashboard(board.id, { name: name.trim() });
    await reloadBoards();
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

  const renderWidget = (w: Widget) => (
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
                <Button variant="ghost" onClick={renameBoard}>Rename</Button>
                <Button
                  variant="ghost"
                  onClick={deleteBoard}
                  style={{ color: "#c77", borderColor: "#5a3636" }}
                >
                  Delete
                </Button>
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
              .sort((a, b) => a.y - b.y || a.x - b.x)
              .map((w) => (
                <div key={w.id} style={{ minHeight: ROW_H }}>
                  {renderWidget(w)}
                </div>
              ))}
          </div>
        ) : (
          <BoardGrid
            widgets={widgets}
            edit={edit}
            onChange={patchWidget}
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
          onClose={() => { setAdding(false); setConfiguring(null); }}
          onSave={(spec) => {
            if (configuring) {
              patchWidget(configuring.id, { ...configuring, ...spec });
            } else {
              // Place a new widget at the bottom-left, sized per type.
              const y = widgets.reduce((m, w) => Math.max(m, w.y + w.h), 0);
              addWidget({ id: newId(), x: 0, y, w: spec.w ?? 6, h: spec.h ?? 4, ...spec });
            }
            setAdding(false);
            setConfiguring(null);
          }}
        />
      )}

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

      {/* Kiosk: the board fills the whole screen (over the nav), view-only — for a
          wall tablet. Sits below fly-outs (z 60) so widget controls still open. */}
      {kiosk && board && (
        <div style={{ position: "fixed", inset: 0, zIndex: 40, background: color.void, display: "flex", flexDirection: "column", padding: "1.1rem" }}>
          <button
            onClick={() => setKiosk(false)}
            title="Exit kiosk"
            style={{ position: "absolute", top: 10, right: 14, zIndex: 1, padding: "0.35rem 0.7rem", borderRadius: radius.frame, border: `1px solid ${T.hairline}`, background: alpha(color.void, 0.6), color: T.dim, cursor: "pointer", fontSize: "0.8rem" }}
          >
            ✕ Exit
          </button>
          {widgets.length > 0 && (
            <BoardGrid
              widgets={widgets}
              edit={false}
              onChange={() => {}}
              onConfigure={() => {}}
              onRemove={() => {}}
              renderWidget={renderWidget}
            />
          )}
        </div>
      )}

      {dialogs.element}
    </div>
  );
}

// ── The drag-resize grid ──────────────────────────────────────────────────────

function BoardGrid({
  widgets,
  edit,
  onChange,
  onConfigure,
  onRemove,
  renderWidget,
}: {
  widgets: Widget[];
  edit: boolean;
  onChange: (id: string, next: Widget) => void;
  onConfigure: (w: Widget) => void;
  onRemove: (id: string) => void;
  renderWidget: (w: Widget) => React.ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 1200, h: 600 });
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    setSize({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, []);

  // Both cell dimensions are a ratio of the board's actual size, so a widget keeps
  // its proportions on any screen and the grid always fills the visible area. The
  // canvas matches the viewport; in edit mode it grows a little past it so there's
  // room to drop, and so a stray out-of-range widget stays reachable.
  const cellW = size.w / COLS;
  const cellH = size.h / ROWS;
  const usedRows = widgets.reduce((m, w) => Math.max(m, w.y + w.h), 0);
  const contentRows = edit ? Math.max(ROWS, usedRows) + 2 : Math.max(ROWS, usedRows);
  const canvasH = Math.max(size.h, contentRows * cellH);

  return (
    <div
      ref={ref}
      style={{ flex: 1, minHeight: 0, width: "100%", overflow: "auto", position: "relative", borderRadius: radius.lg }}
    >
      <div
        style={{
          position: "relative",
          width: "100%",
          height: canvasH,
          // Faint grid guides while editing.
          backgroundImage: edit
            ? `linear-gradient(${T.border} 1px, transparent 1px), linear-gradient(90deg, ${T.border} 1px, transparent 1px)`
            : undefined,
          backgroundSize: edit ? `${cellW}px ${cellH}px` : undefined,
        }}
      >
        {widgets.map((w) => (
          <WidgetBox
            key={w.id}
            w={w}
            cols={COLS}
            rows={ROWS}
            cellW={cellW}
            cellH={cellH}
            edit={edit}
            onChange={(next) => onChange(w.id, next)}
            onConfigure={() => onConfigure(w)}
            onRemove={() => onRemove(w.id)}
          >
            {renderWidget(w)}
          </WidgetBox>
        ))}
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
    setDrag({ mode, sx: e.clientX, sy: e.clientY, dx: 0, dy: 0 });
  }
  function move(e: React.PointerEvent) {
    if (!drag) return;
    setDrag({ ...drag, dx: e.clientX - drag.sx, dy: e.clientY - drag.sy });
  }
  function up() {
    if (!drag) return;
    const dCols = Math.round(drag.dx / cellW);
    const dRows = Math.round(drag.dy / cellH);
    if (drag.mode === "move") {
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
  return (
    <div
      onPointerMove={move}
      onPointerUp={up}
      onPointerCancel={up}
      onPointerDown={edit ? (e) => down(e, "move") : undefined}
      style={{
        position: "absolute",
        left: w.x * cellW + (movePx?.dx ?? 0),
        top: w.y * cellH + (movePx?.dy ?? 0),
        width: w.w * cellW - GAP + (sizePx?.dx ?? 0),
        height: w.h * cellH - GAP + (sizePx?.dy ?? 0),
        touchAction: edit ? "none" : undefined,
        cursor: edit ? (drag?.mode === "move" ? "grabbing" : "grab") : undefined,
        zIndex: drag ? 10 : 1,
        borderRadius: radius.frame,
        ...(edit ? { outline: `1px dashed ${T.hairline}`, boxShadow: drag ? "0 8px 24px -8px #000" : undefined } : {}),
      }}
    >
      {/* Content — non-interactive while editing so drags don't trigger controls.
          A size container so readout widgets can scale their type to the box. */}
      <div
        style={{
          width: "100%",
          height: "100%",
          overflow: "hidden",
          pointerEvents: edit ? "none" : "auto",
          containerType: "size",
        }}
      >
        {children}
      </div>

      {edit && (
        <>
          <button onClick={onConfigure} onPointerDown={(e) => e.stopPropagation()} title="Configure" style={CORNER_BTN(4, 28)}>
            <Glyph name="gear" size={13} />
          </button>
          <button onClick={onRemove} onPointerDown={(e) => e.stopPropagation()} title="Remove" style={{ ...CORNER_BTN(4, 4), color: "#c77" }}>
            ×
          </button>
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

  if (w.type === "group") {
    return (
      <GroupWidget
        cfg={cfg as { domain?: string; ids?: string[]; label?: string }}
        lights={lights}
        media={media}
        power={power}
        edit={edit}
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
    const scene = scenes.find((s) => s.id === cfg.scene_id);
    const label = (cfg.name as string) || (isRestore ? "Restore Home" : (scene?.name ?? "Scene"));
    return (
      <button
        disabled={edit}
        onClick={async () => {
          if (isRestore) await restoreDefaultHome();
          else if (scene) await activateScene(scene.id);
        }}
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
        <Glyph name={isRestore ? "restore" : "scene"} size={24} />
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
    if (light) {
      const s: LightState = { ...(light.last_state ?? { on: false }), on: !on };
      onLightUpdate(light.id, s);
      setLightState(light.id, s);
    } else if (mediaDev) {
      onMediaPatch(mediaDev.id, { power: !on });
      setMediaState(mediaDev.id, { power: !on });
    } else if (powerDev) {
      onPowerToggle(powerDev.id, !on);
    }
  }

  const dimmable = !!light?.capabilities.dimmable;
  const showSlider = dimmable || !!mediaDev;
  const sliderVal = Math.round((light?.last_state?.brightness ?? mediaDev?.state.volume ?? 0) as number);
  const onSlide = (v: number) => {
    if (light) onLightUpdate(light.id, { ...(light.last_state ?? { on: true }), on: true, brightness: v });
    else if (mediaDev) onMediaPatch(mediaDev.id, { volume: v });
  };
  const onCommit = (v: number) => {
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => {
      if (light) setLightState(light.id, { on: true, brightness: v });
      else if (mediaDev) setMediaState(mediaDev.id, { volume: v });
    }, 120);
  };

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
          <div style={{ flex: "0 0 40%", minHeight: 22, display: "flex" }}>
            <InlineSlider fill value={sliderVal} accent={accent} unit={mediaDev ? "" : "%"} onChange={onSlide} onCommit={onCommit} />
          </div>
        )}
      </div>
    </div>
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
  const time = now.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", hour12: !h24 });
  const date = now.toLocaleDateString(undefined, { weekday: "long", month: "short", day: "numeric" });
  return (
    <div style={{ ...widgetPlate(color.gold, false), display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: "0.15rem" }}>
      <CornerFiligree />
      <div style={{ fontSize: "clamp(1.4rem, 26cqmin, 5rem)", fontWeight: 600, color: T.text, fontVariantNumeric: "tabular-nums", letterSpacing: "0.02em", lineHeight: 1.05 }}>{time}</div>
      <div style={{ fontSize: "clamp(0.7rem, 8cqmin, 1.4rem)", color: T.dim }}>{date}</div>
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
  onLightUpdate,
  onMediaPatch,
  onPowerToggle,
}: {
  cfg: { domain?: string; ids?: string[]; label?: string; name?: string };
  lights: Light[];
  media: MediaDevice[];
  power: PowerDevice[];
  edit: boolean;
  onLightUpdate: (id: string, st: LightState) => void;
  onMediaPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
  onPowerToggle: (id: string, next: boolean) => void;
}) {
  const domain = cfg.domain ?? "light";
  const ids = cfg.ids ?? [];
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const commitTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  const tLights = domain === "light" ? lights.filter((l) => ids.includes(l.id)) : [];
  const tMedia = domain === "media" ? media.filter((m) => ids.includes(m.id)) : [];
  const tPower = domain === "power" ? power.filter((p) => ids.includes(p.id)) : [];
  const total = tLights.length + tMedia.length + tPower.length;
  const onCount =
    tLights.filter((l) => l.last_state?.on).length +
    tMedia.filter((m) => m.state.power).length +
    tPower.filter((p) => p.state.on).length;
  const anyOn = onCount > 0;
  const accent = domain === "light" ? T.accent : domain === "media" ? T.media : color.gold;
  const hasEditor = domain === "light" || domain === "media";

  function togglePower() {
    const next = !anyOn;
    for (const l of tLights) {
      const s: LightState = { ...(l.last_state ?? { on: false }), on: next };
      onLightUpdate(l.id, s);
      setLightState(l.id, s);
    }
    for (const m of tMedia) {
      onMediaPatch(m.id, { power: next });
      setMediaState(m.id, { power: next });
    }
    for (const p of tPower) onPowerToggle(p.id, next);
  }

  // Per-light cascade (mirrors the room-header cascade, fanned per device).
  function cascade(change: LightControlChange) {
    if (change.field === "effect") return;
    const updates: [string, LightState][] = [];
    for (const l of tLights) {
      const next: LightState = { ...(l.last_state ?? { on: true }), on: true };
      if (change.field === "brightness") {
        if (l.capabilities.dimmable) next.brightness = change.brightness;
      } else if (change.field === "color") {
        if (l.capabilities.color_rgb) {
          next.color = rgbToXy(...hexToRgb(change.hex));
          next.color_temp_mirek = undefined;
        }
      } else if (l.capabilities.color_temperature) {
        next.color_temp_mirek = change.mirek;
        next.color = undefined;
      }
      onLightUpdate(l.id, next);
      updates.push([l.id, next]);
    }
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => {
      for (const [id, s] of updates) setLightState(id, s);
    }, 200);
  }

  // Media cascade: the editor commits its own `device`; fan the rest.
  function fanMedia(id: string, patch: Partial<MediaDevice["state"]>) {
    const cmd: MediaCommand = {};
    if (patch.volume !== undefined) cmd.volume = patch.volume;
    if (patch.mute !== undefined) cmd.mute = patch.mute;
    if (patch.power !== undefined) cmd.power = patch.power;
    for (const d of tMedia) {
      onMediaPatch(d.id, patch);
      if (d.id !== id && Object.keys(cmd).length > 0) setMediaState(d.id, cmd);
    }
  }

  const lit = tLights.filter((l) => l.last_state?.on);
  const initHex = lit.length ? lightHex(lit[0]) : "#ffb84d";
  const initBrightness = lit.length
    ? Math.round(lit.reduce((s, l) => s + (l.last_state?.brightness ?? 100), 0) / lit.length)
    : 100;
  const initMirek = lit.map((l) => l.last_state?.color_temp_mirek).find((m): m is number => m != null) ?? 366;

  const kindWord = domain === "light" ? "light" : domain === "media" ? "speaker" : "switch";
  const label = cfg.name || cfg.label || `${total} ${kindWord}${total !== 1 ? "s" : ""}`;

  // Inline master controls — brightness for a dimmable light group, volume for a
  // media group; both fan to every member (cascade handles its own debounce).
  const groupDimmable = tLights.some((l) => l.capabilities.dimmable);
  const groupVolume = tMedia.length ? Math.round(tMedia.reduce((s, m) => s + (m.state.volume ?? 0), 0) / tMedia.length) : 0;
  const showSlider = (domain === "light" && groupDimmable && total > 0) || (domain === "media" && tMedia.length > 0);
  const slideBrightness = (v: number) => cascade({ field: "brightness", brightness: v });
  const slideVolumeChange = (v: number) => {
    for (const m of tMedia) onMediaPatch(m.id, { volume: v, power: true });
  };
  const slideVolumeCommit = (v: number) => {
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => {
      for (const m of tMedia) setMediaState(m.id, { volume: v });
    }, 120);
  };

  const litHexes = anyOn ? (domain === "light" && lit.length ? lit.map(lightHex) : [accent]) : undefined;
  const dotColor = litHexes?.[0] ?? accent;
  return (
    <div
      style={{
        ...widgetPlate(accent, anyOn),
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
        <GlyphButton on={anyOn} accent={accent} title="Toggle all" active={false} buttonRef={null} onClick={togglePower} size={32}>
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
            <span style={{ fontSize: "0.78rem", color: anyOn ? accent : T.faint }}>
              {total === 0 ? "No devices" : `${onCount} of ${total} on`}
            </span>
          )}
        </div>
        {showSlider && (
          <div style={{ flex: "0 0 40%", minHeight: 26, display: "flex" }}>
            {domain === "light" ? (
              <InlineSlider fill value={initBrightness} accent={accent} unit="%" onChange={slideBrightness} onCommit={() => {}} />
            ) : (
              <InlineSlider fill value={groupVolume} accent={accent} unit="" onChange={slideVolumeChange} onCommit={slideVolumeCommit} />
            )}
          </div>
        )}
      </div>
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
          on={anyOn}
          onToggle={togglePower}
          onChange={cascade}
          onClose={() => setOpen(false)}
        />
      )}
      {open && domain === "media" && tMedia[0] && ref.current && (
        <MediaEditor device={tMedia[0]} anchor={ref.current} onLocalPatch={fanMedia} onClose={() => setOpen(false)} />
      )}
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

// ── Add / configure widget modal ──────────────────────────────────────────────

type WidgetSpec = Partial<Pick<Widget, "type" | "config" | "w" | "h">> & { type: WidgetType; config: unknown };

function WidgetEditorModal({
  existing,
  lights,
  media,
  power,
  scenes,
  generic,
  onClose,
  onSave,
}: {
  existing: Widget | null;
  lights: Light[];
  media: MediaDevice[];
  power: PowerDevice[];
  scenes: Scene[];
  generic: GenericDevice[];
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

  // clock / label
  const [clockFormat, setClockFormat] = useState<string>((cfg.format as string) ?? "24h");
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

  const devicesFor = (d: string): { id: string; name: string }[] =>
    d === "light" ? lights : d === "power" ? power : media;

  function save() {
    const nm = name.trim() || undefined;
    if (type === "device" || type === "now_playing") {
      const list = type === "now_playing" ? media : devicesFor(domain);
      const id = deviceId || list[0]?.id;
      if (!id) return;
      onSave({
        type,
        config: { domain: type === "now_playing" ? "media" : domain, id, name: nm },
        w: type === "now_playing" ? 8 : 6,
        h: 4,
      });
    } else if (type === "group") {
      if (groupIds.length === 0) return;
      onSave({ type, config: { domain: groupDomain, ids: groupIds, name: nm }, w: 8, h: 4 });
    } else if (type === "scene") {
      onSave({
        type,
        config: { ...(restoreHome ? { restore_home: true } : { scene_id: sceneId }), name: nm },
        w: 6,
        h: 2,
      });
    } else if (type === "sensor") {
      const [pid, did] = sensorDev.split("|");
      if (!pid || !did || !sensorKey) return;
      onSave({ type, config: { provider_id: pid, device_id: did, key: sensorKey, name: nm }, w: 4, h: 4 });
    } else if (type === "weather") {
      const [pid, did] = sensorDev.split("|");
      if (!pid || !did) return;
      onSave({ type, config: { provider_id: pid, device_id: did, name: nm }, w: 6, h: 4 });
    } else if (type === "clock") {
      onSave({ type, config: { format: clockFormat, name: nm }, w: 6, h: 4 });
    } else if (type === "label") {
      onSave({ type, config: { text: labelText.trim() || nm || "Label", heading: labelHeading }, w: 8, h: 2 });
    } else {
      onSave({ type, config: { ...control, name: nm }, w: 4, h: 4 });
    }
  }

  return (
    <Modal title={existing ? "Configure widget" : "Add widget"} onClose={onClose}>
      {!existing && (
        <Field label="Type">
          <div style={{ display: "flex", flexWrap: "wrap", gap: "0.4rem" }}>
            {(["device", "group", "now_playing", "scene", "control", "sensor", "weather", "clock", "label"] as WidgetType[]).map((t) => (
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
              options={(type === "now_playing" ? media : devicesFor(domain)).map((d) => ({ value: d.id, label: d.name }))}
              placeholder="Choose a device"
            />
          </Field>
        </>
      )}

      {type === "group" && (
        <>
          <Field label="Domain">
            <Select
              value={groupDomain}
              onChange={(d) => { setGroupDomain(d); setGroupIds([]); }}
              options={[{ value: "light", label: "Lights" }, { value: "media", label: "Media / speakers" }, { value: "power", label: "Switches / plugs" }]}
            />
          </Field>
          <Field label="Devices">
            <div style={{ maxHeight: 200, overflowY: "auto", display: "flex", flexDirection: "column", gap: "0.25rem" }}>
              {(groupDomain === "light" ? lights : groupDomain === "media" ? media : power).map((d) => (
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
          </Field>
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
              <Select value={sceneId} onChange={setSceneId} options={scenes.map((s) => ({ value: s.id, label: s.name }))} placeholder="Choose a scene" />
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
        <Field label="Format">
          <Segmented
            value={clockFormat}
            onChange={setClockFormat}
            options={[{ value: "24h", label: "24-hour" }, { value: "12h", label: "12-hour" }]}
          />
        </Field>
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
  // Which domains a kind can target.
  const pool: { domain: "light" | "media" | "power"; list: { id: string; name: string }[] }[] =
    kind === "brightness"
      ? [{ domain: "light", list: lights }]
      : kind === "volume"
        ? [{ domain: "media", list: media }]
        : [
            { domain: "light", list: lights },
            { domain: "media", list: media },
            { domain: "power", list: power },
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
        <div style={{ display: "flex", flexWrap: "wrap", gap: "0.3rem", maxHeight: 92, overflowY: "auto" }}>
          {CONTROL_GLYPH_OPTIONS.map((g) => (
            <button
              key={g.name}
              title={g.label}
              onClick={() => onChange({ ...control, glyph: g.name })}
              style={{ ...GLYPH_OPT, ...(control.glyph === g.name ? CHIP_ON : {}) }}
            >
              <Glyph name={g.name} size={18} />
            </button>
          ))}
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
  sensor: "Sensor",
  weather: "Weather",
  clock: "Clock",
  label: "Label / heading",
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
  padding: "0.2rem 0.1rem",
  cursor: "pointer",
};
