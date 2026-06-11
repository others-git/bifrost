import { useEffect, useState } from "react";
import { getLights, logout, getSetupStatus, type Light } from "./api";
import { SetupPage } from "./pages/Setup";
import { LoginPage } from "./pages/Login";
import { DashboardPage } from "./pages/Dashboard";
import { SettingsPage } from "./pages/Settings";
import { S } from "./styles";

type Page = "loading" | "setup" | "login" | "dashboard" | "settings";

export function App() {
  const [page, setPage] = useState<Page>("loading");
  const [lights, setLights] = useState<Light[]>([]);

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
    <div style={{ minHeight: "100vh", background: "#111", color: "#f0f0f0" }}>
      <Nav
        page={page}
        onNavigate={(p) => {
          if (p === "dashboard") refreshLights().then(() => setPage("dashboard"));
          else setPage(p);
        }}
        onLogout={async () => {
          await logout();
          setPage("login");
        }}
      />
      {page === "dashboard" && (
        <DashboardPage
          lights={lights}
          onRefresh={refreshLights}
          onNavigate={(p) => setPage(p)}
        />
      )}
      {page === "settings" && (
        <SettingsPage onNavigate={(p) => setPage(p)} />
      )}
    </div>
  );
}

function Nav({
  page,
  onNavigate,
  onLogout,
}: {
  page: Page;
  onNavigate: (p: "dashboard" | "settings") => void;
  onLogout: () => void;
}) {
  const activeStyle: React.CSSProperties = { color: "#f90", fontWeight: 700 };
  const tabStyle: React.CSSProperties = {
    background: "none",
    border: "none",
    color: "#999",
    cursor: "pointer",
    fontSize: "0.9rem",
    padding: "0.25rem 0",
  };

  return (
    <nav
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        padding: "1rem 2rem",
        borderBottom: "1px solid #222",
        background: "#161616",
      }}
    >
      <span style={{ fontWeight: 800, fontSize: "1.1rem", color: "#f90", letterSpacing: "0.03em" }}>
        Bifrost
      </span>
      <div style={{ display: "flex", gap: "1.5rem", alignItems: "center" }}>
        <button
          onClick={() => onNavigate("dashboard")}
          style={{ ...tabStyle, ...(page === "dashboard" ? activeStyle : {}) }}
        >
          Lights
        </button>
        <button
          onClick={() => onNavigate("settings")}
          style={{ ...tabStyle, ...(page === "settings" ? activeStyle : {}) }}
        >
          Settings
        </button>
        <button onClick={onLogout} style={{ ...tabStyle, color: "#666" }}>
          Sign out
        </button>
      </div>
    </nav>
  );
}
