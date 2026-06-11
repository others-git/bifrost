// In-app replacements for window.alert/confirm/prompt: a styled Modal shell
// plus a useDialogs() hook exposing the same flows as promises.

import { useEffect, useState, type CSSProperties, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { S } from "../styles";

const overlay: CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(0,0,0,0.55)",
  backdropFilter: "blur(2px)",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  zIndex: 100,
};

export function Modal({
  title,
  onClose,
  children,
  width = 380,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  width?: number;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return createPortal(
    <div
      style={overlay}
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        style={{
          ...S.card,
          width,
          maxWidth: "calc(100vw - 2rem)",
          maxHeight: "calc(100vh - 2rem)",
          overflowY: "auto",
          border: "1px solid #2e2e2e",
          boxShadow: "0 12px 40px rgba(0,0,0,0.6)",
        }}
      >
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <h3 style={{ margin: 0, fontSize: "1rem", color: "#eee" }}>{title}</h3>
          <button
            onClick={onClose}
            aria-label="Close"
            style={{ background: "none", border: "none", color: "#777", cursor: "pointer", fontSize: "1.2rem", lineHeight: 1, padding: "0 0.2rem" }}
          >
            ×
          </button>
        </div>
        {children}
      </div>
    </div>,
    document.body,
  );
}

type DialogRequest =
  | { kind: "alert"; title: string; message?: ReactNode; resolve: () => void }
  | {
      kind: "confirm";
      title: string;
      message?: ReactNode;
      confirmLabel?: string;
      danger?: boolean;
      resolve: (ok: boolean) => void;
    }
  | {
      kind: "prompt";
      title: string;
      message?: ReactNode;
      placeholder?: string;
      initial?: string;
      confirmLabel?: string;
      resolve: (value: string | null) => void;
    };

export interface Dialogs {
  alert: (opts: { title: string; message?: ReactNode }) => Promise<void>;
  confirm: (opts: {
    title: string;
    message?: ReactNode;
    confirmLabel?: string;
    danger?: boolean;
  }) => Promise<boolean>;
  /** Resolves to the entered text, or null when cancelled. */
  prompt: (opts: {
    title: string;
    message?: ReactNode;
    placeholder?: string;
    initial?: string;
    confirmLabel?: string;
  }) => Promise<string | null>;
  /** Render this once near the component root. */
  element: ReactNode;
}

export function useDialogs(): Dialogs {
  const [req, setReq] = useState<DialogRequest | null>(null);

  return {
    alert: (opts) =>
      new Promise<void>((resolve) => setReq({ kind: "alert", ...opts, resolve })),
    confirm: (opts) =>
      new Promise<boolean>((resolve) => setReq({ kind: "confirm", ...opts, resolve })),
    prompt: (opts) =>
      new Promise<string | null>((resolve) => setReq({ kind: "prompt", ...opts, resolve })),
    element: req ? <DialogHost req={req} done={() => setReq(null)} /> : null,
  };
}

function DialogHost({ req, done }: { req: DialogRequest; done: () => void }) {
  const [value, setValue] = useState(req.kind === "prompt" ? (req.initial ?? "") : "");

  function cancel() {
    if (req.kind === "alert") req.resolve();
    else if (req.kind === "confirm") req.resolve(false);
    else req.resolve(null);
    done();
  }

  function submit() {
    if (req.kind === "alert") req.resolve();
    else if (req.kind === "confirm") req.resolve(true);
    else req.resolve(value);
    done();
  }

  const confirmLabel =
    req.kind === "alert" ? "OK" : (req.confirmLabel ?? (req.kind === "confirm" ? "Confirm" : "OK"));
  const danger = req.kind === "confirm" && req.danger;

  return (
    <Modal title={req.title} onClose={cancel}>
      {req.message && (
        <div style={{ color: "#aaa", fontSize: "0.875rem", lineHeight: 1.45 }}>{req.message}</div>
      )}
      {req.kind === "prompt" && (
        <input
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
          placeholder={req.placeholder}
          autoFocus
          style={S.input}
        />
      )}
      <div style={{ display: "flex", gap: "0.5rem", justifyContent: "flex-end", marginTop: "0.25rem" }}>
        {req.kind !== "alert" && (
          <button onClick={cancel} style={S.buttonGhost}>
            Cancel
          </button>
        )}
        <button
          onClick={submit}
          autoFocus={req.kind !== "prompt"}
          disabled={req.kind === "prompt" && !value.trim()}
          style={danger ? { ...S.buttonDanger, background: "#a33", color: "#fff" } : S.button}
        >
          {confirmLabel}
        </button>
      </div>
    </Modal>
  );
}
