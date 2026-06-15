// ─────────────────────────────────────────────────────────────────────────────
// Bifrost design tokens — the single source of truth for the visual language.
//
// Every token resolves to a CSS custom property (`--bf-*`). Switching a theme is
// just rewriting those ~17 base properties on <html> (`applyTheme`) — instant,
// no React re-render, and every existing `color.*` / `T.*` / `glassCard` call
// site re-themes for free because they all hold `var()` references. A theme only
// declares ~17 base colours; the translucent/derived tokens (hairline, glass,
// glows, candlelight, tarnish) are computed from those in `applyTheme`.
//
// Default aesthetic: **candlelit cyber-cathedral** — near-black gothic base, neon
// cyan/violet interaction, antique-gold ornament, frosted glass + soft bloom.
// ─────────────────────────────────────────────────────────────────────────────

const v = (name: string) => `var(--bf-${name})`;

/** Semantic colour tokens — `var()` refs into the active theme. Never hardcode
 * hex in components; import these so the theme switcher can repaint everything. */
export const color = {
  void: v("void"), // app background, deepest
  panel: v("panel"), // raised panel / page section
  surface: v("surface"), // card / control surface
  surfaceHi: v("surfaceHi"), // hovered / active surface
  surfaceOff: v("surfaceOff"), // inert / off surface

  text: v("text"),
  dim: v("dim"), // secondary text
  faint: v("faint"), // tertiary / disabled

  cyan: v("cyan"), // primary accent / lights
  violet: v("violet"), // secondary accent / audio
  gold: v("gold"), // ornament / highlight / power
  goldBright: v("goldBright"),
  oxblood: v("oxblood"), // depth / gothic shadow
  rose: v("rose"), // danger
  good: v("good"), // success / online
  ink: v("ink"), // dark text on a neon fill

  hairline: v("hairline"), // gold-tinted divider — the "filigree" line
  border: v("border"), // neutral control border
} as const;

/** Per-domain accent — keeps lights/audio/power visually distinct (tight core). */
export const domain = {
  light: color.cyan,
  audio: color.violet,
  power: color.gold,
} as const;

/** Soft neon bloom colours (translucent) for box-shadows / drop-shadows. */
export const glowColor = {
  cyan: v("glowCyan"),
  violet: v("glowViolet"),
  gold: v("glowGold"),
} as const;

/** A bloom shadow around an active control, in the given (opaque) accent. */
export const glow = (c: string, spread = 18) => `0 0 ${spread}px -6px ${c}`;

/** A translucent tint of any colour — a literal hex (`#38bdf8`) **or** a `var()`
 * token. Uses CSS `color-mix`, so it replaces the old `${hex}33` alpha-suffix
 * trick, which silently breaks once a token is a `var()` reference. */
export const alpha = (c: string, a: number) =>
  `color-mix(in srgb, ${c} ${Math.round(a * 100)}%, transparent)`;

/** Spacing scale (px). */
export const space = { xs: 4, sm: 8, md: 12, lg: 16, xl: 24, xxl: 32 } as const;

/** Corner radii (px). Two languages on purpose: **framed surfaces are angular**
 * (`frame` ≈ square — cards/sheets/modals, so the gold corner filigree reads as
 * an engraved plate), while **controls stay round** (`sm`/`md` buttons, `pill`
 * toggles). Don't round a framed surface or square-off a pill. */
export const radius = { frame: 3, sm: 8, md: 12, lg: 16, xl: 20, pill: 999 } as const;

/** Frosted-glass panel surface (cyberpunk-HUD). Pair with `backdropFilter`. */
export const glass = {
  background: v("glassBg"),
  backdropFilter: "blur(14px)",
  WebkitBackdropFilter: "blur(14px)",
} as const;

/** Gothic panel gradient — a whisper of top-light over near-black. */
export const panelGradient = `linear-gradient(176deg, ${color.panel} 0%, ${color.surfaceOff} 100%)`;

/** The frosted gothic-glass card — the signature surface: glass with a gold
 * filigree hairline, an inner top-light, a gold inner-glow, and a deep float
 * shadow. Add a domain `glow()` on top for an active edge. */
