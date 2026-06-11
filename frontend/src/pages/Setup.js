import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useState } from "react";
import { postSetup } from "../api";
import { S } from "../styles";
export function SetupPage({ onComplete }) {
    const [password, setPassword] = useState("");
    const [confirm, setConfirm] = useState("");
    const [error, setError] = useState("");
    const [loading, setLoading] = useState(false);
    async function submit(e) {
        e.preventDefault();
        setError("");
        if (password !== confirm) {
            setError("Passwords do not match.");
            return;
        }
        setLoading(true);
        const result = await postSetup(password);
        setLoading(false);
        if ("error" in result)
            setError(result.error);
        else
            onComplete();
    }
    return (_jsx("div", { style: S.center, children: _jsxs("form", { onSubmit: submit, style: { ...S.card, width: 320 }, children: [_jsx("h1", { style: { margin: "0 0 0.25rem", fontSize: "1.6rem", color: "#f90" }, children: "Bifrost" }), _jsx("p", { style: { margin: "0 0 0.5rem", color: "#888", fontSize: "0.9rem" }, children: "Create a password to secure your hub." }), _jsx("input", { type: "password", placeholder: "Password (min 8 characters)", value: password, onChange: (e) => setPassword(e.target.value), style: S.input, minLength: 8, required: true, autoFocus: true }), _jsx("input", { type: "password", placeholder: "Confirm password", value: confirm, onChange: (e) => setConfirm(e.target.value), style: S.input, required: true }), error && _jsx("p", { style: { color: "#f66", margin: 0, fontSize: "0.875rem" }, children: error }), _jsx("button", { type: "submit", style: S.button, disabled: loading, children: loading ? "Setting up…" : "Set password" })] }) }));
}
