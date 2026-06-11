import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useEffect, useState } from "react";
import { addProvider, discoverLights, getProviderStatus, getProviderTypes, getProviders, removeProvider, } from "../api";
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
    async function handleAdded() {
        setShowAdd(false);
        await loadProviders();
        showToast("Provider added.");
    }
    return (_jsxs("div", { style: { padding: "2rem", maxWidth: 720, margin: "0 auto" }, children: [_jsx("h2", { style: { margin: "0 0 1.5rem", fontSize: "1.2rem", color: "#ccc" }, children: "Providers" }), toast && (_jsx("div", { style: { background: "#1e3a1e", border: "1px solid #2a5a2a", borderRadius: 8, padding: "0.6rem 1rem", marginBottom: "1rem", color: "#8f8", fontSize: "0.875rem" }, children: toast })), _jsxs("div", { style: { display: "flex", flexDirection: "column", gap: "0.75rem" }, children: [providers.length === 0 && !showAdd && (_jsx("p", { style: { color: "#666", margin: 0 }, children: "No providers configured." })), providers.map((p) => (_jsx(ProviderCard, { provider: p, onRemove: () => handleRemove(p.id), onDiscover: () => handleDiscover(p.id) }, p.id)))] }), showAdd ? (_jsx("div", { style: { marginTop: "1.5rem" }, children: _jsx(AddProviderForm, { types: types, onAdded: handleAdded, onCancel: () => setShowAdd(false) }) })) : (_jsx("button", { onClick: () => setShowAdd(true), style: { ...S.button, marginTop: "1.5rem" }, children: "+ Add Provider" }))] }));
}
function ProviderCard({ provider, onRemove, onDiscover, }) {
    const [status, setStatus] = useState(null);
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
    return (_jsxs("div", { style: { ...S.card, flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: "1rem" }, children: [_jsxs("div", { style: { minWidth: 0 }, children: [_jsx("div", { style: { fontWeight: 600 }, children: provider.name }), _jsxs("div", { style: { display: "flex", alignItems: "center", gap: "0.5rem", marginTop: "0.25rem" }, children: [_jsx("span", { style: { color: "#888", fontSize: "0.8rem" }, children: provider.provider_type }), status && _jsx(StatusBadge, { state: status.state })] })] }), _jsxs("div", { style: { display: "flex", gap: "0.5rem", flexShrink: 0 }, children: [_jsx("button", { onClick: handleDiscover, disabled: discovering, style: S.buttonGhost, children: discovering ? "…" : "Discover" }), _jsx("button", { onClick: onRemove, style: S.buttonDanger, children: "Remove" })] })] }));
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
    const schema = types.find((t) => t.provider_type === selectedType)?.schema ?? [];
    // Clear credentials when the provider type changes.
    useEffect(() => { setCredentials({}); }, [selectedType]);
    async function submit(e) {
        e.preventDefault();
        setError("");
        setLoading(true);
        const result = await addProvider(name, selectedType, credentials);
        setLoading(false);
        if ("error" in result)
            setError(result.error);
        else
            onAdded();
    }
    function setField(fieldName, value) {
        setCredentials((prev) => ({ ...prev, [fieldName]: value }));
    }
    return (_jsxs("form", { onSubmit: submit, style: { ...S.card, border: "1px solid #333" }, children: [_jsx("h3", { style: { margin: 0, fontSize: "1rem", color: "#ccc" }, children: "Add Provider" }), _jsxs("label", { style: labelStyle, children: [_jsx("span", { children: "Name" }), _jsx("input", { value: name, onChange: (e) => setName(e.target.value), placeholder: "e.g. Living Room Hue", style: S.input, required: true, autoFocus: true })] }), _jsxs("label", { style: labelStyle, children: [_jsx("span", { children: "Type" }), _jsx("select", { value: selectedType, onChange: (e) => setSelectedType(e.target.value), style: { ...S.input, cursor: "pointer" }, children: types.map((t) => (_jsx("option", { value: t.provider_type, children: t.provider_type }, t.provider_type))) })] }), schema.map((field) => (_jsxs("label", { style: labelStyle, children: [_jsxs("span", { children: [field.label, field.required && _jsx("span", { style: { color: "#f90" }, children: " *" })] }), field.hint && _jsx("span", { style: { color: "#666", fontSize: "0.78rem" }, children: field.hint }), _jsx("input", { type: field.kind === "password" ? "password" : "text", value: credentials[field.name] ?? "", onChange: (e) => setField(field.name, e.target.value), style: S.input, required: field.required, autoComplete: field.kind === "password" ? "new-password" : "off" })] }, field.name))), error && _jsx("p", { style: { color: "#f66", margin: 0, fontSize: "0.875rem" }, children: error }), _jsxs("div", { style: { display: "flex", gap: "0.5rem" }, children: [_jsx("button", { type: "submit", style: S.button, disabled: loading || types.length === 0, children: loading ? "Adding…" : "Add" }), _jsx("button", { type: "button", onClick: onCancel, style: S.buttonGhost, children: "Cancel" })] })] }));
}
const labelStyle = {
    display: "flex",
    flexDirection: "column",
    gap: "0.3rem",
    fontSize: "0.875rem",
    color: "#aaa",
};
