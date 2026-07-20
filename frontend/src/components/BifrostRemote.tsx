// Smart-remote pieces for a TV / streamer — a `useRemote` hook plus embeddable
// panels: `KeysPad` (the engraved cross-keys plate, plus nav/transport rows),
// `ScryPad` (the full-bleed Scrying Glass gesture slab, plus a nav row) and
// `RemoteApps` (the launchable app grid). These are composed into the unified
// "AIO TV Control" fly-out as three peer tabs (see MediaControls' MediaEditor
// / TvAio) — Keys and Scry are alternative NAVIGATION surfaces, not a mode
// toggle on one panel, so Scry can own the whole fly-out's surface area
// without a keys plate competing for room. All driven through the shared
// remote API (session → the same service layer as v1/MCP).

import { useEffect, useState } from "react";
import { useViewport } from "../useViewport";
import {
  getRemoteApps,
  getRemoteCommands,
  getRemoteState,
  sendRemoteCommand,
  setRemoteAppPin,
  setRemoteCommandPin,
  type RemoteApp,
  type RemoteCommand,
  type RemoteCommandInfo,
  type RemoteKey,
  assistantSay,
} from "../api";
import { T, ACCENT, alpha, color, font, gildedRule, glow, radius } from "../theme";
import { Glyph } from "./glyphs";
import { CornerFiligree } from "./ornament";
import { ScryingGlass } from "./ScryingGlass";

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

/** Back / Home / Menu — the three discrete keys every navigation surface keeps
 * outside itself (a gesture flick or a D-pad tap can't express them). */
function NavRow({ onKey }: { onKey: (k: RemoteKey) => void }) {
  return (
    <Row>
      <KeyNiche glyph="back" label="Back" onClick={() => onKey("back")} />
      <KeyNiche glyph="home" label="Home" onClick={() => onKey("home")} />
      <KeyNiche glyph="menu" label="Menu" onClick={() => onKey("menu")} />
    </Row>
  );
}

/** The Keys plate: cross-keys D-pad, nav row, and transport — an engraved
 * panel for anyone who wants discrete tap targets (a mouse, or fingers that
 * prefer buttons to gestures). One of the two peer navigation tabs; see
 * `ScryPad` for the gesture alternative. */
export function KeysPad({ press }: { press: (k: RemoteKey) => () => void }) {
  const onKey = (k: RemoteKey) => press(k)();
  return (
    <div
      style={{
        position: "relative",
        borderRadius: radius.frame,
        border: `1px solid ${T.cardBorder}`,
        background: alpha(color.text, 0.02),
        padding: "0.8rem",
        display: "flex",
        flexDirection: "column",
        gap: "0.75rem",
      }}
    >
      <CornerFiligree />
      <CrossKeys onKey={onKey} />
      <div aria-hidden style={{ height: 1, background: gildedRule, opacity: 0.5 }} />
      <NavRow onKey={onKey} />
      <Row>
        <KeyNiche glyph="prev" label="Previous" onClick={() => onKey("previous")} />
        <KeyNiche glyph="play_pause" label="Play / pause" onClick={() => onKey("play_pause")} />
        <KeyNiche glyph="next" label="Next" onClick={() => onKey("next")} />
      </Row>
    </div>
  );
}

/** The Scrying Glass plate: the gesture slab filling all the height its parent
 * gives it, with just the nav row beneath — "eyes on the TV, not on the
 * phone" means nothing else competes for the surface. The caller (`TvAio`)
 * is what actually maximizes that parent height when this tab is open. */
export function ScryPad({ press }: { press: (k: RemoteKey) => () => void }) {
  const onKey = (k: RemoteKey) => press(k)();
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.7rem", flex: 1, minHeight: 0 }}>
      {/* `flex: 1` fills a bounded ancestor (the mobile fly-out's maximized
          sheet); `minHeight` is the floor everywhere else (desktop's popover
          and the tablet modal size to content, so there's no space to grow
          into) — still a big, deliberately "maximized" glass either way. */}
      <div style={{ flex: 1, minHeight: "min(56vh, 480px)", display: "grid" }}>
        <ScryingGlass onKey={onKey} height="100%" />
      </div>
      <NavRow onKey={onKey} />
    </div>
  );
}

/** The Keys mode: a plus-shaped cross of five engraved keys — four chamfered
 * direction niches around a raised violet OK signet. Angular, no dial. */
