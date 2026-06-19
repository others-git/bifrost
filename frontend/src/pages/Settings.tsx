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
  forgetKiosk,
  getKioskUpdateConfig,
  setKioskUpdateConfig,
  getKioskUpdateStatus,
  refreshKioskUpdate,
  getRooms,
  type ApiKey,
  type AiEndpoint,
  type AiRole,
  type Kiosk,
  type KioskUpdateManifest,
  type Room,
  type ConnectionStatus,
  type CredentialField,
  type Provider,
  type ProviderType,
} from "../api";
import { QRCodeSVG } from "qrcode.react";
import { useDialogs, type Dialogs, Modal } from "../components/dialogs";
import { PageHeader, SectionLabel } from "../components/PageHeader";
import { ThemeSwitcher } from "../components/ThemeSwitcher";
import { Select } from "../components/Select";
import { useViewport } from "../useViewport";
import { ACCENT, S } from "../styles";
import { Button, Switch } from "../components/controls";
import { speak } from "../tts";

interface Props {
  onNavigate: (page: "dashboard") => void;
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

export function SettingsPage({ onNavigate: _onNavigate }: Props) {
  const dialogs = useDialogs();
  const { isMobile } = useViewport();
  const [providers, setProviders] = useState<Provider[]>([]);
  const [types, setTypes] = useState<ProviderType[]>([]);
  const [showAdd, setShowAdd] = useState(false);
  // When the user clicks "Add" on a found device, the add form opens pre-filled.
  const [prefill, setPrefill] = useState<AddPrefill | null>(null);
  const [toast, setToast] = useState("");
  const [tab, setTab] = useState<SettingsTab>("providers");

  function openAdd(p: AddPrefill | null) {
    setPrefill(p);
    setShowAdd(true);
  }

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

  async function handleRemove(id: string) {
    const ok = await dialogs.confirm({
      title: "Remove provider",
      message: "Remove this provider? Associated lights will be deleted.",
      confirmLabel: "Remove",
      danger: true,
    });
    if (!ok) return;
    await removeProvider(id);
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
    <div style={{ padding: isMobile ? "1rem 0.85rem" : "2rem", maxWidth: 720, margin: "0 auto" }}>
      <PageHeader title="Settings" />

      {/* Tabs — keeps a growing Settings page legible. Scrolls on narrow screens. */}
      <div
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

          <div style={{ display: "flex", flexDirection: "column", gap: "0.75rem" }}>
            {providers.length === 0 && !showAdd && (
              <p style={{ color: "var(--bf-faint)", margin: 0 }}>No providers configured.</p>
            )}
            {providers.map((p) => (
              <ProviderCard
                key={p.id}
                provider={p}
                types={types}
                onCredentialsSaved={() => showToast("Credentials updated — reconnecting.")}
                onRemove={() => handleRemove(p.id)}
                onDiscover={() => handleDiscover(p.id)}
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
              />
            ))}
          </div>

          {showAdd ? (
            <div style={{ marginTop: "1.5rem" }}>
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

      {tab === "voice" && <AiEndpointsSection dialogs={dialogs} />}

      {tab === "clients" && <ClientsTab dialogs={dialogs} />}

      {tab === "appearance" && (
        <div style={{ marginTop: "0.25rem" }}>
          <ThemeSwitcher />
        </div>
      )}

      {tab === "developer" && <DeveloperTab />}

      {dialogs.element}
    </div>
  );
}

// ── Found devices (auto-discovery) ───────────────────────────────────────────

/** Pre-fills the add-provider form from a found device. */
interface AddPrefill {
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
function DeveloperTab() {
  const [devMode, setDevMode] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    getSettings().then((s) => setDevMode(!!s.dev_mode));
  }, []);

  async function toggle(next: boolean) {
    setSaving(true);
    const res = await updateSettings({ dev_mode: next });
    setSaving(false);
    if (!("error" in res)) setDevMode(!!res.dev_mode);
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

      <label
        style={{
          ...S.card,
          flexDirection: "row",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "0.8rem 1rem",
          cursor: devMode === null ? "default" : "pointer",
          gap: "1rem",
        }}
      >
        <div style={{ minWidth: 0 }}>
          <div style={{ fontWeight: 600, fontSize: "0.92rem" }}>Enable developer mode</div>
          <div style={{ color: "var(--bf-faint)", fontSize: "0.76rem", marginTop: "0.15rem" }}>
            {devMode ? "On — dev surfaces and /api/dev are live." : "Off — production behaviour."}
          </div>
        </div>
        <input
          type="checkbox"
          disabled={devMode === null || saving}
          checked={!!devMode}
          onChange={(e) => toggle(e.target.checked)}
          style={{ width: 20, height: 20, accentColor: ACCENT, flexShrink: 0, cursor: "pointer" }}
        />
      </label>

      {devMode && (
        <>
          <DevInfoCard />
          <DevProviderDebugSection />
        </>
      )}
    </section>
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
              onClick={() => navigator.clipboard?.writeText(fresh.key)} style={{ padding: "0.3rem 0.6rem", fontSize: "0.78rem" }}
            >
              Copy
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

function ProviderCard({
  provider,
  types,
  onCredentialsSaved,
  onRemove,
  onDiscover,
  onImportGroups,
  onSetPrune,
}: {
  provider: Provider;
  types: ProviderType[];
  onCredentialsSaved: () => void;
  onRemove: () => void;
  onDiscover: () => Promise<void>;
  onImportGroups: () => Promise<void>;
  onSetPrune: (prune: boolean) => Promise<void>;
}) {
  const { isMobile } = useViewport();
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
    await onDiscover();
    setDiscovering(false);
  }

  async function handleImport() {
    setImporting(true);
    try {
      await onImportGroups();
    } finally {
      setImporting(false);
    }
  }

  return (
    <div style={{ ...S.card, gap: "0.75rem" }}>
      <div
        style={{
          display: "flex",
          flexDirection: isMobile ? "column" : "row",
          alignItems: isMobile ? "stretch" : "center",
          justifyContent: "space-between",
          gap: isMobile ? "0.6rem" : "1rem",
        }}
      >
        <div style={{ minWidth: 0 }}>
          <div style={{ fontWeight: 600 }}>{provider.name}</div>
          <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", marginTop: "0.25rem" }}>
            <span style={{ color: "var(--bf-dim)", fontSize: "0.8rem" }}>
              {provider.type_name}
              {provider.domain === "audio" ? " · Audio" : ""}
              {provider.domain === "integration" ? " · Integration" : ""}
            </span>
            {status && <StatusBadge state={status.state} />}
          </div>
        </div>
        <div style={{ display: "flex", gap: "0.5rem", flexShrink: 0, flexWrap: "wrap" }}>
          <Button variant="ghost" onClick={handleDiscover} disabled={discovering}>
            {discovering ? "…" : "Discover"}
          </Button>
          <Button variant="ghost"
            onClick={handleImport}
            disabled={importing}
            title="Sync this provider's rooms/zones into Bifrost Rooms"
          >
            {importing ? "…" : "Sync"}
          </Button>
          <Button variant="ghost"
            onClick={() => setEditingCreds((v) => !v)}
            title="Reconfigure this provider's IP and credentials"
          >
            {editingCreds ? "Close" : "Edit"}
          </Button>
          <Button variant="danger" onClick={onRemove}>Remove</Button>
        </div>
      </div>
      <label
        style={{ display: "flex", alignItems: "flex-start", gap: "0.55rem", cursor: "pointer", color: "#9a9488", fontSize: "0.78rem" }}
      >
        <input
          type="checkbox"
          checked={provider.prune}
          onChange={(e) => onSetPrune(e.target.checked)}
          style={{ width: 16, height: 16, marginTop: 1, accentColor: ACCENT, flexShrink: 0, cursor: "pointer" }}
        />
        <span>
          <strong style={{ color: provider.prune ? ACCENT : "var(--bf-dim)", fontWeight: 600 }}>Prune on discover</strong>
          {" — "}
          {provider.prune
            ? "the next discover removes devices this provider no longer reports (and drops them from rooms)."
            : "devices stay even if the provider stops reporting them. Enable to auto-remove them."}
        </span>
      </label>

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
            </div>
            {isHueAppKey && pairMsg && (
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
  const [scanning, setScanning] = useState(false);
  const [scanned, setScanned] = useState(false);
  const [found, setFound] = useState<DiscoveredDevice[]>([]);

  const selected = types.find((t) => t.provider_type === selectedType);
  const schema: CredentialField[] = selected?.schema ?? [];

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
          onChange={setSelectedType}
          style={{ width: "100%" }}
          options={(["light", "audio", "integration"] as const).flatMap((kind) =>
            types
              .filter((t) => t.kind === kind)
              .map((t) => ({
                value: t.provider_type,
                label: t.display_name,
                group: kind === "light" ? "Lights" : kind === "audio" ? "Audio" : "Integrations",
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
        return (
          <label key={field.name} style={labelStyle}>
            <span>
              {field.label}
              {field.required && <span style={{ color: ACCENT }}> *</span>}
            </span>
            {!isHueAppKey && field.hint && (
              <span style={{ color: "var(--bf-faint)", fontSize: "0.78rem" }}>{field.hint}</span>
            )}
            {isHueAppKey && (
              <span style={{ color: "var(--bf-faint)", fontSize: "0.78rem" }}>
                Press the link button on the bridge, then click Pair — or paste a key manually.
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
            </div>
            {isHueAppKey && pairMsg && (
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

function KiosksSection({
  dialogs,
  latest,
}: {
  dialogs: Dialogs;
  latest: KioskUpdateManifest | null;
}) {
  const [kiosks, setKiosks] = useState<Kiosk[]>([]);
  const [rooms, setRooms] = useState<Room[]>([]);
  // kioskId → target version we pushed, so the row can show "Updating…" until the
  // kiosk re-checks-in reporting that version (it goes offline mid-install).
  const [updating, setUpdating] = useState<Record<string, string>>({});
  async function load() {
    setKiosks(await getKiosks());
  }
  useEffect(() => {
    load();
    getRooms().then(setRooms);
  }, []);

  // While any update is in flight, poll so the version/online status refresh as
  // the kiosk downloads, installs, restarts, and checks back in.
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
        dashboard), or de-authorize it (revoke its key). Pair a new one above via{" "}
        <strong>Pair a device</strong>.
      </p>
      <div style={{ display: "flex", flexDirection: "column", gap: "0.5rem" }}>
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
          </div>
        ))}
      </div>
    </section>
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
