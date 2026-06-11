import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useEffect, useState } from "react";
import { addProvider, createRoom, discoverLights, getProviderGroups, getLights, getProviderStatus, getProviderTypes, getProviders, syncProviderGroups, pairHueBridge, getRooms, removeRoom, removeProvider, setRoomDirectLights, setRoomLinks, updateProviderCredentials, } from "../api";
import { S } from "../styles";
export function SettingsPage({ onNavigate: _onNavigate }) {
    const [providers, setProviders] = useState([]);
    const [types, setTypes] = useState([]);
    const [showAdd, setShowAdd] = useState(false);
    const [toast, setToast] = useState("");
    async function loadProviders() {
        setProviders(await getProviders());
    }
    useEffect(() => {
        loadProviders();
        getProviderTypes().then(setTypes);
    }, []);
    function showToast(msg) {
        setToast(msg);
        setTimeout(() => setToast(""), 3000);
    }
    async function handleRemove(id) {
        if (!window.confirm("Remove this provider? Associated lights will be deleted."))
            return;
        await removeProvider(id);
        await loadProviders();
    }
    async function handleDiscover(id) {
        const result = await discoverLights(id);
        await loadProviders();
        showToast(`Discovered ${result.discovered} light${result.discovered !== 1 ? "s" : ""}.`);
    }
    async function handleAdded(id) {
        setShowAdd(false);
        await loadProviders();
        // Run discovery right away so lights appear without an extra click.
        try {
            const result = await discoverLights(id);
            showToast(`Provider added — found ${result.discovered} light${result.discovered !== 1 ? "s" : ""}.`);
        }
        catch {
            showToast("Provider added. Discovery failed — check the connection and try Discover.");
        }
    }
    return (_jsxs("div", { style: { padding: "2rem", maxWidth: 720, margin: "0 auto" }, children: [_jsx("h2", { style: { margin: "0 0 1.5rem", fontSize: "1.2rem", color: "#ccc" }, children: "Providers" }), toast && (_jsx("div", { style: { background: "#1e3a1e", border: "1px solid #2a5a2a", borderRadius: 8, padding: "0.6rem 1rem", marginBottom: "1rem", color: "#8f8", fontSize: "0.875rem" }, children: toast })), _jsxs("div", { style: { display: "flex", flexDirection: "column", gap: "0.75rem" }, children: [providers.length === 0 && !showAdd && (_jsx("p", { style: { color: "#666", margin: 0 }, children: "No providers configured." })), providers.map((p) => (_jsx(ProviderCard, { provider: p, types: types, onCredentialsSaved: () => showToast("Credentials updated — reconnecting."), onRemove: () => handleRemove(p.id), onDiscover: () => handleDiscover(p.id), onImportGroups: async () => {
                            const r = await syncProviderGroups(p.id);
                            showToast(r.synced === 0
                                ? "No rooms or zones defined on this provider."
                                : `Synced ${r.synced} room${r.synced !== 1 ? "s" : ""} (${r.rooms_created} created, ${r.rooms_linked} linked).`);
                        } }, p.id)))] }), showAdd ? (_jsx("div", { style: { marginTop: "1.5rem" }, children: _jsx(AddProviderForm, { types: types, onAdded: handleAdded, onCancel: () => setShowAdd(false) }) })) : (_jsx("button", { onClick: () => setShowAdd(true), style: { ...S.button, marginTop: "1.5rem" }, children: "+ Add Provider" })), _jsx(RoomsSection, {})] }));
}
// ── Rooms ────────────────────────────────────────────────────────────────────
function RoomsSection() {
    const [rooms, setRooms] = useState([]);
    const [providerGroups, setProviderGroups] = useState([]);
    const [lights, setLights] = useState([]);
    const [showAdd, setShowAdd] = useState(false);
    async function load() {
        setRooms(await getRooms());
        setProviderGroups(await getProviderGroups());
        const l = await getLights();
        if (l !== "unauthorized")
            setLights(l);
    }
    useEffect(() => { load(); }, []);
    async function handleRemove(id, name) {
        if (!window.confirm(`Delete room "${name}"? Its scenes and plan bindings go with it.`))
            return;
        await removeRoom(id);
        await load();
    }
    return (_jsxs("div", { style: { marginTop: "2.5rem" }, children: [_jsx("h2", { style: { margin: "0 0 0.4rem", fontSize: "1.2rem", color: "#ccc" }, children: "Rooms" }), _jsx("p", { style: { color: "#666", fontSize: "0.8rem", margin: "0 0 1rem" }, children: "A room combines synced provider rooms/zones (links) with directly assigned lights. Use \u201CSync rooms\u201D on a provider to refresh links." }), _jsxs("div", { style: { display: "flex", flexDirection: "column", gap: "0.75rem" }, children: [rooms.length === 0 && !showAdd && (_jsx("p", { style: { color: "#666", margin: 0 }, children: "No rooms yet. Sync a provider, paint one in the planner, or add one here." })), rooms.map((room) => (_jsx(RoomCard, { room: room, lights: lights, providerGroups: providerGroups, onChanged: load, onRemove: () => handleRemove(room.id, room.name) }, room.id)))] }), showAdd ? (_jsx("div", { style: { marginTop: "1rem" }, children: _jsx(RoomEditForm, { lights: lights, providerGroups: providerGroups, initialName: "", initialDirect: [], initialLinks: [], submitLabel: "Create", onSubmit: async (name, directIds, _linkIds) => {
                        await createRoom(name, directIds);
                        setShowAdd(false);
                        await load();
                    }, onCancel: () => setShowAdd(false) }) })) : (_jsx("button", { onClick: () => setShowAdd(true), style: { ...S.button, marginTop: "1rem" }, children: "+ Add Room" }))] }));
}
function RoomCard({ room, lights, providerGroups, onChanged, onRemove, }) {
    const [editing, setEditing] = useState(false);
    if (editing) {
        return (_jsx(RoomEditForm, { lights: lights, providerGroups: providerGroups, initialName: room.name, initialDirect: room.direct_light_ids, initialLinks: room.links.map((l) => l.provider_group_id), submitLabel: "Save", nameLocked: true, onSubmit: async (_name, directIds, linkIds) => {
                await setRoomDirectLights(room.id, directIds);
                await setRoomLinks(room.id, linkIds);
                setEditing(false);
                await onChanged();
            }, onCancel: () => setEditing(false) }));
    }
    return (_jsxs("div", { style: { ...S.card, flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: "1rem" }, children: [_jsxs("div", { style: { minWidth: 0 }, children: [_jsx("div", { style: { fontWeight: 600 }, children: room.name }), _jsxs("div", { style: { color: "#888", fontSize: "0.8rem", marginTop: "0.25rem", display: "flex", gap: "0.5rem", flexWrap: "wrap" }, children: [_jsxs("span", { children: [room.light_ids.length, " light", room.light_ids.length !== 1 ? "s" : ""] }), room.links.map((l) => (_jsxs("span", { title: "Synced provider room/zone", style: { border: "1px solid #333", borderRadius: 4, padding: "0 0.35rem", color: "#9a9", fontSize: "0.72rem" }, children: ["\u21C4 ", l.name] }, l.provider_group_id)))] })] }), _jsxs("div", { style: { display: "flex", gap: "0.5rem", flexShrink: 0 }, children: [_jsx("button", { onClick: () => setEditing(true), style: S.buttonGhost, children: "Edit" }), _jsx("button", { onClick: onRemove, style: S.buttonDanger, children: "Remove" })] })] }));
}
function RoomEditForm({ lights, providerGroups, initialName, initialDirect, initialLinks, submitLabel, nameLocked, onSubmit, onCancel, }) {
    const [name, setName] = useState(initialName);
    const [direct, setDirect] = useState(new Set(initialDirect));
    const [links, setLinks] = useState(new Set(initialLinks));
    const [saving, setSaving] = useState(false);
    function toggleSet(setter, id) {
        setter((prev) => {
            const next = new Set(prev);
            if (next.has(id))
                next.delete(id);
            else
                next.add(id);
            return next;
        });
    }
    // Lights already covered by a selected link (shown, but as link members).
    const linkedLightIds = new Set(providerGroups.filter((pg) => links.has(pg.id)).flatMap((pg) => pg.light_ids));
    async function submit(e) {
        e.preventDefault();
        setSaving(true);
        try {
            await onSubmit(name.trim(), [...direct], [...links]);
        }
        finally {
            setSaving(false);
        }
    }
    return (_jsxs("form", { onSubmit: submit, style: { ...S.card, border: "1px solid #333" }, children: [!nameLocked && (_jsxs("label", { style: labelStyle, children: [_jsx("span", { children: "Name" }), _jsx("input", { value: name, onChange: (e) => setName(e.target.value), placeholder: "e.g. Living Room", style: S.input, required: true, autoFocus: true })] })), nameLocked && _jsx("h3", { style: { margin: 0, fontSize: "1rem", color: "#ccc" }, children: name }), providerGroups.length > 0 && (_jsxs("div", { style: { display: "flex", flexDirection: "column", gap: "0.35rem" }, children: [_jsxs("span", { style: { fontSize: "0.875rem", color: "#aaa" }, children: ["Linked provider rooms ", _jsx("span", { style: { color: "#666" }, children: "(membership syncs automatically)" })] }), providerGroups.map((pg) => (_jsxs("label", { style: { display: "flex", alignItems: "center", gap: "0.5rem", fontSize: "0.875rem", color: "#ccc", cursor: "pointer" }, children: [_jsx("input", { type: "checkbox", checked: links.has(pg.id), onChange: () => toggleSet(setLinks, pg.id), style: { accentColor: "#f90" } }), "\u21C4 ", pg.name, _jsxs("span", { style: { color: "#666", fontSize: "0.75rem" }, children: [pg.light_ids.length, " light", pg.light_ids.length !== 1 ? "s" : ""] })] }, pg.id)))] })), _jsxs("div", { style: { display: "flex", flexDirection: "column", gap: "0.35rem" }, children: [_jsx("span", { style: { fontSize: "0.875rem", color: "#aaa" }, children: "Direct lights" }), lights.map((l) => {
                        const viaLink = linkedLightIds.has(l.id);
                        return (_jsxs("label", { style: {
                                display: "flex",
                                alignItems: "center",
                                gap: "0.5rem",
                                fontSize: "0.875rem",
                                color: viaLink ? "#666" : "#ccc",
                                cursor: viaLink ? "default" : "pointer",
                            }, children: [_jsx("input", { type: "checkbox", checked: viaLink || direct.has(l.id), disabled: viaLink, onChange: () => toggleSet(setDirect, l.id), style: { accentColor: "#f90" } }), l.name, viaLink && _jsx("span", { style: { fontSize: "0.72rem" }, children: "(via link)" })] }, l.id));
                    })] }), _jsxs("div", { style: { display: "flex", gap: "0.5rem" }, children: [_jsx("button", { type: "submit", style: S.button, disabled: saving, children: saving ? "…" : submitLabel }), _jsx("button", { type: "button", onClick: onCancel, style: S.buttonGhost, children: "Cancel" })] })] }));
}
function ProviderCard({ provider, types, onCredentialsSaved, onRemove, onDiscover, onImportGroups, }) {
    const [status, setStatus] = useState(null);
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
        }
        finally {
            setImporting(false);
        }
    }
    return (_jsxs("div", { style: { ...S.card, gap: "0.75rem" }, children: [_jsxs("div", { style: { display: "flex", alignItems: "center", justifyContent: "space-between", gap: "1rem" }, children: [_jsxs("div", { style: { minWidth: 0 }, children: [_jsx("div", { style: { fontWeight: 600 }, children: provider.name }), _jsxs("div", { style: { display: "flex", alignItems: "center", gap: "0.5rem", marginTop: "0.25rem" }, children: [_jsx("span", { style: { color: "#888", fontSize: "0.8rem" }, children: provider.provider_type }), status && _jsx(StatusBadge, { state: status.state })] })] }), _jsxs("div", { style: { display: "flex", gap: "0.5rem", flexShrink: 0 }, children: [_jsx("button", { onClick: handleDiscover, disabled: discovering, style: S.buttonGhost, children: discovering ? "…" : "Discover" }), _jsx("button", { onClick: handleImport, disabled: importing, title: "Sync the provider's rooms/zones into Bifrost Rooms", style: S.buttonGhost, children: importing ? "…" : "Sync rooms" }), _jsx("button", { onClick: () => setEditingCreds((v) => !v), title: "Re-enter credentials (e.g. after a BIFROST_SECRET change)", style: S.buttonGhost, children: editingCreds ? "Close" : "Edit credentials" }), _jsx("button", { onClick: onRemove, style: S.buttonDanger, children: "Remove" })] })] }), editingCreds && (_jsx(EditCredentialsForm, { provider: provider, schema: types.find((t) => t.provider_type === provider.provider_type)?.schema ?? [], onSaved: () => {
                    setEditingCreds(false);
                    onCredentialsSaved();
                }, onCancel: () => setEditingCreds(false) }))] }));
}
/// Re-enter credentials for an existing provider. The provider row (and all
/// lights, scenes, groups, plans referencing it) stays intact.
function EditCredentialsForm({ provider, schema, onSaved, onCancel, }) {
    const [credentials, setCredentials] = useState({});
    const [error, setError] = useState("");
    const [saving, setSaving] = useState(false);
    const [pairing, setPairing] = useState(false);
    const [pairMsg, setPairMsg] = useState("");
    function setField(name, value) {
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
        }
        else if (result.error === "link_button_not_pressed") {
            setPairMsg("Press the round link button on the bridge, then click Pair again.");
        }
        else {
            setPairMsg(`Could not reach the bridge: ${result.message}`);
        }
    }
    async function submit(e) {
        e.preventDefault();
        setError("");
        setSaving(true);
        const result = await updateProviderCredentials(provider.id, credentials);
        setSaving(false);
        if ("error" in result)
            setError(result.error);
        else
            onSaved();
    }
    return (_jsxs("form", { onSubmit: submit, style: { display: "flex", flexDirection: "column", gap: "0.6rem", borderTop: "1px solid #2a2a2a", paddingTop: "0.75rem" }, children: [schema.map((field) => {
                const isHueAppKey = provider.provider_type === "hue" && field.name === "app_key";
                return (_jsxs("label", { style: labelStyle, children: [_jsxs("span", { children: [field.label, field.required && _jsx("span", { style: { color: "#f90" }, children: " *" })] }), _jsxs("div", { style: { display: "flex", gap: "0.5rem" }, children: [_jsx("input", { type: field.kind === "password" ? "password" : "text", value: credentials[field.name] ?? "", onChange: (e) => setField(field.name, e.target.value), style: { ...S.input, flex: 1 }, required: field.required, autoComplete: field.kind === "password" ? "new-password" : "off" }), isHueAppKey && (_jsx("button", { type: "button", onClick: handlePair, disabled: pairing || !(credentials.bridge_ip ?? "").trim(), style: S.buttonGhost, children: pairing ? "Pairing…" : "Pair" }))] }), isHueAppKey && pairMsg && (_jsx("span", { style: { fontSize: "0.78rem", color: pairMsg.startsWith("✓") ? "#4d4" : "#fa0" }, children: pairMsg }))] }, field.name));
            }), error && _jsx("p", { style: { color: "#f66", margin: 0, fontSize: "0.875rem" }, children: error }), _jsxs("div", { style: { display: "flex", gap: "0.5rem" }, children: [_jsx("button", { type: "submit", style: S.button, disabled: saving, children: saving ? "Saving…" : "Save credentials" }), _jsx("button", { type: "button", onClick: onCancel, style: S.buttonGhost, children: "Cancel" })] })] }));
}
function StatusBadge({ state }) {
    const color = state === "connected" || state === "ok" ? "#4d4"
        : state === "connecting" || state === "reconnecting" ? "#fa0"
            : state === "failed" ? "#f44"
                : "#666";
    return (_jsxs("span", { style: { display: "inline-flex", alignItems: "center", gap: "0.3rem", fontSize: "0.75rem", color }, children: [_jsx("span", { style: { width: 7, height: 7, borderRadius: "50%", background: color, display: "inline-block" } }), state] }));
}
function AddProviderForm({ types, onAdded, onCancel, }) {
    const [selectedType, setSelectedType] = useState(types[0]?.provider_type ?? "");
    const [name, setName] = useState("");
    const [credentials, setCredentials] = useState({});
    const [error, setError] = useState("");
    const [loading, setLoading] = useState(false);
    const [pairing, setPairing] = useState(false);
    const [pairMsg, setPairMsg] = useState("");
    const schema = types.find((t) => t.provider_type === selectedType)?.schema ?? [];
    // Clear credentials when the provider type changes.
    useEffect(() => { setCredentials({}); setPairMsg(""); }, [selectedType]);
    async function submit(e) {
        e.preventDefault();
        setError("");
        setLoading(true);
        const result = await addProvider(name, selectedType, credentials);
        setLoading(false);
        if ("error" in result)
            setError(result.error);
        else
            onAdded(result.id);
    }
    function setField(fieldName, value) {
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
        }
        else if (result.error === "link_button_not_pressed") {
            setPairMsg("Press the round link button on the bridge, then click Pair again.");
        }
        else {
            setPairMsg(`Could not reach the bridge: ${result.message}`);
        }
    }
    return (_jsxs("form", { onSubmit: submit, style: { ...S.card, border: "1px solid #333" }, children: [_jsx("h3", { style: { margin: 0, fontSize: "1rem", color: "#ccc" }, children: "Add Provider" }), _jsxs("label", { style: labelStyle, children: [_jsx("span", { children: "Name" }), _jsx("input", { value: name, onChange: (e) => setName(e.target.value), placeholder: "e.g. Living Room Hue", style: S.input, required: true, autoFocus: true })] }), _jsxs("label", { style: labelStyle, children: [_jsx("span", { children: "Type" }), _jsx("select", { value: selectedType, onChange: (e) => setSelectedType(e.target.value), style: { ...S.input, cursor: "pointer" }, children: types.map((t) => (_jsx("option", { value: t.provider_type, children: t.provider_type }, t.provider_type))) })] }), schema.map((field) => {
                // Hue's app key comes from link-button pairing, not manual entry.
                const isHueAppKey = selectedType === "hue" && field.name === "app_key";
                return (_jsxs("label", { style: labelStyle, children: [_jsxs("span", { children: [field.label, field.required && _jsx("span", { style: { color: "#f90" }, children: " *" })] }), !isHueAppKey && field.hint && (_jsx("span", { style: { color: "#666", fontSize: "0.78rem" }, children: field.hint })), isHueAppKey && (_jsx("span", { style: { color: "#666", fontSize: "0.78rem" }, children: "Press the link button on the bridge, then click Pair \u2014 or paste a key manually." })), _jsxs("div", { style: { display: "flex", gap: "0.5rem" }, children: [_jsx("input", { type: field.kind === "password" ? "password" : "text", value: credentials[field.name] ?? "", onChange: (e) => setField(field.name, e.target.value), style: { ...S.input, flex: 1 }, required: field.required, autoComplete: field.kind === "password" ? "new-password" : "off" }), isHueAppKey && (_jsx("button", { type: "button", onClick: handlePair, disabled: pairing || !(credentials.bridge_ip ?? "").trim(), style: S.buttonGhost, children: pairing ? "Pairing…" : "Pair" }))] }), isHueAppKey && pairMsg && (_jsx("span", { style: {
                                fontSize: "0.78rem",
                                color: pairMsg.startsWith("✓") ? "#4d4" : "#fa0",
                            }, children: pairMsg }))] }, field.name));
            }), error && _jsx("p", { style: { color: "#f66", margin: 0, fontSize: "0.875rem" }, children: error }), _jsxs("div", { style: { display: "flex", gap: "0.5rem" }, children: [_jsx("button", { type: "submit", style: S.button, disabled: loading || types.length === 0, children: loading ? "Adding…" : "Add" }), _jsx("button", { type: "button", onClick: onCancel, style: S.buttonGhost, children: "Cancel" })] })] }));
}
const labelStyle = {
    display: "flex",
    flexDirection: "column",
    gap: "0.3rem",
    fontSize: "0.875rem",
    color: "#aaa",
};
