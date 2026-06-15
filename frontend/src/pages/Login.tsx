import { useState } from "react";
import { login } from "../api";
import { S } from "../styles";
import { Button } from "../components/controls";

export function LoginPage({ onSuccess }: { onSuccess: () => void }) {
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
        <h1 className="bifrost-brand" style={{ margin: "0 0 0.5rem", fontSize: "1.6rem", letterSpacing: "0.05em" }}>BIFROST</h1>
        <input
          type="password"
          placeholder="Password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          style={S.input}
          autoFocus
          required
        />
        {error && <p style={{ color: "#f66", margin: 0, fontSize: "0.875rem" }}>{error}</p>}
        <Button type="submit" disabled={loading}>
          {loading ? "Signing in…" : "Sign in"}
        </Button>
      </form>
    </div>
  );
}
