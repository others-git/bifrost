import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useState } from "react";
import { login } from "../api";
import { S } from "../styles";
export function LoginPage({ onSuccess }) {
    const [password, setPassword] = useState("");
    const [error, setError] = useState("");
    const [loading, setLoading] = useState(false);
    async function submit(e) {
        e.preventDefault();
        setError("");
        setLoading(true);
        const ok = await login(password);
        setLoading(false);
        if (ok)
            onSuccess();
        else
            setError("Wrong password.");
    }
    return (_jsx("div", { style: S.center, children: _jsxs("form", { onSubmit: submit, style: { ...S.card, width: 300 }, children: [_jsx("h1", { style: { margin: "0 0 0.5rem", fontSize: "1.6rem", color: "#f90" }, children: "Bifrost" }), _jsx("input", { type: "password", placeholder: "Password", value: password, onChange: (e) => setPassword(e.target.value), style: S.input, autoFocus: true, required: true }), error && _jsx("p", { style: { color: "#f66", margin: 0, fontSize: "0.875rem" }, children: error }), _jsx("button", { type: "submit", style: S.button, disabled: loading, children: loading ? "Signing in…" : "Sign in" })] }) }));
}
