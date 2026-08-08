// Audio devices page: speakers, receivers, and zones with full media controls.
// Separate from Lights — these are their own device class. Live state arrives
// via the audio_state SSE stream (Onkyo push) with a slow poll as a fallback.

import { useCallback, useEffect, useState } from "react";
import {
  getMediaDevices,
  getMediaDevice,
  setMediaState,
  groupMediaDevice,
  ungroupMediaDevice,
  discoverProvider,
  type MediaDevice,
} from "../api";
import { MediaControls, KIND_LABEL, PowerButton } from "../components/MediaControls";
import { SelectRow } from "../components/SelectRow";
import { PageHeader, SectionLabel } from "../components/PageHeader";
import { Glyph } from "../components/glyphs";
import { useViewport } from "../useViewport";
import { useEvents } from "../useEvents";
import { T, domain, color, alpha } from "../theme";
import { pageShell } from "../styles";

const ACCENT = domain.media; // violet — audio's counterpart to the lamps' glow

// Pinned devices are a per-client convenience (this page is session-only UI, not
// part of /api/v1 or MCP), so the favourites live in localStorage by device id.
const PIN_KEY = "bifrost.media.pinned";
function loadPinned(): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(PIN_KEY) ?? "[]") as string[]);
  } catch {
    return new Set();
  }
}
function savePinned(ids: Set<string>) {
  try {
    localStorage.setItem(PIN_KEY, JSON.stringify([...ids]));
  } catch {
    /* storage full / disabled — pins just won't persist */
  }
}

const isPlaying = (d: MediaDevice) => d.state.now_playing?.play_state === "playing";

/** Sort within a section: playing first, then reachable, then by name — so the
 * devices you're most likely to reach for surface at the top. */
function bySalience(a: MediaDevice, b: MediaDevice): number {
  const score = (d: MediaDevice) => (isPlaying(d) ? 2 : 0) + (d.state.reachable === false ? -1 : 0);
  return score(b) - score(a) || a.name.localeCompare(b.name);
}

/** Devices to show on the Audio control surface: drop de-dup **shadows** (a
 * duplicate of a native device — e.g. a Sonos also imported via HA) and disabled
 * devices. Both are managed on the Devices page, not controlled here. */
const controllable = (list: MediaDevice[]) =>
  list.filter((d) => !d.shadowed_by && !d.companion_of && d.enabled !== false);

export function MediaPage() {
  const [devices, setDevices] = useState<MediaDevice[]>([]);
  const [loading, setLoading] = useState(true);
  const [pinned, setPinned] = useState<Set<string>>(loadPinned);
  const { isMobile, isCompact } = useViewport();

  function togglePin(id: string) {
    setPinned((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      savePinned(next);
      return next;
    });
  }

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;

    async function refresh(live: boolean) {
      const list = controllable(await getMediaDevices());
      if (cancelled) return;
      if (live && list.length > 0) {
        const fresh = await Promise.all(list.map((d) => getMediaDevice(d.id)));
        if (cancelled) return;
        setDevices(fresh.flatMap((d, i) => (d ? [d] : [list[i]])));
      } else {
        setDevices(list);
      }
      setLoading(false);
      timer = setTimeout(() => refresh(true), 30000);
    }

    refresh(true);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, []);

  // Receiver-initiated changes (volume knob, track changes) arrive as full
  // snapshots — replace, don't merge.
  useEvents({
    media_state: (raw) => {
      const ev = JSON.parse(raw.data) as {
        provider_id: string;
        device_id: string;
        state: MediaDevice["state"];
      };
      setDevices((prev) =>
        prev.map((d) =>
          d.provider_id === ev.provider_id && d.device_id === ev.device_id
            ? { ...d, state: ev.state }
            : d,
        ),
      );
    },
  });

  function patchLocal(id: string, patch: Partial<MediaDevice["state"]>) {
    setDevices((prev) =>
      prev.map((d) => (d.id === id ? { ...d, state: { ...d.state, ...patch } } : d)),
    );
  }

  // Re-fetch the device list with live state — used after a grouping change,
  // which alters the household topology (a synced-group zone appears/vanishes).
  const reloadDevices = useCallback(async () => {
    const list = controllable(await getMediaDevices());
    if (list.length === 0) {
      setDevices([]);
      return;
    }
    const fresh = await Promise.all(list.map((d) => getMediaDevice(d.id)));
    setDevices(fresh.flatMap((d, i) => (d ? [d] : [list[i]])));
  }, []);

  // After grouping/ungrouping, re-discover the provider so the new topology
  // (and any synced-group zone device) is reflected, then reload.
  const onGroupingChanged = useCallback(
    async (providerId: string) => {
      try {
        await discoverProvider(providerId);
      } catch {
        // Best-effort: even without a fresh discovery, reload shows live state.
      }
      await reloadDevices();
    },
    [reloadDevices],
  );

  const playingCount = devices.filter(isPlaying).length;
  const status =
    devices.length > 0
      ? `${playingCount > 0 ? `${playingCount} playing · ` : ""}${devices.length} device${devices.length !== 1 ? "s" : ""}`
      : undefined;

  const pinnedDevices = devices.filter((d) => pinned.has(d.id)).sort(bySalience);
  const restDevices = devices.filter((d) => !pinned.has(d.id)).sort(bySalience);

  const peersOf = (d: MediaDevice) =>
    devices.filter(
      (o) =>
        o.id !== d.id &&
        o.provider_id === d.provider_id &&
        o.kind !== "zone" &&
        o.capabilities.grouping,
    );

  const grid = (list: MediaDevice[]) => (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fill, minmax(300px, 1fr))",
        gap: isMobile ? "0.65rem" : "1rem",
        alignItems: "start",
      }}
    >
      {list.map((d) => (
        <MediaDeviceCard
          key={d.id}
          device={d}
          peers={peersOf(d)}
          pinned={pinned.has(d.id)}
          onTogglePin={() => togglePin(d.id)}
          onLocalPatch={patchLocal}
          onGroupingChanged={onGroupingChanged}
          compact={isCompact}
        />
      ))}
    </div>
  );

  return (
    <div style={{ ...pageShell(isMobile), color: T.text }}>
      <PageHeader title="Media" status={status} />

      {loading ? (
        <p style={{ color: T.faint }}>Loading…</p>
      ) : devices.length === 0 ? (
        <div style={{ textAlign: "center", padding: "4rem 0", color: T.faint }}>
          <p style={{ margin: "0 0 0.6rem", display: "flex", justifyContent: "center", opacity: 0.6 }}>
            <Glyph name="speaker" size={34} />
          </p>
          <p style={{ margin: "0 0 0.4rem" }}>No media devices yet.</p>
          <p style={{ margin: 0, fontSize: "0.875rem" }}>
            Add a Sonos or Onkyo provider in Settings, then run Discover.
          </p>
        </div>
      ) : pinnedDevices.length > 0 ? (
        <>
          <SectionLabel style={{ marginBottom: "0.7rem" }}>Pinned</SectionLabel>
          {grid(pinnedDevices)}
          <SectionLabel style={{ margin: "1.6rem 0 0.7rem" }}>All devices</SectionLabel>
          {grid(restDevices)}
        </>
      ) : (
        grid(restDevices)
      )}
    </div>
  );
}

