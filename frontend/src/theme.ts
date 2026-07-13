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
  /** Engraved section-header text (e.g. "HOME SCENES"). Its own themeable knob,
   * defaulting to `gold` — a theme can recolour these labels without moving the
   * ornament gold. Falls back to gold if a theme predates the token. */
  textAccent: "var(--bf-textAccent, var(--bf-gold))",
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
  media: color.violet,
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

/** The recessed "lit niche" surface shared by every device-lit control — the
 * `GlyphButton` and the Boards widget plates. Lights in `accent` when `on`
 * (accent top-light + outer bloom + inner glow), dark/tarnished when off; `active`
 * gives the cyan selection edge. Spread into a `style`; the caller owns size/radius. */
export const nicheStyle = (accent: string, on: boolean, active = false) => ({
  color: on ? accent : color.dim,
  background: on
    ? `radial-gradient(130% 130% at 50% 0%, ${alpha(accent, 0.19)}, transparent 62%), ${color.surface}`
    : color.surfaceOff,
  border: `1px solid ${active ? color.cyan : on ? alpha(accent, 0.4) : color.hairline}`,
  boxShadow: on
    ? `${glow(accent, 22)}, inset 0 0 16px -9px ${accent}`
    : "inset 0 1px 0 rgba(236,230,240,0.04), inset 0 0 18px -13px #000",
  textShadow: on ? `0 0 12px ${alpha(accent, 0.67)}` : undefined,
});

/** Spacing scale (px). */
export const space = { xs: 4, sm: 8, md: 12, lg: 16, xl: 24, xxl: 32 } as const;

/** The minimum comfortable touch target (px) — every interactive element on a
 * compact viewport must reach it, either by actually being this big or by
 * wearing a `hitHalo`. */
export const TOUCH = 44;

/** Invisible touch halo: grows a control's hit box to at least `target`×`target`
 * px without moving anything on screen — transparent padding compensated by
 * negative margin. Apply to the OUTER interactive element (with `background:
 * "none"`/`border: "none"`) and paint the visual chrome on an inner span, so a
 * 24px close ×, a slim switch pill, or a thin slider keeps its designed size
 * while the finger gets the full square. */
export function hitHalo(w: number, h: number, target = TOUCH) {
  const px = Math.max(0, Math.round((target - w) / 2));
  const py = Math.max(0, Math.round((target - h) / 2));
  return { padding: `${py}px ${px}px`, margin: `${-py}px ${-px}px` } as const;
}

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
  media: color.violet,
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
  /** Engraved section-header text colour. Optional — defaults to `gold`. */
  textAccent?: string;
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
  /** Selector grouping (e.g. "Sacred Gothic"). Absent → the "Custom" set. */
  category?: string;
  colors: ThemeColors;
}

