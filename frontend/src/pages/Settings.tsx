import { useEffect, useState } from "react";
import {
  addProvider,
  createRoom,
  discoverLights,
  getProviderGroups,
  getLights,
  getProviderStatus,
  getProviderTypes,
  getProviders,
  syncProviderGroups,
  pairHueBridge,
  getRooms,
  removeRoom,
  removeProvider,
  setRoomDirectLights,
  setRoomLinks,
  updateProviderCredentials,
  type ConnectionStatus,
  type CredentialField,
  type ProviderGroupInfo,
  type Room,
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
            types={types}
            onCredentialsSaved={() => showToast("Credentials updated — reconnecting.")}
            onRemove={() => handleRemove(p.id)}
            onDiscover={() => handleDiscover(p.id)}
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

      <RoomsSection />
    </div>
  );
}

// ── Rooms ────────────────────────────────────────────────────────────────────

function RoomsSection() {
  const [rooms, setRooms] = useState<Room[]>([]);
  const [providerGroups, setProviderGroups] = useState<ProviderGroupInfo[]>([]);
  const [lights, setLights] = useState<Light[]>([]);
  const [showAdd, setShowAdd] = useState(false);

  async function load() {
    setRooms(await getRooms());
    setProviderGroups(await getProviderGroups());
    const l = await getLights();
    if (l !== "unauthorized") setLights(l);
  }

  useEffect(() => { load(); }, []);

  async function handleRemove(id: string, name: string) {
    if (!window.confirm(`Delete room "${name}"? Its scenes and plan bindings go with it.`)) return;
    await removeRoom(id);
    await load();
  }

  return (
    <div style={{ marginTop: "2.5rem" }}>
      <h2 style={{ margin: "0 0 0.4rem", fontSize: "1.2rem", color: "#ccc" }}>Rooms</h2>
      <p style={{ color: "#666", fontSize: "0.8rem", margin: "0 0 1rem" }}>
        A room combines synced provider rooms/zones (links) with directly
        assigned lights. Use “Sync rooms” on a provider to refresh links.
      </p>

      <div style={{ display: "flex", flexDirection: "column", gap: "0.75rem" }}>
        {rooms.length === 0 && !showAdd && (
          <p style={{ color: "#666", margin: 0 }}>
            No rooms yet. Sync a provider, paint one in the planner, or add one here.
          </p>
        )}
        {rooms.map((room) => (
          <RoomCard
            key={room.id}
            room={room}
            lights={lights}
            providerGroups={providerGroups}
            onChanged={load}
            onRemove={() => handleRemove(room.id, room.name)}
          />
        ))}
      </div>

      {showAdd ? (
        <div style={{ marginTop: "1rem" }}>
          <RoomEditForm
            lights={lights}
            providerGroups={providerGroups}
            initialName=""
            initialDirect={[]}
            initialLinks={[]}
            submitLabel="Create"
            onSubmit={async (name, directIds, _linkIds) => {
              await createRoom(name, directIds);
              setShowAdd(false);
              await load();
            }}
            onCancel={() => setShowAdd(false)}
          />
        </div>
      ) : (
        <button onClick={() => setShowAdd(true)} style={{ ...S.button, marginTop: "1rem" }}>
          + Add Room
        </button>
      )}
    </div>
  );
}

function RoomCard({
  room,
  lights,
  providerGroups,
  onChanged,
  onRemove,
}: {
  room: Room;
  lights: Light[];
  providerGroups: ProviderGroupInfo[];
  onChanged: () => Promise<void>;
  onRemove: () => void;
}) {
  const [editing, setEditing] = useState(false);

  if (editing) {
    return (
      <RoomEditForm
        lights={lights}
        providerGroups={providerGroups}
        initialName={room.name}
        initialDirect={room.direct_light_ids}
        initialLinks={room.links.map((l) => l.provider_group_id)}
        submitLabel="Save"
        nameLocked
        onSubmit={async (_name, directIds, linkIds) => {
          await setRoomDirectLights(room.id, directIds);
          await setRoomLinks(room.id, linkIds);
          setEditing(false);
          await onChanged();
        }}
        onCancel={() => setEditing(false)}
      />
    );
  }

  return (
    <div style={{ ...S.card, flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: "1rem" }}>
      <div style={{ minWidth: 0 }}>
        <div style={{ fontWeight: 600 }}>{room.name}</div>
        <div style={{ color: "#888", fontSize: "0.8rem", marginTop: "0.25rem", display: "flex", gap: "0.5rem", flexWrap: "wrap" }}>
          <span>{room.light_ids.length} light{room.light_ids.length !== 1 ? "s" : ""}</span>
          {room.links.map((l) => (
            <span
              key={l.provider_group_id}
              title="Synced provider room/zone"
              style={{ border: "1px solid #333", borderRadius: 4, padding: "0 0.35rem", color: "#9a9", fontSize: "0.72rem" }}
            >
              ⇄ {l.name}
            </span>
          ))}
        </div>
      </div>
      <div style={{ display: "flex", gap: "0.5rem", flexShrink: 0 }}>
        <button onClick={() => setEditing(true)} style={S.buttonGhost}>Edit</button>
        <button onClick={onRemove} style={S.buttonDanger}>Remove</button>
      </div>
    </div>
  );
}

