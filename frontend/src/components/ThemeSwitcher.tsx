// Theme switcher — pick a built-in look, or roll a random one. Each chip previews
// a theme in *its own* colours (literal hex from `theme.colors`, not the active
// CSS vars), while the surrounding chrome uses the active theme like everything
// else. "Generate" applies a random cohesive theme immediately; "Keep" persists
// it to localStorage so it survives reloads.

import { useState } from "react";
import {
  activeThemeId,
  allThemes,
  deleteTheme,
  randomTheme,
  saveTheme,
  setActiveTheme,
  themeFromKeyColors,
  THEME_CATEGORIES,
  type Theme,
  type ThemeKeyColors,
} from "../theme";
import { color, font, labelType, radius } from "../theme";
import { S } from "../styles";
import { Button } from "./controls";
import { Modal } from "./dialogs";
import { Glyph } from "./glyphs";

export function ThemeSwitcher() {
  const [list, setList] = useState<Theme[]>(() => allThemes());
  const [active, setActive] = useState<Theme>(() => {
    const id = activeThemeId();
    const all = allThemes();
    return all.find((t) => t.id === id) ?? all[0];
  });

  // A freshly generated theme is "active" but not yet in the saved list.
  const unsaved = !!active.custom && !list.some((t) => t.id === active.id);
  const chips = unsaved ? [...list, active] : list;

  // Group into the named sets (built-ins carry a category); anything without one
  // — generated/saved customs — falls into a trailing "Custom" set.
  const groups = [
    ...THEME_CATEGORIES.map((label) => ({
      label: label as string,
      themes: chips.filter((t) => t.category === label),
    })),
    { label: "Custom", themes: chips.filter((t) => !t.category) },
  ].filter((g) => g.themes.length > 0);

  function pick(t: Theme) {
    setActiveTheme(t);
    setActive(t);
  }
  function generate() {
    const t = randomTheme();
    setActiveTheme(t);
    setActive(t);
  }
  const [customOpen, setCustomOpen] = useState(false);
  function saveCustom(name: string, keys: ThemeKeyColors) {
    const customCount = list.filter((t) => t.custom).length;
    const t = themeFromKeyColors(name.trim() || `Custom ${customCount + 1}`, keys);
    saveTheme(t);
    setActiveTheme(t);
    setActive(t);
    setList(allThemes());
    setCustomOpen(false);
  }
  function keep() {
    const customCount = list.filter((t) => t.custom).length;
    const named: Theme =
      active.name === "Generated" ? { ...active, name: `Custom ${customCount + 1}` } : active;
    saveTheme(named);
    setActive(named);
    setList(allThemes());
  }
  function remove(t: Theme) {
    deleteTheme(t.id);
    const next = allThemes();
    setList(next);
    if (active.id === t.id) pick(next[0]);
  }

  return (
    <div>
      <div style={{ display: "flex", flexDirection: "column", gap: "1.2rem" }}>
        {groups.map((g) => (
          <div key={g.label}>
            <div
              style={{
                ...labelType,
                fontSize: "0.68rem",
                color: color.dim,
                marginBottom: "0.5rem",
              }}
            >
              {g.label}
            </div>
            <div
              style={{
                display: "grid",
                gridTemplateColumns: "repeat(auto-fill, minmax(148px, 1fr))",
                gap: "0.6rem",
              }}
            >
              {g.themes.map((t) => (
                <ThemeChip
                  key={t.id}
                  theme={t}
                  active={t.id === active.id}
                  onPick={() => pick(t)}
                  onDelete={t.custom ? () => remove(t) : undefined}
                />
              ))}
            </div>
          </div>
        ))}
      </div>
      <div style={{ display: "flex", gap: "0.5rem", marginTop: "0.85rem", alignItems: "center" }}>
        <Button variant="ghost" onClick={generate} style={{ display: "inline-flex", alignItems: "center", gap: "0.4rem" }}>
          <Glyph name="dice" size={15} /> Generate
        </Button>
        <Button variant="ghost" onClick={() => setCustomOpen(true)} style={{ display: "inline-flex", alignItems: "center", gap: "0.4rem" }}>
          <Glyph name="brush" size={15} /> Custom
        </Button>
        {unsaved && (
          <Button variant="accent" onClick={keep}>
            Keep this
          </Button>
        )}
        <span style={{ fontSize: "0.78rem", color: color.faint }}>
          {unsaved ? "Unsaved — Keep it to make it stick." : `${active.name} active`}
        </span>
      </div>
      {customOpen && (
        <CustomThemeModal
          startFrom={active}
          onSave={saveCustom}
          onClose={() => setCustomOpen(false)}
        />
      )}
    </div>
  );
}

/** The five pickable slots, each explaining exactly what it colours — the
 * whole UI derives from these (surfaces/text from the canvas hue; bright and
 * tarnished ornament, oxblood, and neon ink from the accents). */
const KEY_COLOR_FIELDS: {
  key: keyof ThemeKeyColors;
  label: string;
  blurb: string;
}[] = [
  {
    key: "base",
    label: "Canvas",
    blurb: "Tints the whole backdrop — page background, panels, and cards take this hue (kept dark automatically); body text is tinted to match.",
  },
  {
    key: "primary",
    label: "Primary accent",
    blurb: "The main neon: lights, selected states, primary glows, and the app's headline accent.",
  },
  {
    key: "secondary",
    label: "Secondary accent",
    blurb: "The second neon voice: media — speakers, TVs, and audio controls.",
  },
  {
    key: "ornament",
    label: "Ornament",
    blurb: "The metalwork: corner filigree, engraved headers, dividers — and power devices (switches, plugs).",
  },
  {
    key: "danger",
    label: "Danger",
    blurb: "Alerts: errors, offline devices, and destructive buttons.",
  },
];

