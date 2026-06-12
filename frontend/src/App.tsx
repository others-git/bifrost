import { useEffect, useState } from "react";
import { getHealth, getLights, logout, getSetupStatus, type Light } from "./api";
import { SetupPage } from "./pages/Setup";
import { LoginPage } from "./pages/Login";
import { DashboardPage } from "./pages/Dashboard";
import { ScenesPage } from "./pages/Scenes";
import { FloorPlanPage } from "./pages/FloorPlan";
import { SettingsPage } from "./pages/Settings";
import { S } from "./styles";

type Page = "loading" | "setup" | "login" | "dashboard" | "scenes" | "plan" | "settings";

export function App() {
  const [page, setPage] = useState<Page>("loading");
  const [lights, setLights] = useState<Light[]>([]);
  const [version, setVersion] = useState("");

  useEffect(() => {
    getHealth().then((h) => setVersion(h.version));
  }, []);

  async function init() {
    const status = await getSetupStatus();
    if (!status.setup_complete) { setPage("setup"); return; }
    const result = await getLights();
    if (result === "unauthorized") { setPage("login"); return; }
    setLights(result);
    setPage("dashboard");
  }

  useEffect(() => { init(); }, []);

  async function refreshLights() {
    const result = await getLights();
    if (result === "unauthorized") { setPage("login"); return; }
    setLights(result);
  }

  if (page === "loading") {
    return (
      <div style={{ ...S.center, color: "#555", fontSize: "0.9rem" }}>Loading…</div>
    );
  }
  if (page === "setup") return <SetupPage onComplete={() => setPage("login")} />;
  if (page === "login") return <LoginPage onSuccess={() => init()} />;

  return (
    <div style={{ display: "flex", minHeight: "100vh", background: "#111", color: "#f0f0f0" }}>
      <NavTray
        version={version}
        page={page}
        onNavigate={(p) => {
          if (p === "dashboard" || p === "scenes" || p === "plan")
            refreshLights().then(() => setPage(p));
          else setPage(p);
        }}
        onLogout={async () => {
          await logout();
          setPage("login");
        }}
      />
      <main style={{ flex: 1, minWidth: 0 }}>
        {page === "dashboard" && (
          <DashboardPage
            lights={lights}
            onRefresh={refreshLights}
            onNavigate={(p) => setPage(p)}
          />
        )}
        {page === "scenes" && <ScenesPage lights={lights} />}
        {page === "plan" && <FloorPlanPage lights={lights} />}
        {page === "settings" && (
          <SettingsPage onNavigate={(p) => setPage(p)} />
        )}
      </main>
    </div>
  );
}

const NAV_COLLAPSED_KEY = "bifrost.nav.collapsed";

function NavTray({
  version,
  page,
  onNavigate,
  onLogout,
}: {
  version: string;
  page: Page;
  onNavigate: (p: "dashboard" | "scenes" | "plan" | "settings") => void;
  onLogout: () => void;
}) {
  const [collapsed, setCollapsed] = useState(
    () => localStorage.getItem(NAV_COLLAPSED_KEY) === "1",
  );

  function toggle() {
    setCollapsed((c) => {
      localStorage.setItem(NAV_COLLAPSED_KEY, c ? "0" : "1");
      return !c;
    });
  }

  const items: { id: "dashboard" | "scenes" | "plan" | "settings"; glyph: string; label: string }[] = [
    { id: "dashboard", glyph: "◉", label: "Lights" },
    { id: "scenes", glyph: "✦", label: "Scenes" },
    { id: "plan", glyph: "▦", label: "Floor Plan" },
    { id: "settings", glyph: "⚙", label: "Settings" },
  ];

  return (
    <nav
      style={{
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
      }}
    >
      {/* Brand */}
      <div
        style={{
          display: "flex",
          alignItems: "baseline",
          gap: "0.5rem",
          padding: "0.25rem 0.6rem 0.9rem",
          overflow: "hidden",
          whiteSpace: "nowrap",
        }}
      >
        <span style={{ fontWeight: 800, fontSize: "1.05rem", color: "#f90", letterSpacing: "0.03em" }}>
          {collapsed ? "B" : "Bifrost"}
        </span>
        {!collapsed && version && (
          <span style={{ fontSize: "0.7rem", color: "#666" }}>v{version}</span>
        )}
      </div>

      {items.map((item) => {
        const active = page === item.id;
        return (
          <button
            key={item.id}
            onClick={() => onNavigate(item.id)}
            title={collapsed ? item.label : undefined}
            style={{
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
            }}
          >
            <span style={{ fontSize: "1rem", width: 18, textAlign: "center", flexShrink: 0 }}>
              {item.glyph}
            </span>
            {!collapsed && item.label}
          </button>
        );
      })}

      <span style={{ flex: 1 }} />

      <button
        onClick={onLogout}
        title={collapsed ? "Sign out" : undefined}
        style={{
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
        }}
      >
        <span style={{ fontSize: "1rem", width: 18, textAlign: "center", flexShrink: 0 }}>⏻</span>
        {!collapsed && "Sign out"}
      </button>

      <button
        onClick={toggle}
        title={collapsed ? "Expand navigation" : "Collapse navigation"}
        style={{
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
        }}
      >
        {collapsed ? "»" : "«"}
      </button>
    </nav>
  );
}
