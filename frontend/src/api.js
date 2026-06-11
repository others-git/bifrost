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
export async function getPlans() {
    const res = await fetch("/api/plans");
    if (!res.ok)
        return [];
    return res.json();
}
export async function createPlan(name, width, height) {
    const res = await fetch("/api/plans", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name, width, height }),
    });
    if (!res.ok)
        throw new Error((await res.text()) || `HTTP ${res.status}`);
    return res.json();
}
export async function getPlan(id) {
    const res = await fetch(`/api/plans/${id}`);
    if (!res.ok)
        throw new Error(`HTTP ${res.status}`);
    return res.json();
}
export async function removePlan(id) {
    await fetch(`/api/plans/${id}`, { method: "DELETE" });
}
export async function putPlanLayout(id, tiles, walls) {
    const res = await fetch(`/api/plans/${id}/layout`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ tiles, walls }),
    });
    if (!res.ok)
        throw new Error((await res.text()) || `HTTP ${res.status}`);
}
export async function putPlanLights(id, placements) {
    const res = await fetch(`/api/plans/${id}/lights`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ placements }),
    });
    if (!res.ok)
        throw new Error((await res.text()) || `HTTP ${res.status}`);
}
/** Inverse of rgbToXy — CIE xy + Y brightness back to sRGB, for rendering. */
export function xyToRgb(x, y, brightness) {
    if (y <= 0)
        return [0, 0, 0];
    const Y = brightness;
    const X = (Y / y) * x;
    const Z = (Y / y) * (1 - x - y);
    let r = X * 1.656492 - Y * 0.354851 - Z * 0.255038;
    let g = -X * 0.707196 + Y * 1.655397 + Z * 0.036152;
    let b = X * 0.051713 - Y * 0.121364 + Z * 1.01153;
    const gam = (c) => (c <= 0.0031308 ? 12.92 * c : 1.055 * Math.pow(c, 1 / 2.4) - 0.055);
    r = gam(Math.max(0, r));
    g = gam(Math.max(0, g));
    b = gam(Math.max(0, b));
    const m = Math.max(r, g, b, 1); // normalize overshoot instead of clipping hue
    return [Math.round((r / m) * 255), Math.round((g / m) * 255), Math.round((b / m) * 255)];
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
