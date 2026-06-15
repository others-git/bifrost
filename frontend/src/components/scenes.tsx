// Shared scene primitives: presets, gradient rendering, and the scene editor.
// Used by both the Scenes page and the Floor Plan's room controller so the two
// stay visually and behaviourally identical.

import { useState } from "react";
import { ACCENT, S } from "../styles";
import { LightEditor } from "./LightEditor";
import { Modal } from "./dialogs";
import type { PaletteScene } from "../api";

export type ScenePreset = { name: string; brightness: number; palette: string[] };

/** Hue-style default scenes. Single-color presets read as a smooth wash;
 *  multi-color ones distribute round-robin across a room's lights. */
export const SCENE_PRESETS: ScenePreset[] = [
  { name: "Relax", brightness: 55, palette: ["#ffb46b"] },
  { name: "Read", brightness: 100, palette: ["#ffe4b3"] },
  { name: "Concentrate", brightness: 100, palette: ["#f4f7ff"] },
  { name: "Energize", brightness: 100, palette: ["#d6e8ff"] },
  { name: "Bright", brightness: 100, palette: ["#fff3df"] },
  { name: "Dimmed", brightness: 30, palette: ["#ffcf9a"] },
  { name: "Nightlight", brightness: 5, palette: ["#ff9b3d"] },
  { name: "Sunset", brightness: 75, palette: ["#ff7d33", "#ff5e9c", "#ffb04d"] },
  { name: "Tropical Twilight", brightness: 70, palette: ["#ff6f61", "#a86bd6", "#ffb347"] },
  { name: "Savanna Sunset", brightness: 80, palette: ["#ff9a3d", "#ff5e62", "#ffd194"] },
  { name: "Spring Blossom", brightness: 85, palette: ["#ffafbd", "#ffc3a0", "#f5a3c7"] },
  { name: "Aurora", brightness: 65, palette: ["#22d3ee", "#4ade80", "#8b5cf6"] },
  { name: "Ocean", brightness: 70, palette: ["#1fb6ff", "#0ea5e9", "#5eead4"] },
  { name: "Forest", brightness: 70, palette: ["#34d399", "#a3e635", "#16a34a"] },
];

/** CSS background for a scene's palette: a smooth wash for one color, a left-to-
 *  right gradient across many. Empty palette (brightness-only) reads as neutral. */
export function paletteGradient(palette: string[]): string {
  if (palette.length === 0) return "linear-gradient(135deg, #2b2b2b, #1c1c1c)";
  if (palette.length === 1) {
    const c = palette[0];
    return `linear-gradient(135deg, color-mix(in srgb, ${c} 88%, #fff), ${c} 55%, color-mix(in srgb, ${c} 78%, #000))`;
  }
  const stops = palette
    .map((c, i) => `${c} ${Math.round((i / (palette.length - 1)) * 100)}%`)
    .join(", ");
  return `linear-gradient(90deg, ${stops})`;
}

/** A gradient swatch summarising a palette. Replaces the old per-color dots. */
export function SceneSwatch({
  palette,
  width = 28,
  height = 14,
  radius = 4,
}: {
  palette: string[];
  width?: number | string;
  height?: number | string;
  radius?: number;
}) {
  return (
    <span
      aria-hidden
      style={{
        display: "inline-block",
        width,
        height,
        borderRadius: radius,
        background: paletteGradient(palette),
        boxShadow: "inset 0 0 0 1px rgba(255,255,255,0.08)",
        flexShrink: 0,
      }}
    />
  );
}

/** The pretty "open scenes" button shown inside a room's color editor. */
export function SceneButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        gap: "0.5rem",
        width: "100%",
        marginTop: "0.1rem",
        padding: "0.5rem",
        borderRadius: 10,
        border: "1px solid rgba(255,255,255,0.12)",
        color: "#f4ecda",
        cursor: "pointer",
        fontSize: "0.82rem",
        fontWeight: 600,
        letterSpacing: "0.02em",
        background:
          "linear-gradient(90deg, rgba(56,189,248,0.22), rgba(167,139,250,0.18) 45%, rgba(244,114,182,0.16))",
        boxShadow: "inset 0 1px 0 rgba(255,255,255,0.06)",
      }}
    >
      <span aria-hidden style={{ fontSize: "0.9rem" }}>✦</span>
      Scenes
    </button>
  );
}

