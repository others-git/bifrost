import { useCallback, useEffect, useRef, useState } from "react";
import {
  addProvider,
  discoverProvider,
  getProviderConfig,
  getProviderStatus,
  getProviderTypes,
  getProviders,
  setProviderPrune,
  syncProviderGroups,
  pairHueBridge,
  pairNanoleaf,
  pairSmartTv,
  pairSmartTvRemote,
  scanForDevices,
  discoverAllDevices,
  type DiscoveredDevice,
  type FoundDevice,
  removeProvider,
  updateProviderCredentials,
  getApiKeys,
  createApiKey,
  revokeApiKey,
  createEnrollmentToken,
  getSettings,
  updateSettings,
  getDevInfo,
  getDevProviders,
  getDevProviderDebug,
  type DevProvider,
  getAiEndpoints,
  putAiEndpoint,
  deleteAiEndpoint,
  testAiEndpoint,
  getKiosks,
  kioskCommand,
  kioskDeauth,
  setKioskRoom,
  setKioskBoard,
  setKioskPlan,
  setKioskMic,
  setKioskAwareOverride,
  type KioskHourMode,
  type ControlTarget,
  type AwareOverrideMode,
  type Light,
  type PowerDevice,
  getDashboards,
  forgetKiosk,
  getKioskUpdateConfig,
  setKioskUpdateConfig,
  getKioskUpdateStatus,
  refreshKioskUpdate,
  getRooms,
  type ApiKey,
  type AiEndpoint,
  type AiRole,
  type Dashboard,
  type Kiosk,
  type KioskUpdateManifest,
  type Room,
  clearDevEvents,
  getDevEvents,
  type ConnectionStatus,
  type CredentialField,
  type DevEvent,
  type Provider,
  type ProviderType,
  getLights,
  getMediaDevices,
  getPowerDevices,
  getSensors,
  type MediaDevice,
} from "../api";
import { QRCodeSVG } from "qrcode.react";
import { useDialogs, type Dialogs, Modal } from "../components/dialogs";
import { PageHeader, SectionLabel } from "../components/PageHeader";
import { ThemeSwitcher } from "../components/ThemeSwitcher";
import { Select } from "../components/Select";
import { useMediaQuery, useViewport } from "../useViewport";
import { ACCENT, S, pageShell, tileGrid } from "../styles";
import { pickableLights, pickableMedia, pickablePower } from "../deviceSelectors";
import { Button, Segmented, Switch } from "../components/controls";
import { OptionCheckList } from "../components/deviceOptions";
import { alpha, color } from "../theme";
import { Glyph } from "../components/glyphs";
import { speak } from "../tts";
import { copyText } from "../clipboard";

interface Props {
  onNavigate: (page: "dashboard") => void;
  /** When set (from the Devices "Detected" tab), open the Add Provider form
   * pre-filled with this device; `onConsumeAdd` clears it once consumed. */
  initialAdd?: AddPrefill | null;
  onConsumeAdd?: () => void;
  /** Fired when the Developer tab's dev-mode toggle commits, so dev-gated
   * chrome elsewhere (the Floor Plan nav entry) updates live. */
  onDevModeChange?: (on: boolean) => void;
}

type SettingsTab = "providers" | "voice" | "clients" | "appearance" | "developer";
const SETTINGS_TABS: { id: SettingsTab; label: string }[] = [
  { id: "providers", label: "Providers" },
  { id: "voice", label: "Voice & AI" },
  { id: "clients", label: "Clients" },
  { id: "appearance", label: "Appearance" },
  { id: "developer", label: "Developer" },
];

/** Served inside the kiosk WebView (it appends `BifrostKiosk/<v>` to the UA).
 * The Clients tab manages *other* kiosks remotely — a wall fixture shouldn't be
 * managing the fleet from its own face, so we hide it there. */
const IS_KIOSK = /\bBifrostKiosk\//.test(navigator.userAgent);
const VISIBLE_TABS = SETTINGS_TABS.filter((t) => t.id !== "clients" || !IS_KIOSK);

