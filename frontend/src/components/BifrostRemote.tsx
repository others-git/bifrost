// Smart-remote pieces for a TV / streamer — a `useRemote` hook plus two embeddable
// panels: `RemotePad` (the canonical keypad — circular D-pad + OK, then nav,
// volume, and transport rows) and `RemoteApps` (the launchable app grid). These
// are composed into the unified "AIO TV Control" fly-out (see MediaControls'
// MediaEditor); all driven through the shared remote API (session → the same
// service layer as v1/MCP).

import { useEffect, useState } from "react";
import {
  getRemoteApps,
  getRemoteCommands,
  getRemoteState,
  sendRemoteCommand,
  setRemoteAppPin,
  type RemoteApp,
  type RemoteCommand,
  type RemoteCommandInfo,
  type RemoteKey,
} from "../api";
import { T, ACCENT, alpha } from "../theme";
import { Glyph } from "./glyphs";

/** Live apps + foreground app for `remoteId`, with the command helpers. Polls the
 * current app on a short interval so the launchable grid's highlight stays fresh. */
export function useRemote(remoteId: string) {
  const [currentApp, setCurrentApp] = useState<string | undefined>(undefined);
  const [apps, setApps] = useState<RemoteApp[]>([]);

  useEffect(() => {
    let alive = true;
    const load = () =>
      getRemoteState(remoteId).then((s) => {
        if (alive && s) setCurrentApp(s.current_app);
      });
    load();
    getRemoteApps(remoteId).then((a) => {
      if (alive) setApps(a);
    });
    const t = setInterval(load, 4000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [remoteId]);

  const send = (cmd: RemoteCommand) =>
    sendRemoteCommand(remoteId, cmd).then((err) => {
      if (err) console.warn("remote command failed:", err);
    });
  const press = (k: RemoteKey) => () => send({ key: { key: k } });
  async function togglePin(app: RemoteApp) {
    await setRemoteAppPin(remoteId, app.package, !app.pinned);
    setApps(await getRemoteApps(remoteId));
  }

  return { currentApp, apps, send, press, togglePin };
}

/** The keypad — circular D-pad + OK on top, then the nav and transport rows
 * (Prev · Play/Pause · Next). Volume lives above the keypad as a slider, not here. */
export function RemotePad({ press }: { press: (k: RemoteKey) => () => void }) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.9rem" }}>
      {/* Circular D-pad */}
      <div
        style={{
          position: "relative",
          width: 196,
          height: 196,
          alignSelf: "center",
          borderRadius: "50%",
          background: "radial-gradient(circle at 50% 38%, rgba(255,255,255,0.06), rgba(255,255,255,0.015) 70%)",
          border: `1px solid ${T.border}`,
          boxShadow: "inset 0 1px 0 rgba(255,255,255,0.06), 0 8px 20px rgba(0,0,0,0.45)",
        }}
      >
        <DpadBtn dir="up" onClick={press("up")} />
        <DpadBtn dir="left" onClick={press("left")} />
        <DpadBtn dir="right" onClick={press("right")} />
        <DpadBtn dir="down" onClick={press("down")} />
        <button
          onClick={press("select")}
          title="OK / Select"
          aria-label="OK / Select"
          style={{
            position: "absolute",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            width: 74,
            height: 74,
            borderRadius: "50%",
            border: `1px solid ${alpha(ACCENT, 0.4)}`,
            background: "radial-gradient(circle at 50% 35%, rgba(56,189,248,0.22), rgba(56,189,248,0.05))",
            color: "#fff",
            fontWeight: 700,
            fontSize: "0.92rem",
            letterSpacing: "0.04em",
            cursor: "pointer",
            boxShadow: "0 2px 12px rgba(0,0,0,0.45), inset 0 1px 0 rgba(255,255,255,0.14)",
          }}
        >
          OK
        </button>
      </div>

      {/* Nav */}
      <Row>
        <Key glyph="back" label="Back" onClick={press("back")} />
        <Key glyph="home" label="Home" onClick={press("home")} />
        <Key glyph="menu" label="Menu" onClick={press("menu")} />
      </Row>

      {/* Transport */}
      <Row>
        <Key glyph="prev" label="Previous" onClick={press("previous")} />
        <Key glyph="play_pause" label="Play / pause" onClick={press("play_pause")} />
        <Key glyph="next" label="Next" onClick={press("next")} />
      </Row>
    </div>
  );
}

/** The expanded ("full") remote — every native command the device exposes (a
 * Bravia's IRCC catalogue), as a raw passthrough grid that slides open beneath the
 * keypad. Renders nothing when the device has no catalogue. */