function CrossKeys({ onKey }: { onKey: (k: RemoteKey) => void }) {
  const cell = 58;
  const dirs: { k: RemoteKey; rot: number; area: string; label: string }[] = [
    { k: "up", rot: 180, area: "1 / 2", label: "Up" },
    { k: "left", rot: 90, area: "2 / 1", label: "Left" },
    { k: "right", rot: -90, area: "2 / 3", label: "Right" },
    { k: "down", rot: 0, area: "3 / 2", label: "Down" },
  ];
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: `repeat(3, ${cell}px)`,
        gridTemplateRows: `repeat(3, ${cell}px)`,
        gap: 7,
        justifyContent: "center",
        alignSelf: "center",
      }}
    >
      {dirs.map((d) => (
        <div key={d.k} style={{ gridArea: d.area, display: "grid" }}>
          <KeyNiche glyph="chevron" rotate={d.rot} label={d.label} square onClick={() => onKey(d.k)} />
        </div>
      ))}
      <button
        onClick={() => onKey("select")}
        title="OK / Select"
        aria-label="OK / Select"
        style={{
          gridArea: "2 / 2",
          width: cell,
          height: cell,
          borderRadius: radius.sm,
          border: `1px solid ${alpha(color.violet, 0.55)}`,
          background: `radial-gradient(circle at 50% 32%, ${alpha(color.violet, 0.28)}, ${alpha(color.violet, 0.08)} 75%)`,
          boxShadow: `${glow(color.violet, 16)}, inset 0 1px 0 rgba(255,255,255,0.14)`,
          color: color.text,
          fontFamily: font.display,
          fontWeight: 700,
          fontSize: "0.86rem",
          letterSpacing: "0.1em",
          cursor: "pointer",
        }}
      >
        OK
      </button>
    </div>
  );
}

/** One engraved remote key — an inset niche that lights violet from within
 * while pressed (material response, never a gray highlight). */
function KeyNiche({
  glyph,
  label,
  onClick,
  rotate = 0,
  square = false,
}: {
  glyph: string;
  label: string;
  onClick: () => void;
  rotate?: number;
  square?: boolean;
}) {
  const [lit, setLit] = useState(false);
  return (
    <button
      onClick={onClick}
      onPointerDown={() => setLit(true)}
      onPointerUp={() => setLit(false)}
      onPointerLeave={() => setLit(false)}
      onPointerCancel={() => setLit(false)}
      title={label}
      aria-label={label}
      style={{
        width: square ? "100%" : undefined,
        height: square ? "100%" : 48,
        flex: square ? undefined : 1,
        maxWidth: square ? undefined : 120,
        minWidth: 44,
        minHeight: 44,
        borderRadius: radius.sm,
        border: `1px solid ${lit ? alpha(color.violet, 0.6) : T.cardBorder}`,
        background: lit
          ? `radial-gradient(circle at 50% 40%, ${alpha(color.violet, 0.22)}, transparent 75%), rgba(0,0,0,0.35)`
          : "rgba(0,0,0,0.3)",
        boxShadow: lit
          ? `inset 0 0 14px -4px ${alpha(color.violet, 0.7)}`
          : "inset 0 2px 6px rgba(0,0,0,0.5), inset 0 -1px 0 rgba(255,255,255,0.04)",
        color: lit ? color.violet : T.dim,
        cursor: "pointer",
        display: "grid",
        placeItems: "center",
        transition: "color 0.12s, border-color 0.12s, box-shadow 0.12s, background 0.12s",
      }}
    >
      <span style={{ display: "grid", transform: rotate ? `rotate(${rotate}deg)` : undefined }}>
        <Glyph name={glyph} size={20} />
      </span>
    </button>
  );
}

/** A text-entry row — types literal text into the TV's focused field (search
 * boxes, login forms). Enter or the send button submits, then clears. Renders
 * for any remote whose provider implements text input (Bravia, HA). */
export function RemoteTextEntry({ send }: { send: (cmd: RemoteCommand) => void }) {
  const [text, setText] = useState("");
  const submit = () => {
    const t = text.trim();
    if (!t) return;
    send({ text: { text: t } });
    setText("");
  };
  return (
    <div style={{ display: "flex", gap: "0.5rem" }}>
      <input
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") submit();
        }}
        placeholder="Type on TV…"
        aria-label="Send text to TV"
        style={{
          flex: 1,
          minHeight: 44,
          padding: "0 0.8rem",
          borderRadius: 11,
          border: `1px solid ${T.border}`,
          background: T.surface,
          color: T.text,
          fontSize: "0.85rem",
          outline: "none",
        }}
      />
      <button
        onClick={submit}
        disabled={!text.trim()}
        title="Send text"
        aria-label="Send text"
        style={{
          width: 44,
          minHeight: 44,
          borderRadius: 11,
          border: `1px solid ${alpha(ACCENT, 0.4)}`,
          background: text.trim() ? alpha(ACCENT, 0.12) : T.surface,
          color: text.trim() ? ACCENT : T.faint,
          cursor: text.trim() ? "pointer" : "default",
          display: "grid",
          placeItems: "center",
        }}
      >
        <Glyph name="send" size={18} />
      </button>
    </div>
  );
}

