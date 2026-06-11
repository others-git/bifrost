import { useEffect, useState } from "react";
import {
  addProvider,
  discoverLights,
  getProviderStatus,
  getProviderTypes,
  getProviders,
  removeProvider,
  type ConnectionStatus,
  type CredentialField,
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

  async function handleAdded() {
    setShowAdd(false);
    await loadProviders();
    showToast("Provider added.");
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
    </div>
  );
}

function ProviderCard({
  provider,
  onRemove,
  onDiscover,
}: {
  provider: Provider;
  onRemove: () => void;
  onDiscover: () => Promise<void>;
}) {
  const [status, setStatus] = useState<ConnectionStatus | null>(null);
  const [discovering, setDiscovering] = useState(false);

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
  onAdded: () => void;
  onCancel: () => void;
}) {
  const [selectedType, setSelectedType] = useState(types[0]?.provider_type ?? "");
  const [name, setName] = useState("");
  const [credentials, setCredentials] = useState<Record<string, string>>({});
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  const schema: CredentialField[] = types.find((t) => t.provider_type === selectedType)?.schema ?? [];

  // Clear credentials when the provider type changes.
  useEffect(() => { setCredentials({}); }, [selectedType]);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    const result = await addProvider(name, selectedType, credentials);
    setLoading(false);
    if ("error" in result) setError(result.error);
    else onAdded();
  }

  function setField(fieldName: string, value: string) {
    setCredentials((prev) => ({ ...prev, [fieldName]: value }));
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

      {schema.map((field) => (
        <label key={field.name} style={labelStyle}>
          <span>
            {field.label}
            {field.required && <span style={{ color: "#f90" }}> *</span>}
          </span>
          {field.hint && <span style={{ color: "#666", fontSize: "0.78rem" }}>{field.hint}</span>}
          <input
            type={field.kind === "password" ? "password" : "text"}
            value={credentials[field.name] ?? ""}
            onChange={(e) => setField(field.name, e.target.value)}
            style={S.input}
            required={field.required}
            autoComplete={field.kind === "password" ? "new-password" : "off"}
          />
        </label>
      ))}

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
