import { useState } from "react";
import { postSetup } from "../api";
import { S } from "../styles";
import { Button } from "../components/controls";

export function SetupPage({ onComplete }: { onComplete: () => void }) {
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setError("");
    if (password !== confirm) { setError("Passwords do not match."); return; }
    setLoading(true);
    const result = await postSetup(password);
    setLoading(false);
    if ("error" in result) setError(result.error);
    else onComplete();
  }

  return (
    <div style={S.center}>
      <form onSubmit={submit} style={{ ...S.card, width: 320 }}>
        <h1 className="bifrost-brand" style={{ margin: "0 0 0.25rem", fontSize: "1.6rem", letterSpacing: "0.05em" }}>BIFROST</h1>
        <p style={{ margin: "0 0 0.5rem", color: "#888", fontSize: "0.9rem" }}>
          Create a password to secure your hub.
        </p>
        <input
          type="password"
          placeholder="Password (min 8 characters)"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          style={S.input}
          minLength={8}
          required
          autoFocus
        />
        <input
          type="password"
          placeholder="Confirm password"
          value={confirm}
          onChange={(e) => setConfirm(e.target.value)}
          style={S.input}
          required
        />
        {error && <p style={{ color: "#f66", margin: 0, fontSize: "0.875rem" }}>{error}</p>}
        <Button type="submit" disabled={loading}>
          {loading ? "Setting up…" : "Set password"}
        </Button>
      </form>
    </div>
  );
}