/** Speak a phrase into the TV's own voice assistant — Bifrost synthesizes it
 * and streams it to the TV, which hears and acts on it ("play Bob's Burgers",
 * "what's the weather"). The whole Assistant, driven by text. */
export function AssistantSay({ deviceId }: { deviceId: string }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState("");
  const submit = async () => {
    const t = text.trim();
    if (!t || busy) return;
    setBusy(true);
    setMsg("");
    const err = await assistantSay(deviceId, t);
    setBusy(false);
    if (err) {
      setMsg(err);
    } else {
      setMsg("Sent to the TV's assistant.");
      setText("");
    }
  };
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.35rem" }}>
      <span style={{ fontSize: "0.7rem", letterSpacing: "0.06em", textTransform: "uppercase", color: T.dim }}>
        Ask the TV's assistant
      </span>
      <div style={{ display: "flex", gap: "0.5rem" }}>
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
          placeholder="e.g. play Bob's Burgers"
          aria-label="Ask the TV's assistant"
          style={{
            flex: 1,
            minHeight: 44,
            padding: "0 0.8rem",
            borderRadius: 11,
            border: `1px solid ${T.border}`,
            background: T.surface,
            color: T.text,
            fontSize: "0.85rem",
            outline: "none",
          }}
        />
        <button
          onClick={submit}
          disabled={!text.trim() || busy}
          title="Speak to the TV's assistant"
          aria-label="Speak to the TV's assistant"
          style={{
            width: 44,
            minHeight: 44,
            borderRadius: 11,
            border: `1px solid ${alpha(ACCENT, 0.4)}`,
            background: text.trim() && !busy ? alpha(ACCENT, 0.12) : T.surface,
            color: text.trim() && !busy ? ACCENT : T.faint,
            cursor: text.trim() && !busy ? "pointer" : "default",
            display: "grid",
            placeItems: "center",
          }}
        >
          <Glyph name="mic" size={18} />
        </button>
      </div>
      {msg && <span style={{ fontSize: "0.74rem", color: T.faint }}>{msg}</span>}
    </div>
  );
}

/** The grid layout shared by the favourites strip and the full catalogue. */
const CMD_GRID: React.CSSProperties = {
  display: "grid",
  gridTemplateColumns: "repeat(auto-fill, minmax(72px, 1fr))",
  gap: "0.4rem",
};

/** One native-command button: tap to send, ★ (top-right) to pin/unpin. Mirrors
 * the app grid's pin affordance. */
