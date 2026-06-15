// The shared page header — every top-level page announces itself the same way:
// an engraved (Cinzel) uppercase title, optional inline status + right-aligned
// actions, an optional description, and a gilded hairline rule beneath. This is
// the pattern that carries the design identity onto every screen, not just the
// hero — see the design-consistency pass.

import type { ReactNode } from "react";
import { color, font, gildedRule } from "../theme";

export function PageHeader({
  title,
  status,
  description,
  actions,
}: {
  title: string;
  /** Small dim text beside the title (e.g. "11 devices"). */
  status?: ReactNode;
  /** A line of dim copy below the title. */
  description?: ReactNode;
  /** Right-aligned controls (e.g. a Refresh button). */
  actions?: ReactNode;
}) {
  return (
    <header style={{ marginBottom: "1.4rem" }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: "0.9rem", flexWrap: "wrap" }}>
        <h1
          style={{
            margin: 0,
            fontFamily: font.display,
            fontSize: "1.15rem",
            letterSpacing: "0.18em",
            textTransform: "uppercase",
            fontWeight: 700,
            color: color.text,
          }}
        >
          {title}
        </h1>
        {status && <span style={{ fontSize: "0.78rem", color: color.dim }}>{status}</span>}
        {actions && <span style={{ marginLeft: "auto", display: "flex", gap: "0.5rem" }}>{actions}</span>}
      </div>
      {description && (
        <p style={{ margin: "0.55rem 0 0", fontSize: "0.85rem", color: color.dim, maxWidth: 640, lineHeight: 1.45 }}>
          {description}
        </p>
      )}
      {/* Gilded rule — gold leading into the neon, the cathedral-meets-HUD seam. */}
      <div aria-hidden style={{ marginTop: "0.7rem", height: 1, background: gildedRule }} />
    </header>
  );
}

/** The engraved section label — uppercase Cinzel for in-page section headers
 * (LIGHTS / FAVORITES / API KEYS …). Use in place of a bare `<h2>`/`<h3>`. */
export function SectionLabel({ children, style }: { children: ReactNode; style?: React.CSSProperties }) {
  return (
    <h2
      style={{
        margin: 0,
        fontFamily: font.display,
        fontSize: "0.82rem",
        letterSpacing: "0.16em",
        textTransform: "uppercase",
        fontWeight: 600,
        color: color.dim,
        ...style,
      }}
    >
      {children}
    </h2>
  );
}
