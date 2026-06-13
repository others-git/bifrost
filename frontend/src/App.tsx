import { useEffect, useState, type MouseEvent } from "react";
import { getHealth, getLights, logout, getSetupStatus, type Light } from "./api";
import { SetupPage } from "./pages/Setup";
import { LoginPage } from "./pages/Login";
import { DashboardPage } from "./pages/Dashboard";
import { AudioPage } from "./pages/Audio";
import { ScenesPage } from "./pages/Scenes";
import { RoomsPage } from "./pages/Rooms";
import { FloorPlanPage } from "./pages/FloorPlan";
import { SettingsPage } from "./pages/Settings";
import { S } from "./styles";
import { useViewport } from "./useViewport";

/** Pages reachable from the nav tray. */
type NavPage = "dashboard" | "audio" | "scenes" | "rooms" | "plan" | "settings";
type Page = "loading" | "setup" | "login" | NavPage;

/** "BIFROST" in Elder Futhark — ᛒ(B) ᛁ(I) ᚠ(F) ᚱ(R) ᛟ(O) ᛋ(S) ᛏ(T). */
const BRAND_RUNES = "ᛒᛁᚠᚱᛟᛋᛏ";

/** The runic wordmark. `compact` shows just the leading rune (collapsed nav).
 * Gradient comes from the `.bifrost-brand` class; the hover flare follows the
 * cursor via the `--fx`/`--fy`/`--fa` custom properties we set here. */
function Brand({ compact = false, fontSize }: { compact?: boolean; fontSize: string }) {
  // Park the flare under the pointer so only the runes near it brighten.
  const track = (e: MouseEvent<HTMLSpanElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    e.currentTarget.style.setProperty("--fx", `${e.clientX - r.left}px`);
    e.currentTarget.style.setProperty("--fy", `${e.clientY - r.top}px`);
  };
  const flare = (on: boolean) => (e: MouseEvent<HTMLSpanElement>) =>
    e.currentTarget.style.setProperty("--fa", on ? "0.95" : "0");
  return (
    <span
      className="bifrost-brand"
      role="img"
      aria-label="Bifrost"
      title="Bifrost"
      onMouseMove={track}
      onMouseEnter={flare(true)}
      onMouseLeave={flare(false)}
      style={{ fontWeight: 800, fontSize, letterSpacing: "0.14em", cursor: "default" }}
    >
      {compact ? "ᛒ" : BRAND_RUNES}
    </span>
  );
}

const NAV_ITEMS: { id: NavPage; glyph: string; label: string }[] = [
  { id: "dashboard", glyph: "◉", label: "Lights" },
  { id: "audio", glyph: "♪", label: "Audio" },
  { id: "scenes", glyph: "✦", label: "Scenes" },
  { id: "rooms", glyph: "⌂", label: "Rooms" },
  { id: "plan", glyph: "▦", label: "Floor Plan" },
  { id: "settings", glyph: "⚙", label: "Settings" },
];

export function App() {
  const [page, setPage] = useState<Page>("loading");
  const [lights, setLights] = useState<Light[]>([]);
  const [version, setVersion] = useState("");
  const { isMobile } = useViewport();

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

  const navigate = (p: NavPage) => {
    if (p === "dashboard" || p === "scenes" || p === "plan")
      refreshLights().then(() => setPage(p));
    else setPage(p);
  };
  const onLogout = async () => {
    await logout();
    setPage("login");
  };

  return (
    <div
      style={{
        display: "flex",
        flexDirection: isMobile ? "column" : "row",
        minHeight: "100vh",
        background: "#111",
        color: "#f0f0f0",
      }}
    >
      {isMobile ? (
        <MobileTopBar version={version} page={page} onLogout={onLogout} />
      ) : (
        <NavTray version={version} page={page} onNavigate={navigate} onLogout={onLogout} />
      )}

      <main
        style={{
          flex: 1,
          minWidth: 0,
          // Clear the fixed bottom tab bar (plus the phone's home-bar inset).
          paddingBottom: isMobile ? "calc(58px + env(safe-area-inset-bottom))" : 0,
        }}
      >
        {page === "dashboard" && (
          <DashboardPage lights={lights} onRefresh={refreshLights} onNavigate={(p) => setPage(p)} />
        )}
        {page === "audio" && <AudioPage />}
        {page === "scenes" && <ScenesPage />}
        {page === "rooms" && <RoomsPage />}
        {page === "plan" &&
          (isMobile ? (
            <div style={{ padding: "3rem 1.2rem", textAlign: "center", color: "#888" }}>
              The Floor Plan is available on a larger screen.
            </div>
          ) : (
            <FloorPlanPage lights={lights} />
          ))}
        {page === "settings" && <SettingsPage onNavigate={(p) => setPage(p)} />}
      </main>

      {isMobile && <BottomNav page={page} onNavigate={navigate} />}
    </div>
  );
}