export function ExpandedRemote({
  remoteId,
  send,
}: {
  remoteId: string;
  send: (cmd: RemoteCommand) => void;
}) {
  const [commands, setCommands] = useState<RemoteCommandInfo[]>([]);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    let alive = true;
    getRemoteCommands(remoteId).then((c) => {
      if (alive) setCommands(c);
    });
    return () => {
      alive = false;
    };
  }, [remoteId]);

  if (commands.length === 0) return null;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.5rem" }}>
      <button
        onClick={() => setOpen((v) => !v)}
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          width: "100%",
          padding: "0.45rem 0.7rem",
          borderRadius: 10,
          border: `1px solid ${T.border}`,
          background: alpha(ACCENT, 0.06),
          color: T.dim,
          cursor: "pointer",
          fontSize: "0.8rem",
        }}
      >
        <span>Full remote · {commands.length}</span>
        <span style={{ display: "grid", transform: open ? "rotate(180deg)" : "none", transition: "transform 0.15s" }}>
          <Glyph name="chevron" size={16} />
        </span>
      </button>
      {open && (
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(72px, 1fr))", gap: "0.4rem" }}>
          {commands.map((c) => (
            <button
              key={c.token}
              onClick={() => send({ native: { token: c.token } })}
              title={c.name}
              style={{
                minHeight: 38,
                padding: "0.3rem 0.4rem",
                borderRadius: 9,
                border: `1px solid ${T.border}`,
                background: T.surface,
                color: T.text,
                cursor: "pointer",
                fontSize: "0.72rem",
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
            >
              {c.name}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** The launchable app grid (pinned + recents): tap to launch, ★ to pin. */
export function RemoteApps({
  apps,
  currentApp,
  onLaunch,
  onPin,
}: {
  apps: RemoteApp[];
  currentApp?: string;
  onLaunch: (pkg: string) => void;
  onPin: (app: RemoteApp) => void;
}) {
  if (apps.length === 0) {
    return (
      <div style={{ fontSize: "0.8rem", color: T.faint }}>
        No apps yet — they appear here as you open them on the TV.
      </div>
    );
  }
  return (
    <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(72px, 1fr))", gap: "0.5rem" }}>
      {apps.map((a) => {
        const active = a.package === currentApp;
        return (
          <div key={a.package} style={{ position: "relative" }}>
            <button
              onClick={() => onLaunch(a.package)}
              title={`Launch ${a.name}`}
              style={{
                width: "100%",
                minHeight: 66,
                borderRadius: 12,
                border: `1px solid ${active ? ACCENT : T.border}`,
                background: active ? `${alpha(ACCENT, 0.1)}` : T.surface,
                color: T.text,
                cursor: "pointer",
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                gap: "0.35rem",
                padding: "0.55rem 0.35rem 0.45rem",
              }}
            >
              <span
                style={{
                  width: 28,
                  height: 28,
                  borderRadius: "50%",
                  display: "grid",
                  placeItems: "center",
                  fontSize: "0.85rem",
                  fontWeight: 700,
                  background: active ? ACCENT : "rgba(255,255,255,0.08)",
                  color: active ? "#0b1220" : T.text,
                }}
              >
                {a.name.charAt(0).toUpperCase()}
              </span>
              <span style={{ fontSize: "0.72rem", maxWidth: "100%", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                {a.name}
              </span>
            </button>
            <button
              onClick={() => onPin(a)}
              title={a.pinned ? "Unpin" : "Pin"}
              aria-label={a.pinned ? "Unpin" : "Pin"}
              style={{ position: "absolute", top: 3, right: 3, background: "none", border: "none", cursor: "pointer", color: a.pinned ? ACCENT : T.faint, padding: 2, display: "grid", placeItems: "center", lineHeight: 1 }}
            >
              <Glyph name={a.pinned ? "star_fill" : "star"} size={13} />
            </button>
          </div>
        );
      })}
    </div>
  );
}

/** A centred row of remote keys. */
function Row({ children }: { children: React.ReactNode }) {
  return <div style={{ display: "flex", justifyContent: "center", gap: "0.7rem" }}>{children}</div>;
}

/** One directional button on the circular D-pad — a themed chevron glyph, rotated
 * per direction (transparent, blends into the pad). */
function DpadBtn({ dir, onClick }: { dir: "up" | "down" | "left" | "right"; onClick: () => void }) {
  const label = { up: "Up", down: "Down", left: "Left", right: "Right" }[dir];
  const rot = { up: 180, down: 0, left: 90, right: -90 }[dir]; // chevron points down by default
  const pos: React.CSSProperties =
    dir === "up"
      ? { top: 8, left: "50%", transform: "translateX(-50%)" }
      : dir === "down"
        ? { bottom: 8, left: "50%", transform: "translateX(-50%)" }
        : dir === "left"
          ? { left: 10, top: "50%", transform: "translateY(-50%)" }
          : { right: 10, top: "50%", transform: "translateY(-50%)" };
  return (
    <button
      onClick={onClick}
      title={label}
      aria-label={label}
      style={{
        position: "absolute",
        ...pos,
        width: 46,
        height: 46,
        borderRadius: 14,
        border: "none",
        background: "transparent",
        color: T.dim,
        cursor: "pointer",
        display: "grid",
        placeItems: "center",
      }}
    >
      <span style={{ display: "grid", transform: `rotate(${rot}deg)` }}>
        <Glyph name="chevron" size={22} />
      </span>
    </button>
  );
}

/** A single remote key — a themed glyph in a rounded tile. */
function Key({ glyph, label, onClick }: { glyph: string; label: string; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      title={label}
      aria-label={label}
      style={{
        width: 46,
        height: 46,
        borderRadius: 13,
        border: `1px solid ${T.border}`,
        background: T.surface,
        color: T.text,
        cursor: "pointer",
        display: "grid",
        placeItems: "center",
        flexShrink: 0,
        boxShadow: "inset 0 1px 0 rgba(255,255,255,0.05)",
      }}
    >
      <Glyph name={glyph} size={20} />
    </button>
  );
}
