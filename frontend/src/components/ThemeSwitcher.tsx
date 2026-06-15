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
  type Theme,
} from "../theme";
import { color, font, radius } from "../theme";
import { S } from "../styles";

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

  function pick(t: Theme) {
    setActiveTheme(t);
    setActive(t);
  }
  function generate() {
    const t = randomTheme();
    setActiveTheme(t);
    setActive(t);
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
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fill, minmax(148px, 1fr))",
          gap: "0.6rem",
        }}
      >
        {chips.map((t) => (
          <ThemeChip
            key={t.id}
            theme={t}
            active={t.id === active.id}
            onPick={() => pick(t)}
            onDelete={t.custom ? () => remove(t) : undefined}
          />
        ))}
      </div>
      <div style={{ display: "flex", gap: "0.5rem", marginTop: "0.85rem", alignItems: "center" }}>
        <button onClick={generate} style={S.buttonGhost}>
          🎲 Generate
        </button>
        {unsaved && (
          <button onClick={keep} style={S.buttonAccent}>
            Keep this
          </button>
        )}
        <span style={{ fontSize: "0.78rem", color: color.faint }}>
          {unsaved ? "Unsaved — Keep it to make it stick." : `${active.name} active`}
        </span>
      </div>
    </div>
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
