// Shared scene primitives: the "open scenes" button shown in a room's color
// editor, and the per-room scene picker modal. A scene is a full-state snapshot
// (each light's color/temperature/effect + power on/off), so these just save the
// current room state and re-activate saved Room Scenes — no palette/preset model.

import { useState } from "react";
import { Modal } from "./dialogs";
import { Glyph } from "./glyphs";
import type { Scene } from "../api";

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
        minHeight: 44,
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
      <span aria-hidden style={{ display: "grid", placeItems: "center" }}><Glyph name="scene" size={15} /></span>
      Scenes
    </button>
  );
}

/**
 * Per-room scene picker: lists this room's saved Room Scenes (activate / delete)
 * and captures the room's **current full state** as a new one. `scenes` should
 * already be filtered to this room. The page supplies the apply/save/delete
 * behaviour so each surface keeps its own state handling.
 */
export function SceneModal({
  roomName,
  scenes,
  busy,
  onApply,
  onSave,
  onDelete,
  onClose,
}: {
  roomName: string;
  scenes: Scene[];
  busy: boolean;
  onApply: (sceneId: string) => void;
  onSave: (name: string) => void;
  onDelete: (sceneId: string) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState("");

  return (
    <Modal title={`Scenes · ${roomName}`} onClose={onClose} width={460}>
      <p style={{ margin: "0.4rem 0 0.8rem", color: "#8c8676", fontSize: "0.82rem" }}>
        Re-apply a saved snapshot of this room — exact colors, temperatures, and effects.
        Lights stay individually adjustable.
      </p>

      {scenes.length === 0 ? (
        <p style={{ color: "var(--bf-faint)", fontSize: "0.85rem", margin: "0 0 0.8rem" }}>
          No scenes for this room yet — set it the way you like, then save it below.
        </p>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.45rem", marginBottom: "0.9rem" }}>
          {scenes.map((s) => (
            <div
              key={s.id}
              style={{
                display: "flex",
                alignItems: "center",
                gap: "0.5rem",
                padding: "0.5rem 0.6rem",
                borderRadius: 10,
                border: "1px solid #2e2c26",
                background: "#1b1a16",
              }}
            >
              <button
                onClick={() => onApply(s.id)}
                disabled={busy}
                style={{
                  flex: 1,
                  display: "flex",
                  flexDirection: "column",
                  gap: "0.15rem",
                  background: "none",
                  border: "none",
                  color: "#e9e2d2",
                  cursor: busy ? "default" : "pointer",
                  textAlign: "left",
                }}
              >
                <span style={{ fontSize: "0.86rem", fontWeight: 600 }}>{s.name}</span>
                <span style={{ fontSize: "0.7rem", color: "#7e7866" }}>
                  {s.lights} light{s.lights === 1 ? "" : "s"}
                  {s.power > 0 ? ` · ${s.power} power` : ""}
                </span>
              </button>
              <button
                onClick={() => onDelete(s.id)}
                disabled={busy}
                title="Delete scene"
                style={{
                  background: "none",
                  border: "none",
                  color: "#9a6b5a",
                  cursor: busy ? "default" : "pointer",
                  fontSize: "1rem",
                  padding: "0 0.2rem",
                }}
              >
                ×
              </button>
            </div>
          ))}
        </div>
      )}

      <div style={{ display: "flex", gap: "0.4rem" }}>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="New scene name"
          onKeyDown={(e) => {
            if (e.key === "Enter" && name.trim() && !busy) {
              onSave(name.trim());
              setName("");
            }
          }}
          style={{
            flex: 1,
            padding: "0.55rem 0.7rem",
            borderRadius: 10,
            border: "1px solid #3a372e",
            background: "rgba(255,255,255,0.04)",
            color: "#e9e2d2",
            fontSize: "0.82rem",
            outline: "none",
          }}
        />
        <button
          onClick={() => {
            if (name.trim()) {
              onSave(name.trim());
              setName("");
            }
          }}
          disabled={busy || !name.trim()}
          style={{
            padding: "0.55rem 0.8rem",
            borderRadius: 10,
            border: "1px dashed #3a372e",
            background: "transparent",
            color: name.trim() ? "#cfc7b2" : "#6b6556",
            cursor: busy || !name.trim() ? "default" : "pointer",
            fontSize: "0.82rem",
            whiteSpace: "nowrap",
          }}
        >
          + Save room
        </button>
      </div>
    </Modal>
  );
}