function CommandButton({
  cmd,
  onSend,
  onTogglePin,
}: {
  cmd: RemoteCommandInfo;
  onSend: () => void;
  onTogglePin: () => void;
}) {
  return (
    <div style={{ position: "relative" }}>
      <button
        onClick={onSend}
        title={cmd.name}
        style={{
          width: "100%",
          minHeight: 44,
          padding: "0.3rem 1rem 0.3rem 0.4rem", // right room for the ★
          borderRadius: 9,
          border: `1px solid ${cmd.pinned ? alpha(ACCENT, 0.5) : T.border}`,
          background: cmd.pinned ? alpha(ACCENT, 0.08) : T.surface,
          color: T.text,
          cursor: "pointer",
          fontSize: "0.72rem",
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
      >
        {cmd.name}
      </button>
      <button
        onClick={onTogglePin}
        title={cmd.pinned ? "Unpin" : "Pin"}
        aria-label={cmd.pinned ? "Unpin" : "Pin"}
        style={{
          position: "absolute",
          top: 0,
          right: 0,
          bottom: 0,
          background: "none",
          border: "none",
          cursor: "pointer",
          color: cmd.pinned ? ACCENT : T.faint,
          padding: "0 10px",
          display: "grid",
          placeItems: "center",
          lineHeight: 1,
        }}
      >
        <Glyph name={cmd.pinned ? "star_fill" : "star"} size={11} />
      </button>
    </div>
  );
}

/** The expanded ("full") remote — every native command the device exposes (a
 * Bravia's IRCC catalogue). Pinned commands form an always-visible favourites
 * strip; the rest live behind a height-capped, scrollable "Full remote" sheet so
 * a long catalogue never runs off-screen. Renders nothing without a catalogue. */
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

  const togglePin = async (c: RemoteCommandInfo) => {
    // Optimistic flip, then reconcile with the server's view.
    setCommands((cs) => cs.map((x) => (x.token === c.token ? { ...x, pinned: !x.pinned } : x)));
    await setRemoteCommandPin(remoteId, c.token, !c.pinned);
    setCommands(await getRemoteCommands(remoteId));
  };

  if (commands.length === 0) return null;
  const favourites = commands.filter((c) => c.pinned);
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.5rem" }}>
      {favourites.length > 0 && (
        <div style={CMD_GRID}>
          {favourites.map((c) => (
            <CommandButton
              key={c.token}
              cmd={c}
              onSend={() => send({ native: { token: c.token } })}
              onTogglePin={() => togglePin(c)}
            />
          ))}
        </div>
      )}
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
        <div
          style={{
            ...CMD_GRID,
            maxHeight: "min(46vh, 340px)",
            overflowY: "auto",
            // a touch of right padding so the scrollbar doesn't sit on the ★s
            paddingRight: 4,
            WebkitOverflowScrolling: "touch",
          }}
        >
          {commands.map((c) => (
            <CommandButton
              key={c.token}
              cmd={c}
              onSend={() => send({ native: { token: c.token } })}
              onTogglePin={() => togglePin(c)}
            />
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
  // Compact (phones + tablets) tiles run ~50% larger — the desktop sizing
  // made fingertip-sized launch targets on a phone.
  const { isCompact } = useViewport();
  const tileMin = isCompact ? 108 : 72;
  const avatar = isCompact ? 42 : 28;
  if (apps.length === 0) {
    return (
      <div style={{ fontSize: "0.8rem", color: T.faint }}>
        Couldn't read the TV's app list — apps also appear here as you open them.
      </div>
    );
  }
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: `repeat(auto-fill, minmax(${tileMin}px, 1fr))`,
        gap: isCompact ? "0.6rem" : "0.5rem",
      }}
    >
      {apps.map((a) => {
        const active = a.package === currentApp;
        return (
          <div key={a.package} style={{ position: "relative" }}>
            <button
              onClick={() => onLaunch(a.activity ?? a.package)}
              title={`Launch ${a.name}`}
              style={{
                width: "100%",
                minHeight: isCompact ? 100 : 66,
                borderRadius: 12,
                border: `1px solid ${active ? ACCENT : T.border}`,
                background: active ? `${alpha(ACCENT, 0.1)}` : T.surface,
                color: T.text,
                cursor: "pointer",
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                justifyContent: "center",
                gap: isCompact ? "0.45rem" : "0.35rem",
                padding: isCompact ? "0.7rem 0.4rem 0.55rem" : "0.55rem 0.35rem 0.45rem",
              }}
            >
              <span
                style={{
                  width: avatar,
                  height: avatar,
                  borderRadius: "50%",
                  display: "grid",
                  placeItems: "center",
                  fontSize: isCompact ? "1.05rem" : "0.85rem",
                  fontWeight: 700,
                  background: active ? ACCENT : "rgba(255,255,255,0.08)",
                  color: active ? "#0b1220" : T.text,
                }}
              >
                {a.name.charAt(0).toUpperCase()}
              </span>
              <span style={{ fontSize: isCompact ? "0.8rem" : "0.72rem", maxWidth: "100%", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                {a.name}
              </span>
            </button>
            {/* Fixed CORNER hit box — an absolute top/right/bottom stretch made
                the pin own the tile's whole right edge, stealing launch taps. */}
            <button
              onClick={() => onPin(a)}
              title={a.pinned ? "Unpin" : "Pin"}
              aria-label={a.pinned ? "Unpin" : "Pin"}
              style={{
                position: "absolute",
                top: 0,
                right: 0,
                width: isCompact ? 44 : 30,
                height: isCompact ? 44 : 30,
                background: "none",
                border: "none",
                cursor: "pointer",
                color: a.pinned ? ACCENT : T.faint,
                display: "grid",
                placeItems: "start end",
                padding: "6px 8px 0 0",
                lineHeight: 1,
              }}
            >
              <Glyph name={a.pinned ? "star_fill" : "star"} size={isCompact ? 15 : 13} />
            </button>
          </div>
        );
      })}
    </div>
  );
}

/** A centred row of remote keys. */
function Row({ children }: { children: React.ReactNode }) {
  return <div style={{ display: "flex", justifyContent: "center", gap: "0.6rem" }}>{children}</div>;
}

