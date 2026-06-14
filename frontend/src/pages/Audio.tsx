// Audio devices page: speakers, receivers, and zones with full media controls.
// Separate from Lights — these are their own device class. Live state arrives
// via the audio_state SSE stream (Onkyo push) with a slow poll as a fallback.

import { useCallback, useEffect, useState } from "react";
import {
  getAudioDevices,
  getAudioDevice,
  setAudioState,
  groupAudioDevice,
  ungroupAudioDevice,
  discoverProvider,
  type AudioDevice,
} from "../api";
import { AudioControls, KIND_LABEL, PowerButton } from "../components/AudioControls";
import { SelectRow } from "../components/SelectRow";
import { useViewport } from "../useViewport";

const ACCENT = "#a78bfa"; // violet — audio's counterpart to the lamps' warm glow

/** Devices to show on the Audio control surface: drop de-dup **shadows** (a
 * duplicate of a native device — e.g. a Sonos also imported via HA) and disabled
 * devices. Both are managed on the Devices page, not controlled here. */
const controllable = (list: AudioDevice[]) =>
  list.filter((d) => !d.shadowed_by && d.enabled !== false);

const T = {
  text: "#eae4d6",
  dim: "#97907e",
  faint: "#6b6557",
  panel: "linear-gradient(176deg, #1a1916 0%, #141311 100%)",
  panelBorder: "#2b2822",
  card: "#1d1c18",
  cardOff: "#171613",
  cardBorder: "#2c2922",
};

export function AudioPage() {
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [loading, setLoading] = useState(true);
  const { isMobile } = useViewport();

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;

    async function refresh(live: boolean) {
      const list = controllable(await getAudioDevices());
      if (cancelled) return;
      if (live && list.length > 0) {
        const fresh = await Promise.all(list.map((d) => getAudioDevice(d.id)));
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
  useEffect(() => {
    const es = new EventSource("/api/events");
    es.addEventListener("audio_state", (raw) => {
      const ev = JSON.parse((raw as MessageEvent).data) as {
        provider_id: string;
        device_id: string;
        state: AudioDevice["state"];
      };
      setDevices((prev) =>
        prev.map((d) =>
          d.provider_id === ev.provider_id && d.device_id === ev.device_id
            ? { ...d, state: ev.state }
            : d,
        ),
      );
    });
    es.onerror = () => {};
    return () => es.close();
  }, []);

  function patchLocal(id: string, patch: Partial<AudioDevice["state"]>) {
    setDevices((prev) =>
      prev.map((d) => (d.id === id ? { ...d, state: { ...d.state, ...patch } } : d)),
    );
  }

  // Re-fetch the device list with live state — used after a grouping change,
  // which alters the household topology (a synced-group zone appears/vanishes).
  const reloadDevices = useCallback(async () => {
    const list = controllable(await getAudioDevices());
    if (list.length === 0) {
      setDevices([]);
      return;
    }
    const fresh = await Promise.all(list.map((d) => getAudioDevice(d.id)));
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

  return (
    <div style={{ padding: isMobile ? "1rem 0.85rem" : "2rem", maxWidth: 1020, margin: "0 auto", color: T.text }}>
      <header style={{ marginBottom: "1.4rem" }}>
        <div style={{ display: "flex", alignItems: "baseline", gap: "0.9rem" }}>
          <h1
            style={{
              margin: 0,
              fontSize: "1rem",
              textTransform: "uppercase",
              letterSpacing: "0.22em",
              fontWeight: 700,
            }}
          >
            Audio
          </h1>
          {devices.length > 0 && (
            <span style={{ fontSize: "0.78rem", color: T.dim }}>
              {devices.length} device{devices.length !== 1 ? "s" : ""}
            </span>
          )}
        </div>
        <div
          aria-hidden
          style={{
            marginTop: "0.7rem",
            height: 1,
            background:
              "linear-gradient(90deg, rgba(167,139,250,0.6), rgba(56,189,248,0.25) 55%, transparent)",
          }}
        />
      </header>

      {loading ? (
        <p style={{ color: T.faint }}>Loading…</p>
      ) : devices.length === 0 ? (
        <div style={{ textAlign: "center", padding: "4rem 0", color: T.faint }}>
          <p style={{ margin: "0 0 0.5rem", fontSize: "1.4rem" }}>🔇</p>
          <p style={{ margin: "0 0 0.4rem" }}>No audio devices yet.</p>
          <p style={{ margin: 0, fontSize: "0.875rem" }}>
            Add a Sonos or Onkyo provider in Settings, then run Discover.
          </p>
        </div>
      ) : (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: isMobile
              ? "repeat(2, minmax(0, 1fr))"
              : "repeat(auto-fill, minmax(320px, 1fr))",
            gap: isMobile ? "0.6rem" : "1rem",
          }}
        >
          {devices.map((d) => (
            <AudioDeviceCard
              key={d.id}
              device={d}
              peers={devices.filter(
                (o) =>
                  o.id !== d.id &&
                  o.provider_id === d.provider_id &&
                  o.kind !== "zone" &&
                  o.capabilities.grouping,
              )}
              onLocalPatch={patchLocal}
              onGroupingChanged={onGroupingChanged}
              compact={isMobile}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function AudioDeviceCard({
  device,
  peers,
  onLocalPatch,
  onGroupingChanged,
  compact = false,
}: {
  device: AudioDevice;
  peers: AudioDevice[];
  onLocalPatch: (id: string, patch: Partial<AudioDevice["state"]>) => void;
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
    setAudioState(device.id, { power: !s.power });
  }

  // This card's speaker coordinates the group; each selected peer joins it.
  async function group(memberIds: string[]) {
    setGroupBusy(true);
    try {
      for (const m of memberIds) await groupAudioDevice(m, device.id);
      setGroupOpen(false);
      await onGroupingChanged(device.provider_id);
    } finally {
      setGroupBusy(false);
    }
  }

  async function ungroup() {
    setGroupBusy(true);
    try {
      await ungroupAudioDevice(device.id);
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
        border: `1px solid ${s.power || playing ? `${ACCENT}55` : T.cardBorder}`,
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
              background: groupOpen ? `${ACCENT}1f` : "transparent",
              color: groupOpen ? ACCENT : T.dim,
              cursor: "pointer",
              fontSize: "0.72rem",
              flexShrink: 0,
            }}
          >
            <span style={{ fontSize: "0.85rem", lineHeight: 1 }}>⛓</span>
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
        <AudioControls device={device} onLocalPatch={onLocalPatch} compact={compact} />
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
  peers: AudioDevice[];
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
        border: `1px solid ${ACCENT}44`,
        borderRadius: 10,
        background: `${ACCENT}0d`,
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
