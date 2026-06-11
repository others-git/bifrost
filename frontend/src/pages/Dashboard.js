import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useEffect, useRef, useState } from "react";
import { activateScene, createScene, getGroups, getScenes, removeScene, rgbToXy, setGroupState, setLightState, } from "../api";
import { S } from "../styles";
export function DashboardPage({ lights, onRefresh, onNavigate }) {
    // Local copy so SSE events can update individual lights without a full server round-trip.
    const [localLights, setLocalLights] = useState(lights);
    // Keep in sync when the parent does a full refresh (authoritative server state wins).
    useEffect(() => { setLocalLights(lights); }, [lights]);
    // Real-time light state from Hue SSE → our SSE → browser.
    useEffect(() => {
        const es = new EventSource("/api/events");
        es.addEventListener("light_state", (raw) => {
            const { device_id, state } = JSON.parse(raw.data);
            setLocalLights((prev) => prev.map((l) => (l.device_id === device_id ? { ...l, last_state: state } : l)));
        });
        // Browser reconnects automatically on error; nothing to do here.
        es.onerror = () => { };
        return () => es.close();
    }, []); // open once per mount — reconnect is handled by the browser
    function handleLocalUpdate(id, state) {
        setLocalLights((prev) => prev.map((l) => (l.id === id ? { ...l, last_state: state } : l)));
    }
    return (_jsxs("div", { style: { padding: "2rem", maxWidth: 960, margin: "0 auto" }, children: [localLights.length > 0 && _jsx(SceneBar, { onActivated: onRefresh }), localLights.length > 0 && _jsx(GroupBar, { onChanged: onRefresh }), localLights.length === 0 ? (_jsxs("div", { style: { textAlign: "center", padding: "4rem 0", color: "#666" }, children: [_jsx("p", { style: { margin: "0 0 0.75rem" }, children: "No lights found." }), _jsxs("p", { style: { margin: 0, fontSize: "0.875rem" }, children: ["Add a provider in", " ", _jsx("button", { onClick: () => onNavigate("settings"), style: {
                                    background: "none",
                                    border: "none",
                                    color: "#f90",
                                    cursor: "pointer",
                                    fontSize: "0.875rem",
                                    padding: 0,
                                }, children: "Settings" }), " ", "and run discovery."] })] })) : (_jsx("div", { style: {
                    display: "grid",
                    gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))",
                    gap: "1rem",
                }, children: localLights.map((light) => (_jsx(LightCard, { light: light, onLocalUpdate: handleLocalUpdate, onChanged: onRefresh }, light.id))) }))] }));
}
function SceneBar({ onActivated }) {
    const [scenes, setScenes] = useState([]);
    const [busy, setBusy] = useState("");
    async function load() {
        setScenes(await getScenes());
    }
    useEffect(() => { load(); }, []);
    async function handleActivate(id) {
        setBusy(id);
        try {
            await activateScene(id);
            onActivated(); // refresh light states from the server
        }
        finally {
            setBusy("");
        }
    }
    async function handleSave() {
        const name = window.prompt("Scene name (saves the current state of all lights):");
        if (!name?.trim())
            return;
        await createScene(name.trim());
        await load();
    }
    async function handleRemove(id, name) {
        if (!window.confirm(`Delete scene "${name}"?`))
            return;
        await removeScene(id);
        await load();
    }
    return (_jsxs("div", { style: { display: "flex", flexWrap: "wrap", gap: "0.5rem", alignItems: "center", marginBottom: "1.25rem" }, children: [scenes.map((s) => (_jsxs("span", { style: { display: "inline-flex" }, children: [_jsx("button", { onClick: () => handleActivate(s.id), disabled: busy === s.id, title: `Apply "${s.name}" (${s.lights} light${s.lights !== 1 ? "s" : ""})`, style: { ...S.buttonGhost, borderRadius: "6px 0 0 6px" }, children: busy === s.id ? "…" : s.name }), _jsx("button", { onClick: () => handleRemove(s.id, s.name), title: "Delete scene", style: { ...S.buttonGhost, borderRadius: "0 6px 6px 0", borderLeft: "none", padding: "0.45rem 0.55rem", color: "#866" }, children: "\u00D7" })] }, s.id))), _jsx("button", { onClick: handleSave, style: S.buttonGhost, title: "Save the current light states as a scene", children: "+ Save scene" })] }));
}
function GroupBar({ onChanged }) {
    const [groups, setGroups] = useState([]);
    const [busy, setBusy] = useState("");
    useEffect(() => {
        getGroups().then(setGroups);
    }, []);
    async function setAll(id, on) {
        setBusy(id);
        try {
            await setGroupState(id, { on });
            onChanged();
        }
        finally {
            setBusy("");
        }
    }
    if (groups.length === 0)
        return null;
    return (_jsx("div", { style: { display: "flex", flexWrap: "wrap", gap: "0.5rem", alignItems: "center", marginBottom: "1.25rem" }, children: groups.map((g) => (_jsxs("span", { style: { display: "inline-flex", alignItems: "center", gap: "0.4rem", border: "1px solid #333", borderRadius: 6, padding: "0.3rem 0.3rem 0.3rem 0.7rem" }, children: [_jsx("span", { style: { fontSize: "0.85rem", color: "#ccc" }, children: g.name }), _jsx("button", { onClick: () => setAll(g.id, true), disabled: busy === g.id, style: { ...S.buttonGhost, padding: "0.25rem 0.55rem", fontSize: "0.75rem" }, children: "On" }), _jsx("button", { onClick: () => setAll(g.id, false), disabled: busy === g.id, style: { ...S.buttonGhost, padding: "0.25rem 0.55rem", fontSize: "0.75rem" }, children: "Off" })] }, g.id))) }));
}
function LightCard({ light, onLocalUpdate, onChanged, }) {
    const serverBrightness = light.last_state?.brightness ?? 100;
    const [localBrightness, setLocalBrightness] = useState(serverBrightness);
    const [localHex, setLocalHex] = useState("#ffb84d");
    const commitTimer = useRef(undefined);
    const colorTimer = useRef(undefined);
    const isOn = light.last_state?.on ?? false;
    // Sync slider when a server update (refresh or SSE) changes brightness.
    useEffect(() => { setLocalBrightness(serverBrightness); }, [serverBrightness]);
    function handleColorChange(hex) {
        setLocalHex(hex);
        const r = parseInt(hex.slice(1, 3), 16);
        const g = parseInt(hex.slice(3, 5), 16);
        const b = parseInt(hex.slice(5, 7), 16);
        const color = rgbToXy(r, g, b);
        const next = { ...(light.last_state ?? { on: true }), on: true, color };
        onLocalUpdate(light.id, next);
        clearTimeout(colorTimer.current);
        colorTimer.current = setTimeout(() => { setLightState(light.id, next); }, 200);
    }
    async function toggle() {
        const next = { ...(light.last_state ?? { on: false }), on: !isOn };
        onLocalUpdate(light.id, next); // optimistic update
        await setLightState(light.id, next);
        onChanged(); // fallback full refresh (catches Govee etc.)
    }
    function handleBrightnessChange(value) {
        setLocalBrightness(value);
        onLocalUpdate(light.id, { ...(light.last_state ?? { on: true }), on: true, brightness: value });
        clearTimeout(commitTimer.current);
        commitTimer.current = setTimeout(async () => {
            await setLightState(light.id, {
                ...(light.last_state ?? { on: true }),
                on: true,
                brightness: value,
            });
        }, 200);
    }
    return (_jsxs("div", { style: { ...S.card, opacity: isOn ? 1 : 0.6, transition: "opacity 0.2s" }, children: [_jsxs("div", { style: { display: "flex", justifyContent: "space-between", alignItems: "center" }, children: [_jsx("span", { style: {
                            fontWeight: 600,
                            fontSize: "0.95rem",
                            overflow: "hidden",
                            textOverflow: "ellipsis",
                            whiteSpace: "nowrap",
                        }, children: light.name }), _jsx(Toggle, { on: isOn, onToggle: toggle })] }), light.capabilities.dimmable && (_jsx("input", { type: "range", min: 1, max: 100, value: localBrightness, disabled: !isOn, onChange: (e) => handleBrightnessChange(Number(e.target.value)), style: {
                    width: "100%",
                    marginTop: "0.25rem",
                    accentColor: "#f90",
                    cursor: isOn ? "pointer" : "default",
                } })), light.capabilities.color_rgb && (_jsxs("div", { style: { display: "flex", alignItems: "center", gap: "0.5rem", marginTop: "0.4rem" }, children: [_jsx("input", { type: "color", value: localHex, disabled: !isOn, onChange: (e) => handleColorChange(e.target.value), style: {
                            width: 36,
                            height: 24,
                            padding: 0,
                            border: "1px solid #444",
                            borderRadius: 4,
                            background: "none",
                            cursor: isOn ? "pointer" : "default",
                        } }), _jsx("span", { style: { fontSize: "0.75rem", color: "#888" }, children: "Color" })] }))] }));
}
function Toggle({ on, onToggle }) {
    return (_jsx("button", { onClick: onToggle, style: {
            flexShrink: 0,
            width: 44,
            height: 24,
            borderRadius: 12,
            border: "none",
            cursor: "pointer",
            background: on ? "#f90" : "#444",
            position: "relative",
            transition: "background 0.2s",
        }, children: _jsx("span", { style: {
                position: "absolute",
                top: 3,
                left: on ? 23 : 3,
                width: 18,
                height: 18,
                borderRadius: "50%",
                background: "#fff",
                transition: "left 0.2s",
            } }) }));
}
