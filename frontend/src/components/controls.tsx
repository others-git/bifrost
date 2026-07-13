// Shared control primitives — the round, interactive half of the design system
// (frames are angular; controls are round). Themed via tokens, so they re-skin
// with the theme switcher. Replaces the per-page on/off toggles and the bespoke
// Color/White segmented control.

import { useState, type ButtonHTMLAttributes, type CSSProperties, type ReactNode } from "react";
import { color, alpha, glow, hitHalo, radius } from "../theme";
import { Glyph } from "./glyphs";
import { useViewport } from "../useViewport";

// ── Button ───────────────────────────────────────────────────────────────────

export type ButtonVariant = "primary" | "accent" | "ghost" | "danger" | "subtle";

/** The style for a button variant — the single source both `<Button>` and the
 * `S.button*` shims derive from, so inline and component buttons can't drift. */
export function buttonStyle(variant: ButtonVariant = "primary"): CSSProperties {
  const base: CSSProperties = {
    borderRadius: radius.sm,
    cursor: "pointer",
    fontFamily: "inherit",
  };
  switch (variant) {
    case "primary":
      return { ...base, padding: "0.6rem 1.2rem", border: "none", background: color.cyan, color: color.ink, fontWeight: 700, fontSize: "0.95rem", boxShadow: glow(color.cyan, 16) };
    case "accent":
      return { ...base, padding: "0.45rem 0.9rem", border: `1px solid ${color.cyan}`, background: "transparent", color: color.cyan, fontWeight: 600, fontSize: "0.85rem" };
    case "ghost":
      return { ...base, padding: "0.5rem 0.9rem", border: `1px solid ${color.border}`, background: "transparent", color: color.dim, fontSize: "0.875rem" };
    case "danger":
      return { ...base, padding: "0.5rem 0.9rem", border: `1px solid ${alpha(color.rose, 0.53)}`, background: "transparent", color: color.rose, fontSize: "0.875rem" };
    case "subtle":
      return { ...base, padding: "0.35rem 0.6rem", border: "none", background: "transparent", color: color.dim, fontSize: "0.85rem" };
  }
}

/** The shared button. Pick a `variant`; pass any native button prop through.
 * `style` merges last (per-call overrides like width/margin). Disabled dims +
 * blocks the pointer; a subtle hover brighten gives the affordance inline spreads
 * couldn't. */
export function Button({
  variant = "primary",
  style,
  disabled,
  children,
  ...rest
}: { variant?: ButtonVariant } & ButtonHTMLAttributes<HTMLButtonElement>) {
  const [hover, setHover] = useState(false);
  const { isCompact } = useViewport();
  return (
    <button
      {...rest}
      disabled={disabled}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        ...buttonStyle(variant),
        // Touch-sized on compact; `subtle` is an inline row action, so it gets
        // a lighter bump rather than inflating dense rows.
        minHeight: isCompact ? (variant === "subtle" ? 36 : 44) : undefined,
        opacity: disabled ? 0.5 : 1,
        cursor: disabled ? "default" : "pointer",
        filter: hover && !disabled ? "brightness(1.12)" : undefined,
        transition: "filter 0.15s, opacity 0.15s",
        ...style,
      }}
    >
      {children}
    </button>
  );
}

const KNOB = 18;
const PAD = 3;
const LONG = 44;
const SHORT = 24;

/** The glassy electric on/off switch — a sliding knob in a neon pill. Horizontal
 * by default; `vertical` slides up=on (the room toggle). Themed by `accent`. */
export function Switch({
  on,
  onChange,
  disabled,
  accent = color.cyan,
  vertical = false,
}: {
  on: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  accent?: string;
  vertical?: boolean;
}) {
  const w = vertical ? SHORT : LONG;
  const h = vertical ? LONG : SHORT;
  const onEnd = (vertical ? h : w) - KNOB - PAD; // knob offset at the "on" end
  // Horizontal: on slides right. Vertical: on slides up (smaller `top`).
  const knobPos = vertical
    ? { left: PAD, top: on ? PAD : onEnd }
    : { top: PAD, left: on ? onEnd : PAD };

  return (
    // The slim pill is the visual; the button around it is an invisible ≥44px
    // touch halo so the switch is tappable without fattening the design.
    <button
      onClick={(e) => {
        e.stopPropagation();
        if (!disabled) onChange(!on);
      }}
      disabled={disabled}
      role="switch"
      aria-checked={on}
      aria-label={on ? "Turn off" : "Turn on"}
      style={{
        ...hitHalo(w, h),
        flexShrink: 0,
        background: "none",
        border: "none",
        display: "grid",
        placeItems: "center",
        cursor: disabled ? "default" : "pointer",
        opacity: disabled ? 0.5 : 1,
      }}
    >
      <span
        aria-hidden
        style={{
          position: "relative",
          display: "block",
          width: w,
          height: h,
          borderRadius: 999,
          border: `1px solid ${on ? alpha(accent, 0.55) : color.border}`,
          background: on
            ? `linear-gradient(${vertical ? 180 : 90}deg, ${alpha(accent, 0.45)}, ${alpha(accent, 0.12)} 70%), ${color.surfaceOff}`
            : alpha(color.text, 0.05),
          boxShadow: on
            ? `${glow(accent, 16)}, inset 0 1px 0 ${alpha(color.text, 0.22)}`
            : `inset 0 1px 0 ${alpha(color.text, 0.06)}`,
          backdropFilter: "blur(4px)",
          WebkitBackdropFilter: "blur(4px)",
          transition: "background 0.2s, box-shadow 0.2s, border-color 0.2s",
          boxSizing: "border-box",
        }}
      >
        <span
          style={{
            position: "absolute",
            ...knobPos,
            width: KNOB,
            height: KNOB,
            borderRadius: "50%",
            background: on ? color.text : alpha(color.text, 0.4),
            boxShadow: on
              ? `0 0 8px ${alpha(accent, 0.9)}, 0 1px 2px rgba(0,0,0,0.45)`
              : "0 1px 2px rgba(0,0,0,0.35)",
            transition: "top 0.2s, left 0.2s, background 0.2s, box-shadow 0.2s",
          }}
        />
      </span>
    </button>
  );
}