function MediaDeviceCard({
  device,
  peers,
  pinned,
  onTogglePin,
  onLocalPatch,
  onGroupingChanged,
  compact = false,
}: {
  device: MediaDevice;
  peers: MediaDevice[];
  pinned: boolean;
  onTogglePin: () => void;
  onLocalPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
  onGroupingChanged: (providerId: string) => Promise<void>;
  compact?: boolean;
}) {
  const s = device.state;
  const offline = s.reachable === false;
  const playing = s.now_playing?.play_state === "playing";
  const cap = device.capabilities;
  const canGroup = cap.grouping && !offline;
  const [groupOpen, setGroupOpen] = useState(false);
  const [groupBusy, setGroupBusy] = useState(false);

  function togglePower() {
    onLocalPatch(device.id, { power: !s.power });
    setMediaState(device.id, { power: !s.power });
  }

  // This card's speaker coordinates the group; each selected peer joins it.
  async function group(memberIds: string[]) {
    setGroupBusy(true);
    try {
      for (const m of memberIds) await groupMediaDevice(m, device.id);
      setGroupOpen(false);
      await onGroupingChanged(device.provider_id);
    } finally {
      setGroupBusy(false);
    }
  }

  async function ungroup() {
    setGroupBusy(true);
    try {
      await ungroupMediaDevice(device.id);
      setGroupOpen(false);
      await onGroupingChanged(device.provider_id);
    } finally {
      setGroupBusy(false);
    }
  }

  return (
    <section
      style={{
        background: s.power || playing ? T.panel : T.cardOff,
        border: `1px solid ${s.power || playing ? `${alpha(ACCENT, 0.33)}` : T.cardBorder}`,
        borderRadius: 14,
        overflow: "hidden",
        opacity: offline ? 0.5 : 1,
        boxShadow:
          s.power || playing ? `0 0 28px -14px ${ACCENT}` : "inset 0 1px 0 rgba(255,255,255,0.03)",
      }}
    >
      <header
        style={{
          display: "flex",
          alignItems: "center",
          gap: "0.6rem",
          padding: compact ? "0.6rem 0.7rem 0.5rem" : "0.85rem 1rem 0.6rem",
          borderBottom: `1px solid ${T.cardBorder}`,
        }}
      >
        <div style={{ minWidth: 0, flex: 1 }}>
          <div
            style={{
              fontWeight: 600,
              fontSize: compact ? "0.86rem" : "0.98rem",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {device.name}
          </div>
          <div style={{ fontSize: "0.7rem", color: T.faint, marginTop: "0.1rem" }}>
            {KIND_LABEL[device.kind] ?? device.kind}
          </div>
        </div>
        <button
          onClick={onTogglePin}
          title={pinned ? "Unpin" : "Pin to top"}
          aria-label={pinned ? "Unpin device" : "Pin device"}
          aria-pressed={pinned}
          style={{
            display: "grid",
            placeItems: "center",
            width: 28,
            height: 28,
            borderRadius: 7,
            border: `1px solid ${pinned ? alpha(color.gold, 0.5) : "transparent"}`,
            background: pinned ? alpha(color.gold, 0.12) : "transparent",
            color: pinned ? color.gold : T.faint,
            cursor: "pointer",
            flexShrink: 0,
          }}
        >
          <Glyph name="star" size={15} />
        </button>
        {canGroup && (
          <button
            onClick={() => setGroupOpen((v) => !v)}
            title="Group with other speakers"
            aria-label="Group speakers"
            style={{
              display: "flex",
              alignItems: "center",
              gap: "0.3rem",
              padding: "0.25rem 0.5rem",
              borderRadius: 7,
              border: `1px solid ${groupOpen ? ACCENT : T.cardBorder}`,
              background: groupOpen ? `${alpha(ACCENT, 0.12)}` : "transparent",
              color: groupOpen ? ACCENT : T.dim,
              cursor: "pointer",
              fontSize: "0.72rem",
              flexShrink: 0,
            }}
          >
            <span style={{ display: "grid", placeItems: "center" }}><Glyph name="link" size={15} /></span>
            {!compact && "Group"}
          </button>
        )}
        {/* Receivers/zones have a real power state; speakers don't (power=play). */}
        {cap.sources && !offline && <PowerButton on={s.power} onToggle={togglePower} />}
        {offline && (
          <span
            style={{
              fontSize: "0.7rem",
              color: "#c66",
              border: "1px solid #533",
              borderRadius: 4,
              padding: "0.1rem 0.4rem",
            }}
          >
            offline
          </span>
        )}
      </header>

      <div style={{ padding: compact ? "0.6rem 0.7rem 0.7rem" : "0.9rem 1rem 1rem" }}>
        {groupOpen && canGroup && (
          <GroupPanel peers={peers} busy={groupBusy} onGroup={group} onUngroup={ungroup} />
        )}
        <MediaControls device={device} onLocalPatch={onLocalPatch} compact={compact} />
      </div>
    </section>
  );
}

/** Pick other speakers to play in sync with this one (which coordinates the
 * group), or leave the current group. Sonos-native grouping, not Bifrost Rooms. */
function GroupPanel({
  peers,
  busy,
  onGroup,
  onUngroup,
}: {
  peers: MediaDevice[];
  busy: boolean;
  onGroup: (memberIds: string[]) => void;
  onUngroup: () => void;
}) {
  const [sel, setSel] = useState<Set<string>>(new Set());

  function toggle(id: string) {
    setSel((prev) => {
      const n = new Set(prev);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });
  }

  return (
    <div
      style={{
        border: `1px solid ${alpha(ACCENT, 0.27)}`,
        borderRadius: 10,
        background: `${alpha(ACCENT, 0.05)}`,
        padding: "0.6rem 0.7rem",
        marginBottom: "0.8rem",
        display: "flex",
        flexDirection: "column",
        gap: "0.45rem",
      }}
    >
      <div style={{ fontSize: "0.74rem", color: T.dim, letterSpacing: "0.04em" }}>
        Play in sync with
      </div>
      {peers.length === 0 ? (
        <span style={{ fontSize: "0.8rem", color: T.faint }}>No other groupable speakers.</span>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem" }}>
          {peers.map((p) => (
            <SelectRow key={p.id} accent={ACCENT} checked={sel.has(p.id)} onToggle={() => toggle(p.id)}>
              {p.name}
            </SelectRow>
          ))}
        </div>
      )}
      <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap" }}>
        <button
          onClick={() => onGroup([...sel])}
          disabled={busy || sel.size === 0}
          style={{
            padding: "0.4rem 0.85rem",
            borderRadius: 8,
            border: `1px solid ${ACCENT}`,
            background: "transparent",
            color: ACCENT,
            cursor: busy || sel.size === 0 ? "default" : "pointer",
            opacity: busy || sel.size === 0 ? 0.5 : 1,
            fontSize: "0.82rem",
            fontWeight: 600,
          }}
        >
          {busy ? "Grouping…" : "Group"}
        </button>
        <button
          onClick={onUngroup}
          disabled={busy}
          title="Remove this speaker from its current group"
          style={{
            padding: "0.4rem 0.85rem",
            borderRadius: 8,
            border: `1px solid ${T.cardBorder}`,
            background: "transparent",
            color: T.dim,
            cursor: busy ? "default" : "pointer",
            fontSize: "0.82rem",
          }}
        >
          Leave group
        </button>
      </div>
    </div>
  );
}