export const glassCard = {
  background: `linear-gradient(176deg, ${v("glassTop")} 0%, ${v("glassBot")} 100%)`,
  backdropFilter: "blur(16px)",
  WebkitBackdropFilter: "blur(16px)",
  border: `1px solid ${color.hairline}`,
  borderRadius: radius.frame,
  boxShadow: `inset 0 1px 0 ${v("glassSheen")}, inset 0 0 30px -18px ${v("goldInner")}, 0 14px 34px -16px rgba(0,0,0,0.78)`,
} as const;

/** The gilded rule under a `PageHeader` — gold leading into the neon. */
export const gildedRule = `linear-gradient(90deg, ${v("ruleGold")}, ${v("ruleViolet")} 42%, ${v("ruleCyan")} 70%, transparent)`;

/** Panning aurora backdrop for the nav chrome — derived from the surface tones. */
export const navAurora = `linear-gradient(135deg, ${color.panel} 0%, ${color.surface} 50%, ${color.panel} 100%)`;

/** Type families. `display` = engraved-Roman gothic (headings/labels), `body` =
 * Inter (controls/content), `rune` = the Elder-Futhark wordmark fallback chain. */
export const font = {
  display: '"Cinzel", "Inter Variable", Georgia, serif',
  body: '"Inter Variable", system-ui, sans-serif',
  rune: '"Noto Sans Runic", "Segoe UI Historic", "Segoe UI Symbol", "Apple Symbols", "Inter Variable", system-ui, sans-serif',
} as const;

/** The engraved-gothic uppercase label — section headers, device-card titles. */
export const labelType = {
  fontFamily: font.display,
  textTransform: "uppercase" as const,
  letterSpacing: "0.16em",
  fontWeight: 600,
};

/** Accent — kept exported for back-compat (= primary neon). */
export const ACCENT = color.cyan;
export const ACCENT_GLOW = glowColor.cyan;

// ── Migration-compatible theme object ────────────────────────────────────────
// Pages read this shared `T` (the superset of keys the old per-page copies used).
export const T = {
  text: color.text,
  dim: color.dim,
  faint: color.faint,
  accent: color.cyan,
  audio: color.violet,
  power: color.gold,
  panel: panelGradient,
  panelBorder: color.hairline,
  card: color.surface,
  cardOff: color.surfaceOff,
  cardBorder: color.border,
  hairline: color.hairline,
  good: color.good,
  bad: color.rose,
  border: color.border,
  surface: glass.background,
  gold: color.gold,
  oxblood: color.oxblood,
} as const;

// ═══════════════════════════════════════════════════════════════════════════
// Theme system — palettes, derivation, application, persistence, generation.
// ═══════════════════════════════════════════════════════════════════════════

/** The ~17 base colours a theme must declare. Everything else is derived. */
export interface ThemeColors {
  void: string;
  panel: string;
  surface: string;
  surfaceHi: string;
  surfaceOff: string;
  text: string;
  dim: string;
  faint: string;
  cyan: string; // primary accent
  violet: string; // secondary accent
  gold: string; // ornament
  goldBright: string;
  oxblood: string;
  rose: string;
  good: string;
  ink: string; // dark text on a neon fill
  tarnish: string; // dormant-ornament base (the "off" filigree)
}

export interface Theme {
  id: string;
  name: string;
  /** Built-ins ship with the app; `custom` themes live in localStorage. */
  custom?: boolean;
  colors: ThemeColors;
}

// ── colour math (no deps) ────────────────────────────────────────────────────
const clampByte = (n: number) => Math.max(0, Math.min(255, Math.round(n)));
function hexToRgb(hex: string): [number, number, number] {
  const h = hex.replace("#", "");
  const f = h.length === 3 ? h.split("").map((c) => c + c).join("") : h;
  return [parseInt(f.slice(0, 2), 16), parseInt(f.slice(2, 4), 16), parseInt(f.slice(4, 6), 16)];
}
function rgbToHex(r: number, g: number, b: number): string {
  return "#" + [r, g, b].map((x) => clampByte(x).toString(16).padStart(2, "0")).join("");
}
function withAlpha(hex: string, a: number): string {
  const [r, g, b] = hexToRgb(hex);
  return `rgba(${r},${g},${b},${a})`;
}
function mix(a: string, b: string, t: number): string {
  const A = hexToRgb(a);
  const B = hexToRgb(b);
  return rgbToHex(A[0] + (B[0] - A[0]) * t, A[1] + (B[1] - A[1]) * t, A[2] + (B[2] - A[2]) * t);
}
function hslToHex(h: number, s: number, l: number): string {
  const k = (n: number) => (n + h / 30) % 12;
  const a = s * Math.min(l, 1 - l);
  const f = (n: number) => l - a * Math.max(-1, Math.min(k(n) - 3, 9 - k(n), 1));
  return rgbToHex(f(0) * 255, f(8) * 255, f(4) * 255);
}

