import { useEffect, useState } from "react";
import {
  addProvider,
  createGroup,
  discoverLights,
  getGroups,
  getLights,
  getProviderStatus,
  getProviderTypes,
  getProviders,
  importProviderGroups,
  pairHueBridge,
  removeGroup,
  removeProvider,
  setGroupMembers,
  type ConnectionStatus,
  type CredentialField,
  type Group,
  type Light,
  type Provider,
  type ProviderType,
} from "../api";
import { S } from "../styles";

interface Props {
  onNavigate: (page: "dashboard") => void;
}

export function SettingsPage({ onNavigate: _onNavigate }: Props) {
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
    if (!window.confirm("Remove this provider? Associated lights will be deleted.")) return;
    await removeProvider(id);
    await loadProviders();
  }

  async function handleDiscover(id: string) {
    const result = await discoverLights(id);
    await loadProviders();
    showToast(`Discovered ${result.discovered} light${result.discovered !== 1 ? "s" : ""}.`);
  }

  async function handleAdded(id: string) {
    setShowAdd(false);
    await loadProviders();
    // Run discovery right away so lights appear without an extra click.
    try {
      const result = await discoverLights(id);
      showToast(`Provider added — found ${result.discovered} light${result.discovered !== 1 ? "s" : ""}.`);
    } catch {
      showToast("Provider added. Discovery failed — check the connection and try Discover.");
    }
  }

  return (
    <div style={{ padding: "2rem", maxWidth: 720, margin: "0 auto" }}>
      <h2 style={{ margin: "0 0 1.5rem", fontSize: "1.2rem", color: "#ccc" }}>Providers</h2>

      {toast && (
        <div style={{ background: "#1e3a1e", border: "1px solid #2a5a2a", borderRadius: 8, padding: "0.6rem 1rem", marginBottom: "1rem", color: "#8f8", fontSize: "0.875rem" }}>
          {toast}
        </div>
      )}

      <div style={{ display: "flex", flexDirection: "column", gap: "0.75rem" }}>
        {providers.length === 0 && !showAdd && (
          <p style={{ color: "#666", margin: 0 }}>No providers configured.</p>
        )}
        {providers.map((p) => (
          <ProviderCard
            key={p.id}
            provider={p}
            onRemove={() => handleRemove(p.id)}
            onDiscover={() => handleDiscover(p.id)}
            onImportGroups={async () => {
              const r = await importProviderGroups(p.id);
              showToast(
                r.found === 0
                  ? "No rooms or zones defined on this provider."
                  : `Imported ${r.imported} of ${r.found} room${r.found !== 1 ? "s" : ""} as groups.`,
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

      <GroupsSection />
    </div>
  );
}

// ── Groups ───────────────────────────────────────────────────────────────────

function GroupsSection() {
  const [groups, setGroups] = useState<Group[]>([]);
  const [lights, setLights] = useState<Light[]>([]);
  const [showAdd, setShowAdd] = useState(false);

  async function load() {
    setGroups(await getGroups());
    const l = await getLights();
    if (l !== "unauthorized") setLights(l);
  }

  useEffect(() => { load(); }, []);

  async function handleRemove(id: string, name: string) {
    if (!window.confirm(`Delete group "${name}"?`)) return;
    await removeGroup(id);
    await load();
  }

  return (
    <div style={{ marginTop: "2.5rem" }}>
      <h2 style={{ margin: "0 0 1rem", fontSize: "1.2rem", color: "#ccc" }}>Groups</h2>

      <div style={{ display: "flex", flexDirection: "column", gap: "0.75rem" }}>
        {groups.length === 0 && !showAdd && (
          <p style={{ color: "#666", margin: 0 }}>
            No groups yet. Group lights to control a whole room at once.
          </p>
        )}
        {groups.map((g) => (
          <GroupCard
            key={g.id}
            group={g}
            lights={lights}
            onChanged={load}
            onRemove={() => handleRemove(g.id, g.name)}
          />
        ))}
      </div>

      {showAdd ? (
        <div style={{ marginTop: "1rem" }}>
          <GroupForm
            lights={lights}
            initialName=""
            initialMembers={[]}
            submitLabel="Create"
            onSubmit={async (name, ids) => {
              await createGroup(name, ids);
              setShowAdd(false);
              await load();
            }}
            onCancel={() => setShowAdd(false)}
          />
        </div>
      ) : (
        <button
          onClick={() => setShowAdd(true)}
          disabled={lights.length === 0}
          title={lights.length === 0 ? "Discover some lights first" : undefined}
          style={{ ...S.button, marginTop: "1rem" }}
        >
          + Add Group
        </button>
      )}
    </div>
  );
}

function GroupCard({
  group,
  lights,
  onChanged,
  onRemove,
}: {
  group: Group;
  lights: Light[];
  onChanged: () => Promise<void>;
  onRemove: () => void;
}) {
  const [editing, setEditing] = useState(false);

  if (editing) {
    return (
      <GroupForm
        lights={lights}
        initialName={group.name}
        initialMembers={group.light_ids}
        submitLabel="Save"
        nameLocked
        onSubmit={async (_name, ids) => {
          await setGroupMembers(group.id, ids);
          setEditing(false);
          await onChanged();
        }}
        onCancel={() => setEditing(false)}
      />
    );
  }

  return (
    <div style={{ ...S.card, flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: "1rem" }}>
      <div>
        <div style={{ fontWeight: 600 }}>{group.name}</div>
        <div style={{ color: "#888", fontSize: "0.8rem", marginTop: "0.25rem" }}>
          {group.light_ids.length} light{group.light_ids.length !== 1 ? "s" : ""}
        </div>
      </div>
      <div style={{ display: "flex", gap: "0.5rem", flexShrink: 0 }}>
        <button onClick={() => setEditing(true)} style={S.buttonGhost}>Edit</button>
        <button onClick={onRemove} style={S.buttonDanger}>Remove</button>
      </div>
    </div>
  );
}

function GroupForm({
  lights,
  initialName,
  initialMembers,
  submitLabel,
  nameLocked,
  onSubmit,
  onCancel,
}: {
  lights: Light[];
  initialName: string;
  initialMembers: string[];
  submitLabel: string;
  nameLocked?: boolean;
  onSubmit: (name: string, lightIds: string[]) => Promise<void>;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initialName);
  const [members, setMembers] = useState<Set<string>>(new Set(initialMembers));
  const [saving, setSaving] = useState(false);

  function toggle(id: string) {
    setMembers((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    try {
      await onSubmit(name.trim(), [...members]);
    } finally {
      setSaving(false);
    }
  }

  return (
    <form onSubmit={submit} style={{ ...S.card, border: "1px solid #333" }}>
      {!nameLocked && (
        <label style={labelStyle}>
          <span>Name</span>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. Living Room"
            style={S.input}
            required
            autoFocus
          />
        </label>
      )}
      {nameLocked && <h3 style={{ margin: 0, fontSize: "1rem", color: "#ccc" }}>{name}</h3>}

      <div style={{ display: "flex", flexDirection: "column", gap: "0.35rem" }}>
        <span style={{ fontSize: "0.875rem", color: "#aaa" }}>Lights</span>
        {lights.map((l) => (
          <label
            key={l.id}
            style={{ display: "flex", alignItems: "center", gap: "0.5rem", fontSize: "0.875rem", color: "#ccc", cursor: "pointer" }}
          >
            <input
              type="checkbox"
              checked={members.has(l.id)}
              onChange={() => toggle(l.id)}
              style={{ accentColor: "#f90" }}
            />
            {l.name}
          </label>
        ))}
      </div>

      <div style={{ display: "flex", gap: "0.5rem" }}>
        <button type="submit" style={S.button} disabled={saving || members.size === 0}>
          {saving ? "…" : submitLabel}
        </button>
        <button type="button" onClick={onCancel} style={S.buttonGhost}>
          Cancel
        </button>
      </div>
    </form>
  );
}

function ProviderCard({
  provider,
  onRemove,
  onDiscover,
  onImportGroups,
}: {
  provider: Provider;
  onRemove: () => void;
  onDiscover: () => Promise<void>;
  onImportGroups: () => Promise<void>;
}) {
  const [status, setStatus] = useState<ConnectionStatus | null>(null);
  const [discovering, setDiscovering] = useState(false);
  const [importing, setImporting] = useState(false);

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
    <div style={{ ...S.card, flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: "1rem" }}>
      <div style={{ minWidth: 0 }}>
        <div style={{ fontWeight: 600 }}>{provider.name}</div>
        <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", marginTop: "0.25rem" }}>
          <span style={{ color: "#888", fontSize: "0.8rem" }}>{provider.provider_type}</span>
          {status && <StatusBadge state={status.state} />}
        </div>
      </div>
      <div style={{ display: "flex", gap: "0.5rem", flexShrink: 0 }}>
        <button onClick={handleDiscover} disabled={discovering} style={S.buttonGhost}>
          {discovering ? "…" : "Discover"}
        </button>
        <button
          onClick={handleImport}
          disabled={importing}
          title="Import the provider's rooms/zones as Bifrost groups"
          style={S.buttonGhost}
        >
          {importing ? "…" : "Import rooms"}
        </button>
        <button onClick={onRemove} style={S.buttonDanger}>Remove</button>
      </div>
    </div>
  );
}

function StatusBadge({ state }: { state: string }) {
  const color =
    state === "connected" || state === "ok" ? "#4d4"
    : state === "connecting" || state === "reconnecting" ? "#fa0"
    : state === "failed" ? "#f44"
    : "#666";
  return (
    <span style={{ display: "inline-flex", alignItems: "center", gap: "0.3rem", fontSize: "0.75rem", color }}>
      <span style={{ width: 7, height: 7, borderRadius: "50%", background: color, display: "inline-block" }} />
      {state}
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

  const schema: CredentialField[] = types.find((t) => t.provider_type === selectedType)?.schema ?? [];

  // Clear credentials when the provider type changes.
  useEffect(() => { setCredentials({}); setPairMsg(""); }, [selectedType]);

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
    <form onSubmit={submit} style={{ ...S.card, border: "1px solid #333" }}>
      <h3 style={{ margin: 0, fontSize: "1rem", color: "#ccc" }}>Add Provider</h3>

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
        <select
          value={selectedType}
          onChange={(e) => setSelectedType(e.target.value)}
          style={{ ...S.input, cursor: "pointer" }}
        >
          {types.map((t) => (
            <option key={t.provider_type} value={t.provider_type}>
              {t.provider_type}
            </option>
          ))}
        </select>
      </label>

      {schema.map((field) => {
        // Hue's app key comes from link-button pairing, not manual entry.
        const isHueAppKey = selectedType === "hue" && field.name === "app_key";
        return (
          <label key={field.name} style={labelStyle}>
            <span>
              {field.label}
              {field.required && <span style={{ color: "#f90" }}> *</span>}
            </span>
            {!isHueAppKey && field.hint && (
              <span style={{ color: "#666", fontSize: "0.78rem" }}>{field.hint}</span>
            )}
            {isHueAppKey && (
              <span style={{ color: "#666", fontSize: "0.78rem" }}>
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
                  color: pairMsg.startsWith("✓") ? "#4d4" : "#fa0",
                }}
              >
                {pairMsg}
              </span>
            )}
          </label>
        );
      })}

      {error && <p style={{ color: "#f66", margin: 0, fontSize: "0.875rem" }}>{error}</p>}

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
  color: "#aaa",
};