/** Order the appearance selector renders its theme sets in. */
export const THEME_CATEGORIES = [
  "Sacred Gothic",
  "Deep Water",
  "Verdant",
  "Industrial",
  "Warm Dusk",
] as const;

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
    // Engraved-label text: its own knob, defaulting to the theme's gold.
    textAccent: c.textAccent ?? c.gold,
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
  // ── Sacred Gothic ──────────────────────────────────────────────────────────
  {
    id: "candlelit",
    name: "Candlelit Cathedral",
    category: "Sacred Gothic",
    colors: {
      void: "#0a0806", panel: "#14110d", surface: "#1c1814", surfaceHi: "#29241c", surfaceOff: "#141009",
      text: "#efe9e2", dim: "#aaa49a", faint: "#726b60",
      cyan: "#38bdf8", violet: "#a78bfa", gold: "#c8a24b", goldBright: "#e6c878", textAccent: "#e8cd8a",
      oxblood: "#6e1f2e", rose: "#d23651", good: "#5fb87a", ink: "#140d04", tarnish: "#8a7647",
    },
  },
  {
    id: "obsidian-vespers",
    name: "Obsidian Vespers",
    category: "Sacred Gothic",
    colors: {
      void: "#08070b", panel: "#100d15", surface: "#16121e", surfaceHi: "#211a2c", surfaceOff: "#0d0a12",
      text: "#ece7f3", dim: "#a39bb2", faint: "#695f76",
      cyan: "#7c9cff", violet: "#b385ff", gold: "#c7b06e", goldBright: "#e6cf95", textAccent: "#ddc98f",
      oxblood: "#4a2b5e", rose: "#f04a6e", good: "#5fb887", ink: "#0a0712", tarnish: "#7c6c52",
    },
  },
  {
    id: "sanguine-choir",
    name: "Sanguine Choir",
    category: "Sacred Gothic",
    colors: {
      void: "#0c0708", panel: "#160d0f", surface: "#1e1316", surfaceHi: "#2b1a1e", surfaceOff: "#130a0c",
      text: "#f1e7e6", dim: "#b59a9c", faint: "#705c5e",
      cyan: "#ef5566", violet: "#c178b0", gold: "#cda152", goldBright: "#ecc77f", textAccent: "#e7b58f",
      oxblood: "#6e1f2e", rose: "#ff6f5b", good: "#6fc488", ink: "#160808", tarnish: "#8a6a4a",
    },
  },
  {
    id: "velvet-eclipse",
    name: "Velvet Eclipse",
    category: "Sacred Gothic",
    colors: {
      void: "#0a0810", panel: "#120f1d", surface: "#181428", surfaceHi: "#241d3a", surfaceOff: "#0f0c1a",
      text: "#ece9f6", dim: "#a39fbb", faint: "#686284",
      cyan: "#6f7bff", violet: "#b07cff", gold: "#e0b964", goldBright: "#f5d68c", textAccent: "#ecd18a",
      oxblood: "#3f2a66", rose: "#f24a7a", good: "#5fc0a0", ink: "#0a0810", tarnish: "#7e6c54",
    },
  },
  {
    id: "cinder-reliquary",
    name: "Cinder Reliquary",
    category: "Sacred Gothic",
    colors: {
      void: "#0a0908", panel: "#141210", surface: "#1b1714", surfaceHi: "#27231e", surfaceOff: "#110e0c",
      text: "#efebe1", dim: "#aaa194", faint: "#6c6457",
      cyan: "#ff7a52", violet: "#b78fa8", gold: "#cbb588", goldBright: "#e6d3a4", textAccent: "#ddcc9e",
      oxblood: "#6a2a22", rose: "#ff6f5b", good: "#9bbf6f", ink: "#140b06", tarnish: "#8a7a55",
    },
  },
  // ── Deep Water ─────────────────────────────────────────────────────────────
  {
    id: "abyssal",
    name: "Abyssal Bloom",
    category: "Deep Water",
    colors: {
      void: "#04100f", panel: "#07191a", surface: "#0b2122", surfaceHi: "#123032", surfaceOff: "#061616",
      text: "#e6f7f4", dim: "#8fb3ad", faint: "#4f6f6b",
      cyan: "#5eead4", violet: "#38bdf8", gold: "#9fe0b0", goldBright: "#c7f0d0", textAccent: "#a9efd0",
      oxblood: "#1e6b5e", rose: "#ff6b6b", good: "#5fe0a0", ink: "#03100e", tarnish: "#4a7a6a",
    },
  },
  {
    id: "tidepool-dusk",
    name: "Tidepool Dusk",
    category: "Deep Water",
    colors: {
      void: "#06100f", panel: "#0a1a1b", surface: "#0f2426", surfaceHi: "#173336", surfaceOff: "#081617",
      text: "#e7f3f2", dim: "#93b2b2", faint: "#556f70",
      cyan: "#45d3da", violet: "#ff9180", gold: "#d7b98a", goldBright: "#efd6a8", textAccent: "#e8d3a0",
      oxblood: "#2a5e66", rose: "#ff6f6f", good: "#5fe0a0", ink: "#06100f", tarnish: "#7e8a6e",
    },
  },
  {
    id: "halcyon-drift",
    name: "Halcyon Drift",
    category: "Deep Water",
    colors: {
      void: "#060f12", panel: "#0a181c", surface: "#0f2127", surfaceHi: "#182f37", surfaceOff: "#081519",
      text: "#e6f1f5", dim: "#93acb6", faint: "#556a74",
      cyan: "#6fc8e0", violet: "#8fb6e8", gold: "#9fc0c0", goldBright: "#c6dede", textAccent: "#bcd8da",
      oxblood: "#2a5560", rose: "#ec8a8a", good: "#7fd0b0", ink: "#061214", tarnish: "#6a8088",
    },
  },
  {
    id: "aurora-mire",
    name: "Aurora Mire",
    category: "Deep Water",
    colors: {
      void: "#060c08", panel: "#0b150e", surface: "#101e14", surfaceHi: "#1a2e20", surfaceOff: "#0a130d",
      text: "#e8f2e8", dim: "#97b29c", faint: "#586e5c",
      cyan: "#4fe0b0", violet: "#9f7cff", gold: "#a8c878", goldBright: "#cde6a0", textAccent: "#bfe0a0",
      oxblood: "#2a5e3e", rose: "#ff6f6f", good: "#5fe07a", ink: "#060c08", tarnish: "#6a8a5a",
    },
  },
  // ── Verdant ────────────────────────────────────────────────────────────────
  {
    id: "witchlight",
    name: "Witchlight",
    category: "Verdant",
    colors: {
      void: "#060a07", panel: "#0c130d", surface: "#121b14", surfaceHi: "#1d291f", surfaceOff: "#0a110c",
      text: "#e8f2e6", dim: "#9ab39a", faint: "#5b6f5b",
      cyan: "#6bff9f", violet: "#b07cff", gold: "#9fd07a", goldBright: "#c6eca0", textAccent: "#b6f0a0",
      oxblood: "#2e5e3a", rose: "#ff5f7f", good: "#5fe07a", ink: "#060c07", tarnish: "#6e8a55",
    },
  },
  {
    id: "verdigris-crypt",
    name: "Verdigris Crypt",
    category: "Verdant",
    colors: {
      void: "#07100e", panel: "#0c1a17", surface: "#112320", surfaceHi: "#1b322d", surfaceOff: "#0a1714",
      text: "#e4f2ee", dim: "#8fb2a8", faint: "#506f68",
      cyan: "#4fd0b8", violet: "#6fb0d0", gold: "#c08a5a", goldBright: "#e0ad7c", textAccent: "#a8e0c0",
      oxblood: "#2a5e52", rose: "#f06a6a", good: "#5fd09a", ink: "#07100e", tarnish: "#7a8a6a",
    },
  },
  {
    id: "mossgrave",
    name: "Mossgrave",
    category: "Verdant",
    colors: {
      void: "#090a07", panel: "#11140d", surface: "#171b12", surfaceHi: "#23291c", surfaceOff: "#0f120b",
      text: "#edf0e2", dim: "#a6ad93", faint: "#686d56",
      cyan: "#8fc24f", violet: "#7fb0a0", gold: "#cabf94", goldBright: "#e6dcae", textAccent: "#cdd6a0",
      oxblood: "#4a5e2a", rose: "#e07a6a", good: "#8fcf5f", ink: "#0a0c06", tarnish: "#8a8a5a",
    },
  },
  // ── Industrial ─────────────────────────────────────────────────────────────
  {
    id: "frostpunk",
    name: "Frostpunk",
    category: "Industrial",
    colors: {
      void: "#080a0f", panel: "#10141c", surface: "#161c26", surfaceHi: "#222b38", surfaceOff: "#0d1119",
      text: "#eaf1fb", dim: "#9aa8bd", faint: "#5a6678",
      cyan: "#7dd3fc", violet: "#a5b4fc", gold: "#b9c4d4", goldBright: "#dde6f0", textAccent: "#c6d4e8",
      oxblood: "#3a4a66", rose: "#f87171", good: "#6ee7b7", ink: "#07101a", tarnish: "#6a7686",
    },
  },
  {
    id: "stormglass",
    name: "Stormglass",
    category: "Industrial",
    colors: {
      void: "#070a0e", panel: "#0e131b", surface: "#141b25", surfaceHi: "#202a38", surfaceOff: "#0c1019",
      text: "#eaf0fa", dim: "#9aa7bd", faint: "#59657a",
      cyan: "#4fa8ff", violet: "#8f9cff", gold: "#aebccf", goldBright: "#d6e2f0", textAccent: "#cfe0ff",
      oxblood: "#2e4060", rose: "#ff5f6f", good: "#5fd0a0", ink: "#060a10", tarnish: "#6a7888",
    },
  },
  {
    id: "plasma-vault",
    name: "Plasma Vault",
    category: "Industrial",
    colors: {
      void: "#08070c", panel: "#100e16", surface: "#16141f", surfaceHi: "#221e2e", surfaceOff: "#0d0b14",
      text: "#efeaf4", dim: "#a89fb0", faint: "#665f70",
      cyan: "#3fe0ff", violet: "#ff5fd0", gold: "#c0a8d0", goldBright: "#e0cdee", textAccent: "#ecb0e6",
      oxblood: "#5a2a5e", rose: "#ff5f8f", good: "#5fe0c0", ink: "#08070c", tarnish: "#7a6c84",
    },
  },
  {
    id: "ironbloom",
    name: "Ironbloom",
    category: "Industrial",
    colors: {
      void: "#0a0807", panel: "#14110e", surface: "#1b1713", surfaceHi: "#28221c", surfaceOff: "#110d0a",
      text: "#efe9e3", dim: "#aaa093", faint: "#6c6355",
      cyan: "#ff4d88", violet: "#7aaecf", gold: "#b8945e", goldBright: "#e0b888", textAccent: "#e0b890",
      oxblood: "#7a2e1e", rose: "#ff6f5b", good: "#9bbf6f", ink: "#140b06", tarnish: "#8a6a48",
    },
  },
  {
    id: "neon-monastery",
    name: "Neon Monastery",
    category: "Industrial",
    colors: {
      void: "#0a0a0c", panel: "#131316", surface: "#1a1a1f", surfaceHi: "#26262d", surfaceOff: "#111114",
      text: "#eceaf0", dim: "#a4a2ab", faint: "#67646f",
      cyan: "#34e0ff", violet: "#ff4df0", gold: "#b8b0c4", goldBright: "#ddd6e8", textAccent: "#d8c0f0",
      oxblood: "#4a2e5e", rose: "#ff5f7f", good: "#5fe0b0", ink: "#0a0a0c", tarnish: "#7a7684",
    },
  },
  {
    id: "voidbloom",
    name: "Voidbloom",
    category: "Industrial",
    colors: {
      void: "#050407", panel: "#0d0a12", surface: "#14101c", surfaceHi: "#1f1a2c", surfaceOff: "#0b0810",
      text: "#efe8f6", dim: "#a89eb6", faint: "#675d78",
      cyan: "#7c5cff", violet: "#ff5fe0", gold: "#c79ad0", goldBright: "#e6c4ec", textAccent: "#e6a0f0",
      oxblood: "#4a2060", rose: "#ff5fa0", good: "#5fe0c0", ink: "#050407", tarnish: "#806c8a",
    },
  },
  // ── Warm Dusk ──────────────────────────────────────────────────────────────
  {
    id: "emberforge",
    name: "Emberforge",
    category: "Warm Dusk",
    colors: {
      void: "#0d0805", panel: "#170d08", surface: "#1f140c", surfaceHi: "#2c1d12", surfaceOff: "#150c06",
      text: "#f3e8dc", dim: "#b39a86", faint: "#6e5b4a",
      cyan: "#ff8a3c", violet: "#6fd3ff", gold: "#d99a52", goldBright: "#f0c074", textAccent: "#efb15f",
      oxblood: "#7a2418", rose: "#ff5a4d", good: "#9bcf5f", ink: "#170a04", tarnish: "#8a5a30",
    },
  },
  {
    id: "copperline-dusk",
    name: "Copperline Dusk",
    category: "Warm Dusk",
    colors: {
      void: "#0b0809", panel: "#150f10", surface: "#1d1416", surfaceHi: "#2a1e20", surfaceOff: "#130c0e",
      text: "#f2e8e6", dim: "#b59c98", faint: "#715c59",
      cyan: "#ff9a5c", violet: "#a07ccf", gold: "#cd8a52", goldBright: "#ecb47a", textAccent: "#ecc090",
      oxblood: "#6e2a2e", rose: "#ff6f5b", good: "#9bbf6f", ink: "#150a07", tarnish: "#8a6448",
    },
  },
  {
    id: "gilded-ash",
    name: "Gilded Ash",
    category: "Warm Dusk",
    colors: {
      void: "#0a0908", panel: "#141210", surface: "#1b1815", surfaceHi: "#27231e", surfaceOff: "#110f0c",
      text: "#efebe3", dim: "#a9a194", faint: "#6b6358",
      cyan: "#f0a85a", violet: "#a896a0", gold: "#b89a5e", goldBright: "#e0c486", textAccent: "#d8c290",
      oxblood: "#6a2e26", rose: "#ef6f5b", good: "#9bbf6f", ink: "#140c07", tarnish: "#8a7850",
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
