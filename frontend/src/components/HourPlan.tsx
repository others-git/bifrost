// The paintable 24-hour plan timeline — the one hour-plan surface, shared by
// the kiosk display plan (Settings → Clients) and the automation editor's
// timer trigger. Brush chips double as the legend; the strip paints by click
// or drag (pointer-captured); hour numbers sit above, AM · PM flank the strip,
// and the current hour wears an inset ring. The plan string is 24 mode
// characters, one per local hour — the same `hour_modes` encoding both
// backends validate.

import { useEffect, useRef, useState } from "react";
import { alpha, hitHalo, TOUCH } from "../theme";
import { useViewport } from "../useViewport";

export interface HourPlanMode {
  /** The stored plan character ('W' | 'S' | 'A'). */
  mode: string;
  label: string;
  hint: string;
  color: string;
  /** Cell fill opacity — the "off" mode paints dimmer so active hours pop. */
  fill?: number;
}

/** Controlled editor: `onChange` fires per painted cell (keep it in state),
 * `onCommit` on pointer-up with the finished stroke — the save point for
 * callers that persist per stroke (the kiosk plan); callers that save on a
 * form submit (the automation editor) can leave it out. */
export function HourPlanEditor({
  plan,
  modes,
  disabled,
  initialBrush,
  leading,
  trailing,
  onChange,
  onCommit,
}: {
  plan: string;
  modes: HourPlanMode[];
  /** Greys the strip and blocks painting (the feature's master switch is off). */
  disabled?: boolean;
  /** The brush preselected before the first chip tap (defaults to the first mode). */
  initialBrush?: string;
  /** Extra inline controls rendered before / after the brush chips, so a
   * caller's master switch or timeout field shares the legend row. */
  leading?: React.ReactNode;
  trailing?: React.ReactNode;
  onChange: (plan: string) => void;
  onCommit?: (plan: string) => void;
}) {
  const { isCompact } = useViewport();
  const [brush, setBrush] = useState(initialBrush ?? modes[0]?.mode ?? "");
  const barRef = useRef<HTMLDivElement>(null);
  // Painting mutates a ref alongside the parent's state so pointer-up commits
  // the fresh stroke, not a stale render's copy.
  const planRef = useRef(plan);
  const painting = useRef(false);
  useEffect(() => {
    planRef.current = plan;
  }, [plan]);

  function paintAt(clientX: number) {
    const r = barRef.current?.getBoundingClientRect();
    if (!r || r.width === 0) return;
    const i = Math.max(0, Math.min(23, Math.floor(((clientX - r.left) / r.width) * 24)));
    if (planRef.current[i] === brush) return;
    planRef.current = planRef.current.slice(0, i) + brush + planRef.current.slice(i + 1);
    onChange(planRef.current);
  }

  const modeColor = (m: string) => modes.find((x) => x.mode === m)?.color ?? "var(--bf-faint)";
  const modeFill = (m: string) => modes.find((x) => x.mode === m)?.fill ?? 0.5;
  const nowHour = new Date().getHours();

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.45rem" }}>
      <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", flexWrap: "wrap" }}>
        {leading}
        {/* Brush chips double as the legend. The chip keeps its slim designed
            height, so the button wears a hitHalo (44px vertical hit area) with
            the visuals on an inner span. */}
        <span style={{ display: "inline-flex", gap: "0.3rem", opacity: disabled ? 0.55 : 1 }}>
          {modes.map(({ mode, label, hint, color: c }) => (
            <button
              key={mode}
              onClick={() => setBrush(mode)}
              title={hint}
              style={{
                background: "none",
                border: "none",
                cursor: "pointer",
                display: "inline-flex",
                ...hitHalo(TOUCH, 22),
              }}
            >
              <span
                style={{
                  display: "inline-flex",
                  alignItems: "center",
                  gap: "0.3rem",
                  padding: "0.15rem 0.5rem",
                  borderRadius: 999,
                  fontSize: "0.72rem",
                  color: brush === mode ? "var(--bf-text)" : "var(--bf-dim)",
                  background: brush === mode ? alpha(c, 0.22) : "transparent",
                  border: `1px solid ${brush === mode ? c : "var(--bf-hairline, #333)"}`,
                }}
              >
                <span
                  style={{ width: 8, height: 8, borderRadius: 2, background: alpha(c, 0.85), flexShrink: 0 }}
                />
                {label}
              </span>
            </button>
          ))}
        </span>
        {trailing}
      </div>

      {/* The 24-hour timeline — click or drag to paint with the selected mode.
          One contiguous strip (touching square cells, hairline hour seams),
          flanked AM · PM. */}
      <div style={{ opacity: disabled ? 0.45 : 1, display: "flex", alignItems: "flex-end", gap: "0.4rem" }}>
        <span style={{ fontSize: "0.62rem", color: "var(--bf-dim)", letterSpacing: "0.08em", paddingBottom: 8 }}>AM</span>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(24, 1fr)", marginBottom: 2 }}>
            {Array.from({ length: 24 }, (_, i) => (
              <span
                key={i}
                style={{ textAlign: "center", fontSize: "0.6rem", color: i === nowHour ? "var(--bf-text)" : "var(--bf-faint)" }}
              >
                {i}
              </span>
            ))}
          </div>
          <div
            ref={barRef}
            onPointerDown={(e) => {
              if (disabled) return;
              e.preventDefault();
              (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
              painting.current = true;
              paintAt(e.clientX);
            }}
            onPointerMove={(e) => {
              if (painting.current) paintAt(e.clientX);
            }}
            onPointerUp={() => {
              if (!painting.current) return;
              painting.current = false;
              onCommit?.(planRef.current);
            }}
            onPointerCancel={() => {
              painting.current = false;
            }}
            title="Click or drag to paint hours with the selected mode"
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(24, 1fr)",
              border: "1px solid var(--bf-hairline, #333)",
              touchAction: "none",
              cursor: disabled ? "default" : "pointer",
            }}
          >
            {plan.split("").map((m, i) => (
              <span
                key={i}
                style={{
                  // Hour cells can't each be 44px wide (24 across a phone), so
                  // compact gets the height instead — a taller strip is the
                  // touch affordance a drag-paint surface can honestly offer.
                  height: isCompact ? TOUCH : 26,
                  background: alpha(modeColor(m), modeFill(m)),
                  // Hairline hour seams instead of per-cell chrome; the current
                  // hour wears an INSET ring so touching neighbours don't overlap.
                  borderRight: i < 23 ? "1px solid rgba(0,0,0,0.35)" : "none",
                  boxShadow: i === nowHour ? "inset 0 0 0 1px var(--bf-text)" : undefined,
                }}
              />
            ))}
          </div>
        </div>
        <span style={{ fontSize: "0.62rem", color: "var(--bf-dim)", letterSpacing: "0.08em", paddingBottom: 8 }}>PM</span>
      </div>
    </div>
  );
}
