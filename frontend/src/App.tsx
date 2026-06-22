import { useEffect, useState, type MouseEvent } from "react";
import { getHealth, getLights, logout, getSetupStatus, kioskLogin, type Light } from "./api";
import { Glyph } from "./components/glyphs";
import { SetupPage } from "./pages/Setup";
import { LoginPage } from "./pages/Login";
import { DashboardPage } from "./pages/Dashboard";
import { BoardsPage } from "./pages/Boards";
import { MediaPage } from "./pages/Media";
import { DevicesPage } from "./pages/Devices";
import { ScenesPage } from "./pages/Scenes";
import { RoomsPage } from "./pages/Rooms";
import { FloorPlanPage } from "./pages/FloorPlan";
import { SettingsPage, type AddPrefill } from "./pages/Settings";
import { S } from "./styles";
import { color, font, navAurora as NAV_AURORA, alpha } from "./theme";
import { useViewport } from "./useViewport";
import { useAutoReloadOnNewBuild } from "./useAutoReload";
import { VoiceFeedback } from "./components/VoiceFeedback";
import { PushToTalk } from "./components/PushToTalk";

/** Pages reachable from the nav tray. */
type NavPage = "dashboard" | "boards" | "media" | "devices" | "scenes" | "rooms" | "plan" | "settings";
type Page = "loading" | "setup" | "login" | NavPage;

/** True when served inside the Bifrost kiosk WebView (it appends
 * `BifrostKiosk/<version>` to its User-Agent). A wall fixture is paired by QR
 * and deauthed remotely by the controller, so we hide the Sign-out button there
 * — otherwise any passerby could tap it and knock the tablet offline. */
const IS_KIOSK = /\bBifrostKiosk\//.test(navigator.userAgent);

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
  { id: "dashboard", glyph: "◉", label: "Control" },
  { id: "boards", glyph: "▦", label: "Boards" },
  { id: "media", glyph: "♪", label: "Media" },
  { id: "devices", glyph: "▤", label: "Devices" },
  { id: "scenes", glyph: "✦", label: "Scenes" },
  { id: "rooms", glyph: "⌂", label: "Rooms" },
  { id: "plan", glyph: "▦", label: "Floor Plan" },
  { id: "settings", glyph: "⚙", label: "Settings" },
];

export function App() {
  const [page, setPage] = useState<Page>("loading");
  // A device picked on the Devices "Detected" tab, handed to Settings to pre-fill
  // the Add Provider form.
  const [pendingAdd, setPendingAdd] = useState<AddPrefill | null>(null);
  const [lights, setLights] = useState<Light[]>([]);
  const [version, setVersion] = useState("");
  const { isMobile, isCompact } = useViewport();

  // Kiosk self-update: reload when a new frontend build is deployed.
  useAutoReloadOnNewBuild();

  useEffect(() => {
    getHealth().then((h) => setVersion(h.version));
  }, []);

  // A paired kiosk trades its `bfr_key` cookie for a session instead of showing
  // login — both on first load and when a session later expires. Returns the
  // lights result after the (single) re-auth attempt.
  async function lightsWithKioskAuth() {
    let result = await getLights();
    if (result === "unauthorized" && IS_KIOSK && (await kioskLogin())) {
      result = await getLights();
    }
    return result;
  }

  async function init() {
    const status = await getSetupStatus();
    if (!status.setup_complete) { setPage("setup"); return; }
    const result = await lightsWithKioskAuth();
    if (result === "unauthorized") { setPage("login"); return; }
    setLights(result);
    setPage("dashboard");
  }

  useEffect(() => { init(); }, []);

  async function refreshLights() {
    const result = await lightsWithKioskAuth();
    if (result === "unauthorized") { setPage("login"); return; }
    setLights(result);
  }

  if (page === "loading") {
    return (
      <div style={{ ...S.center, color: color.faint, fontSize: "0.9rem" }}>Loading…</div>
    );
  }
  if (page === "setup") return <SetupPage onComplete={() => setPage("login")} />;
  if (page === "login") return <LoginPage onSuccess={() => init()} version={version} />;

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
        flexDirection: isCompact ? "column" : "row",
        minHeight: "100vh",
        background: "transparent",
        color: color.text,
      }}
    >
      {isCompact ? (
        <MobileTopBar version={version} page={page} onLogout={onLogout} />
      ) : (
        <NavTray version={version} page={page} onNavigate={navigate} onLogout={onLogout} />
      )}

      <main
        style={{
          flex: 1,
          minWidth: 0,
          // Flex column so a page can fill the viewport height via `flex: 1`
          // and bottom-anchor content (e.g. the Restore Home seal) reliably —
          // percentage heights don't resolve through a flex-grown item.
          display: "flex",
          flexDirection: "column",
          // Clear the fixed bottom tab bar (plus the device's home-bar inset).
          paddingBottom: isCompact ? "calc(58px + env(safe-area-inset-bottom))" : 0,
        }}
      >
        {page === "dashboard" && (
          <DashboardPage lights={lights} onRefresh={refreshLights} onNavigate={(p) => setPage(p)} />
        )}
        {page === "boards" && <BoardsPage />}
        {page === "media" && <MediaPage />}
        {page === "devices" && (
          <DevicesPage
            onAddDetected={(p) => {
              setPendingAdd(p);
              setPage("settings");
            }}
          />
        )}
        {page === "scenes" && <ScenesPage />}
        {page === "rooms" && <RoomsPage />}
        {page === "plan" &&
          (isMobile ? (
            <div style={{ padding: "3rem 1.2rem", textAlign: "center", color: color.dim }}>
              The Floor Plan is available on a larger screen.
            </div>
          ) : (
            <FloorPlanPage lights={lights} />
          ))}
        {page === "settings" && (
          <SettingsPage
            onNavigate={(p) => setPage(p)}
            initialAdd={pendingAdd}
            onConsumeAdd={() => setPendingAdd(null)}
          />
        )}
      </main>

      {isCompact && <BottomNav page={page} onNavigate={navigate} showPlan={!isMobile} />}

      {/* Wake-word feedback overlay — driven by the kiosk app via
          window.bifrostVoice. Non-blocking; present on every signed-in page. */}
      <VoiceFeedback />

      {/* Push-to-talk mic. On browser/phone it captures via getUserMedia → upload;
          on the kiosk it drives the *native* voice pipeline through the
          `window.bifrostKioskPtt` bridge (the WebView's getUserMedia can't run over
          the plain-HTTP LAN origin). */}
      <PushToTalk />
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
        justifyContent: "flex-end",
        gap: "0.6rem",
        padding: "calc(0.5rem + env(safe-area-inset-top)) 0.9rem 0.5rem",
        borderBottom: `1px solid ${color.hairline}`,
        backgroundImage: NAV_AURORA,
        position: "sticky",
        top: 0,
        zIndex: 30,
      }}
    >
      {/* Brand + current page, centered in the bar; controls stay flush right.
          On compact the in-page PageHeader is hidden, so this is where the page
          name lives (alongside the active bottom tab). */}
      <div
        style={{
          position: "absolute",
          left: "50%",
          transform: "translateX(-50%)",
          display: "flex",
          alignItems: "center",
          gap: "0.6rem",
        }}
      >
        <Brand fontSize="1.05rem" />
        {title && <span style={{ fontSize: "0.85rem", fontFamily: font.display, letterSpacing: "0.08em", color: color.dim }}>{title}</span>}
      </div>
      {version && <span style={{ fontSize: "0.65rem", color: color.faint }}>v{version}</span>}
      {/* No Sign-out on the kiosk: deauth is the controller's job, not a tap. */}
      {!IS_KIOSK && (
        <button
          onClick={onLogout}
          aria-label="Sign out"
          style={{ display: "grid", placeItems: "center", background: "none", border: "none", color: color.faint, cursor: "pointer", padding: "0.2rem 0.3rem" }}
        >
          <Glyph name="logout" size={18} />
        </button>
      )}
    </header>
  );
}