/** A segmented (tabbed) control — one active option, filled; the rest ghosted.
 * Each option may carry its own `activeBg` (a gradient) for expressive tabs like
 * Color/White; otherwise the active fill is the accent. Touch-sized on compact. */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  compact,
  accent = color.cyan,
  variant = "fill",
}: {
  value: T;
  options: { value: T; label: ReactNode; activeBg?: string }[];
  onChange: (v: T) => void;
  compact?: boolean;
  accent?: string;
  /** `fill` = solid accent active pill (default); `outline` = a glassy accent-tinted
   * pill with a hairline outline (subtler, for page-level tabs). */
  variant?: "fill" | "outline";
}) {
  return (
    <div
      style={{
        display: "flex",
        gap: 4,
        padding: 3,
        borderRadius: 12,
        background: alpha(color.text, 0.06),
        border: `1px solid ${alpha(color.text, 0.08)}`,
      }}
    >
      {options.map((o) => {
        const active = o.value === value;
        return (
          <button
            key={o.value}
            onClick={() => onChange(o.value)}
            aria-pressed={active}
            style={{
              flex: 1,
              padding: compact ? "0.55rem 0.4rem" : "0.3rem 0.4rem",
              minHeight: compact ? 44 : undefined,
              fontSize: compact ? "0.9rem" : "0.78rem",
              fontWeight: 600,
              letterSpacing: "0.02em",
              whiteSpace: "nowrap",
              borderRadius: 9,
              border: "none",
              cursor: "pointer",
              transition: "color 0.15s, background 0.15s, box-shadow 0.15s",
              ...(active
                ? variant === "outline"
                  ? {
                      color: color.text,
                      background: alpha(accent, 0.12),
                      boxShadow: `inset 0 0 0 1px ${alpha(accent, 0.5)}, 0 1px 8px ${alpha(accent, 0.14)}`,
                    }
                  : { color: color.ink, background: o.activeBg ?? accent }
                : { color: color.dim, background: "transparent" }),
            }}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

// ── Power toggle ─────────────────────────────────────────────────────────────

/** The shared **power button** — a round, glyph-based on/off control lit in its
 * domain accent when on (cyan = light, violet = audio, gold = power), dim and
 * outlined when off. Lives in a fly-out header (`FlyoutHeader` actions) so every
 * device type powers on/off the same way and in the same place. */
export function PowerToggle({
  on,
  onToggle,
  accent = color.cyan,
  disabled = false,
  size = 34,
}: {
  on: boolean;
  onToggle: () => void;
  accent?: string;
  disabled?: boolean;
  size?: number;
}) {
  // Default sized a touch larger than the close button — it's the primary
  // control. The painted ring keeps its designed size; the button around it is
  // an invisible ≥44px touch halo.
  return (
    <button
      onClick={() => !disabled && onToggle()}
      disabled={disabled}
      aria-label={on ? "Turn off" : "Turn on"}
      aria-pressed={on}
      title="Power"
      style={{
        ...hitHalo(size, size),
        display: "grid",
        placeItems: "center",
        flexShrink: 0,
        background: "none",
        border: "none",
        cursor: disabled ? "not-allowed" : "pointer",
        opacity: disabled ? 0.5 : 1,
      }}
    >
      <span
        style={{
          display: "grid",
          placeItems: "center",
          width: size,
          height: size,
          borderRadius: "50%",
          background: on ? alpha(accent, 0.16) : "transparent",
          border: `1px solid ${on ? accent : color.hairline}`,
          color: on ? accent : color.faint,
          boxShadow: on ? glow(accent, 10) : "none",
          boxSizing: "border-box",
        }}
      >
        <Glyph name="power" size={Math.round(size * 0.54)} />
      </span>
    </button>
  );
}
