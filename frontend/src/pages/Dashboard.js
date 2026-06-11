import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useEffect, useRef, useState } from "react";
import { setLightState } from "../api";
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
    return (_jsx("div", { style: { padding: "2rem", maxWidth: 960, margin: "0 auto" }, children: localLights.length === 0 ? (_jsxs("div", { style: { textAlign: "center", padding: "4rem 0", color: "#666" }, children: [_jsx("p", { style: { margin: "0 0 0.75rem" }, children: "No lights found." }), _jsxs("p", { style: { margin: 0, fontSize: "0.875rem" }, children: ["Add a provider in", " ", _jsx("button", { onClick: () => onNavigate("settings"), style: {
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
            }, children: localLights.map((light) => (_jsx(LightCard, { light: light, onLocalUpdate: handleLocalUpdate, onChanged: onRefresh }, light.id))) })) }));
}
function LightCard({ light, onLocalUpdate, onChanged, }) {
    const serverBrightness = light.last_state?.brightness ?? 100;
    const [localBrightness, setLocalBrightness] = useState(serverBrightness);
    const commitTimer = useRef(undefined);
    const isOn = light.last_state?.on ?? false;
    // Sync slider when a server update (refresh or SSE) changes brightness.
    useEffect(() => { setLocalBrightness(serverBrightness); }, [serverBrightness]);
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
                } }))] }));
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
