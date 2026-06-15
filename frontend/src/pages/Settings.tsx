import { useEffect, useState } from "react";
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
  type DiscoveredDevice,
  removeProvider,
  updateProviderCredentials,
  getApiKeys,
  createApiKey,
  revokeApiKey,
  getSettings,
  updateSettings,
  type ApiKey,
  type ConnectionStatus,
  type CredentialField,
  type Provider,
  type ProviderType,
} from "../api";
import { useDialogs, type Dialogs } from "../components/dialogs";
import { PageHeader, SectionLabel } from "../components/PageHeader";
import { ThemeSwitcher } from "../components/ThemeSwitcher";
import { Select } from "../components/Select";
import { useViewport } from "../useViewport";
import { ACCENT, S } from "../styles";

interface Props {
  onNavigate: (page: "dashboard") => void;
}

export function SettingsPage({ onNavigate: _onNavigate }: Props) {
  const dialogs = useDialogs();
  const { isMobile } = useViewport();
  const [providers, setProviders] = useState<Provider[]>([]);
  const [types, setTypes] = useState<ProviderType[]>([]);
  const [showAdd, setShowAdd] = useState(false);
  const [toast, setToast] = useState("");

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

      <SectionLabel style={{ marginBottom: "0.8rem" }}>Appearance</SectionLabel>
      <div style={{ marginBottom: "2rem" }}>
        <ThemeSwitcher />
      </div>

      <SectionLabel style={{ marginBottom: "1.1rem" }}>Providers</SectionLabel>

      {toast && (
        <div style={{ background: "#1e3a1e", border: "1px solid #2a5a2a", borderRadius: 8, padding: "0.6rem 1rem", marginBottom: "1rem", color: "var(--bf-good)", fontSize: "0.875rem" }}>
          {toast}
        </div>
      )}

      {providers.length > 0 && (
        <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap", marginBottom: "1rem" }}>
          <button onClick={() => runAll("discover")} disabled={!!running} style={S.buttonGhost} title="Discover devices on every provider">
            {running === "discover" ? "Discovering…" : "Discover all"}
          </button>
          <button onClick={() => runAll("sync")} disabled={!!running} style={S.buttonGhost} title="Discover devices and mirror rooms/zones for every provider">
            {running === "sync" ? "Syncing…" : "Sync all"}
          </button>
          <button
            onClick={() => runAll("prune-sync")}
            disabled={!!running}
            style={S.buttonGhost}
            title="Force-prune devices providers no longer report, then discover + sync — across all providers"
          >
            {running === "prune-sync" ? "Pruning…" : "Prune + Sync all"}
          </button>
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
            types={types}
            onAdded={handleAdded}
            onCancel={() => setShowAdd(false)}
          />
        </div>
      ) : (
        <button
          onClick={() => setShowAdd(true)}
          style={{ ...S.button, marginTop: "1.5rem" }}
        >
          + Add Provider
        </button>
      )}

      <ExpandedLanSection />
      <ApiKeysSection dialogs={dialogs} />
      {dialogs.element}
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
        <button onClick={save} disabled={saving} style={S.button}>
          {saving ? "Saving…" : "Save"}
        </button>
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

// ── API keys ─────────────────────────────────────────────────────────────────

function ApiKeysSection({ dialogs }: { dialogs: Dialogs }) {
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [name, setName] = useState("");
  const [creating, setCreating] = useState(false);
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
            <button
              onClick={() => navigator.clipboard?.writeText(fresh.key)}
              style={{ ...S.buttonGhost, padding: "0.3rem 0.6rem", fontSize: "0.78rem" }}
            >
              Copy
            </button>
            <button
              onClick={() => setFresh(null)}
              style={{ ...S.buttonGhost, padding: "0.3rem 0.6rem", fontSize: "0.78rem" }}
            >
              Done
            </button>
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
            <button
              onClick={() => revoke(k)}
              style={{ ...S.buttonDanger, padding: "0.3rem 0.7rem", fontSize: "0.78rem" }}
            >
              Revoke
            </button>
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
        <button onClick={create} disabled={creating || !name.trim()} style={S.button}>
          {creating ? "Creating…" : "Create key"}
        </button>
      </div>
    </section>
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
          <button onClick={handleDiscover} disabled={discovering} style={S.buttonGhost}>
            {discovering ? "…" : "Discover"}
          </button>
          <button
            onClick={handleImport}
            disabled={importing}
            title="Sync this provider's rooms/zones into Bifrost Rooms"
            style={S.buttonGhost}
          >
            {importing ? "…" : "Sync"}
          </button>
          <button
            onClick={() => setEditingCreds((v) => !v)}
            title="Reconfigure this provider's IP and credentials"
            style={S.buttonGhost}
          >
            {editingCreds ? "Close" : "Edit"}
          </button>
          <button onClick={onRemove} style={S.buttonDanger}>Remove</button>
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
                <button
                  type="button"
                  onClick={handlePair}
                  disabled={pairing || !(credentials.bridge_ip ?? "").trim()}
                  style={S.buttonGhost}
                >
                  {pairing ? "Pairing…" : "Pair"}
                </button>
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
        <button type="submit" style={S.button} disabled={saving}>
          {saving ? "Saving…" : "Save"}
        </button>
        <button type="button" onClick={onCancel} style={S.buttonGhost}>Cancel</button>
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
  onAdded,
  onCancel,
}: {
  types: ProviderType[];
  onAdded: (id: string) => void;
  onCancel: () => void;
}) {
  const [selectedType, setSelectedType] = useState(types[0]?.provider_type ?? "");
  const [name, setName] = useState("");
  const [credentials, setCredentials] = useState<Record<string, string>>({});
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [pairing, setPairing] = useState(false);
  const [pairMsg, setPairMsg] = useState("");
  const [scanning, setScanning] = useState(false);
  const [scanned, setScanned] = useState(false);
  const [found, setFound] = useState<DiscoveredDevice[]>([]);

  const selected = types.find((t) => t.provider_type === selectedType);
  const schema: CredentialField[] = selected?.schema ?? [];

  // Clear per-type state when the provider type changes.
  useEffect(() => {
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
          <button
            type="button"
            onClick={handleScan}
            disabled={scanning}
            style={S.buttonGhost}
          >
            {scanning ? "Scanning network…" : "Scan network for devices"}
          </button>
          {found.map((d) => (
            <button
              key={d.host}
              type="button"
              onClick={() => applyFound(d)}
              title={`Use ${d.host}`}
              style={{
                ...S.buttonGhost,
                textAlign: "left",
                fontSize: "0.82rem",
                borderColor: ACCENT,
              }}
            >
              {d.label ? `${d.label} · ${d.host}` : d.host}
            </button>
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
                <button
                  type="button"
                  onClick={handlePair}
                  disabled={pairing || !(credentials.bridge_ip ?? "").trim()}
                  style={S.buttonGhost}
                >
                  {pairing ? "Pairing…" : "Pair"}
                </button>
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
        <button type="submit" style={S.button} disabled={loading || types.length === 0}>
          {loading ? "Adding…" : "Add"}
        </button>
        <button type="button" onClick={onCancel} style={S.buttonGhost}>
          Cancel
        </button>
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
