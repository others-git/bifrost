// Typed wrappers for every REST endpoint Bifrost exposes.
export async function getSetupStatus() {
    const res = await fetch("/api/setup/status");
    return res.json();
}
export async function postSetup(password) {
    const res = await fetch("/api/setup", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ password }),
    });
    if (res.ok)
        return { ok: true };
    return { error: (await res.text()) || `HTTP ${res.status}` };
}
export async function login(password) {
    const res = await fetch("/api/auth/login", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ password }),
    });
    return res.ok;
}
export async function logout() {
    await fetch("/api/auth/logout", { method: "POST" });
}
export async function getLights() {
    const res = await fetch("/api/lights");
    if (res.status === 401)
        return "unauthorized";
    if (!res.ok)
        throw new Error(`HTTP ${res.status}`);
    return res.json();
}
export async function setLightState(id, state) {
    await fetch(`/api/lights/${id}`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(state),
    });
}
export async function getProviders() {
    const res = await fetch("/api/providers");
    if (!res.ok)
        return [];
    return res.json();
}
export async function getProviderTypes() {
    const res = await fetch("/api/providers/types");
    if (!res.ok)
        return [];
    return res.json();
}
export async function getProviderStatus(id) {
    const res = await fetch(`/api/providers/${id}/status`);
    if (!res.ok)
        return { state: "unknown" };
    return res.json();
}
export async function addProvider(name, provider_type, credentials) {
    const res = await fetch("/api/providers", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name, provider_type, credentials }),
    });
    if (res.ok)
        return res.json();
    return { error: (await res.text()) || `HTTP ${res.status}` };
}
export async function removeProvider(id) {
    await fetch(`/api/providers/${id}`, { method: "DELETE" });
}
export async function discoverLights(id) {
    const res = await fetch(`/api/providers/${id}/discover`, { method: "POST" });
    if (!res.ok)
        throw new Error(`HTTP ${res.status}`);
    return res.json();
}