/** Build-your-own appearance from the five colours the theme actually runs on.
 * Starts from the active theme's values so "tweak what I have" is one edit. */
function CustomThemeModal({
  startFrom,
  onSave,
  onClose,
}: {
  startFrom: Theme;
  onSave: (name: string, keys: ThemeKeyColors) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState("");
  const [keys, setKeys] = useState<ThemeKeyColors>(() => ({
    base: startFrom.colors.surface,
    primary: startFrom.colors.cyan,
    secondary: startFrom.colors.violet,
    ornament: startFrom.colors.gold,
    danger: startFrom.colors.rose,
  }));

  return (
    <Modal title="Custom appearance" onClose={onClose} width={440}>
      <p style={{ fontSize: "0.8rem", color: color.faint, marginTop: 0 }}>
        Five colours drive the whole look — everything else (surface shades, text, glows) is
        derived so the result hangs together.
      </p>
      <label style={{ display: "block", marginBottom: "0.8rem" }}>
        <span style={{ fontSize: "0.78rem", color: color.dim, display: "block", marginBottom: 4 }}>
          Name
        </span>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="My appearance"
          style={S.input}
        />
      </label>
      <div style={{ display: "flex", flexDirection: "column", gap: "0.65rem" }}>
        {KEY_COLOR_FIELDS.map((f) => (
          <label
            key={f.key}
            style={{ display: "flex", alignItems: "center", gap: "0.7rem", cursor: "pointer" }}
          >
            <input
              type="color"
              value={keys[f.key]}
              onChange={(e) => setKeys((k) => ({ ...k, [f.key]: e.target.value }))}
              style={{
                width: 46,
                height: 46,
                padding: 2,
                border: `1px solid ${color.hairline}`,
                borderRadius: 8,
                background: "transparent",
                cursor: "pointer",
                flexShrink: 0,
              }}
            />
            <span style={{ minWidth: 0 }}>
              <span style={{ display: "block", fontSize: "0.84rem", color: color.text, fontWeight: 600 }}>
                {f.label}
              </span>
              <span style={{ display: "block", fontSize: "0.74rem", color: color.faint, lineHeight: 1.35 }}>
                {f.blurb}
              </span>
            </span>
          </label>
        ))}
      </div>
      {/* Live strip preview in the chip's own vocabulary. */}
      <div style={{ display: "flex", gap: 4, marginTop: "0.9rem" }}>
        {[keys.base, keys.primary, keys.secondary, keys.ornament, keys.danger].map((x, i) => (
          <span
            key={i}
            style={{
              width: 22,
              height: 22,
              borderRadius: 4,
              background: x,
              border: "1px solid rgba(0,0,0,0.35)",
              boxShadow: i > 0 ? `0 0 8px -3px ${x}` : "none",
            }}
          />
        ))}
      </div>
      <div style={{ display: "flex", justifyContent: "flex-end", gap: "0.5rem", marginTop: "1rem" }}>
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" onClick={() => onSave(name, keys)}>
          Save &amp; apply
        </Button>
      </div>
    </Modal>
  );
}

function ThemeChip({
  theme,
  active,
  onPick,
  onDelete,
}: {
  theme: Theme;
  active: boolean;
  onPick: () => void;
  onDelete?: () => void;
}) {
  const c = theme.colors;
  return (
    <div style={{ position: "relative" }}>
      <button
        onClick={onPick}
        style={{
          width: "100%",
          textAlign: "left",
          cursor: "pointer",
          padding: "0.6rem",
          background: c.void,
          borderRadius: radius.frame,
          border: `1px solid ${active ? c.gold : "rgba(255,255,255,0.12)"}`,
          boxShadow: active ? `inset 0 0 0 1px ${c.gold}, 0 0 18px -7px ${c.cyan}` : "none",
        }}
      >
        <div style={{ display: "flex", gap: 4, marginBottom: "0.5rem" }}>
          {[c.surface, c.cyan, c.violet, c.gold, c.rose].map((x, i) => (
            <span
              key={i}
              style={{
                width: 16,
                height: 16,
                borderRadius: 3,
                background: x,
                border: "1px solid rgba(0,0,0,0.35)",
                boxShadow: i > 0 ? `0 0 8px -3px ${x}` : "none",
              }}
            />
          ))}
        </div>
        <span
          style={{
            fontFamily: font.display,
            fontSize: "0.78rem",
            letterSpacing: "0.05em",
            color: c.text,
            display: "block",
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {theme.name}
        </span>
      </button>
      {onDelete && (
        <button
          onClick={onDelete}
          title="Delete theme"
          aria-label="Delete theme"
          style={{
            position: "absolute",
            top: 3,
            right: 3,
            width: 20,
            height: 20,
            display: "grid",
            placeItems: "center",
            borderRadius: 4,
            border: "none",
            background: "rgba(0,0,0,0.45)",
            color: c.dim,
            cursor: "pointer",
            fontSize: "0.95rem",
            lineHeight: 1,
          }}
        >
          ×
        </button>
      )}
    </div>
  );
}