/** Expand a theme's base colours into the full set of `--bf-*` variables. */
function deriveVars(c: ThemeColors): Record<string, string> {
  return {
    void: c.void,
    panel: c.panel,
    surface: c.surface,
    surfaceHi: c.surfaceHi,
    surfaceOff: c.surfaceOff,
    text: c.text,
    dim: c.dim,
    faint: c.faint,
    cyan: c.cyan,
    violet: c.violet,
    gold: c.gold,
    goldBright: c.goldBright,
    oxblood: c.oxblood,
    rose: c.rose,
    good: c.good,
    ink: c.ink,
    // Derived translucency / blends.
    hairline: withAlpha(c.gold, 0.2),
    border: withAlpha(c.text, 0.1),
    glassBg: withAlpha(c.surface, 0.62),
    glassTop: withAlpha(c.surface, 0.66),
    glassBot: withAlpha(c.surfaceOff, 0.76),
    glassSheen: withAlpha(c.text, 0.06),
    goldInner: withAlpha(c.gold, 0.35),
    glowCyan: withAlpha(c.cyan, 0.55),
    glowViolet: withAlpha(c.violet, 0.5),
    glowGold: withAlpha(c.gold, 0.45),
    candle: mix(c.void, c.gold, 0.16), // warm candlelight glow up top
    tarnish: c.tarnish,
    tarnishHi: mix(c.tarnish, "#e6d4a0", 0.22),
    tarnishLo: mix(c.tarnish, "#000000", 0.28),
    ruleGold: withAlpha(c.gold, 0.55),
    ruleViolet: withAlpha(c.violet, 0.22),
    ruleCyan: withAlpha(c.cyan, 0.12),
  };
}

/** Paint a theme onto <html> by setting its `--bf-*` custom properties. */
export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  for (const [k, val] of Object.entries(deriveVars(theme.colors))) {
    root.style.setProperty(`--bf-${k}`, val);
  }
}

// ── Built-in themes ──────────────────────────────────────────────────────────

export const THEMES: Theme[] = [
  {
    id: "candlelit",
    name: "Candlelit Cathedral",
    colors: {
      void: "#09080b", panel: "#121116", surface: "#19171d", surfaceHi: "#23202a", surfaceOff: "#121016",
      text: "#ece6f0", dim: "#a6a1ab", faint: "#6f6772",
      cyan: "#38bdf8", violet: "#a78bfa", gold: "#c8a24b", goldBright: "#e6c878",
      oxblood: "#6e1f2e", rose: "#f43f5e", good: "#5fb87a", ink: "#04121b", tarnish: "#8a7647",
    },
  },
  {
    id: "abyssal",
    name: "Abyssal Bloom",
    colors: {
      void: "#04100f", panel: "#07191a", surface: "#0b2122", surfaceHi: "#123032", surfaceOff: "#061616",
      text: "#e6f7f4", dim: "#8fb3ad", faint: "#4f6f6b",
      cyan: "#5eead4", violet: "#38bdf8", gold: "#9fe0b0", goldBright: "#c7f0d0",
      oxblood: "#1e6b5e", rose: "#ff6b6b", good: "#5fe0a0", ink: "#03100e", tarnish: "#4a7a6a",
    },
  },
  {
    id: "emberforge",
    name: "Emberforge",
    colors: {
      void: "#0d0805", panel: "#170d08", surface: "#1f140c", surfaceHi: "#2c1d12", surfaceOff: "#150c06",
      text: "#f3e8dc", dim: "#b39a86", faint: "#6e5b4a",
      cyan: "#ff8a3c", violet: "#6fd3ff", gold: "#d99a52", goldBright: "#f0c074",
      oxblood: "#7a2418", rose: "#ff5a4d", good: "#9bcf5f", ink: "#170a04", tarnish: "#8a5a30",
    },
  },
  {
    id: "frostpunk",
    name: "Frostpunk",
    colors: {
      void: "#080a0f", panel: "#10141c", surface: "#161c26", surfaceHi: "#222b38", surfaceOff: "#0d1119",
      text: "#eaf1fb", dim: "#9aa8bd", faint: "#5a6678",
      cyan: "#7dd3fc", violet: "#a5b4fc", gold: "#b9c4d4", goldBright: "#dde6f0",
      oxblood: "#3a4a66", rose: "#f87171", good: "#6ee7b7", ink: "#07101a", tarnish: "#6a7686",
    },
  },
];