/** Slim top bar for phones — brand + sign-out (the bottom tab bar has no room). */
function MobileTopBar({
  version,
  page,
  onLogout,
}: {
  version: string;
  page: Page;
  onLogout: () => void;
}) {
  const title = NAV_ITEMS.find((i) => i.id === page)?.label;
  return (
    <header
      className="bifrost-aurora"
      style={{
        display: "flex",
        alignItems: "center",
        gap: "0.6rem",
        padding: "calc(0.5rem + env(safe-area-inset-top)) 0.9rem 0.5rem",
        borderBottom: "1px solid #1c2430",
        backgroundImage:
          "linear-gradient(135deg, #10131a 0%, #0f1a22 22%, #161326 48%, #0e1f28 74%, #10131a 100%)",
        position: "sticky",
        top: 0,
        zIndex: 30,
      }}
    >
      <Brand fontSize="1.05rem" />
      {title && <span style={{ fontSize: "0.85rem", color: "#8b93a1" }}>{title}</span>}
      <span style={{ flex: 1 }} />
      {version && <span style={{ fontSize: "0.65rem", color: "#5d6878" }}>v{version}</span>}
      <button
        onClick={onLogout}
        aria-label="Sign out"
        style={{ background: "none", border: "none", color: "#6b7585", cursor: "pointer", fontSize: "1.1rem", padding: "0.2rem 0.3rem" }}
      >
        ⏻
      </button>
    </header>
  );
}

/** Bottom tab bar for phones — thumb-reachable, fixed to the viewport bottom. */
function BottomNav({ page, onNavigate }: { page: Page; onNavigate: (p: NavPage) => void }) {
  return (
    <nav
      style={{
        position: "fixed",
        left: 0,
        right: 0,
        bottom: 0,
        zIndex: 40,
        display: "flex",
        borderTop: "1px solid #1c2430",
        background: "#0d1117",
        paddingBottom: "env(safe-area-inset-bottom)",
      }}
    >
      {/* Floor Plan is a desktop tool — hidden from the mobile tab bar for now. */}
      {NAV_ITEMS.filter((item) => item.id !== "plan").map((item) => {
        const active = page === item.id;
        return (
          <button
            key={item.id}
            onClick={() => onNavigate(item.id)}
            aria-label={item.label}
            style={{
              flex: 1,
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              gap: "0.15rem",
              padding: "0.5rem 0.2rem",
              background: "none",
              border: "none",
              color: active ? "#9adcff" : "#7c8595",
              cursor: "pointer",
            }}
          >
            <span style={{ fontSize: "1.25rem", lineHeight: 1 }}>{item.glyph}</span>
            <span style={{ fontSize: "0.62rem", fontWeight: active ? 700 : 400 }}>{item.label}</span>
          </button>
        );
      })}
    </nav>
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
  onNavigate: (p: NavPage) => void;
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

  const items = NAV_ITEMS;

  return (
    <nav
      className="bifrost-aurora"
      style={{
        width: collapsed ? 56 : 200,
        flexShrink: 0,
        display: "flex",
        flexDirection: "column",
        borderRight: "1px solid #1c2430",
        // Deep aurora tints (indigo / teal / violet) drifting diagonally —
        // the .bifrost-aurora class pans this oversized gradient slowly.
        backgroundImage:
          "linear-gradient(135deg, #10131a 0%, #0f1a22 22%, #161326 48%, #0e1f28 74%, #10131a 100%)",
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
        <Brand compact={collapsed} fontSize="1.15rem" />
        {!collapsed && version && (
          <span style={{ fontSize: "0.7rem", color: "#5d6878" }}>v{version}</span>
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
              background: active ? "rgba(125,211,252,0.09)" : "none",
              border: "none",
              borderRadius: 8,
              boxShadow: active ? "inset 2px 0 0 0 rgba(125,211,252,0.75)" : "none",
              color: active ? "#9adcff" : "#8b93a1",
              fontWeight: active ? 700 : 400,
              fontSize: "0.9rem",
              cursor: "pointer",
              whiteSpace: "nowrap",
              overflow: "hidden",
              transition: "background 0.15s, color 0.15s",
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
          color: "#6b7585",
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
          border: "1px solid #232c3a",
          borderRadius: 8,
          color: "#6b7585",
          fontSize: "0.85rem",
          cursor: "pointer",
        }}
      >
        {collapsed ? "»" : "«"}
      </button>
    </nav>
  );
}