/** Bottom tab bar for phones — thumb-reachable, fixed to the viewport bottom. */
function BottomNav({
  page,
  onNavigate,
  showPlan,
}: {
  page: Page;
  onNavigate: (p: NavPage) => void;
  showPlan: boolean;
}) {
  return (
    <nav
      style={{
        position: "fixed",
        left: 0,
        right: 0,
        bottom: 0,
        zIndex: 40,
        display: "flex",
        borderTop: `1px solid ${color.hairline}`,
        background: "rgba(13,8,16,0.86)",
        backdropFilter: "blur(12px)",
        WebkitBackdropFilter: "blur(12px)",
        paddingBottom: "env(safe-area-inset-bottom)",
      }}
    >
      {/* Floor Plan needs room to draw — shown on tablet fixtures, hidden on phones. */}
      {NAV_ITEMS.filter((item) => item.id !== "plan" || showPlan).map((item) => {
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
              color: active ? color.cyan : color.faint,
              textShadow: active ? `0 0 12px ${alpha(color.cyan, 0.53)}` : undefined,
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
        borderRight: `1px solid ${color.hairline}`,
        // Deep aurora tints (aubergine / oxblood / violet) drifting diagonally —
        // the .bifrost-aurora class pans this oversized gradient slowly.
        backgroundImage: NAV_AURORA,
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
          <span style={{ fontSize: "0.7rem", color: color.faint }}>v{version}</span>
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
              background: active ? "rgba(56,189,248,0.10)" : "none",
              border: "none",
              borderRadius: 8,
              boxShadow: active ? `inset 2px 0 0 0 ${color.cyan}` : "none",
              color: active ? color.cyan : color.dim,
              letterSpacing: "0.02em",
              fontWeight: active ? 700 : 500,
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

      {/* No Sign-out on the kiosk: deauth is the controller's job, not a tap. */}
      {!IS_KIOSK && (
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
            color: color.faint,
            fontSize: "0.9rem",
            cursor: "pointer",
            whiteSpace: "nowrap",
            overflow: "hidden",
          }}
        >
          <span style={{ width: 18, display: "grid", placeItems: "center", flexShrink: 0 }}><Glyph name="logout" size={18} /></span>
          {!collapsed && "Sign out"}
        </button>
      )}

      <button
        onClick={toggle}
        title={collapsed ? "Expand navigation" : "Collapse navigation"}
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          padding: "0.45rem 0",
          background: "none",
          border: `1px solid ${color.border}`,
          borderRadius: 8,
          color: color.faint,
          fontSize: "0.85rem",
          cursor: "pointer",
        }}
      >
        {collapsed ? "»" : "«"}
      </button>
    </nav>
  );
}