/**
 * Modal listing every global scene as a gradient tile; clicking one applies it
 * to a room. The footer captures the room's current look as a new scene.
 * `onApply`/`onSave` are supplied by the caller so each page keeps its own
 * apply/capture behaviour.
 */
export function SceneModal({
  roomName,
  scenes,
  busy,
  onApply,
  onSave,
  onClose,
}: {
  roomName: string;
  scenes: PaletteScene[];
  busy: boolean;
  onApply: (sceneId: string) => void;
  onSave: () => void;
  onClose: () => void;
}) {
  return (
    <Modal title={`Scenes · ${roomName}`} onClose={onClose} width={460}>
      <p style={{ margin: "0.4rem 0 0.8rem", color: "#8c8676", fontSize: "0.82rem" }}>
        Apply a saved look to this room. Lights stay individually adjustable.
      </p>

      {scenes.length === 0 ? (
        <p style={{ color: "#777", fontSize: "0.85rem", margin: "0 0 0.8rem" }}>
          No scenes yet — save this room's current colors below, or build one on the Scenes page.
        </p>
      ) : (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(auto-fill, minmax(130px, 1fr))",
            gap: "0.55rem",
            marginBottom: "0.9rem",
          }}
        >
          {scenes.map((s) => (
            <button
              key={s.id}
              onClick={() => onApply(s.id)}
              disabled={busy}
              title={s.palette.length > 0 ? s.palette.join(" ") : "brightness only"}
              style={{
                display: "flex",
                flexDirection: "column",
                gap: "0.4rem",
                padding: "0.5rem",
                borderRadius: 10,
                border: "1px solid #2e2c26",
                background: "#1b1a16",
                color: "#e9e2d2",
                cursor: busy ? "default" : "pointer",
                textAlign: "left",
              }}
            >
              <SceneSwatch palette={s.palette} width={116} height={34} radius={6} />
              <span style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", gap: "0.4rem" }}>
                <span style={{ fontSize: "0.82rem", fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                  {s.name}
                </span>
                {s.brightness != null && (
                  <span style={{ fontSize: "0.68rem", color: "#7e7866" }}>{Math.round(s.brightness)}%</span>
                )}
              </span>
            </button>
          ))}
        </div>
      )}

      <button
        onClick={onSave}
        disabled={busy}
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          gap: "0.45rem",
          width: "100%",
          padding: "0.55rem",
          borderRadius: 10,
          border: "1px dashed #3a372e",
          background: "transparent",
          color: "#cfc7b2",
          cursor: busy ? "default" : "pointer",
          fontSize: "0.82rem",
        }}
      >
        + Save this room's look as a scene
      </button>
    </Modal>
  );
}

