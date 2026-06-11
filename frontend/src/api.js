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
export async function getScenes() {
    const res = await fetch("/api/scenes");
    if (!res.ok)
        return [];
    return res.json();
}
export async function createScene(name) {
    const res = await fetch("/api/scenes", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name }),
    });
    if (!res.ok)
        throw new Error(`HTTP ${res.status}`);
    return res.json();
}
export async function activateScene(id) {
    const res = await fetch(`/api/scenes/${id}/activate`, { method: "POST" });
    if (!res.ok)
        throw new Error(`HTTP ${res.status}`);
    return res.json();
}
export async function removeScene(id) {
    await fetch(`/api/scenes/${id}`, { method: "DELETE" });
}
export async function getGroups() {
    const res = await fetch("/api/groups");
    if (!res.ok)
        return [];
    return res.json();
}
export async function createGroup(name, light_ids) {
    const res = await fetch("/api/groups", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name, light_ids }),
    });
    if (!res.ok)
        throw new Error(`HTTP ${res.status}`);
    return res.json();
}
export async function removeGroup(id) {
    await fetch(`/api/groups/${id}`, { method: "DELETE" });
}
export async function setGroupMembers(id, light_ids) {
    await fetch(`/api/groups/${id}/lights`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ light_ids }),
    });
}
export async function setGroupState(id, state) {
    const res = await fetch(`/api/groups/${id}/state`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(state),
    });
    if (!res.ok)
        throw new Error(`HTTP ${res.status}`);
    return res.json();
}
/** One Hue link-button pairing attempt. 409 means the button wasn't pressed yet. */
export async function pairHueBridge(bridgeIp) {
    const res = await fetch("/api/providers/hue/pair", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ bridge_ip: bridgeIp }),
    });
    return res.json();
}
/**
 * Convert sRGB to CIE xy + Y brightness using the Hue Wide RGB D65 matrix —
 * the same math as the server's `Color::from_rgb`.
 */
export function rgbToXy(r, g, b) {
    const lin = (c) => {
        const v = c / 255;
        return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
    };
    const rl = lin(r), gl = lin(g), bl = lin(b);
    const X = rl * 0.664511 + gl * 0.154324 + bl * 0.162028;
    const Y = rl * 0.283881 + gl * 0.668433 + bl * 0.047685;
    const Z = rl * 0.000088 + gl * 0.07231 + bl * 0.986039;
    const sum = X + Y + Z;
    if (sum === 0)
        return { x: 0, y: 0, brightness: 0 };
    return { x: X / sum, y: Y / sum, brightness: Y };
}
export async function discoverLights(id) {
    const res = await fetch(`/api/providers/${id}/discover`, { method: "POST" });
    if (!res.ok)
        throw new Error(`HTTP ${res.status}`);
    return res.json();
}