function RoomEditForm({
  lights,
  providerGroups,
  initialName,
  initialDirect,
  initialLinks,
  submitLabel,
  nameLocked,
  onSubmit,
  onCancel,
}: {
  lights: Light[];
  providerGroups: ProviderGroupInfo[];
  initialName: string;
  initialDirect: string[];
  initialLinks: string[];
  submitLabel: string;
  nameLocked?: boolean;
  onSubmit: (name: string, directIds: string[], linkIds: string[]) => Promise<void>;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initialName);
  const [direct, setDirect] = useState<Set<string>>(new Set(initialDirect));
  const [links, setLinks] = useState<Set<string>>(new Set(initialLinks));
  const [saving, setSaving] = useState(false);

  function toggleSet(setter: React.Dispatch<React.SetStateAction<Set<string>>>, id: string) {
    setter((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  // Lights already covered by a selected link (shown, but as link members).
  const linkedLightIds = new Set(
    providerGroups.filter((pg) => links.has(pg.id)).flatMap((pg) => pg.light_ids),
  );

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    try {
      await onSubmit(name.trim(), [...direct], [...links]);
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

      {providerGroups.length > 0 && (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.35rem" }}>
          <span style={{ fontSize: "0.875rem", color: "#aaa" }}>
            Linked provider rooms <span style={{ color: "#666" }}>(membership syncs automatically)</span>
          </span>
          {providerGroups.map((pg) => (
            <label
              key={pg.id}
              style={{ display: "flex", alignItems: "center", gap: "0.5rem", fontSize: "0.875rem", color: "#ccc", cursor: "pointer" }}
            >
              <input
                type="checkbox"
                checked={links.has(pg.id)}
                onChange={() => toggleSet(setLinks, pg.id)}
                style={{ accentColor: "#f90" }}
              />
              ⇄ {pg.name}
              <span style={{ color: "#666", fontSize: "0.75rem" }}>
                {pg.light_ids.length} light{pg.light_ids.length !== 1 ? "s" : ""}
              </span>
            </label>
          ))}
        </div>
      )}

      <div style={{ display: "flex", flexDirection: "column", gap: "0.35rem" }}>
        <span style={{ fontSize: "0.875rem", color: "#aaa" }}>Direct lights</span>
        {lights.map((l) => {
          const viaLink = linkedLightIds.has(l.id);
          return (
            <label
              key={l.id}
              style={{
                display: "flex",
                alignItems: "center",
                gap: "0.5rem",
                fontSize: "0.875rem",
                color: viaLink ? "#666" : "#ccc",
                cursor: viaLink ? "default" : "pointer",
              }}
            >
              <input
                type="checkbox"
                checked={viaLink || direct.has(l.id)}
                disabled={viaLink}
                onChange={() => toggleSet(setDirect, l.id)}
                style={{ accentColor: "#f90" }}
              />
              {l.name}
              {viaLink && <span style={{ fontSize: "0.72rem" }}>(via link)</span>}
            </label>
          );
        })}
      </div>

      <div style={{ display: "flex", gap: "0.5rem" }}>
        <button type="submit" style={S.button} disabled={saving}>
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
  types,
  onCredentialsSaved,
  onRemove,
  onDiscover,
  onImportGroups,
}: {
  provider: Provider;
  types: ProviderType[];
  onCredentialsSaved: () => void;
  onRemove: () => void;
  onDiscover: () => Promise<void>;
  onImportGroups: () => Promise<void>;
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
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: "1rem" }}>
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
            title="Sync the provider's rooms/zones into Bifrost Rooms"
            style={S.buttonGhost}
          >
            {importing ? "…" : "Sync rooms"}
          </button>
          <button
            onClick={() => setEditingCreds((v) => !v)}
            title="Re-enter credentials (e.g. after a BIFROST_SECRET change)"
            style={S.buttonGhost}
          >
            {editingCreds ? "Close" : "Edit credentials"}
          </button>
          <button onClick={onRemove} style={S.buttonDanger}>Remove</button>
        </div>
      </div>
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

/// Re-enter credentials for an existing provider. The provider row (and all
/// lights, scenes, groups, plans referencing it) stays intact.
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
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [pairing, setPairing] = useState(false);
  const [pairMsg, setPairMsg] = useState("");

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
    <form onSubmit={submit} style={{ display: "flex", flexDirection: "column", gap: "0.6rem", borderTop: "1px solid #2a2a2a", paddingTop: "0.75rem" }}>
      {schema.map((field) => {
        const isHueAppKey = provider.provider_type === "hue" && field.name === "app_key";
        return (
          <label key={field.name} style={labelStyle}>
            <span>
              {field.label}
              {field.required && <span style={{ color: "#f90" }}> *</span>}
            </span>
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
              <span style={{ fontSize: "0.78rem", color: pairMsg.startsWith("✓") ? "#4d4" : "#fa0" }}>
                {pairMsg}
              </span>
            )}
          </label>
        );
      })}
      {error && <p style={{ color: "#f66", margin: 0, fontSize: "0.875rem" }}>{error}</p>}
      <div style={{ display: "flex", gap: "0.5rem" }}>
        <button type="submit" style={S.button} disabled={saving}>
          {saving ? "Saving…" : "Save credentials"}
        </button>
        <button type="button" onClick={onCancel} style={S.buttonGhost}>Cancel</button>
      </div>
    </form>
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
