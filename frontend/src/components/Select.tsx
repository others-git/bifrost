// The shared themed dropdown — replaces native <select> (whose OS-rendered
// option list ignores the theme and looks jarring) and the several bespoke
// picker re-implementations. A trigger button shows the current option (or a
// placeholder), and the option list is portaled to <body> (so it escapes a
// dimmed/translucent card) anchored below the trigger, flipping up if needed.
// On compact viewports the list is a bottom sheet.

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { useViewport } from "../useViewport";
import { sheetStyle } from "./sheet";
import { color, radius, alpha, glass } from "../theme";

export interface SelectOption<T extends string> {
  value: T;
  label: ReactNode;
  /** Optional group heading; a header row renders when it changes (like optgroup). */
  group?: string;
}

/** The dropdown/menu panel surface — frosted gothic glass with a gold hairline
 * and the sharp frame radius. Shared so `Select` and the anchored pickers look
 * identical (and unmistakably part of the design, not an OS dropdown). */
export const menuSurface: CSSProperties = {
  padding: 5,
  background: glass.background,
  backdropFilter: "blur(16px)",
  WebkitBackdropFilter: "blur(16px)",
  border: `1px solid ${alpha(color.gold, 0.32)}`,
  borderRadius: radius.frame,
  boxShadow: "inset 0 1px 0 rgba(236,230,240,0.05), 0 16px 38px -8px rgba(0,0,0,0.8)",
};

/** One selectable row in a dropdown/menu — active (current) + hover states. The
 * single row used by `Select` and the Devices pickers. */
export function MenuItem({
  active,
  compact,
  onClick,
  children,
}: {
  active?: boolean;
  compact?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  const [hover, setHover] = useState(false);
  return (
    <button
      onClick={onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        display: "block",
        width: "100%",
        textAlign: "left",
        padding: compact ? "0.7rem 0.7rem" : "0.45rem 0.6rem",
        borderRadius: 6,
        border: "none",
        background: active ? alpha(color.cyan, 0.13) : hover ? alpha(color.text, 0.06) : "transparent",
        color: active ? color.cyan : color.text,
        fontSize: compact ? "0.95rem" : "0.84rem",
        cursor: "pointer",
      }}
    >
      {children}
    </button>
  );
}

/** A non-interactive group heading inside a menu (the optgroup label). */
export function MenuHeader({ children }: { children: ReactNode }) {
  return (
    <div
      style={{
        padding: "0.45rem 0.6rem 0.2rem",
        fontSize: "0.68rem",
        letterSpacing: "0.12em",
        textTransform: "uppercase",
        color: color.faint,
      }}
    >
      {children}
    </div>
  );
}

export function Select<T extends string>({
  value,
  options,
  onChange,
  placeholder = "Select…",
  disabled,
  width,
  title,
  empty,
  style,
}: {
  value?: T;
  options: SelectOption<T>[];
  onChange: (value: T) => void;
  placeholder?: ReactNode;
  disabled?: boolean;
  width?: number | string;
  title?: string;
  /** Shown in the open list when there are no options. */
  empty?: ReactNode;
  style?: CSSProperties;
}) {
  const { isCompact } = useViewport();
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; top: number; minWidth: number } | null>(null);

  const current = options.find((o) => o.value === value);

  useLayoutEffect(() => {
    if (!open || isCompact) return;
    const trigger = triggerRef.current;
    const list = listRef.current;
    if (!trigger || !list) return;
    const r = trigger.getBoundingClientRect();
    const w = list.offsetWidth;
    const h = list.offsetHeight;
    let left = Math.max(8, Math.min(r.left, window.innerWidth - w - 8));
    let top = r.bottom + 4;
    if (top + h > window.innerHeight - 8) top = r.top - 4 - h; // flip up
    top = Math.max(8, top);
    setPos({ left, top, minWidth: r.width });
  }, [open, isCompact]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (triggerRef.current?.contains(t) || listRef.current?.contains(t)) return;
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    const t = setTimeout(() => document.addEventListener("pointerdown", onDown), 0);
    window.addEventListener("keydown", onKey);
    return () => {
      clearTimeout(t);
      document.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const list = (
    <div
      ref={listRef}
      style={
        isCompact
          ? { ...sheetStyle, padding: 6, gap: 2 }
          : {
              position: "fixed",
              left: pos?.left ?? -9999,
              top: pos?.top ?? -9999,
              visibility: pos ? "visible" : "hidden",
              zIndex: 70,
              minWidth: pos?.minWidth,
              maxHeight: 300,
              overflowY: "auto",
              ...menuSurface,
            }
      }
    >
      {options.length === 0 ? (
        <div style={{ padding: "0.5rem 0.6rem", color: color.faint, fontSize: "0.82rem" }}>
          {empty ?? "No options"}
        </div>
      ) : (
        options.map((o, i) => (
          <div key={o.value}>
            {o.group && o.group !== options[i - 1]?.group && <MenuHeader>{o.group}</MenuHeader>}
            <MenuItem
              active={o.value === value}
              compact={isCompact}
              onClick={() => {
                onChange(o.value);
                setOpen(false);
              }}
            >
              {o.label}
            </MenuItem>
          </div>
        ))
      )}
    </div>
  );

  return (
    <>
      <button
        ref={triggerRef}
        disabled={disabled}
        title={title}
        onClick={() => !disabled && setOpen((o) => !o)}
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: "0.5rem",
          width,
          padding: "0.4rem 0.6rem",
          borderRadius: radius.sm,
          border: `1px solid ${open ? color.cyan : color.border}`,
          background: color.surfaceOff,
          color: disabled ? color.faint : current ? color.text : color.dim,
          fontSize: "0.82rem",
          cursor: disabled ? "default" : "pointer",
          transition: "border-color 0.15s",
          ...style,
        }}
      >
        <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {current ? current.label : placeholder}
        </span>
        <span
          aria-hidden
          style={{
            color: color.dim,
            fontSize: "0.7rem",
            flexShrink: 0,
            transform: open ? "rotate(180deg)" : "none",
            transition: "transform 0.15s",
          }}
        >
          ▾
        </span>
      </button>
      {open && createPortal(list, document.body)}
    </>
  );
}