/** Inline editor: presets, name, brightness, palette, with a live gradient preview. */
export function SceneEditor({
  onSave,
  onCancel,
}: {
  onSave: (scene: { name: string; brightness?: number; palette: string[] }) => Promise<void>;
  onCancel?: () => void;
}) {
  const [name, setName] = useState("");
  const [brightness, setBrightness] = useState(80);
  const [palette, setPalette] = useState<string[]>(["#ff9900"]);
  const [saving, setSaving] = useState(false);
  const [colorEdit, setColorEdit] = useState<{ index: number; anchor: HTMLElement } | null>(null);

  function setColor(i: number, value: string) {
    setPalette((prev) => prev.map((c, j) => (j === i ? value : c)));
  }

  async function save() {
    if (!name.trim()) return;
    setSaving(true);
    try {
      await onSave({ name: name.trim(), brightness, palette });
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: "0.45rem",
        borderTop: "1px solid #2a2a2a",
        paddingTop: "0.5rem",
      }}
    >
      <div style={{ display: "flex", flexWrap: "wrap", gap: "0.25rem" }}>
        {SCENE_PRESETS.map((p) => (
          <button
            key={p.name}
            onClick={() => {
              setName(p.name);
              setBrightness(p.brightness);
              setPalette([...p.palette]);
            }}
            title={`Preset: ${p.palette.join(" ")} @ ${p.brightness}%`}
            style={{
              ...S.buttonGhost,
              padding: "0.2rem 0.45rem",
              fontSize: "0.72rem",
              display: "inline-flex",
              alignItems: "center",
              gap: "0.35rem",
            }}
          >
            <SceneSwatch palette={p.palette} width={18} height={10} />
            {p.name}
          </button>
        ))}
      </div>

      {/* Live preview of the scene as it will read in the list. */}
      <SceneSwatch palette={palette} width="100%" height={20} radius={6} />

      <input
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="Scene name"
        style={{ ...S.input, fontSize: "0.8rem" }}
      />

      <label
        style={{
          display: "flex",
          alignItems: "center",
          gap: "0.5rem",
          fontSize: "0.75rem",
          color: "#888",
        }}
      >
        Brightness
        <input
          type="range"
          min={1}
          max={100}
          value={brightness}
          onChange={(e) => setBrightness(Number(e.target.value))}
          style={{ flex: 1, accentColor: ACCENT }}
        />
        <span style={{ width: 32, textAlign: "right" }}>{brightness}%</span>
      </label>

      <div style={{ display: "flex", alignItems: "center", gap: "0.3rem", flexWrap: "wrap" }}>
        <span style={{ fontSize: "0.75rem", color: "#888" }}>Palette</span>
        {palette.map((c, i) => (
          <span key={i} style={{ display: "inline-flex", alignItems: "center" }}>
            <button
              onClick={(e) => setColorEdit({ index: i, anchor: e.currentTarget })}
              title="Edit color"
              style={{
                width: 26,
                height: 22,
                padding: 0,
                border: "1px solid #444",
                borderRadius: 4,
                background: c,
                cursor: "pointer",
              }}
            />
            {palette.length > 1 && (
              <button
                onClick={() => setPalette((prev) => prev.filter((_, j) => j !== i))}
                title="Remove color"
                style={{
                  background: "none",
                  border: "none",
                  color: "#866",
                  cursor: "pointer",
                  fontSize: "0.7rem",
                  padding: "0 0.15rem",
                }}
              >
                ×
              </button>
            )}
          </span>
        ))}
        {palette.length < 6 && (
          <button
            onClick={() => setPalette((prev) => [...prev, "#ffffff"])}
            style={{ ...S.buttonGhost, padding: "0.15rem 0.45rem", fontSize: "0.75rem" }}
          >
            +
          </button>
        )}
      </div>
      <span style={{ fontSize: "0.68rem", color: "#555" }}>
        Colors are spread across the room's lights in turn.
      </span>

      <div style={{ display: "flex", gap: "0.4rem" }}>
        <button
          onClick={save}
          disabled={saving || !name.trim()}
          style={{ ...S.button, padding: "0.35rem 0.7rem", fontSize: "0.78rem" }}
        >
          {saving ? "Saving…" : "Save scene"}
        </button>
        {onCancel && (
          <button
            onClick={onCancel}
            style={{ ...S.buttonGhost, padding: "0.35rem 0.7rem", fontSize: "0.78rem" }}
          >
            Cancel
          </button>
        )}
      </div>

      {colorEdit && (
        <LightEditor
          anchor={colorEdit.anchor}
          title="Palette color"
          initialHex={palette[colorEdit.index]}
          showBrightness={false}
          onChange={(ch) => { if (ch.field === "color") setColor(colorEdit.index, ch.hex); }}
          onClose={() => setColorEdit(null)}
        />
      )}
    </div>
  );
}