// ── Generator — random but cohesive dark themes ──────────────────────────────

/** Generate a random, internally-cohesive dark theme. Picks a base hue for the
 * canvas, two harmonised neon accents, and a metallic ornament; derives the dark
 * surface ramp + light ink from the base hue so the result always hangs together. */
export function randomTheme(): Theme {
  const rnd = (min: number, max: number) => min + Math.random() * (max - min);
  const baseHue = Math.floor(rnd(0, 360));
  const accentHue = Math.floor(rnd(0, 360));
  // Second accent: harmonised (analogous or complementary) to the first.
  const accent2Hue = (accentHue + (Math.random() < 0.5 ? 40 : 180) + rnd(-15, 15)) % 360;
  const ornamentHue = rnd(0, 1) < 0.7 ? rnd(38, 50) : rnd(0, 360); // usually warm gold
  const warm = rnd(0, 1) < 0.5;

  const colors: ThemeColors = {
    void: hslToHex(baseHue, 0.3, 0.045),
    panel: hslToHex(baseHue, 0.24, 0.075),
    surface: hslToHex(baseHue, 0.2, 0.105),
    surfaceHi: hslToHex(baseHue, 0.18, 0.155),
    surfaceOff: hslToHex(baseHue, 0.26, 0.06),
    text: hslToHex(baseHue, 0.12, 0.93),
    dim: hslToHex(baseHue, 0.1, 0.66),
    faint: hslToHex(baseHue, 0.08, 0.42),
    cyan: hslToHex(accentHue, 0.82, 0.62),
    violet: hslToHex(accent2Hue, 0.74, 0.7),
    gold: hslToHex(ornamentHue, 0.5, 0.55),
    goldBright: hslToHex(ornamentHue, 0.55, 0.7),
    oxblood: hslToHex(warm ? 350 : baseHue, 0.5, 0.27),
    rose: hslToHex(350, 0.84, 0.62),
    good: hslToHex(145, 0.5, 0.55),
    ink: hslToHex(accentHue, 0.4, 0.06),
    tarnish: hslToHex(ornamentHue, 0.34, 0.36),
  };
  const id = `gen-${Date.now().toString(36)}`;
  return { id, name: "Generated", custom: true, colors };
}

// ── Persistence ──────────────────────────────────────────────────────────────

const ACTIVE_KEY = "bifrost.theme.active";
const SAVED_KEY = "bifrost.theme.saved";

export function getSavedThemes(): Theme[] {
  try {
    const raw = localStorage.getItem(SAVED_KEY);
    return raw ? (JSON.parse(raw) as Theme[]) : [];
  } catch {
    return [];
  }
}

function writeSaved(list: Theme[]) {
  try {
    localStorage.setItem(SAVED_KEY, JSON.stringify(list));
  } catch {
    /* storage full / disabled — non-fatal */
  }
}

/** Built-ins + user-saved custom themes. */
export function allThemes(): Theme[] {
  return [...THEMES, ...getSavedThemes()];
}

/** Persist a custom (generated/edited) theme so it survives reloads. */
export function saveTheme(theme: Theme) {
  const list = getSavedThemes().filter((t) => t.id !== theme.id);
  writeSaved([...list, { ...theme, custom: true }]);
}

export function deleteTheme(id: string) {
  writeSaved(getSavedThemes().filter((t) => t.id !== id));
}

/** Apply a theme and remember it as the active one. */
export function setActiveTheme(theme: Theme) {
  applyTheme(theme);
  try {
    localStorage.setItem(ACTIVE_KEY, theme.id);
  } catch {
    /* non-fatal */
  }
}

export function activeThemeId(): string {
  try {
    return localStorage.getItem(ACTIVE_KEY) ?? THEMES[0].id;
  } catch {
    return THEMES[0].id;
  }
}

/** Apply the saved active theme (or the default) — call once before first render
 * so the CSS variables exist before any component paints. */
export function initTheme() {
  const id = activeThemeId();
  const theme = allThemes().find((t) => t.id === id) ?? THEMES[0];
  applyTheme(theme);
}