export function SettingsPage({ onNavigate: _onNavigate, initialAdd, onConsumeAdd, onDevModeChange }: Props) {
  const dialogs = useDialogs();
  const { isMobile, isCompact } = useViewport();
  // Masonry column count for the providers list, scaled with window width
  // (thresholds ≈ tileGrid's 480px tracks + gaps + page padding).
  const wide2 = useMediaQuery("(min-width: 1060px)");
  const wide3 = useMediaQuery("(min-width: 1550px)");
  const wide4 = useMediaQuery("(min-width: 2040px)");
  const providerCols = isMobile ? 1 : wide4 ? 4 : wide3 ? 3 : wide2 ? 2 : 1;
  const [providers, setProviders] = useState<Provider[]>([]);
  const [types, setTypes] = useState<ProviderType[]>([]);
  // Per-provider inventory: a device count for every card, and the media rows
  // device-centric providers (Smart TV) render as per-device rows.
  const [inventory, setInventory] = useState<{
    counts: Map<string, number>;
    tvs: Map<string, MediaDevice[]>;
  }>({ counts: new Map(), tvs: new Map() });
  const loadInventory = useCallback(async () => {
    const [lights, media, power, sensors] = await Promise.all([
      getLights(),
      getMediaDevices(),
      getPowerDevices(),
      getSensors(),
    ]);
    const counts = new Map<string, number>();
    const bump = (pid: string) => counts.set(pid, (counts.get(pid) ?? 0) + 1);
    if (lights !== "unauthorized") for (const l of lights) bump(l.provider_id);
    const tvs = new Map<string, MediaDevice[]>();
    for (const d of media) {
      bump(d.provider_id);
      const arr = tvs.get(d.provider_id) ?? [];
      arr.push(d);
      tvs.set(d.provider_id, arr);
    }
    for (const d of power) bump(d.provider_id);
    for (const d of sensors) bump(d.provider_id);
    setInventory({ counts, tvs });
  }, []);
  useEffect(() => {
    loadInventory();
  }, [loadInventory]);
  const [showAdd, setShowAdd] = useState(false);
  // When the user clicks "Add" on a found device, the add form opens pre-filled.
  const [prefill, setPrefill] = useState<AddPrefill | null>(null);
  const [toast, setToast] = useState("");
  const [tab, setTab] = useState<SettingsTab>("providers");

  function openAdd(p: AddPrefill | null) {
    setPrefill(p);
    setShowAdd(true);
  }

  // Arriving from the Devices "Detected" tab: jump to Providers and open the
  // pre-filled add form (pairing/keys completed there).
  useEffect(() => {
    if (initialAdd) {
      setTab("providers");
      openAdd(initialAdd);
      onConsumeAdd?.();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialAdd]);

  async function loadProviders() {
    setProviders(await getProviders());
  }

  useEffect(() => {
    loadProviders();
    getProviderTypes().then(setTypes);
  }, []);

  function showToast(msg: string) {
    setToast(msg);
    setTimeout(() => setToast(""), 3000);
  }

  async function handleRemove(provider: Provider) {
    // A device-centric row (one TV / one plug) reads as removing a DEVICE;
    // a hub/account row is the whole provider and everything it imported.
    const ok = await dialogs.confirm(
      DEVICE_CENTRIC_TYPES.has(provider.provider_type)
        ? {
            title: "Remove device",
            message: `Remove ${provider.name}? Its imported devices and any pairing will be deleted.`,
            confirmLabel: "Remove",
            danger: true,
          }
        : {
            title: "Remove provider",
            message: `Remove this provider? Associated ${
              provider.domain === "media"
                ? "audio devices"
                : provider.domain === "power"
                  ? "power devices"
                  : provider.domain === "integration"
                    ? "devices"
                    : "lights"
            } will be deleted.`,
            confirmLabel: "Remove",
            danger: true,
          },
    );
    if (!ok) return;
    await removeProvider(provider.id);
    await loadProviders();
  }

  const [running, setRunning] = useState("");

  async function handleDiscover(id: string) {
    const result = await discoverProvider(id);
    await loadProviders();
    showToast(
      `Discovered ${result.discovered} device${result.discovered !== 1 ? "s" : ""}` +
        (result.pruned ? `, pruned ${result.pruned}.` : "."),
    );
  }

  async function handleAdded(id: string) {
    setShowAdd(false);
    await loadProviders();
    try {
      const result = await discoverProvider(id);
      showToast(`Provider added — found ${result.discovered} device${result.discovered !== 1 ? "s" : ""}.`);
    } catch {
      showToast("Provider added. Discovery failed — check the connection and try Discover.");
    }
  }

  /** Run discover (and optionally sync-groups) across every provider. */
  async function runAll(mode: "discover" | "sync" | "prune-sync") {
    setRunning(mode);
    let discovered = 0;
    let pruned = 0;
    let synced = 0;
    for (const p of providers) {
      try {
        const d = await discoverProvider(p.id, mode === "prune-sync" ? { prune: true } : undefined);
        discovered += d.discovered;
        pruned += d.pruned;
        if (mode !== "discover") {
          const s = await syncProviderGroups(p.id);
          synced += s.synced;
        }
      } catch {
        /* skip a failing provider, keep going */
      }
    }
    await loadProviders();
    setRunning("");
    const tail = pruned ? `, pruned ${pruned}` : "";
    showToast(
      mode === "discover"
        ? `Discovered ${discovered} device${discovered !== 1 ? "s" : ""}${tail}.`
        : `Discovered ${discovered}${tail}; synced ${synced} room${synced !== 1 ? "s" : ""}.`,
    );
  }

  return (
    <div style={pageShell(isMobile)}>
      <PageHeader title="Settings" />

      {/* Tabs — keeps a growing Settings page legible. Scrolls on narrow screens. */}
      <div
        className="bf-noscroll"
        style={{
          display: "flex",
          gap: "0.15rem",
          borderBottom: "1px solid var(--bf-border)",
          margin: "0 0 1.5rem",
          // overflow-x:auto alone makes the browser compute overflow-y as auto
          // too, which spawns a stray vertical scrollbar (the up/down arrows).
          // Pin the vertical axis hidden so only horizontal scrolling shows.
          overflowX: "auto",
          overflowY: "hidden",
        }}
      >
        {VISIBLE_TABS.map((t) => {
          const active = tab === t.id;
          return (
            <button
              key={t.id}
              onClick={() => setTab(t.id)}
              style={{
                background: "none",
                border: "none",
                cursor: "pointer",
                whiteSpace: "nowrap",
                padding: isMobile ? "0.7rem 0.95rem" : "0.55rem 0.95rem",
                fontSize: "0.92rem",
                fontWeight: active ? 600 : 500,
                color: active ? ACCENT : "var(--bf-dim)",
                borderBottom: `2px solid ${active ? ACCENT : "transparent"}`,
                marginBottom: -1,
                transition: "color 0.15s",
              }}
            >
              {t.label}
            </button>
          );
        })}
      </div>

      {toast && (
        <div style={{ background: "#1e3a1e", border: "1px solid #2a5a2a", borderRadius: 8, padding: "0.6rem 1rem", marginBottom: "1rem", color: "var(--bf-good)", fontSize: "0.875rem" }}>
          {toast}
        </div>
      )}

      {tab === "providers" && (
        <>
          {providers.length > 0 && (
            <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap", marginBottom: "1rem" }}>
              <Button variant="ghost" onClick={() => runAll("discover")} disabled={!!running} title="Discover devices on every provider">
                {running === "discover" ? "Discovering…" : "Discover all"}
              </Button>
              <Button variant="ghost" onClick={() => runAll("sync")} disabled={!!running} title="Discover devices and mirror rooms/zones for every provider">
                {running === "sync" ? "Syncing…" : "Sync all"}
              </Button>
              <Button variant="ghost"
                onClick={() => runAll("prune-sync")}
                disabled={!!running}
                title="Force-prune devices providers no longer report, then discover + sync — across all providers"
              >
                {running === "prune-sync" ? "Pruning…" : "Prune + Sync all"}
              </Button>
            </div>
          )}

          {providers.length === 0 && !showAdd && (
            <p style={{ color: "var(--bf-faint)", margin: 0 }}>No providers configured.</p>
          )}
          {/* Masonry (CSS columns), not a grid: a grid's rows are as tall as
              their tallest card, so one tall device-group card stranded the
              next row's cards far below the short hubs beside it. Columns
              pack fluidly — a tall card just makes its own column longer. */}
          <div style={{ columnCount: providerCols, columnGap: "0.75rem" }}>
            {groupProviders(providers).map((group) => {
              const providerType = group[0].provider_type;
              const wrap = (key: string, child: React.ReactNode) => (
                <div key={key} style={{ breakInside: "avoid", marginBottom: "0.75rem" }}>
                  {child}
                </div>
              );
              // Device-centric types (one row = one LAN device) get the
              // shared type card with per-device rows; hub/account providers
              // stay full cards — even two Hue bridges are each a real
              // provider deserving their own Discover/Sync.
              if (DEVICE_CENTRIC_TYPES.has(providerType)) {
                return wrap(
                  `group:${providerType}`,
                  <DeviceProviderGroup
                    providers={group}
                    typeLabel={
                      types.find((t) => t.provider_type === providerType)?.display_name ??
                      group[0].type_name
                    }
                    types={types}
                    tvs={inventory.tvs}
                    isCompact={isCompact}
                    onRemove={handleRemove}
                    onCredentialsSaved={() => showToast("Credentials updated — reconnecting.")}
                    reload={async () => {
                      await loadProviders();
                      await loadInventory();
                    }}
                    showToast={showToast}
                  />,
                );
              }
              return group.map((p) =>
                wrap(
                  p.id,
                  <ProviderCard
                    provider={p}
                    types={types}
                    deviceCount={inventory.counts.get(p.id) ?? 0}
                    onCredentialsSaved={() => showToast("Credentials updated — reconnecting.")}
                    onRemove={() => handleRemove(p)}
                    onDiscover={() => handleDiscover(p.id)}
                    onPruneNow={async () => {
                      const d = await discoverProvider(p.id, { prune: true });
                      showToast(`Pruned ${d.pruned}, discovered ${d.discovered}.`);
                      loadInventory();
                    }}
                    onSetPrune={async (prune) => {
                      await setProviderPrune(p.id, prune);
                      await loadProviders();
                    }}
                    onImportGroups={async () => {
                      const r = await syncProviderGroups(p.id);
                      showToast(
                        r.synced === 0
                          ? "No rooms or zones defined on this provider."
                          : `Synced ${r.synced} room${r.synced !== 1 ? "s" : ""} (${r.rooms_created} created, ${r.rooms_linked} linked).`,
                      );
                    }}
                  />,
                ),
              );
            })}
          </div>

          {showAdd ? (
            <div style={{ marginTop: "1.5rem", maxWidth: 680 }}>
              <AddProviderForm
                key={prefill ? `${prefill.provider_type}:${prefill.name}` : "blank"}
                types={types}
                prefill={prefill}
                onAdded={(id) => {
                  setPrefill(null);
                  handleAdded(id);
                }}
                onCancel={() => {
                  setPrefill(null);
                  setShowAdd(false);
                }}
              />
            </div>
          ) : (
            <Button onClick={() => openAdd(null)} style={{ marginTop: "1.5rem" }}>
              + Add Provider
            </Button>
          )}

          {!showAdd && <FoundDevicesSection onAdd={openAdd} />}

          <ExpandedLanSection />
        </>
      )}

      {/* Form-shaped tabs keep a readable column (left-aligned, not centered);
          full-bleed is for card collections, not input stacks. */}
      {tab === "voice" && (
        <div style={{ maxWidth: 760 }}>
          <AiEndpointsSection dialogs={dialogs} />
        </div>
      )}

      {tab === "clients" && <ClientsTab dialogs={dialogs} />}

      {tab === "appearance" && (
        <div style={{ marginTop: "0.25rem" }}>
          <ThemeSwitcher />
        </div>
      )}

      {tab === "developer" && (
        <div style={{ maxWidth: 900 }}>
          <DeveloperTab onDevModeChange={onDevModeChange} />
        </div>
      )}

      {dialogs.element}
    </div>
  );
}

// ── Found devices (auto-discovery) ───────────────────────────────────────────

/** Pre-fills the add-provider form from a found device. */
export interface AddPrefill {
  provider_type: string;
  name: string;
  credentials: Record<string, string>;
}

/**
 * One-button "find what's on my network": scans every discoverable provider
 * type at once (no type to pick first) and lists devices not yet configured.
 * Clicking one opens the add-provider form pre-filled. Credential-free LAN
 * discovery only (SSDP/eISCP/Govee-LAN) — cloud providers still need a key.
 */
function FoundDevicesSection({ onAdd }: { onAdd: (p: AddPrefill) => void }) {
  const [scanning, setScanning] = useState(false);
  const [scanned, setScanned] = useState(false);
  const [found, setFound] = useState<FoundDevice[]>([]);

  async function scan() {
    setScanning(true);
    setScanned(false);
    setFound(await discoverAllDevices());
    setScanned(true);
    setScanning(false);
  }

  return (
    <div style={{ marginTop: "1.5rem" }}>
      <Button variant="ghost" onClick={scan} disabled={scanning}>
        {scanning ? "Scanning network…" : "Scan for new devices"}
      </Button>
      {found.length > 0 && (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.5rem", marginTop: "0.75rem" }}>
          {found.map((d) => (
            <div
              key={`${d.provider_type}:${d.host}`}
              style={{
                ...S.card,
                flexDirection: "row",
                alignItems: "center",
                justifyContent: "space-between",
                padding: "0.6rem 0.9rem",
                gap: "1rem",
              }}
            >
              <div style={{ minWidth: 0 }}>
                <div style={{ fontWeight: 600, fontSize: "0.9rem" }}>{d.label ?? d.host}</div>
                <div style={{ color: "var(--bf-faint)", fontSize: "0.75rem" }}>
                  {d.type_name} · {d.host}
                </div>
              </div>
              <Button
                onClick={() =>
                  onAdd({
                    provider_type: d.provider_type,
                    name: d.label ?? d.host,
                    credentials: Object.fromEntries(
                      Object.entries(d.credentials).map(([k, v]) => [k, String(v)]),
                    ),
                  })
                }
                style={{ flexShrink: 0 }}
              >
                Add
              </Button>
            </div>
          ))}
        </div>
      )}
      {scanned && found.length === 0 && (
        <p style={{ color: "var(--bf-faint)", fontSize: "0.78rem", margin: "0.6rem 0 0", maxWidth: 560 }}>
          No new devices found. Auto-discovery finds credential-free LAN gear (Sonos, Onkyo) and
          needs host networking in Docker; cloud providers (Hue, Govee, LIFX) still need a key —
          add those with <strong>+ Add Provider</strong>.
        </p>
      )}
    </div>
  );
}

// ── Expanded-LAN scan ──────────────────────────────────────────────────────────

/**
 * Extra private subnets the "Scan network" button should sweep, beyond the
 * container's own subnet. Lets auto-detect reach devices on a different LAN
 * without host networking (unicast routes across subnets; broadcast doesn't).
 */
function ExpandedLanSection() {
  const [subnets, setSubnets] = useState<string[]>([]);
  const [text, setText] = useState("");
  const [msg, setMsg] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    getSettings().then((s) => {
      setSubnets(s.expanded_lan_scan);
      setText(s.expanded_lan_scan.join(", "));
    });
  }, []);

  async function save() {
    setSaving(true);
    setMsg("");
    const list = text
      .split(/[,\s]+/)
      .map((s) => s.trim())
      .filter(Boolean);
    const result = await updateSettings({ expanded_lan_scan: list });
    setSaving(false);
    if ("error" in result) {
      setMsg(result.error);
    } else {
      setSubnets(result.expanded_lan_scan);
      setText(result.expanded_lan_scan.join(", "));
      setMsg(
        result.expanded_lan_scan.length
          ? "✓ Saved."
          : "✓ Saved — scanning the local subnet only.",
      );
    }
  }

  return (
    <div style={{ marginTop: "2rem" }}>
      <SectionLabel style={{ marginBottom: "0.4rem" }}>Expanded-LAN scan</SectionLabel>
      <p style={{ color: "var(--bf-dim)", fontSize: "0.85rem", margin: "0 0 0.75rem", maxWidth: 560 }}>
        By default, <strong>Scan network</strong> only searches Bifrost's own subnet. If Bifrost
        runs in a container on a different network than your devices (e.g. bridged Docker), list the
        device LAN(s) here as <code>/24</code> networks and the scan will reach across to them. Only
        private networks (10/8, 172.16/12, 192.168/16) are allowed; up to 8. This widens the
        HTTP-based light scans (WLED/Tasmota/Shelly) — Hue/Sonos/Onkyo use broadcast discovery that
        can't cross a subnet either way.
      </p>
      <div style={{ display: "flex", gap: "0.5rem", alignItems: "flex-start", maxWidth: 560 }}>
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="192.168.1.0/24, 10.0.0.0/24"
          style={{ ...S.input, flex: 1 }}
          spellCheck={false}
        />
        <Button onClick={save} disabled={saving}>
          {saving ? "Saving…" : "Save"}
        </Button>
      </div>
      {msg && (
        <span
          style={{
            display: "block",
            marginTop: "0.5rem",
            fontSize: "0.8rem",
            color: msg.startsWith("✓") ? "var(--bf-good)" : "#fa0",
          }}
        >
          {msg}
        </span>
      )}
      {subnets.length > 0 && (
        <div style={{ marginTop: "0.6rem", display: "flex", flexWrap: "wrap", gap: "0.4rem" }}>
          {subnets.map((s) => (
            <span
              key={s}
              style={{
                fontSize: "0.78rem",
                color: ACCENT,
                border: `1px solid ${ACCENT}`,
                borderRadius: 6,
                padding: "0.1rem 0.45rem",
              }}
            >
              {s}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

// ── Developer mode ──────────────────────────────────────────────────────────

/**
 * Developer tab: a global **dev-mode** switch plus contributor diagnostics that a
 * normal deploy never needs. When on, the backend's `/api/dev` surface is exposed
 * (provider debug, build info) — reachable from here AND directly over the API
 * (session or Bearer key) so tooling and the assistant can read it too.
 */
function DeveloperTab({ onDevModeChange }: { onDevModeChange?: (on: boolean) => void }) {
  const [devMode, setDevMode] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    getSettings().then((s) => setDevMode(!!s.dev_mode));
  }, []);

  async function toggle(next: boolean) {
    setSaving(true);
    const res = await updateSettings({ dev_mode: next });
    setSaving(false);
    if (!("error" in res)) {
      setDevMode(!!res.dev_mode);
      onDevModeChange?.(!!res.dev_mode);
    }
  }

  return (
    <section style={{ marginTop: "0.25rem" }}>
      <SectionLabel style={{ marginBottom: "0.4rem" }}>Developer mode</SectionLabel>
      <p style={{ margin: "0 0 1rem", color: "var(--bf-faint)", fontSize: "0.85rem", maxWidth: 560 }}>
        Surfaces contributor diagnostics a live deploy has no use for: per-provider debug
        (raw upstream capabilities, the ones Bifrost doesn't model yet), build info, and the{" "}
        <code style={{ color: "#9ab" }}>/api/dev</code> API (session or Bearer key). Leave it{" "}
        <strong>off</strong> on a normal hub — every dev surface is invisible while it's off.
      </p>

      <div
        style={{
          ...S.card,
          flexDirection: "row",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "0.8rem 1rem",
          gap: "1rem",
        }}
      >
        <div style={{ minWidth: 0 }}>
          <div style={{ fontWeight: 600, fontSize: "0.92rem" }}>Enable developer mode</div>
          <div style={{ color: "var(--bf-faint)", fontSize: "0.76rem", marginTop: "0.15rem" }}>
            {devMode ? "On — dev surfaces and /api/dev are live." : "Off — production behaviour."}
          </div>
        </div>
        <Switch on={!!devMode} disabled={devMode === null || saving} onChange={toggle} />
      </div>

      {devMode && (
        <>
          <DevInfoCard />
          <DevEventLog />
          <DevProviderDebugSection />
        </>
      )}
    </section>
  );
}

/** Live server event log: everything Bifrost traces at debug+ (automations
 * firing and their skip reasons, the voice pipeline, device state pushes,
 * discovery, composite routing), captured server-side regardless of RUST_LOG.
 * Polls while visible; pause to scroll back, filter by area, clear to reset. */
function DevEventLog() {
  const AREAS: { value: string; label: string }[] = [
    { value: "", label: "All areas" },
    { value: "bifrost::automation", label: "Automations" },
    { value: "bifrost::voice", label: "Voice" },
    { value: "bifrost::events", label: "Device state" },
    { value: "bifrost::discover", label: "Discovery" },
    { value: "bifrost::composite", label: "Composite" },
    { value: "bifrost::smarttv", label: "Smart TV" },
  ];
  const LEVELS: { value: string; label: string }[] = [
    { value: "", label: "All levels" },
    { value: "warn", label: "Warnings +" },
    { value: "error", label: "Errors" },
  ];
  const [events, setEvents] = useState<DevEvent[]>([]);
  const [area, setArea] = useState("");
  const [level, setLevel] = useState("");
  const [paused, setPaused] = useState(false);
  const lastSeq = useRef(0);

  // Poll while mounted (the tab is open) and not paused. Changing the area or
  // level filter re-reads from the start so history under the new filter shows
  // too (the server filters the whole ring buffer, not just fresh entries).
  useEffect(() => {
    let alive = true;
    lastSeq.current = 0;
    setEvents([]);
    async function poll() {
      if (!alive || paused) return;
      const batch = await getDevEvents(lastSeq.current, area || undefined, level || undefined);
      if (!alive || !batch) return;
      lastSeq.current = batch.last_seq;
      if (batch.entries.length > 0) {
        // Newest first; cap what the panel keeps.
        setEvents((prev) => [...batch.entries.reverse(), ...prev].slice(0, 500));
      }
    }
    poll();
    const t = setInterval(poll, 2000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [area, level, paused]);

  const mono = "ui-monospace, SFMono-Regular, Menlo, monospace";
  const timeOf = (ts: string) => ts.slice(11, 19);

  return (
    <div style={{ ...S.card, marginTop: "1rem", padding: "0.8rem 1rem", gap: "0.6rem" }}>
      <div style={{ display: "flex", alignItems: "center", gap: "0.6rem", flexWrap: "wrap" }}>
        <div style={{ fontWeight: 600, fontSize: "0.92rem", flex: 1, minWidth: 120 }}>Event log</div>
        <Select value={area} options={AREAS} onChange={setArea} width={160} />
        <Select value={level} options={LEVELS} onChange={setLevel} width={124} />
        <Button variant="ghost" onClick={() => setPaused((p) => !p)}>
          {paused ? "Resume" : "Pause"}
        </Button>
        <Button
          variant="ghost"
          onClick={async () => {
            await clearDevEvents();
            setEvents([]);
          }}
        >
          Clear
        </Button>
      </div>
      <div
        style={{
          fontFamily: mono,
          fontSize: "0.72rem",
          maxHeight: 380,
          overflowY: "auto",
          display: "flex",
          flexDirection: "column",
          gap: 2,
          background: "rgba(0,0,0,0.3)",
          borderRadius: 8,
          padding: "0.5rem 0.6rem",
        }}
      >
        {events.length === 0 && (
          <span style={{ color: "var(--bf-faint)" }}>
            Waiting for events… act on a device, run an automation, or speak a command.
          </span>
        )}
        {events.map((e) => (
          <div key={e.seq} style={{ display: "flex", gap: "0.5rem", alignItems: "baseline", minWidth: 0 }}>
            <span style={{ color: "var(--bf-faint)", flexShrink: 0 }}>{timeOf(e.ts)}</span>
            <span
              style={{
                color: e.level === "ERROR" ? "#f88" : e.level === "WARN" ? "#fc6" : ACCENT,
                flexShrink: 0,
                minWidth: 86,
                overflow: "hidden",
                textOverflow: "ellipsis",
              }}
              title={e.target}
            >
              {e.target.replace(/^bifrost::/, "")}
            </span>
            <span style={{ color: "var(--bf-text, #eee)", minWidth: 0 }}>
              {e.message}
              {e.fields &&
                Object.entries(e.fields).map(([k, v]) => (
                  <span key={k} style={{ color: "var(--bf-faint)" }}>
                    {" "}
                    {k}={v}
                  </span>
                ))}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

/** Compact build/runtime readout (version, debug vs release, provider count). */
function DevInfoCard() {
  const [info, setInfo] = useState<Record<string, unknown> | null>(null);
  useEffect(() => {
    getDevInfo().then(setInfo);
  }, []);
  if (!info) return null;
  return (
    <div style={{ ...S.card, marginTop: "1rem", gap: "0.3rem" }}>
      <SectionLabel style={{ marginBottom: "0.3rem" }}>Build</SectionLabel>
      <div style={{ display: "flex", flexWrap: "wrap", gap: "0.4rem 1.2rem", fontSize: "0.8rem", color: "var(--bf-dim)" }}>
        {Object.entries(info)
          .filter(([k]) => k !== "dev_mode")
          .map(([k, v]) => (
            <span key={k}>
              <span style={{ color: "var(--bf-faint)" }}>{k}</span>{" "}
              <code style={{ color: ACCENT }}>{String(v)}</code>
            </span>
          ))}
      </div>
    </div>
  );
}

/** Per-provider debug expanders. Lazily fetches a provider's `debug_info` only
 * when its row is expanded (a debug fetch may hit the provider's cloud API). */
function DevProviderDebugSection() {
  const [providers, setProviders] = useState<DevProvider[]>([]);
  useEffect(() => {
    getDevProviders().then(setProviders);
  }, []);

  return (
    <div style={{ marginTop: "1.25rem" }}>
      <SectionLabel style={{ marginBottom: "0.4rem" }}>Provider debug</SectionLabel>
      {providers.length === 0 ? (
        <p style={{ color: "var(--bf-faint)", margin: 0, fontSize: "0.85rem" }}>No providers configured.</p>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.5rem" }}>
          {providers.map((p) => (
            <DevProviderRow key={p.id} provider={p} />
          ))}
        </div>
      )}
    </div>
  );
}

function DevProviderRow({ provider }: { provider: DevProvider }) {
  const [open, setOpen] = useState(false);
  const [data, setData] = useState<unknown | null>(null);
  const [loading, setLoading] = useState(false);

  async function expand() {
    const next = !open;
    setOpen(next);
    if (next && data === null && provider.has_debug) {
      setLoading(true);
      setData(await getDevProviderDebug(provider.id));
      setLoading(false);
    }
  }

  return (
    <div style={{ ...S.card, gap: "0.5rem", padding: "0.7rem 1rem" }}>
      <button
        onClick={expand}
        style={{
          background: "none",
          border: "none",
          cursor: "pointer",
          padding: 0,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          width: "100%",
          color: "inherit",
        }}
      >
        <span style={{ minWidth: 0, textAlign: "left" }}>
          <span style={{ fontWeight: 600, fontSize: "0.9rem" }}>{provider.name}</span>{" "}
          <span style={{ color: "var(--bf-faint)", fontSize: "0.76rem" }}>{provider.provider_type}</span>
        </span>
        <span style={{ color: "var(--bf-faint)", fontSize: "0.78rem", flexShrink: 0 }}>
          {provider.has_debug ? (open ? "▾" : "▸") : "no debug"}
        </span>
      </button>
      {open && provider.has_debug && (
        <pre
          style={{
            margin: 0,
            background: "var(--bf-void, #0b0b10)",
            border: "1px solid var(--bf-hairline, #333)",
            borderRadius: 6,
            padding: "0.6rem 0.7rem",
            fontSize: "0.74rem",
            color: "#cdd",
            overflowX: "auto",
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
          }}
        >
          {loading ? "Loading…" : data === null ? "No debug info." : JSON.stringify(data, null, 2)}
        </pre>
      )}
    </div>
  );
}

// ── API keys ─────────────────────────────────────────────────────────────────

function ApiKeysSection({ dialogs }: { dialogs: Dialogs }) {
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [name, setName] = useState("");
  const [creating, setCreating] = useState(false);
  const [pairing, setPairing] = useState(false);
  // The plaintext of a just-created key, shown once until dismissed.
  const [fresh, setFresh] = useState<{ name: string; key: string } | null>(null);
  const [copied, setCopied] = useState(false);

  async function copyKey(key: string) {
    const ok = await copyText(key);
    setCopied(ok);
    if (ok) setTimeout(() => setCopied(false), 2000);
  }

  async function load() {
    setKeys(await getApiKeys());
  }
  useEffect(() => {
    load();
  }, []);

  async function create() {
    if (!name.trim()) return;
    setCreating(true);
    try {
      const created = await createApiKey(name.trim());
      setFresh({ name: created.name, key: created.key });
      setName("");
      await load();
    } finally {
      setCreating(false);
    }
  }

  async function revoke(k: ApiKey) {
    const ok = await dialogs.confirm({
      title: "Revoke API key",
      message: `Revoke "${k.name}"? Apps using it will immediately lose access.`,
      confirmLabel: "Revoke",
      danger: true,
    });
    if (!ok) return;
    await revokeApiKey(k.id);
    await load();
  }

  return (
    <section style={{ marginTop: "2.5rem" }}>
      <SectionLabel style={{ marginBottom: "0.4rem" }}>API keys</SectionLabel>
      <p style={{ margin: "0 0 1rem", color: "var(--bf-faint)", fontSize: "0.85rem" }}>
        Grant other apps full access to your lights and rooms via the public{" "}
        <code style={{ color: "#9ab" }}>/api/v1</code> API. Send the key as{" "}
        <code style={{ color: "#9ab" }}>Authorization: Bearer &lt;key&gt;</code>.
      </p>

      <div
        style={{
          ...S.card,
          flexDirection: "row",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "0.7rem 1rem",
          marginBottom: "1rem",
          gap: "1rem",
        }}
      >
        <div style={{ minWidth: 0 }}>
          <div style={{ fontWeight: 600, fontSize: "0.9rem" }}>Pair a device</div>
          <div style={{ color: "var(--bf-faint)", fontSize: "0.74rem" }}>
            Scan a QR from the tablet to authorize it — no key to type.
          </div>
        </div>
        <Button onClick={() => setPairing(true)}>Pair…</Button>
      </div>

      {pairing && (
        <PairDeviceModal
          onClose={() => {
            setPairing(false);
            void load(); // a freshly paired device's key shows up in the list
          }}
        />
      )}

      {fresh && (
        <div
          style={{
            background: "#1e2a1e",
            border: "1px solid #2a5a2a",
            borderRadius: 8,
            padding: "0.8rem 1rem",
            marginBottom: "1rem",
          }}
        >
          <div style={{ fontSize: "0.8rem", color: "var(--bf-good)", marginBottom: "0.4rem" }}>
            Copy “{fresh.name}” now — it won't be shown again.
          </div>
          <code
            style={{
              display: "block",
              wordBreak: "break-all",
              fontSize: "0.8rem",
              color: "#dfe",
              background: "#0d140d",
              borderRadius: 6,
              padding: "0.5rem 0.6rem",
            }}
          >
            {fresh.key}
          </code>
          <div style={{ display: "flex", gap: "0.5rem", marginTop: "0.5rem" }}>
            <Button variant="ghost"
              onClick={() => copyKey(fresh.key)} style={{ padding: "0.3rem 0.6rem", fontSize: "0.78rem" }}
            >
              {copied ? "Copied ✓" : "Copy"}
            </Button>
            <Button variant="ghost"
              onClick={() => setFresh(null)} style={{ padding: "0.3rem 0.6rem", fontSize: "0.78rem" }}
            >
              Done
            </Button>
          </div>
        </div>
      )}

      <div style={{ display: "flex", flexDirection: "column", gap: "0.5rem", marginBottom: "1rem" }}>
        {keys.length === 0 && <p style={{ color: "var(--bf-faint)", margin: 0, fontSize: "0.85rem" }}>No keys yet.</p>}
        {keys.map((k) => (
          <div
            key={k.id}
            style={{
              ...S.card,
              flexDirection: "row",
              alignItems: "center",
              justifyContent: "space-between",
              padding: "0.7rem 1rem",
            }}
          >
            <div style={{ minWidth: 0 }}>
              <div style={{ fontWeight: 600, fontSize: "0.9rem" }}>{k.name}</div>
              <div style={{ color: "var(--bf-faint)", fontSize: "0.74rem" }}>
                <code>{k.prefix}…</code>
                {k.last_used ? ` · last used ${k.last_used}` : " · never used"}
              </div>
            </div>
            <Button variant="danger"
              onClick={() => revoke(k)} style={{ padding: "0.3rem 0.7rem", fontSize: "0.78rem" }}
            >
              Revoke
            </Button>
          </div>
        ))}
      </div>

      <div style={{ display: "flex", gap: "0.5rem" }}>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && create()}
          placeholder="New key name (e.g. Home Assistant)"
          style={{ ...S.input, flex: 1 }}
        />
        <Button onClick={create} disabled={creating || !name.trim()}>
          {creating ? "Creating…" : "Create key"}
        </Button>
      </div>
    </section>
  );
}

// Mints a pairing token and renders it as a QR the companion app scans. The
// payload carries the server origin + token so the device knows where to redeem
// it. The token is single-use and short-lived; we count it down and let the user
// re-mint when it lapses.
function PairDeviceModal({ onClose }: { onClose: () => void }) {
  const [token, setToken] = useState<string | null>(null);
  const [secondsLeft, setSecondsLeft] = useState(0);
  const [error, setError] = useState<string | null>(null);

  async function mint() {
    setError(null);
    setToken(null);
    try {
      const t = await createEnrollmentToken();
      setToken(t.token);
      setSecondsLeft(t.expires_in_secs);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't create a pairing code.");
    }
  }

  useEffect(() => {
    void mint();
  }, []);

  useEffect(() => {
    if (secondsLeft <= 0) return;
    const t = setInterval(() => setSecondsLeft((s) => Math.max(0, s - 1)), 1000);
    return () => clearInterval(t);
  }, [secondsLeft > 0]);

  const expired = token !== null && secondsLeft <= 0;
  // What the tablet scans: where to redeem (origin) + the one-time token.
  const payload = token
    ? JSON.stringify({ v: 1, base_url: window.location.origin, token })
    : "";

  return (
    <Modal title="Pair a device" onClose={onClose} width={340}>
      <p style={{ margin: "0.8rem 0 1rem", color: "var(--bf-faint)", fontSize: "0.84rem" }}>
        On the tablet, open the Bifrost app → <strong>Pair</strong>, then point its
        camera at this code. It authorizes itself — nothing to type.
      </p>

      {error && (
        <p style={{ color: "var(--bf-bad, #e57)", fontSize: "0.84rem" }}>{error}</p>
      )}

      <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: "0.8rem" }}>
        {token && (
          <div
            style={{
              background: "#fff",
              padding: "0.9rem",
              borderRadius: 10,
              // Dim + non-scannable once it's lapsed, to avoid a stale scan.
              opacity: expired ? 0.25 : 1,
              transition: "opacity 0.2s",
            }}
          >
            {/* Bigger modules + the spec quiet zone (margin) + error correction
                so a soft-focus tablet camera can lock on. marginSize=0 made it
                near-unscannable on poor cameras. */}
            <QRCodeSVG value={payload} size={288} marginSize={4} level="M" />
          </div>
        )}
        {!token && !error && (
          <div style={{ color: "var(--bf-faint)", fontSize: "0.85rem", padding: "3rem 0" }}>
            Generating…
          </div>
        )}

        {expired ? (
          <Button onClick={() => void mint()}>Generate a new code</Button>
        ) : (
          token && (
            <div style={{ color: "var(--bf-faint)", fontSize: "0.78rem" }}>
              Expires in {Math.floor(secondsLeft / 60)}:
              {String(secondsLeft % 60).padStart(2, "0")}
            </div>
          )
        )}
      </div>
    </Modal>
  );
}

/** Groups configured providers by `provider_type`, preserving first-seen
 * order — a "Provider" is just connection/credentials, so every row of the
 * same type (e.g. two Smart TVs) shares one card instead of each looking like
 * an unrelated top-level integration. */
function groupProviders(providers: Provider[]): Provider[][] {
  const order: string[] = [];
  const byType = new Map<string, Provider[]>();
  for (const p of providers) {
    if (!byType.has(p.provider_type)) {
      order.push(p.provider_type);
      byType.set(p.provider_type, []);
    }
    byType.get(p.provider_type)!.push(p);
  }
  return order.map((t) => byType.get(t)!);
}

/** Provider types where one configured row IS one physical LAN device (a TV,
 * a plug) rather than a hub/account/bridge. These always render under a
 * type-titled `DeviceProviderGroup` card — even alone — because the row's
 * user-chosen name ("Raven Lights") names a *device*, and presenting it as a
 * top-level provider title reads like a phantom integration. Per-type: the
 * row/found-row glyph and the add-button label. */
const DEVICE_CENTRIC: Record<string, { glyph: string; addLabel: string }> = {
  smarttv: { glyph: "tv", addLabel: "Add TV" },
  kasa: { glyph: "outlet", addLabel: "Add device" },
};
/** Exported for the Devices page, which merges these types' provider rows
 * into one section the same way this page merges their cards. */
export const DEVICE_CENTRIC_TYPES = new Set(Object.keys(DEVICE_CENTRIC));

const TV_BRAND_LABELS: Record<string, string> = {
  bravia: "Bravia",
  androidtv: "Android TV",
};

/** Friendly label for a Smart-TV row's connected vendor — shown per-row once
 * multiple TVs share one group card, since "Smart TV" alone no longer tells
 * them apart. */
function tvBrandLabel(brand?: string | null): string {
  const key = brand ?? "bravia";
  return TV_BRAND_LABELS[key] ?? key;
}

/** One card for a whole device-centric provider type. Provider-level actions
 * (Discover-and-scan) live on the card header; each configured row is a
 * device line with only device-scoped actions (pairing, edit credentials,
 * remove). Deliberately its OWN component rather than a mode of
 * `ProviderCard`: the hub card and the device row share primitives (`Button`,
 * `Select`, `StatusBadge`, `EditCredentialsForm`) but almost no layout, and a
 * shared body would be half-disabled conditionals in both directions. */
function DeviceProviderGroup({
  providers,
  typeLabel,
  types,
  tvs,
  isCompact,
  onRemove,
  onCredentialsSaved,
  reload,
  showToast,
}: {
  /** All configured rows of this one device-centric `provider_type`. */
  providers: Provider[];
  typeLabel: string;
  types: ProviderType[];
  /** Media rows per provider id — a Smart-TV row renders its TV's name/IP. */
  tvs: Map<string, MediaDevice[]>;
  isCompact: boolean;
  onRemove: (p: Provider) => void;
  onCredentialsSaved: () => void;
  reload: () => Promise<void>;
  showToast: (msg: string) => void;
}) {
  const providerType = providers[0].provider_type;
  const cfg = DEVICE_CENTRIC[providerType];
  const [discovering, setDiscovering] = useState(false);
  // Found-but-unconfigured devices from the network scan (the server filters
  // out hosts already covered) — rendered as addable rows under the members.
  const [found, setFound] = useState<DiscoveredDevice[]>([]);
  const [addingHost, setAddingHost] = useState<string | null>(null);

  async function handleDiscover() {
    setDiscovering(true);
    try {
      let discovered = 0;
      for (const p of providers) {
        const r = await discoverProvider(p.id);
        discovered += r.discovered;
      }
      await reload();
      // The header Discover also answers "what else of this type is on the
      // network?" — the scan is type-wide, which is why it lives here and
      // not on any single device row.
      setFound(await scanForDevices(providerType));
      showToast(`Discovered ${discovered} device${discovered !== 1 ? "s" : ""}.`);
    } finally {
      setDiscovering(false);
    }
  }

  async function addFound(d: DiscoveredDevice) {
    setAddingHost(d.host);
    try {
      const creds = Object.fromEntries(
        Object.entries(d.credentials as Record<string, unknown>).map(([k, v]) => [k, String(v)]),
      );
      const name = d.label ?? `${typeLabel} (${d.host})`;
      const r = await addProvider(name, providerType, creds);
      if ("error" in r) {
        showToast(`Couldn't add ${name}: ${r.error}`);
        return;
      }
      await discoverProvider(r.id);
      await reload();
      setFound((prev) => prev.filter((x) => x.host !== d.host));
      showToast(`${name} added.`);
    } finally {
      setAddingHost(null);
    }
  }

  return (
    <div style={{ ...S.card, gap: "0.65rem" }}>
      <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", flexWrap: "wrap" }}>
        <span style={{ fontWeight: 700, letterSpacing: "0.05em" }}>{typeLabel}</span>
        <span style={{ color: "var(--bf-faint)", fontSize: "0.78rem" }}>
          {providers.length} device{providers.length !== 1 ? "s" : ""}
        </span>
        <span style={{ flex: 1 }} />
        <Button
          variant="ghost"
          onClick={handleDiscover}
          disabled={discovering}
          title="Refresh every device and scan the network for new ones"
        >
          {discovering ? "…" : "Discover"}
        </Button>
      </div>
      <div style={{ display: "flex", flexDirection: "column" }}>
        {providers.map((p) => (
          <DeviceProviderRow
            key={p.id}
            provider={p}
            glyph={cfg?.glyph ?? "power"}
            tv={(tvs.get(p.id) ?? [])[0]}
            types={types}
            onRemove={() => onRemove(p)}
            onCredentialsSaved={onCredentialsSaved}
          />
        ))}
        {found.map((d) => (
          <FoundNearbyRow
            key={d.host}
            device={d}
            glyph={cfg?.glyph ?? "power"}
            addLabel={cfg?.addLabel ?? "Add device"}
            isCompact={isCompact}
            adding={addingHost === d.host}
            onAdd={() => addFound(d)}
          />
        ))}
      </div>
    </div>
  );
}

/** One configured device inside a `DeviceProviderGroup`: identity + live
 * status on the left, device-scoped actions (remote pairing for TVs, edit
 * credentials, remove) on the right. Provider-level plumbing (Discover/Sync)
 * deliberately does NOT appear here — it's on the group header. */
function DeviceProviderRow({
  provider,
  glyph,
  tv,
  types,
  onRemove,
  onCredentialsSaved,
}: {
  provider: Provider;
  glyph: string;
  /** The row's imported media device (Smart TV) — names the physical set. */
  tv?: MediaDevice;
  types: ProviderType[];
  onRemove: () => void;
  onCredentialsSaved: () => void;
}) {
  const [status, setStatus] = useState<ConnectionStatus | null>(null);
  const [editingCreds, setEditingCreds] = useState(false);
  const isTv = provider.provider_type === "smarttv";
  // Android TV Remote pairing (smart-TV rows only): idle → code entry.
  const [pairStep, setPairStep] = useState<"idle" | "code">("idle");
  const [pairCode, setPairCode] = useState("");
  const [pairBusy, setPairBusy] = useState(false);
  const [pairMsg, setPairMsg] = useState("");

  useEffect(() => {
    getProviderStatus(provider.id).then(setStatus);
    const id = setInterval(() => getProviderStatus(provider.id).then(setStatus), 5000);
    return () => clearInterval(id);
  }, [provider.id]);

  async function handlePairRemote() {
    setPairBusy(true);
    setPairMsg("");
    try {
      const code = pairStep === "code" ? pairCode.trim() : undefined;
      const result = await pairSmartTvRemote(provider.id, code);
      if ("error" in result) {
        setPairMsg(result.message || "Pairing failed.");
      } else if (result.status === "code_displayed") {
        setPairStep("code");
        setPairMsg("Enter the code shown on the TV.");
      } else if (result.status === "paired") {
        setPairStep("idle");
        setPairCode("");
        setPairMsg("Remote paired ✓");
        onCredentialsSaved();
      }
    } catch {
      setPairMsg("Pairing request failed.");
    } finally {
      setPairBusy(false);
    }
  }

  const healthy = !status || ["connected", "ok", "ready"].includes(status.state);
  const paired = provider.remote_paired === true;

  const editBtn = (
    <Button
      variant="ghost"
      onClick={() => setEditingCreds((x) => !x)}
      title="Edit this device's connection details (IP, auth)"
    >
      {editingCreds ? "Close" : "Edit"}
    </Button>
  );
  const removeBtn = (
    <Button variant="ghost" onClick={onRemove} title="Remove this device and everything it imported">
      Remove
    </Button>
  );

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: "0.5rem",
        padding: "0.6rem 0",
        borderTop: "1px solid var(--bf-card-border, rgba(255,255,255,0.07))",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: "0.6rem", flexWrap: "wrap" }}>
        <Glyph name={isTv ? (tv?.glyph ?? glyph) : glyph} size={16} />
        <span
          style={{
            fontWeight: 600,
            fontSize: "0.85rem",
            minWidth: 0,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {isTv ? `${tvBrandLabel(provider.brand)} — ${tv?.name ?? provider.name}` : provider.name}
        </span>
        {healthy ? (
          <span
            title={status ? status.state : "checking…"}
            style={{
              width: 7,
              height: 7,
              borderRadius: "50%",
              background: "var(--bf-good)",
              boxShadow: "0 0 6px var(--bf-good)",
              flexShrink: 0,
            }}
          />
        ) : (
          status && <StatusBadge state={status.state} />
        )}
        {isTv && tv?.state.ip && (
          <span style={{ color: "var(--bf-faint)", fontSize: "0.74rem", fontVariantNumeric: "tabular-nums" }}>
            {tv.state.ip}
          </span>
        )}
        {isTv && !tv && (
          <span style={{ color: "var(--bf-faint)", fontSize: "0.74rem" }}>
            no TV imported — run Discover
          </span>
        )}
        <span style={{ flex: 1 }} />
        {isTv && paired && (
          <span
            title="Android TV Remote paired — keys, apps, and live state ride the native session"
            style={{ color: "var(--bf-good)", fontSize: "0.75rem", display: "inline-flex", alignItems: "center", gap: "0.3rem" }}
          >
            <Glyph name="remote" size={13} /> remote paired
          </span>
        )}
        {/* Non-TV rows are short enough that the actions share the line. */}
        {!isTv && (
          <>
            {editBtn}
            {removeBtn}
          </>
        )}
      </div>
      {/* TV rows: actions get their OWN line — mixed into the info line's
          flex-wrap they broke unevenly, leaving one straggler button next to
          the "remote paired" chip and the rest below. */}
      {isTv && (
        <div style={{ display: "flex", alignItems: "center", gap: "0.45rem", flexWrap: "wrap" }}>
          {!paired && (
            <Button
              variant="ghost"
              onClick={handlePairRemote}
              disabled={pairBusy}
              title="Pair the Android TV Remote — keys, app launch, and live now-playing"
            >
              {pairBusy ? "…" : pairStep === "code" ? "Confirm code" : "Pair remote"}
            </Button>
          )}
          {paired && pairStep === "idle" && (
            <Button variant="ghost" onClick={handlePairRemote} disabled={pairBusy} title="Re-pair (e.g. after a TV reset)">
              {pairBusy ? "…" : "Re-pair"}
            </Button>
          )}
          {editBtn}
          {removeBtn}
        </div>
      )}
      {isTv && (pairStep === "code" || pairMsg) && (
        <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", flexWrap: "wrap" }}>
          {pairStep === "code" && (
            <input
              value={pairCode}
              onChange={(e) => setPairCode(e.target.value)}
              placeholder="Code on TV (e.g. 1A2B3C)"
              autoComplete="off"
              spellCheck={false}
              style={{ ...S.input, maxWidth: 200, fontFamily: "monospace", letterSpacing: "0.1em" }}
            />
          )}
          {pairStep === "code" && (
            <Button variant="primary" onClick={handlePairRemote} disabled={pairBusy || !pairCode.trim()}>
              {pairBusy ? "…" : "Confirm code"}
            </Button>
          )}
          {pairMsg && <span style={{ fontSize: "0.78rem", color: "var(--bf-dim)" }}>{pairMsg}</span>}
        </div>
      )}
      {editingCreds && (
        <EditCredentialsForm
          provider={provider}
          schema={types.find((t) => t.provider_type === provider.provider_type)?.schema ?? []}
          onSaved={() => {
            setEditingCreds(false);
            onCredentialsSaved();
          }}
          onCancel={() => setEditingCreds(false)}
        />
      )}
    </div>
  );
}

/** One "found but not yet added" device row from a group-level Discover scan. */
function FoundNearbyRow({
  device,
  glyph,
  addLabel,
  isCompact,
  adding,
  onAdd,
}: {
  device: DiscoveredDevice;
  glyph: string;
  addLabel: string;
  isCompact: boolean;
  adding: boolean;
  onAdd: () => void;
}) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: "0.6rem",
        flexWrap: "wrap",
        padding: isCompact ? "0.55rem 0.6rem" : "0.4rem 0.6rem",
        borderRadius: 9,
        border: "1px dashed var(--bf-card-border, rgba(255,255,255,0.14))",
        color: "var(--bf-dim)",
      }}
    >
      <Glyph name={glyph} size={16} />
      <span style={{ fontWeight: 600, fontSize: "0.85rem" }}>{device.label ?? "Device"}</span>
      <span style={{ color: "var(--bf-faint)", fontSize: "0.74rem", fontVariantNumeric: "tabular-nums" }}>
        {device.host}
      </span>
      <span style={{ fontSize: "0.72rem", color: "var(--bf-faint)" }}>found nearby</span>
      <span style={{ flex: 1 }} />
      <Button variant="ghost" onClick={onAdd} disabled={adding} title="Add this device — it becomes a new row here">
        {adding ? "Adding…" : addLabel}
      </Button>
    </div>
  );
}

/** A hub/account/bridge provider's card (Hue, HA, Sonos, Govee, …) — one
 * card per configured row, with the provider-level actions (Discover, Sync,
 * prune, credentials, remove). Device-centric LAN types (Smart TV, Kasa)
 * never render this — they get `DeviceProviderGroup`, whose rows carry only
 * device-scoped actions. */
function ProviderCard({
  provider,
  types,
  deviceCount,
  onCredentialsSaved,
  onRemove,
  onDiscover,
  onPruneNow,
  onImportGroups,
  onSetPrune,
}: {
  provider: Provider;
  types: ProviderType[];
  /** Devices this provider currently serves (all domains). */
  deviceCount: number;
  onCredentialsSaved: () => void;
  onRemove: () => void;
  onDiscover: () => Promise<void>;
  /** One-shot: discover with prune (remove devices no longer reported). */
  onPruneNow: () => Promise<void>;
  onImportGroups: () => Promise<void>;
  onSetPrune: (prune: boolean) => Promise<void>;
}) {
  const [status, setStatus] = useState<ConnectionStatus | null>(null);
  const [discovering, setDiscovering] = useState(false);
  const [importing, setImporting] = useState(false);
  const [editingCreds, setEditingCreds] = useState(false);

  useEffect(() => {
    getProviderStatus(provider.id).then(setStatus);
    const id = setInterval(() => getProviderStatus(provider.id).then(setStatus), 5000);
    return () => clearInterval(id);
  }, [provider.id]);

  async function handleDiscover() {
    setDiscovering(true);
    try {
      await onDiscover();
    } finally {
      setDiscovering(false);
    }
  }

  async function handleImport() {
    setImporting(true);
    try {
      await onImportGroups();
    } finally {
      setImporting(false);
    }
  }

  // Rare actions live behind one quiet menu — the routine pair (Discover/Sync)
  // stays visible, Remove stops shouting from every card.
  const menu = (v: string) => {
    if (v === "edit") setEditingCreds((x) => !x);
    else if (v === "prune-now") onPruneNow();
    else if (v === "auto-prune") onSetPrune(!provider.prune);
    else if (v === "remove") onRemove();
  };

  const healthy = !status || ["connected", "ok", "ready"].includes(status.state);

  const statusDot = healthy ? (
    <span
      title={status ? status.state : "checking…"}
      style={{
        width: 7,
        height: 7,
        borderRadius: "50%",
        background: "var(--bf-good)",
        boxShadow: "0 0 6px var(--bf-good)",
        flexShrink: 0,
      }}
    />
  ) : (
    status && <StatusBadge state={status.state} />
  );

  const actionStrip = (
    <div style={{ display: "flex", gap: "0.45rem", flexShrink: 0, alignItems: "center" }}>
      <Button variant="ghost" onClick={handleDiscover} disabled={discovering}>
        {discovering ? "…" : "Discover"}
      </Button>
      <Button
        variant="ghost"
        onClick={handleImport}
        disabled={importing}
        title="Sync this provider's rooms/zones into Bifrost Rooms"
      >
        {importing ? "…" : "Sync"}
      </Button>
      <Select
        options={[
          { value: "edit", label: editingCreds ? "Close credentials" : "Edit credentials" },
          { value: "prune-now", label: "Prune missing devices" },
          {
            value: "auto-prune",
            label: provider.prune ? "Auto-prune on discover ✓" : "Auto-prune on discover",
          },
          { value: "remove", label: "Remove provider…" },
        ]}
        onChange={menu}
        placeholder="⋯"
        width={44}
        title="More actions"
      />
    </div>
  );

  const credsEditor = editingCreds && (
    <EditCredentialsForm
      provider={provider}
      schema={types.find((t) => t.provider_type === provider.provider_type)?.schema ?? []}
      onSaved={() => {
        setEditingCreds(false);
        onCredentialsSaved();
      }}
      onCancel={() => setEditingCreds(false)}
    />
  );

  return (
    <div style={{ ...S.card, gap: "0.65rem" }}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: "0.7rem",
          flexWrap: "wrap",
        }}
      >
        {/* Identity block: engraved name, quiet type line. The status dot only
            speaks (word + colour) when something needs attention. */}
        <div style={{ minWidth: 0, flex: "1 1 180px" }}>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: "0.5rem",
              minWidth: 0,
            }}
          >
            <span
              style={{
                fontWeight: 700,
                letterSpacing: "0.05em",
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
            >
              {provider.name}
            </span>
            {statusDot}
          </div>
          <div style={{ color: "var(--bf-dim)", fontSize: "0.76rem", marginTop: 2 }}>
            {provider.type_name}
            {provider.domain === "media" ? " · Audio" : ""}
            {provider.domain === "integration" ? " · Integration" : ""}
            {deviceCount > 0 && (
              <span style={{ color: "var(--bf-faint)" }}>
                {" "}
                · {deviceCount} device{deviceCount !== 1 ? "s" : ""}
              </span>
            )}
            {provider.prune && (
              <span title="Discover removes devices this provider no longer reports" style={{ color: ACCENT }}>
                {" "}
                · auto-prune
              </span>
            )}
          </div>
        </div>
        {actionStrip}
      </div>

      {credsEditor}
    </div>
  );
}

/// Edit an existing provider's IP and credentials in place. Non-secret fields
/// (host/IP) are prefilled; secret fields stay blank and keep their stored
/// value unless re-entered. The provider row (and all lights, scenes, groups,
/// plans referencing it) stays intact.
function EditCredentialsForm({
  provider,
  schema,
  onSaved,
  onCancel,
}: {
  provider: Provider;
  schema: CredentialField[];
  onSaved: () => void;
  onCancel: () => void;
}) {
  const [credentials, setCredentials] = useState<Record<string, string>>({});
  // When false, stored credentials couldn't be decrypted (e.g. BIFROST_SECRET
  // changed) — every field, secrets included, must be re-entered.
  const [decryptable, setDecryptable] = useState(true);
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [pairing, setPairing] = useState(false);
  const [pairMsg, setPairMsg] = useState("");

  // Prefill the current non-secret values so the user can tweak just the IP.
  useEffect(() => {
    getProviderConfig(provider.id).then((cfg) => {
      if (!cfg) return;
      setDecryptable(cfg.decryptable);
      setCredentials(
        Object.fromEntries(Object.entries(cfg.values).map(([k, v]) => [k, String(v)])),
      );
    });
  }, [provider.id]);

  function setField(name: string, value: string) {
    setCredentials((prev) => ({ ...prev, [name]: value }));
  }

  async function handlePair() {
    setPairing(true);
    setPairMsg("");
    const result = await pairHueBridge(credentials.bridge_ip ?? "");
    setPairing(false);
    if ("app_key" in result) {
      setField("app_key", result.app_key);
      setPairMsg("✓ Paired with bridge.");
    } else if (result.error === "link_button_not_pressed") {
      setPairMsg("Press the round link button on the bridge, then click Pair again.");
    } else {
      setPairMsg(`Could not reach the bridge: ${result.message}`);
    }
  }

  async function handleNanoleafRePair() {
    setPairing(true);
    setPairMsg("");
    const result = await pairNanoleaf(credentials.host ?? "");
    setPairing(false);
    if ("auth_token" in result) {
      setField("auth_token", result.auth_token);
      setPairMsg("✓ Paired with the controller.");
    } else if (result.error === "not_in_pairing_mode") {
      setPairMsg("Hold the controller's power button ~5-7s until the LED flashes, then click Pair again.");
    } else {
      setPairMsg(`Could not reach the controller: ${result.message}`);
    }
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setError("");
    setSaving(true);
    const result = await updateProviderCredentials(provider.id, credentials);
    setSaving(false);
    if ("error" in result) setError(result.error);
    else onSaved();
  }

  return (
    <form onSubmit={submit} style={{ display: "flex", flexDirection: "column", gap: "0.6rem", borderTop: "1px solid var(--bf-surfaceHi)", paddingTop: "0.75rem" }}>
      {!decryptable && (
        <p style={{ color: "#fa0", margin: 0, fontSize: "0.8rem" }}>
          The stored credentials couldn't be read (the encryption secret may have changed).
          Re-enter every field below.
        </p>
      )}
      {schema.map((field) => {
        const isHueAppKey = provider.provider_type === "hue" && field.name === "app_key";
        const isNanoleafToken = provider.provider_type === "nanoleaf" && field.name === "auth_token";
        const isSecret = field.kind === "password";
        // Secret fields keep their stored value when left blank, so they're
        // only required when we couldn't decrypt the existing credentials.
        const required = isSecret ? !decryptable : field.required;
        return (
          <label key={field.name} style={labelStyle}>
            <span>
              {field.label}
              {required && <span style={{ color: ACCENT }}> *</span>}
            </span>
            <div style={{ display: "flex", gap: "0.5rem" }}>
              <input
                type={field.kind === "password" ? "password" : "text"}
                value={credentials[field.name] ?? ""}
                onChange={(e) => setField(field.name, e.target.value)}
                placeholder={isSecret && decryptable ? "Leave blank to keep current" : undefined}
                style={{ ...S.input, flex: 1 }}
                required={required}
                autoComplete={field.kind === "password" ? "new-password" : "off"}
              />
              {isHueAppKey && (
                <Button variant="ghost"
                  type="button"
                  onClick={handlePair}
                  disabled={pairing || !(credentials.bridge_ip ?? "").trim()}
                >
                  {pairing ? "Pairing…" : "Pair"}
                </Button>
              )}
              {isNanoleafToken && (
                <Button variant="ghost"
                  type="button"
                  onClick={handleNanoleafRePair}
                  disabled={pairing || !(credentials.host ?? "").trim()}
                >
                  {pairing ? "Pairing…" : "Pair"}
                </Button>
              )}
            </div>
            {(isHueAppKey || isNanoleafToken) && pairMsg && (
              <span style={{ fontSize: "0.78rem", color: pairMsg.startsWith("✓") ? "var(--bf-good)" : "#fa0" }}>
                {pairMsg}
              </span>
            )}
          </label>
        );
      })}
      {error && <p style={{ color: "var(--bf-rose)", margin: 0, fontSize: "0.875rem" }}>{error}</p>}
      <div style={{ display: "flex", gap: "0.5rem" }}>
        <Button type="submit" disabled={saving}>
          {saving ? "Saving…" : "Save"}
        </Button>
        <Button variant="ghost" type="button" onClick={onCancel}>Cancel</Button>
      </div>
    </form>
  );
}

function StatusBadge({ state }: { state: string }) {
  // "ready" = an on-demand provider (e.g. Sonos) with no persistent connection
  // but fully operational — green, like connected.
  const color =
    state === "connected" || state === "ok" || state === "ready" ? "var(--bf-good)"
    : state === "connecting" || state === "reconnecting" ? "#fa0"
    : state === "failed" ? "var(--bf-rose)"
    : "var(--bf-faint)";
  const label = state === "ready" ? "ready" : state;
  return (
    <span style={{ display: "inline-flex", alignItems: "center", gap: "0.3rem", fontSize: "0.75rem", color }}>
      <span style={{ width: 7, height: 7, borderRadius: "50%", background: color, display: "inline-block" }} />
      {label}
    </span>
  );
}

function AddProviderForm({
  types,
  prefill,
  onAdded,
  onCancel,
}: {
  types: ProviderType[];
  prefill?: AddPrefill | null;
  onAdded: (id: string) => void;
  onCancel: () => void;
}) {
  const [selectedType, setSelectedType] = useState(
    prefill?.provider_type ?? types[0]?.provider_type ?? "",
  );
  const [name, setName] = useState(prefill?.name ?? "");
  const [credentials, setCredentials] = useState<Record<string, string>>(
    prefill?.credentials ?? {},
  );
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [pairing, setPairing] = useState(false);
  const [pairMsg, setPairMsg] = useState("");
  // Smart-TV PIN pairing: once the TV shows a PIN, collect it here for step 2.
  const [tvPin, setTvPin] = useState("");
  const [tvPinStep, setTvPinStep] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [scanned, setScanned] = useState(false);
  const [found, setFound] = useState<DiscoveredDevice[]>([]);

  const selected = types.find((t) => t.provider_type === selectedType);
  const schema: CredentialField[] = selected?.schema ?? [];

  // Default the provider name to the selected type's display name (e.g. "Smart
  // TV"), until the user types their own — so the name field is never empty.
  const autoName = useRef(name);
  useEffect(() => {
    if (!name.trim() && selected?.display_name) {
      setName(selected.display_name);
      autoName.current = selected.display_name;
    }
    // Only seed from the initial type once types load; later changes go via pickType.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [types.length]);

  function pickType(t: string) {
    setSelectedType(t);
    const dn = types.find((x) => x.provider_type === t)?.display_name ?? "";
    // Re-default the name only while it's still an auto value (or blank).
    if (dn && (!name.trim() || name === autoName.current)) {
      setName(dn);
      autoName.current = dn;
    }
  }

  // Clear per-type state when the user switches the provider type — but not on
  // the first render, so a prefilled (found-device) form keeps its credentials.
  const firstRun = useRef(true);
  useEffect(() => {
    if (firstRun.current) {
      firstRun.current = false;
      return;
    }
    setCredentials({});
    setPairMsg("");
    setScanned(false);
    setFound([]);
  }, [selectedType]);

  // Scan the LAN and let the user pick a found device to fill the form.
  async function handleScan() {
    setScanning(true);
    setScanned(false);
    const devices = await scanForDevices(selectedType);
    setFound(devices);
    setScanned(true);
    setScanning(false);
  }

  function applyFound(d: DiscoveredDevice) {
    // Credential values arrive as JSON; the form holds strings.
    const creds = Object.fromEntries(
      Object.entries(d.credentials).map(([k, v]) => [k, String(v)]),
    );
    setCredentials((prev) => ({ ...prev, ...creds }));
    if (!name.trim()) setName(d.label ?? d.host);
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    const result = await addProvider(name, selectedType, credentials);
    setLoading(false);
    if ("error" in result) setError(result.error);
    else onAdded(result.id);
  }

  function setField(fieldName: string, value: string) {
    setCredentials((prev) => ({ ...prev, [fieldName]: value }));
  }

  // Hue link-button pairing: fetch the app key from the bridge so the user
  // never has to curl it manually.
  async function handlePair() {
    setPairing(true);
    setPairMsg("");
    const result = await pairHueBridge(credentials.bridge_ip ?? "");
    setPairing(false);
    if ("app_key" in result) {
      setField("app_key", result.app_key);
      setPairMsg("✓ Paired with bridge.");
    } else if (result.error === "link_button_not_pressed") {
      setPairMsg("Press the round link button on the bridge, then click Pair again.");
    } else {
      setPairMsg(`Could not reach the bridge: ${result.message}`);
    }
  }

  // Nanoleaf pairing: the controller mints the token while its LED flashes.
  async function handleNanoleafPair() {
    setPairing(true);
    setPairMsg("");
    const result = await pairNanoleaf(credentials.host ?? "");
    setPairing(false);
    if ("auth_token" in result) {
      setField("auth_token", result.auth_token);
      setPairMsg("✓ Paired with the controller.");
    } else if (result.error === "not_in_pairing_mode") {
      setPairMsg("Hold the controller's power button ~5-7s until the LED flashes, then click Pair again.");
    } else {
      setPairMsg(`Could not reach the controller: ${result.message}`);
    }
  }

  // Smart-TV (Bravia) PIN pairing — two steps: first call makes the TV show a
  // PIN; the second submits it and stores the returned auth token.
  async function handleTvPair() {
    setPairing(true);
    setPairMsg("");
    const result = await pairSmartTv(credentials.host ?? "", tvPinStep ? tvPin : undefined);
    setPairing(false);
    if ("status" in result && result.status === "paired") {
      setField("auth", result.auth);
      setTvPinStep(false);
      setTvPin("");
      setPairMsg("✓ Paired with TV.");
    } else if ("status" in result && result.status === "pin_displayed") {
      setTvPinStep(true);
      setPairMsg("Enter the PIN shown on the TV, then click Submit PIN.");
    } else if ("status" in result && result.status === "not_required") {
      // IP-control Authentication is "None": the TV takes commands without a
      // token, so there's nothing to pair — just add the provider.
      setField("auth", "");
      setTvPinStep(false);
      setTvPin("");
      setPairMsg("✓ No pairing needed — this TV allows control without a token.");
    } else if ("error" in result) {
      setPairMsg(`Could not reach the TV: ${result.message}`);
    }
  }

  return (
    <form onSubmit={submit} style={{ ...S.card, border: "1px solid var(--bf-border)" }}>
      <h3 style={{ margin: 0, fontSize: "1rem", color: "var(--bf-dim)" }}>Add Provider</h3>

      <label style={labelStyle}>
        <span>Name</span>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="e.g. Living Room Hue"
          style={S.input}
          required
          autoFocus
        />
      </label>

      <label style={labelStyle}>
        <span>Type</span>
        <Select
          value={selectedType}
          onChange={pickType}
          style={{ width: "100%" }}
          options={(["light", "media", "power", "integration"] as const).flatMap((kind) =>
            types
              .filter((t) => t.kind === kind)
              .map((t) => ({
                value: t.provider_type,
                label: t.display_name,
                group:
                  kind === "light"
                    ? "Lights"
                    : kind === "media"
                      ? "Media"
                      : kind === "power"
                        ? "Power"
                        : "Integrations",
              })),
          )}
        />
      </label>

      {selected?.supports_discovery && (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.4rem" }}>
          <Button variant="ghost"
            type="button"
            onClick={handleScan}
            disabled={scanning}
          >
            {scanning ? "Scanning network…" : "Scan network for devices"}
          </Button>
          {found.map((d) => (
            <Button variant="ghost"
              key={d.host}
              type="button"
              onClick={() => applyFound(d)}
              title={`Use ${d.host}`} style={{ textAlign: "left",
                fontSize: "0.82rem",
                borderColor: ACCENT }}
            >
              {d.label ? `${d.label} · ${d.host}` : d.host}
            </Button>
          ))}
          {scanned && found.length === 0 && (
            <span style={{ color: "var(--bf-dim)", fontSize: "0.78rem" }}>
              No devices found. Make sure they're powered on and on the same network
              (auto-detect needs host networking in Docker).
            </span>
          )}
        </div>
      )}

      {schema.map((field) => {
        // Hue's app key comes from link-button pairing, not manual entry.
        const isHueAppKey = selectedType === "hue" && field.name === "app_key";
        // A Smart TV's auth token comes from PIN pairing, not manual entry.
        const isTvAuth = selectedType === "smarttv" && field.name === "auth";
        // A Nanoleaf token comes from power-button pairing, not manual entry.
        const isNanoleafToken = selectedType === "nanoleaf" && field.name === "auth_token";
        return (
          <label key={field.name} style={labelStyle}>
            <span>
              {field.label}
              {field.required && <span style={{ color: ACCENT }}> *</span>}
            </span>
            {!isHueAppKey && !isTvAuth && !isNanoleafToken && field.hint && (
              <span style={{ color: "var(--bf-faint)", fontSize: "0.78rem" }}>{field.hint}</span>
            )}
            {isNanoleafToken && (
              <span style={{ color: "var(--bf-faint)", fontSize: "0.78rem" }}>
                Hold the controller's power button ~5-7s until the LED flashes, then click Pair — or paste a token manually.
              </span>
            )}
            {isHueAppKey && (
              <span style={{ color: "var(--bf-faint)", fontSize: "0.78rem" }}>
                Press the link button on the bridge, then click Pair — or paste a key manually.
              </span>
            )}
            {isTvAuth && (
              <span style={{ color: "var(--bf-faint)", fontSize: "0.78rem" }}>
                Click Pair — the TV shows a PIN — then enter it. (Set the TV's IP above first.)
              </span>
            )}
            <div style={{ display: "flex", gap: "0.5rem" }}>
              <input
                type={field.kind === "password" ? "password" : "text"}
                value={credentials[field.name] ?? ""}
                onChange={(e) => setField(field.name, e.target.value)}
                style={{ ...S.input, flex: 1 }}
                required={field.required}
                autoComplete={field.kind === "password" ? "new-password" : "off"}
              />
              {isHueAppKey && (
                <Button variant="ghost"
                  type="button"
                  onClick={handlePair}
                  disabled={pairing || !(credentials.bridge_ip ?? "").trim()}
                >
                  {pairing ? "Pairing…" : "Pair"}
                </Button>
              )}
              {isTvAuth && !tvPinStep && (
                <Button
                  variant="ghost"
                  type="button"
                  onClick={handleTvPair}
                  disabled={pairing || !(credentials.host ?? "").trim()}
                >
                  {pairing ? "Pairing…" : "Pair"}
                </Button>
              )}
              {isNanoleafToken && (
                <Button
                  variant="ghost"
                  type="button"
                  onClick={handleNanoleafPair}
                  disabled={pairing || !(credentials.host ?? "").trim()}
                >
                  {pairing ? "Pairing…" : "Pair"}
                </Button>
              )}
            </div>
            {isTvAuth && tvPinStep && (
              <div style={{ display: "flex", gap: "0.5rem", marginTop: "0.4rem" }}>
                <input
                  value={tvPin}
                  onChange={(e) => setTvPin(e.target.value)}
                  placeholder="PIN from TV"
                  inputMode="numeric"
                  style={{ ...S.input, flex: 1 }}
                />
                <Button
                  variant="ghost"
                  type="button"
                  onClick={handleTvPair}
                  disabled={pairing || !tvPin.trim()}
                >
                  {pairing ? "Submitting…" : "Submit PIN"}
                </Button>
              </div>
            )}
            {(isHueAppKey || isTvAuth || isNanoleafToken) && pairMsg && (
              <span
                style={{
                  fontSize: "0.78rem",
                  color: pairMsg.startsWith("✓") ? "var(--bf-good)" : "#fa0",
                }}
              >
                {pairMsg}
              </span>
            )}
          </label>
        );
      })}

      {error && <p style={{ color: "var(--bf-rose)", margin: 0, fontSize: "0.875rem" }}>{error}</p>}

      <div style={{ display: "flex", gap: "0.5rem" }}>
        <Button type="submit" disabled={loading || types.length === 0}>
          {loading ? "Adding…" : "Add"}
        </Button>
        <Button variant="ghost" type="button" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

const labelStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: "0.3rem",
  fontSize: "0.875rem",
  color: "var(--bf-dim)",
};

// ── AI model endpoints (Voice & AI tab) ──────────────────────────────────────

const AI_ROLES: { role: AiRole; title: string; blurb: string; placeholder: string; modelHint: string }[] = [
  {
    role: "chat",
    title: "Chat (command LLM)",
    blurb: "Interprets voice commands the built-in grammar can't parse. OpenAI-compatible /chat/completions with tool-calling.",
    placeholder: "e.g. http://localhost:11434/v1",
    modelHint: "model (e.g. qwen2.5:3b)",
  },
  {
    role: "transcription",
    title: "Transcription (speech-to-text)",
    blurb: "Server-side STT for clients that upload audio to /api/voice/listen. The wall-tablet kiosk transcribes on-device with Vosk and does NOT use this — leave it unset unless a client sends raw audio.",
    placeholder: "e.g. http://localhost:8080/v1",
    modelHint: "model (e.g. whisper-1)",
  },
  {
    role: "tts",
    title: "Text-to-speech",
    blurb: "Spoken replies for voice talk-back. Needs an OpenAI-compatible speech server (Piper/Kokoro/openedai-speech, or vocals-mcp serve-openai) — NOT an LLM/Ollama endpoint.",
    placeholder: "e.g. http://localhost:9123/v1",
    modelHint: "voice / model (e.g. tts-1)",
  },
];

function AiEndpointsSection({ dialogs }: { dialogs: Dialogs }) {
  const [eps, setEps] = useState<AiEndpoint[]>([]);
  async function load() {
    setEps(await getAiEndpoints());
  }
  useEffect(() => {
    load();
  }, []);
  return (
    <section style={{ marginTop: "0.25rem" }}>
      <SectionLabel style={{ marginBottom: "0.4rem" }}>AI model endpoints</SectionLabel>
      <p style={{ margin: "0 0 1rem", color: "var(--bf-faint)", fontSize: "0.85rem" }}>
        Optional, pluggable local/OSS models for the voice pipeline (Ollama, LocalAI, …). Each role
        degrades gracefully when unset — the built-in command grammar always works.
      </p>
      <div style={{ display: "flex", flexDirection: "column", gap: "0.75rem" }}>
        {AI_ROLES.map((r) => (
          <AiEndpointCard
            key={r.role}
            meta={r}
            current={eps.find((e) => e.role === r.role)}
            dialogs={dialogs}
            onSaved={load}
          />
        ))}
      </div>
    </section>
  );
}

function AiEndpointCard({
  meta,
  current,
  dialogs,
  onSaved,
}: {
  meta: { role: AiRole; title: string; blurb: string; placeholder: string; modelHint: string };
  current?: AiEndpoint;
  dialogs: Dialogs;
  onSaved: () => void;
}) {
  const [baseUrl, setBaseUrl] = useState("");
  const [model, setModel] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [busy, setBusy] = useState(false);
  const [test, setTest] = useState<{ ok: boolean; message: string } | null>(null);

  // Sync local fields when the loaded endpoint arrives/changes (load is async).
  useEffect(() => {
    setBaseUrl(current?.base_url ?? "");
    setModel(current?.model ?? "");
    setEnabled(current?.enabled ?? true);
    setApiKey("");
  }, [current?.base_url, current?.model, current?.enabled]);

  const configured = !!current;

  async function save() {
    if (!baseUrl.trim() || !model.trim()) {
      await dialogs.alert({ title: "Missing fields", message: "Base URL and model are required." });
      return;
    }
    setBusy(true);
    try {
      await putAiEndpoint(meta.role, {
        base_url: baseUrl.trim(),
        model: model.trim(),
        // Send the key only when the user typed one; otherwise preserve the stored one.
        ...(apiKey.trim() ? { api_key: apiKey.trim() } : {}),
        enabled,
      });
      setApiKey("");
      setTest(null);
      onSaved();
    } catch (e) {
      await dialogs.alert({ title: "Couldn't save", message: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  }

  // Enable/disable persists immediately for an already-configured role (it just
  // flips the stored `enabled` flag, preserving the saved URL/model/key) so the
  // switch reflects real server state — no Save needed. Optimistic with revert.
  async function toggleEnabled(next: boolean) {
    if (!current) return;
    setEnabled(next);
    setBusy(true);
    try {
      await putAiEndpoint(meta.role, {
        base_url: current.base_url,
        model: current.model,
        enabled: next,
      });
      onSaved();
    } catch (e) {
      setEnabled(!next);
      await dialogs.alert({ title: "Couldn't update", message: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  }

  async function runTest() {
    setBusy(true);
    setTest(null);
    try {
      setTest(await testAiEndpoint(meta.role));
    } finally {
      setBusy(false);
    }
  }

  // TTS only: synthesize a short sample and play it in the browser — proves
  // in-app playback end-to-end (vs `runTest`, which only pings /models).
  async function playSample() {
    setBusy(true);
    setTest(null);
    try {
      await speak("Bifrost text to speech is online.", { force: true });
      setTest({ ok: true, message: "playing sample…" });
    } catch (e) {
      setTest({ ok: false, message: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  }

  async function clear() {
    const ok = await dialogs.confirm({
      title: "Clear endpoint",
      message: `Remove the ${meta.title} endpoint?`,
      confirmLabel: "Clear",
      danger: true,
    });
    if (!ok) return;
    await deleteAiEndpoint(meta.role);
    onSaved();
  }

  return (
    <div style={{ ...S.card, gap: "0.6rem" }}>
      <div style={{ minWidth: 0 }}>
        <div style={{ fontWeight: 600, display: "flex", alignItems: "center", gap: "0.5rem" }}>
          {meta.title}
          {configured && (
            <span style={{ fontSize: "0.7rem", color: enabled ? "var(--bf-good)" : "var(--bf-faint)" }}>
              {enabled ? "● enabled" : "○ disabled"}
            </span>
          )}
        </div>
        <div style={{ color: "var(--bf-faint)", fontSize: "0.78rem", marginTop: "0.2rem" }}>{meta.blurb}</div>
      </div>
      <input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder={meta.placeholder} style={{ ...S.input }} />
      <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap" }}>
        <input value={model} onChange={(e) => setModel(e.target.value)} placeholder={meta.modelHint} style={{ ...S.input, flex: 1, minWidth: 130 }} />
        <input
          value={apiKey}
          onChange={(e) => setApiKey(e.target.value)}
          type="password"
          placeholder={current?.has_key ? "•••• key set — blank keeps it" : "API key (optional)"}
          style={{ ...S.input, flex: 1, minWidth: 130 }}
        />
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", flexWrap: "wrap" }}>
        {configured && (
          <label style={{ display: "flex", alignItems: "center", gap: "0.45rem", fontSize: "0.82rem", color: "var(--bf-dim)", cursor: busy ? "default" : "pointer" }}>
            <Switch on={enabled} onChange={toggleEnabled} disabled={busy} /> Enabled
          </label>
        )}
        <div style={{ flex: 1 }} />
        {test && (
          <span style={{ fontSize: "0.78rem", color: test.ok ? "var(--bf-good)" : "var(--bf-rose, #e57)" }}>
            {test.ok ? `✓ ${test.message}` : `✗ ${test.message}`}
          </span>
        )}
        {configured && meta.role === "tts" && (
          <Button variant="ghost" onClick={playSample} disabled={busy} style={{ padding: "0.3rem 0.7rem", fontSize: "0.78rem" }}>
            Play
          </Button>
        )}
        {configured && (
          <Button variant="ghost" onClick={runTest} disabled={busy} style={{ padding: "0.3rem 0.7rem", fontSize: "0.78rem" }}>
            Test
          </Button>
        )}
        {configured && (
          <Button variant="danger" onClick={clear} disabled={busy} style={{ padding: "0.3rem 0.7rem", fontSize: "0.78rem" }}>
            Clear
          </Button>
        )}
        <Button onClick={save} disabled={busy}>
          {busy ? "Saving…" : "Save"}
        </Button>
      </div>
    </div>
  );
}

// ── Kiosks (wall-tablet companion apps) ──────────────────────────────────────

/** Clients tab. Owns the hub's cached-APK manifest so a "Check for updates" in
 * [KioskUpdateSection] live-updates the per-kiosk Update buttons in
 * [KiosksSection] without a tab round-trip (siblings reading one source). */
function ClientsTab({ dialogs }: { dialogs: Dialogs }) {
  const [latest, setLatest] = useState<KioskUpdateManifest | null>(null);
  const reloadLatest = useCallback(() => {
    getKioskUpdateStatus().then((s) => setLatest(s.cached));
  }, []);
  useEffect(() => {
    reloadLatest();
  }, [reloadLatest]);
  return (
    <>
      <ApiKeysSection dialogs={dialogs} />
      <KiosksSection dialogs={dialogs} latest={latest} />
      <KioskUpdateSection dialogs={dialogs} latest={latest} onCacheChanged={reloadLatest} />
    </>
  );
}

/** One-line battery/power summary for a kiosk: level, charging draw (V×I), and
 * temperature. Watts are computed from the reported voltage + current. */
function batteryMeta(k: Kiosk): string {
  if (k.battery_level == null) return "";
  const parts: string[] = [`${k.battery_charging ? "⚡" : "🔋"} ${k.battery_level}%`];
  if (k.battery_voltage_mv != null && k.battery_current_ua != null) {
    // mV × µA → W (sign varies by vendor, so show magnitude while charging).
    const watts = Math.abs((k.battery_voltage_mv / 1000) * (k.battery_current_ua / 1e6));
    if (watts >= 0.05) parts.push(`${watts.toFixed(1)} W`);
  }
  if (k.battery_voltage_mv != null) parts.push(`${(k.battery_voltage_mv / 1000).toFixed(2)} V`);
  if (k.battery_temp_dc != null) parts.push(`${(k.battery_temp_dc / 10).toFixed(1)}°C`);
  if (k.power_source && k.power_source !== "none") parts.push(k.power_source.toUpperCase());
  return parts.join(" · ");
}

/** Compare dotted numeric versions ("0.2.6" vs "0.2.4"): >0 if a>b, <0 if a<b,
 * 0 if equal. Used so the hub only offers a *newer* cached APK, never a downgrade. */
function cmpVersion(a: string, b: string): number {
  const pa = a.split(".").map((n) => parseInt(n, 10) || 0);
  const pb = b.split(".").map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

/** Per-kiosk scheduled quiet hours (display power saving): a toggle plus the
 * sleep/wake times, in the hub's local clock. Enabling defaults to 23:00 → 07:00
 * when unset; editing a time commits on blur. The scheduler emits the same
 * sleep/wake commands a human would, so a manual wake mid-window is respected
 * until the next boundary. */
/** Per-kiosk display plan: a paintable 24-hour timeline where every local
 * hour is one of three modes — Awake (screen forced on), Aware (presence-
 * controlled: wake on motion, off after the no-motion timer), Asleep (screen
 * forced off, beats an occupied room). One picture replaces the old sleep
 * window + presence toggle pair; painting saves as `hour_modes` (mig 0059) and
 * supersedes the legacy fields on this kiosk. */
function KioskDisplayPlan({
  k,
  dialogs,
  onSaved,
}: {
  k: Kiosk;
  dialogs: Dialogs;
  onSaved: () => void;
}) {
  const [enabled, setEnabled] = useState(k.schedule_enabled);
  const [plan, setPlan] = useState<string>(() => seedHourPlan(k));
  const [mins, setMins] = useState(Math.round((k.presence_timeout_secs ?? 600) / 60));
  const [brush, setBrush] = useState<KioskHourMode>("A");
  const [busy, setBusy] = useState(false);
  const barRef = useRef<HTMLDivElement>(null);
  // Painting mutates a ref alongside state so pointer-up saves the fresh plan,
  // not a stale render's copy.
  const planRef = useRef(plan);
  const painting = useRef(false);

  // Re-sync when the parent reloads the kiosk list (another edit, a check-in).
  useEffect(() => {
    setEnabled(k.schedule_enabled);
    const seeded = seedHourPlan(k);
    setPlan(seeded);
    planRef.current = seeded;
    setMins(Math.round((k.presence_timeout_secs ?? 600) / 60));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [k.schedule_enabled, k.hour_modes, k.presence_timeout_secs, k.sleep_at, k.wake_at, k.presence_enabled]);

  async function save(next: { enabled: boolean; hour_modes: string; timeout_secs?: number }) {
    setBusy(true);
    try {
      await setKioskPlan(k.id, next);
      onSaved();
    } catch (e) {
      await dialogs.alert({
        title: "Couldn't save display plan",
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(false);
    }
  }

  function paintAt(clientX: number) {
    const r = barRef.current?.getBoundingClientRect();
    if (!r || r.width === 0) return;
    const i = Math.max(0, Math.min(23, Math.floor(((clientX - r.left) / r.width) * 24)));
    if (planRef.current[i] === brush) return;
    planRef.current = planRef.current.slice(0, i) + brush + planRef.current.slice(i + 1);
    setPlan(planRef.current);
  }

  const MODES: { mode: KioskHourMode; label: string; hint: string; c: string }[] = [
    { mode: "W", label: "Awake", hint: "screen always on", c: color.gold },
    { mode: "A", label: "Aware", hint: "follows room presence — wake on motion, off when empty", c: color.good },
    { mode: "S", label: "Asleep", hint: "screen always off (beats an occupied room)", c: color.violet },
  ];
  const modeColor = (m: string) => MODES.find((x) => x.mode === m)?.c ?? color.faint;
  const nowHour = new Date().getHours();
  const anyAware = plan.includes("A");

  const numInput = {
    background: "var(--bf-panel, #1a1320)",
    color: enabled ? "var(--bf-text, #eee)" : "var(--bf-faint)",
    border: "1px solid var(--bf-hairline, #333)",
    borderRadius: 6,
    padding: "0.25rem 0.4rem",
    fontSize: "0.78rem",
    width: "3.2rem",
  };

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: "0.45rem",
        fontSize: "0.78rem",
        color: "var(--bf-faint)",
        borderTop: "1px solid var(--bf-hairline, #2a2233)",
        paddingTop: "0.5rem",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", flexWrap: "wrap" }}>
        <span style={{ display: "inline-flex", alignItems: "center", gap: "0.3rem", color: "var(--bf-dim)" }}>
          <Glyph name="wx_moon" size={15} /> Display plan
        </span>
        <Switch
          on={enabled}
          disabled={busy}
          onChange={(v) => {
            setEnabled(v);
            save({ enabled: v, hour_modes: planRef.current, timeout_secs: mins * 60 });
          }}
        />
        {/* Brush chips double as the legend. */}
        <span style={{ display: "inline-flex", gap: "0.3rem", opacity: enabled ? 1 : 0.55 }}>
          {MODES.map(({ mode, label, hint, c }) => (
            <button
              key={mode}
              onClick={() => setBrush(mode)}
              title={hint}
              style={{
                display: "inline-flex",
                alignItems: "center",
                gap: "0.3rem",
                padding: "0.15rem 0.5rem",
                borderRadius: 999,
                fontSize: "0.72rem",
                cursor: "pointer",
                color: brush === mode ? "var(--bf-text)" : "var(--bf-dim)",
                background: brush === mode ? alpha(c, 0.22) : "transparent",
                border: `1px solid ${brush === mode ? c : "var(--bf-hairline, #333)"}`,
              }}
            >
              <span style={{ width: 8, height: 8, borderRadius: 2, background: alpha(c, 0.85), flexShrink: 0 }} />
              {label}
            </button>
          ))}
        </span>
        <span style={{ opacity: enabled && anyAware ? 1 : 0.55, display: "inline-flex", alignItems: "center", gap: "0.35rem" }}>
          aware: off after
          <input
            type="number"
            min={1}
            max={60}
            value={mins}
            disabled={busy}
            onChange={(e) => setMins(Math.max(1, Number(e.target.value) || 1))}
            onBlur={() => save({ enabled, hour_modes: planRef.current, timeout_secs: mins * 60 })}
            style={numInput}
          />
          min empty
        </span>
      </div>

      {/* The 24-hour timeline — click or drag to paint with the selected mode.
          One contiguous strip (touching square cells, hairline hour seams),
          flanked AM · PM. */}
      <div style={{ opacity: enabled ? 1 : 0.45, display: "flex", alignItems: "flex-end", gap: "0.4rem" }}>
        <span style={{ fontSize: "0.62rem", color: "var(--bf-dim)", letterSpacing: "0.08em", paddingBottom: 8 }}>AM</span>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(24, 1fr)", marginBottom: 2 }}>
            {Array.from({ length: 24 }, (_, i) => (
              <span key={i} style={{ textAlign: "center", fontSize: "0.6rem", color: i === nowHour ? "var(--bf-text)" : "var(--bf-faint)" }}>
                {i}
              </span>
            ))}
          </div>
          <div
            ref={barRef}
            onPointerDown={(e) => {
              if (busy || !enabled) return;
              e.preventDefault();
              (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
              painting.current = true;
              paintAt(e.clientX);
            }}
            onPointerMove={(e) => {
              if (painting.current) paintAt(e.clientX);
            }}
            onPointerUp={() => {
              if (!painting.current) return;
              painting.current = false;
              save({ enabled, hour_modes: planRef.current, timeout_secs: mins * 60 });
            }}
            onPointerCancel={() => {
              painting.current = false;
            }}
            title="Click or drag to paint hours with the selected mode"
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(24, 1fr)",
              border: "1px solid var(--bf-hairline, #333)",
              touchAction: "none",
              cursor: enabled ? "pointer" : "default",
            }}
          >
            {plan.split("").map((m, i) => (
              <span
                key={i}
                style={{
                  height: 26,
                  background: alpha(modeColor(m), m === "S" ? 0.32 : 0.5),
                  // Hairline hour seams instead of per-cell chrome; the current
                  // hour wears an INSET ring so touching neighbours don't overlap.
                  borderRight: i < 23 ? "1px solid rgba(0,0,0,0.35)" : "none",
                  boxShadow: i === nowHour ? "inset 0 0 0 1px var(--bf-text)" : undefined,
                }}
              />
            ))}
          </div>
        </div>
        <span style={{ fontSize: "0.62rem", color: "var(--bf-dim)", letterSpacing: "0.08em", paddingBottom: 8 }}>PM</span>
      </div>
      <div style={{ display: "flex", justifyContent: "space-between" }}>
        <span style={{ opacity: 0.7 }}>hub local time — paint with the selected mode</span>
        {enabled && anyAware && !k.room_id && (
          <span style={{ color: "var(--bf-rose, #e57)" }}>aware hours need a room with motion sensors — assign one at left</span>
        )}
      </div>
    </div>
  );
}

/** Initial plan for a kiosk that hasn't painted one: derived from its legacy
 * sleep window + presence flag, so the picture opens showing what the old
 * config already did. */
function seedHourPlan(k: Kiosk): string {
  if (k.hour_modes && /^[WSA]{24}$/.test(k.hour_modes)) return k.hour_modes;
  const base: KioskHourMode = k.presence_enabled ? "A" : "W";
  const arr: KioskHourMode[] = Array.from({ length: 24 }, () => base);
  const hour = (s: string | null) => {
    const m = s?.match(/^(\d{1,2}):/);
    return m ? Math.min(23, Number(m[1])) : null;
  };
  const off = hour(k.sleep_at);
  const on = hour(k.wake_at);
  if (k.schedule_enabled && off !== null && on !== null && off !== on) {
    for (let i = off; i !== on; i = (i + 1) % 24) arr[i] = "S";
  }
  return arr.join("");
}

function KiosksSection({
  dialogs,
  latest,
}: {
  dialogs: Dialogs;
  latest: KioskUpdateManifest | null;
}) {
  const { isMobile } = useViewport();
  const [kiosks, setKiosks] = useState<Kiosk[]>([]);
  const [rooms, setRooms] = useState<Room[]>([]);
  const [boards, setBoards] = useState<Dashboard[]>([]);
  // For the "aware override" device picker only — loaded once, not polled
  // (unlike kiosks, these don't need to track live state here).
  const [lights, setLights] = useState<Light[]>([]);
  const [media, setMedia] = useState<MediaDevice[]>([]);
  const [power, setPower] = useState<PowerDevice[]>([]);
  // kioskId → target version we pushed, so the row can show "Updating…" until the
  // kiosk re-checks-in reporting that version (it goes offline mid-install).
  const [updating, setUpdating] = useState<Record<string, string>>({});
  async function load() {
    setKiosks(await getKiosks());
  }
  useEffect(() => {
    load();
    getRooms().then(setRooms);
    getDashboards().then(setBoards);
    getLights().then((l) => setLights(l === "unauthorized" ? [] : l));
    getMediaDevices().then(setMedia);
    getPowerDevices().then(setPower);
  }, []);

  // Kiosk state changes REMOTELY (each check-in refreshes screen state and
  // battery, and consumes the queued command), so nothing on this client can
  // trigger a refetch — poll while the section is on screen, matching the
  // check-in cadence, and skip ticks in a hidden browser tab. (The Media
  // page's self-healing poll is the precedent for remote-origin state.)
  useEffect(() => {
    const t = setInterval(() => {
      if (!document.hidden) load();
    }, 5000);
    return () => clearInterval(t);
  }, []);

  // While any update is in flight, poll faster so the version/online status
  // refresh as the kiosk downloads, installs, restarts, and checks back in.
  useEffect(() => {
    if (Object.keys(updating).length === 0) return;
    const t = setInterval(load, 4000);
    return () => clearInterval(t);
  }, [updating]);

  // Clear the "Updating…" marker once a kiosk reports the target version.
  useEffect(() => {
    setUpdating((u) => {
      let changed = false;
      const next = { ...u };
      for (const k of kiosks) {
        if (next[k.id] && k.app_version === next[k.id]) {
          delete next[k.id];
          changed = true;
        }
      }
      return changed ? next : u;
    });
  }, [kiosks]);

  async function assignRoom(k: Kiosk, roomId: string | null) {
    await setKioskRoom(k.id, roomId);
    await load();
  }
  async function assignBoard(k: Kiosk, boardId: string | null) {
    await setKioskBoard(k.id, boardId);
    await load();
  }

  async function send(k: Kiosk, command: "sleep" | "wake" | "lock") {
    await kioskCommand(k.id, command);
    await load();
  }

  async function pushUpdate(k: Kiosk) {
    if (!latest) return;
    setUpdating((u) => ({ ...u, [k.id]: latest.version_name }));
    await kioskCommand(k.id, "update");
    await load();
  }
  async function deauth(k: Kiosk) {
    const ok = await dialogs.confirm({
      title: "De-authorize kiosk",
      message: `Revoke "${k.name}"'s key? It loses access immediately and must be re-paired via QR.`,
      confirmLabel: "De-auth",
      danger: true,
    });
    if (!ok) return;
    await kioskDeauth(k.id);
    await load();
  }
  async function forget(k: Kiosk) {
    const ok = await dialogs.confirm({
      title: "Forget kiosk",
      message: `Remove "${k.name}" from the list? (Does not revoke its key — de-auth first to do that.)`,
      confirmLabel: "Forget",
      danger: true,
    });
    if (!ok) return;
    await forgetKiosk(k.id);
    await load();
  }

  return (
    <section style={{ marginTop: "2.5rem" }}>
      <SectionLabel style={{ marginBottom: "0.4rem" }}>Kiosks</SectionLabel>
      <p style={{ margin: "0 0 1rem", color: "var(--bf-faint)", fontSize: "0.85rem" }}>
        Wall-tablet companion apps that check in here. Put one to sleep, lock it (sign out of the
        dashboard), or de-authorize it (revoke its key). Set a <strong>sleep schedule</strong> to
        blank the display overnight (power saving) — a manual wake still holds until morning. Pair a
        new one above via <strong>Pair a device</strong>.
      </p>
      <div style={tileGrid(460, isMobile, "0.5rem 0.75rem")}>
        {kiosks.length === 0 && (
          <p style={{ color: "var(--bf-faint)", margin: 0, fontSize: "0.85rem" }}>No kiosks have checked in.</p>
        )}
        {kiosks.map((k) => (
          <div key={k.id} style={{ ...S.card, gap: "0.5rem" }}>
            <div style={{ minWidth: 0 }}>
              <div style={{ fontWeight: 600, display: "flex", alignItems: "center", gap: "0.5rem", flexWrap: "wrap" }}>
                {k.name}
                {/* App version chip — surfaced here, not buried in the meta line. */}
                <span
                  style={{
                    fontSize: "0.68rem",
                    fontWeight: 600,
                    letterSpacing: "0.03em",
                    color: "var(--bf-dim)",
                    background: "rgba(255,255,255,0.05)",
                    border: "1px solid var(--bf-hairline, #333)",
                    borderRadius: 999,
                    padding: "0.05rem 0.45rem",
                  }}
                >
                  {k.app_version ? `v${k.app_version}` : "unknown ver."}
                </span>
                <span style={{ fontSize: "0.7rem", color: k.online ? "var(--bf-good)" : "var(--bf-faint)" }}>
                  {k.online ? "● online" : "○ offline"}
                </span>
                {!k.authorized && <span style={{ fontSize: "0.7rem", color: "var(--bf-rose, #e57)" }}>needs re-pair</span>}
              </div>
              <div style={{ color: "var(--bf-faint)", fontSize: "0.74rem", marginTop: "0.15rem" }}>
                {k.screen_on === null ? "" : k.screen_on ? "screen on · " : "screen off · "}
                {k.last_seen ? `seen ${k.last_seen}` : "never seen"}
                {k.pending_command ? ` · queued: ${k.pending_command}` : ""}
              </div>
              {k.battery_level != null && (
                <div style={{ color: "var(--bf-faint)", fontSize: "0.74rem", marginTop: "0.1rem" }}>
                  {batteryMeta(k)}
                </div>
              )}
            </div>
            <div style={{ display: "flex", gap: "0.4rem", flexWrap: "wrap", alignItems: "center" }}>
              {/* Room = the kiosk's voice context (e.g. "turn on the lights" → its room). */}
              <select
                value={k.room_id ?? ""}
                onChange={(e) => assignRoom(k, e.target.value || null)}
                title="Voice context room for this kiosk"
                style={{
                  background: "var(--bf-panel, #1a1320)",
                  color: "var(--bf-text, #eee)",
                  border: "1px solid var(--bf-hairline, #333)",
                  borderRadius: 6,
                  padding: "0.3rem 0.5rem",
                  fontSize: "0.78rem",
                }}
              >
                <option value="">No room</option>
                {rooms.map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.name}
                  </option>
                ))}
              </select>
              {/* Auto-launch board: the kiosk opens this board full-screen on load. */}
              <select
                value={k.default_board_id ?? ""}
                onChange={(e) => assignBoard(k, e.target.value || null)}
                title="Board this kiosk auto-launches full-screen"
                style={{
                  background: "var(--bf-panel, #1a1320)",
                  color: "var(--bf-text, #eee)",
                  border: "1px solid var(--bf-hairline, #333)",
                  borderRadius: 6,
                  padding: "0.3rem 0.5rem",
                  fontSize: "0.78rem",
                }}
              >
                <option value="">No board</option>
                {boards.map((b) => (
                  <option key={b.id} value={b.id}>
                    {b.name}
                  </option>
                ))}
              </select>
              <Button
                variant="ghost"
                onClick={() => send(k, k.screen_on === false ? "wake" : "sleep")}
                style={{ padding: "0.3rem 0.7rem", fontSize: "0.78rem" }}
              >
                {k.screen_on === false ? "Wake" : "Sleep"}
              </Button>
              <Button variant="ghost" onClick={() => send(k, "lock")} style={{ padding: "0.3rem 0.7rem", fontSize: "0.78rem" }}>
                Lock
              </Button>
              <Button
                variant="danger"
                onClick={() => deauth(k)}
                disabled={!k.authorized}
                style={{ padding: "0.3rem 0.7rem", fontSize: "0.78rem" }}
              >
                De-auth
              </Button>
              <Button variant="danger" onClick={() => forget(k)} style={{ padding: "0.3rem 0.7rem", fontSize: "0.78rem" }}>
                Forget
              </Button>
              {/* Update status: "Updating…" while a push is in flight, else compare
                  the kiosk's version to the hub's cached APK. Only offer an update
                  when the cache is *strictly newer* — never a downgrade (the kiosk
                  can be ahead of a stale cache, e.g. after a local sideload). */}
              {updating[k.id] ? (
                <span style={{ fontSize: "0.74rem", color: "var(--bf-accent, #38bdf8)", whiteSpace: "nowrap" }}>
                  <span className="bifrost-voice-pulse" style={{ display: "inline-block" }}>⟳</span> Updating → {updating[k.id]}…
                </span>
              ) : (
                latest &&
                k.app_version &&
                (cmpVersion(latest.version_name, k.app_version) > 0 ? (
                  <Button
                    onClick={() => pushUpdate(k)}
                    disabled={!k.online || !k.authorized}
                    title={`Push v${latest.version_name} to this kiosk (it pulls + installs over the LAN)`}
                    style={{ padding: "0.3rem 0.7rem", fontSize: "0.78rem" }}
                  >
                    Update → {latest.version_name}
                  </Button>
                ) : (
                  <span style={{ fontSize: "0.74rem", color: "var(--bf-good)", whiteSpace: "nowrap" }}>
                    ✓ up to date
                  </span>
                ))
              )}
            </div>
            <KioskDisplayPlan k={k} dialogs={dialogs} onSaved={load} />
            <KioskMicPresence k={k} onSaved={load} />
            <KioskAwareOverride k={k} lights={lights} media={media} power={power} onSaved={load} />
          </div>
        ))}
      </div>
    </section>
  );
}

// ── Kiosk microphone presence ─────────────────────────────────────────────────

/** Sensitivity described by what actually trips it, not an abstract level. The
 * stored keys stay low/medium/high (the detection margin above ambient). */
export const MIC_SENSITIVITY_OPTIONS = [
  { value: "low", label: "Loud noise" },
  { value: "medium", label: "Speech" },
  { value: "high", label: "Faint sounds" },
];

/** The kiosk's always-on mic as a room occupancy sensor. Level-only by design:
 * the app computes loudness on-device against an adaptive ambient baseline and
 * reports elevated/quiet edges — audio never leaves the kiosk. Enabling mints
 * a real occupancy sensor assigned to the kiosk's room (visible on Devices and
 * in the room's Presence section like any other sensor). */
function KioskMicPresence({ k, onSaved }: { k: Kiosk; onSaved: () => void }) {
  const [busy, setBusy] = useState(false);
  const set = async (enabled: boolean, sensitivity?: string) => {
    setBusy(true);
    try {
      await setKioskMic(k.id, { enabled, ...(sensitivity ? { sensitivity } : {}) });
      onSaved();
    } finally {
      setBusy(false);
    }
  };
  return (
    <div style={{ display: "flex", alignItems: "center", flexWrap: "wrap", gap: "0.6rem", marginTop: "0.55rem" }}>
      <span style={{ fontSize: "0.74rem", color: "var(--bf-dim)" }}>Mic presence</span>
      <Switch on={k.mic_presence} disabled={busy} onChange={(v) => set(v)} />
      {k.mic_presence && (
        <>
          <span style={{ fontSize: "0.72rem", color: "var(--bf-faint)" }}>Reacts to</span>
          <Segmented
            value={k.mic_sensitivity ?? "medium"}
            onChange={(v) => set(true, v)}
            options={MIC_SENSITIVITY_OPTIONS}
          />
          {k.mic_level != null && (
            <span
              title="Last reported sound level (dBFS)"
              style={{ fontSize: "0.72rem", color: "var(--bf-dim)", fontVariantNumeric: "tabular-nums" }}
            >
              {Math.round(k.mic_level)} dB
            </span>
          )}
        </>
      )}
      <span style={{ fontSize: "0.66rem", color: "var(--bf-faint)" }}>
        {k.mic_presence
          ? "How far a sound must rise above the room's ambient hum (loud ≈15 dB · speech ≈10 dB · faint ≈6 dB). Level only — audio never leaves the kiosk."
          : "Sound level only — audio never leaves the kiosk."}
      </span>
    </div>
  );
}

// ── Kiosk aware override ────────────────────────────────────────────────────

/** While any configured device is on, an Aware hour keeps the kiosk's screen
 * awake regardless of room presence — "don't let the screen sleep from a
 * no-motion timeout while the TV is playing". Only shown once the kiosk's
 * plan actually paints an Aware hour (otherwise there's nothing to override).
 * Mixed-domain device picker mirrors the Boards room-control target picker
 * (`ControlEditor`'s "Devices" field) — a flat `domain:id` list, no room
 * grouping, since a target list here is typically one or two devices. */
function KioskAwareOverride({
  k,
  lights,
  media,
  power,
  onSaved,
}: {
  k: Kiosk;
  lights: Light[];
  media: MediaDevice[];
  power: PowerDevice[];
  onSaved: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [targets, setTargets] = useState<ControlTarget[]>(k.aware_override_targets);
  const [mode, setMode] = useState<AwareOverrideMode>(k.aware_override_mode);
  // Re-sync the draft ONLY when the modal opens. The clients list re-polls
  // every few seconds and each poll mints fresh array/object identities for
  // `k`, so depending on the values here would wipe in-progress edits
  // mid-modal every poll tick.
  useEffect(() => {
    if (open) {
      setTargets(k.aware_override_targets);
      setMode(k.aware_override_mode);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  if (!k.hour_modes?.includes("A")) return null;

  const selLights = pickableLights(lights);
  const selMedia = pickableMedia(media);
  const selPower = pickablePower(power);
  const pool: { domain: ControlTarget["domain"]; list: { id: string; name: string }[] }[] = [
    { domain: "light", list: selLights },
    { domain: "media", list: selMedia },
    { domain: "power", list: selPower },
  ];
  const nameFor = (t: ControlTarget) =>
    pool.find((p) => p.domain === t.domain)?.list.find((d) => d.id === t.id)?.name ?? "(removed device)";
  const has = (domain: string, id: string) => targets.some((t) => t.domain === domain && t.id === id);
  const toggle = (domain: ControlTarget["domain"], id: string) =>
    setTargets((cur) =>
      has(domain, id)
        ? cur.filter((t) => !(t.domain === domain && t.id === id))
        : [...cur, { domain, id }],
    );

  const savedNames = k.aware_override_targets.map(nameFor);
  const savedSubject = savedNames.length > 1 ? `any of ${savedNames.join(", ")}` : savedNames[0];
  const summary =
    savedNames.length === 0
      ? "Not configured"
      : k.aware_override_mode === "keep_off"
        ? `Sleeps while ${savedSubject} is on`
        : `Stays awake while ${savedSubject} is on`;

  async function save() {
    setBusy(true);
    try {
      await setKioskAwareOverride(k.id, targets, mode);
      onSaved();
      setOpen(false);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        flexWrap: "wrap",
        gap: "0.5rem",
        marginTop: "0.4rem",
        fontSize: "0.78rem",
        color: "var(--bf-faint)",
      }}
    >
      <span style={{ display: "inline-flex", alignItems: "center", gap: "0.3rem", color: "var(--bf-dim)" }}>
        <Glyph name="bolt" size={15} /> Aware override
      </span>
      <span style={{ flex: 1, minWidth: 0 }}>{summary}</span>
      <Button variant="ghost" onClick={() => setOpen(true)} style={{ padding: "0.2rem 0.6rem", fontSize: "0.74rem" }}>
        Configure
      </Button>
      {open && (
        <Modal title="Aware override" onClose={() => setOpen(false)}>
          <p style={{ fontSize: "0.8rem", color: "var(--bf-faint)", marginTop: 0 }}>
            While any of these is on, an Aware hour{" "}
            {mode === "keep_off" ? (
              <>
                forces <strong>{k.name}</strong>'s screen off — kill the tablet glow during movie
                night — beating room presence.
              </>
            ) : (
              <>
                keeps <strong>{k.name}</strong>'s screen awake regardless of room presence.
              </>
            )}
          </p>
          <div style={{ marginBottom: "0.8rem", maxWidth: 320 }}>
            <Segmented
              value={mode}
              onChange={setMode}
              variant="outline"
              accent={ACCENT}
              options={[
                { value: "keep_on", label: "Keep screen on" },
                { value: "keep_off", label: "Keep screen off" },
              ]}
            />
          </div>
          <OptionCheckList
            options={pool.flatMap(({ domain, list }) =>
              list.map((d) => ({ value: `${domain}:${d.id}`, label: d.name })),
            )}
            selected={targets.map((t) => `${t.domain}:${t.id}`)}
            onToggle={(v) => {
              const sep = v.indexOf(":");
              toggle(v.slice(0, sep) as ControlTarget["domain"], v.slice(sep + 1));
            }}
          />
          <div style={{ display: "flex", justifyContent: "flex-end", gap: "0.5rem", marginTop: "1rem" }}>
            <Button variant="ghost" onClick={() => setOpen(false)}>Cancel</Button>
            <Button variant="primary" onClick={save} disabled={busy}>Save</Button>
          </div>
        </Modal>
      )}
    </div>
  );
}

// ── Kiosk app updates (hub relays GitHub releases to offline LAN kiosks) ──────

function KioskUpdateSection({
  dialogs,
  latest: cached,
  onCacheChanged,
}: {
  dialogs: Dialogs;
  /** The hub's cached manifest (owned by [ClientsTab]). */
  latest: KioskUpdateManifest | null;
  /** Called after a fetch changes the cache, so siblings refresh. */
  onCacheChanged: () => void;
}) {
  const [repo, setRepo] = useState("");
  const [asset, setAsset] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    getKioskUpdateConfig().then((c) => {
      setRepo(c.repo);
      setAsset(c.asset);
      setLoaded(true);
    });
  }, []);

  async function save() {
    setSaving(true);
    const res = await setKioskUpdateConfig(repo.trim(), asset.trim());
    setSaving(false);
    if ("error" in res) {
      await dialogs.alert({ title: "Couldn't save", message: res.error });
      return;
    }
    setRepo(res.repo);
    setAsset(res.asset);
  }

  async function check() {
    setChecking(true);
    const res = await refreshKioskUpdate();
    setChecking(false);
    if ("error" in res) {
      await dialogs.alert({ title: "Update check failed", message: res.error });
      return;
    }
    // Refresh the shared cache state so the per-kiosk Update buttons appear
    // immediately (no tab round-trip).
    onCacheChanged();
    await dialogs.alert({
      title: res.downloaded ? "Update cached" : "Already up to date",
      message: `v${res.manifest.version_name} (build ${res.manifest.version_code}) is ${
        res.downloaded ? "now cached on the hub" : "already cached"
      } and ready to push to kiosks.`,
    });
  }

  const inputStyle: React.CSSProperties = {
    background: "var(--bf-panel, #1a1320)",
    color: "var(--bf-text, #eee)",
    border: "1px solid var(--bf-hairline, #333)",
    borderRadius: 6,
    padding: "0.4rem 0.6rem",
    fontSize: "0.85rem",
    width: "100%",
    boxSizing: "border-box",
  };
  const labelStyle: React.CSSProperties = {
    display: "flex",
    flexDirection: "column",
    gap: "0.25rem",
    fontSize: "0.78rem",
    color: "var(--bf-dim)",
  };

  return (
    <section style={{ marginTop: "2.5rem" }}>
      <SectionLabel style={{ marginBottom: "0.4rem" }}>App updates</SectionLabel>
      <p style={{ margin: "0 0 1rem", color: "var(--bf-faint)", fontSize: "0.85rem" }}>
        Kiosks are offline and only talk to this hub, so the hub fetches the kiosk APK from its
        GitHub release and serves it over the LAN. Set where to pull from, then{" "}
        <strong>Check for updates</strong> to cache the latest build here, ready to push to kiosks.
      </p>
      <div style={{ ...S.card, flexDirection: "column", alignItems: "stretch", gap: "0.7rem", maxWidth: 460 }}>
        <label style={labelStyle}>
          Source repo (owner/name)
          <input
            value={repo}
            onChange={(e) => setRepo(e.target.value)}
            placeholder="others-git/bifrost-kiosk"
            spellCheck={false}
            autoCapitalize="off"
            style={inputStyle}
          />
        </label>
        <label style={labelStyle}>
          Release asset
          <input
            value={asset}
            onChange={(e) => setAsset(e.target.value)}
            placeholder="app-release.apk"
            spellCheck={false}
            autoCapitalize="off"
            style={inputStyle}
          />
        </label>
        <div style={{ display: "flex", alignItems: "center", gap: "0.7rem", flexWrap: "wrap" }}>
          <Button onClick={save} disabled={saving || !loaded || !repo.trim() || !asset.trim()}>
            {saving ? "Saving…" : "Save source"}
          </Button>
          <Button variant="ghost" onClick={check} disabled={checking}>
            {checking ? "Checking…" : "Check for updates"}
          </Button>
          <span style={{ fontSize: "0.78rem", color: "var(--bf-faint)" }}>
            Cached:{" "}
            {cached ? (
              <strong style={{ color: "var(--bf-dim)" }}>
                v{cached.version_name} (build {cached.version_code})
              </strong>
            ) : (
              "none yet"
            )}
          </span>
        </div>
      </div>
    </section>
  );
}
