import { useState } from "react";
import { login } from "../api";
import { S } from "../styles";
import { Button } from "../components/controls";

// "BIFROST" in Elder Futhark — the same runic wordmark the rest of the app uses.
const BRAND_RUNES = "ᛒᛁᚠᚱᛟᛋᛏ";

export function LoginPage({ onSuccess, version }: { onSuccess: () => void; version?: string }) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    const ok = await login(password);
    setLoading(false);
    if (ok) onSuccess();
    else setError("Wrong password.");
  }

  return (
    <div style={S.center}>
      <form onSubmit={submit} style={{ ...S.card, width: 300 }}>
        <h1
          className="bifrost-brand"
          aria-label="Bifrost"
          style={{ margin: "0 0 0.85rem", fontSize: "2.1rem", letterSpacing: "0.12em", textAlign: "center" }}
        >
          {BRAND_RUNES}
        </h1>
        <input
          type="password"
          placeholder="Password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          style={S.input}
          autoFocus
          required
        />
        {error && <p style={{ color: "var(--bf-rose)", margin: 0, fontSize: "0.875rem" }}>{error}</p>}
        <Button type="submit" disabled={loading}>
          {loading ? "Signing in…" : "Sign in"}
        </Button>
        {version && (
          <p style={{ textAlign: "center", color: "var(--bf-faint)", fontSize: "0.72rem", margin: "0.15rem 0 0" }}>
            v{version}
          </p>
        )}
      </form>
    </div>
  );
}
