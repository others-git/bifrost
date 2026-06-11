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
    return (_jsxs("div", { style: { minHeight: "100vh", background: "#111", color: "#f0f0f0" }, children: [_jsx(Nav, { version: version, page: page, onNavigate: (p) => {
                    if (p === "dashboard" || p === "plan")
                        refreshLights().then(() => setPage(p));
                    else
                        setPage(p);
                }, onLogout: async () => {
                    await logout();
                    setPage("login");
                } }), page === "dashboard" && (_jsx(DashboardPage, { lights: lights, onRefresh: refreshLights, onNavigate: (p) => setPage(p) })), page === "plan" && _jsx(FloorPlanPage, { lights: lights }), page === "settings" && (_jsx(SettingsPage, { onNavigate: (p) => setPage(p) }))] }));
}
function Nav({ version, page, onNavigate, onLogout, }) {
    const activeStyle = { color: "#f90", fontWeight: 700 };
    const tabStyle = {
        background: "none",
        border: "none",
        color: "#999",
        cursor: "pointer",
        fontSize: "0.9rem",
        padding: "0.25rem 0",
    };
    return (_jsxs("nav", { style: {
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "1rem 2rem",
            borderBottom: "1px solid #222",
            background: "#161616",
        }, children: [_jsxs("span", { style: { display: "inline-flex", alignItems: "baseline", gap: "0.5rem" }, children: [_jsx("span", { style: { fontWeight: 800, fontSize: "1.1rem", color: "#f90", letterSpacing: "0.03em" }, children: "Bifrost" }), version && (_jsxs("span", { style: { fontSize: "0.7rem", color: "#666" }, children: ["v", version] }))] }), _jsxs("div", { style: { display: "flex", gap: "1.5rem", alignItems: "center" }, children: [_jsx("button", { onClick: () => onNavigate("dashboard"), style: { ...tabStyle, ...(page === "dashboard" ? activeStyle : {}) }, children: "Lights" }), _jsx("button", { onClick: () => onNavigate("plan"), style: { ...tabStyle, ...(page === "plan" ? activeStyle : {}) }, children: "Plan" }), _jsx("button", { onClick: () => onNavigate("settings"), style: { ...tabStyle, ...(page === "settings" ? activeStyle : {}) }, children: "Settings" }), _jsx("button", { onClick: onLogout, style: { ...tabStyle, color: "#666" }, children: "Sign out" })] })] }));
}
