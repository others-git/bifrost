import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useEffect, useState } from "react";
import { getHealth, getLights, logout, getSetupStatus } from "./api";
import { SetupPage } from "./pages/Setup";
import { LoginPage } from "./pages/Login";
import { DashboardPage } from "./pages/Dashboard";
import { FloorPlanPage } from "./pages/FloorPlan";
import { SettingsPage } from "./pages/Settings";
import { S } from "./styles";
export function App() {
    const [page, setPage] = useState("loading");
    const [lights, setLights] = useState([]);
    const [version, setVersion] = useState("");
    useEffect(() => {
        getHealth().then((h) => setVersion(h.version));
    }, []);
    async function init() {
        const status = await getSetupStatus();
        if (!status.setup_complete) {
            setPage("setup");
            return;
        }
        const result = await getLights();
        if (result === "unauthorized") {
            setPage("login");
            return;
        }
        setLights(result);
        setPage("dashboard");
    }
    useEffect(() => { init(); }, []);
    async function refreshLights() {
        const result = await getLights();
        if (result === "unauthorized") {
            setPage("login");
            return;
        }
        setLights(result);
    }
    if (page === "loading") {
        return (_jsx("div", { style: { ...S.center, color: "#555", fontSize: "0.9rem" }, children: "Loading\u2026" }));
    }
    if (page === "setup")
        return _jsx(SetupPage, { onComplete: () => setPage("login") });
    if (page === "login")
        return _jsx(LoginPage, { onSuccess: () => init() });
    return (_jsxs("div", { style: { display: "flex", minHeight: "100vh", background: "#111", color: "#f0f0f0" }, children: [_jsx(NavTray, { version: version, page: page, onNavigate: (p) => {
                    if (p === "dashboard" || p === "plan")
                        refreshLights().then(() => setPage(p));
                    else
                        setPage(p);
                }, onLogout: async () => {
                    await logout();
                    setPage("login");
                } }), _jsxs("main", { style: { flex: 1, minWidth: 0 }, children: [page === "dashboard" && (_jsx(DashboardPage, { lights: lights, onRefresh: refreshLights, onNavigate: (p) => setPage(p) })), page === "plan" && _jsx(FloorPlanPage, { lights: lights }), page === "settings" && (_jsx(SettingsPage, { onNavigate: (p) => setPage(p) }))] })] }));
}
const NAV_COLLAPSED_KEY = "bifrost.nav.collapsed";
function NavTray({ version, page, onNavigate, onLogout, }) {
    const [collapsed, setCollapsed] = useState(() => localStorage.getItem(NAV_COLLAPSED_KEY) === "1");
    function toggle() {
        setCollapsed((c) => {
            localStorage.setItem(NAV_COLLAPSED_KEY, c ? "0" : "1");
            return !c;
        });
    }
    const items = [
        { id: "dashboard", glyph: "◉", label: "Lights" },
        { id: "plan", glyph: "▦", label: "Plan" },
        { id: "settings", glyph: "⚙", label: "Settings" },
    ];
    return (_jsxs("nav", { style: {
            width: collapsed ? 56 : 200,
            flexShrink: 0,
            display: "flex",
            flexDirection: "column",
            borderRight: "1px solid #222",
            background: "#161616",
            padding: "0.75rem 0.5rem",
            gap: "0.25rem",
            transition: "width 0.15s ease",
            position: "sticky",
            top: 0,
            height: "100vh",
            boxSizing: "border-box",
        }, children: [_jsxs("div", { style: {
                    display: "flex",
                    alignItems: "baseline",
                    gap: "0.5rem",
                    padding: "0.25rem 0.6rem 0.9rem",
                    overflow: "hidden",
                    whiteSpace: "nowrap",
                }, children: [_jsx("span", { style: { fontWeight: 800, fontSize: "1.05rem", color: "#f90", letterSpacing: "0.03em" }, children: collapsed ? "B" : "Bifrost" }), !collapsed && version && (_jsxs("span", { style: { fontSize: "0.7rem", color: "#666" }, children: ["v", version] }))] }), items.map((item) => {
                const active = page === item.id;
                return (_jsxs("button", { onClick: () => onNavigate(item.id), title: collapsed ? item.label : undefined, style: {
                        display: "flex",
                        alignItems: "center",
                        gap: "0.65rem",
                        padding: collapsed ? "0.55rem 0" : "0.55rem 0.6rem",
                        justifyContent: collapsed ? "center" : "flex-start",
                        background: active ? "#222" : "none",
                        border: "none",
                        borderRadius: 8,
                        color: active ? "#f90" : "#999",
                        fontWeight: active ? 700 : 400,
                        fontSize: "0.9rem",
                        cursor: "pointer",
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                    }, children: [_jsx("span", { style: { fontSize: "1rem", width: 18, textAlign: "center", flexShrink: 0 }, children: item.glyph }), !collapsed && item.label] }, item.id));
            }), _jsx("span", { style: { flex: 1 } }), _jsxs("button", { onClick: onLogout, title: collapsed ? "Sign out" : undefined, style: {
                    display: "flex",
                    alignItems: "center",
                    gap: "0.65rem",
                    padding: collapsed ? "0.55rem 0" : "0.55rem 0.6rem",
                    justifyContent: collapsed ? "center" : "flex-start",
                    background: "none",
                    border: "none",
                    borderRadius: 8,
                    color: "#666",
                    fontSize: "0.9rem",
                    cursor: "pointer",
                    whiteSpace: "nowrap",
                    overflow: "hidden",
                }, children: [_jsx("span", { style: { fontSize: "1rem", width: 18, textAlign: "center", flexShrink: 0 }, children: "\u23FB" }), !collapsed && "Sign out"] }), _jsx("button", { onClick: toggle, title: collapsed ? "Expand navigation" : "Collapse navigation", style: {
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                    padding: "0.45rem 0",
                    background: "none",
                    border: "1px solid #2a2a2a",
                    borderRadius: 8,
                    color: "#777",
                    fontSize: "0.85rem",
                    cursor: "pointer",
                }, children: collapsed ? "»" : "«" })] }));
}
