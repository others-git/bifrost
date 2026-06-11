// Typed wrappers for every REST endpoint Bifrost exposes.

export interface LightState {
  on: boolean;
  brightness?: number;
  color?: { x: number; y: number; brightness: number };
  color_temp_mirek?: number;
}

/** Partial state carried by live events — absent fields are unchanged. */
export interface LightStatePatch {
  on?: boolean;
  brightness?: number;
  color?: { x: number; y: number; brightness: number };
  color_temp_mirek?: number;
}

/** Merge a live-event patch into an existing state without losing fields. */
export function mergePatch(base: LightState | undefined, patch: LightStatePatch): LightState {
  return {
    on: patch.on ?? base?.on ?? false,
    brightness: patch.brightness ?? base?.brightness,
    color: patch.color ?? base?.color,
    color_temp_mirek: patch.color_temp_mirek ?? base?.color_temp_mirek,
  };
}

export interface LightCapabilities {
  dimmable: boolean;
  color_rgb: boolean;
  color_temperature: boolean;
}

export interface Light {
  id: string;
  name: string;
  provider_id: string;
  device_id: string;
  capabilities: LightCapabilities;
  last_state?: LightState;
}

export interface Provider {
  id: string;
  provider_type: string;
  name: string;
  enabled: boolean;
  created_at: string;
}

export interface CredentialField {
  name: string;
  label: string;
  kind: "text" | "password" | "ipaddress" | "url";
  required: boolean;
  hint?: string;
}

export interface ProviderType {
  provider_type: string;
  schema: CredentialField[];
}

export interface ConnectionStatus {
  state: string;
  since_secs?: number;
  last_event_secs?: number;
  reason?: string;
}

export async function getHealth(): Promise<{ ok: boolean; version: string; uptime_secs: number }> {
  const res = await fetch("/api/health");
  if (!res.ok) return { ok: false, version: "", uptime_secs: 0 };
  return res.json();
}

export async function getSetupStatus(): Promise<{ setup_complete: boolean }> {
  const res = await fetch("/api/setup/status");
  return res.json();
}

export async function postSetup(password: string): Promise<{ ok: true } | { error: string }> {
  const res = await fetch("/api/setup", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ password }),
  });
  if (res.ok) return { ok: true };
  return { error: (await res.text()) || `HTTP ${res.status}` };
}

export async function login(password: string): Promise<boolean> {
  const res = await fetch("/api/auth/login", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ password }),
  });
  return res.ok;
}

export async function logout(): Promise<void> {
  await fetch("/api/auth/logout", { method: "POST" });
}

export async function getLights(): Promise<Light[] | "unauthorized"> {
  const res = await fetch("/api/lights");
  if (res.status === 401) return "unauthorized";
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

export async function setLightState(id: string, state: LightState): Promise<void> {
  await fetch(`/api/lights/${id}`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(state),
  });
}

export async function getProviders(): Promise<Provider[]> {
  const res = await fetch("/api/providers");
  if (!res.ok) return [];
  return res.json();
}

export async function getProviderTypes(): Promise<ProviderType[]> {
  const res = await fetch("/api/providers/types");
  if (!res.ok) return [];
  return res.json();
}

export async function getProviderStatus(id: string): Promise<ConnectionStatus> {
  const res = await fetch(`/api/providers/${id}/status`);
  if (!res.ok) return { state: "unknown" };
  return res.json();
}

export async function addProvider(
  name: string,
  provider_type: string,
  credentials: Record<string, string>,
): Promise<{ id: string } | { error: string }> {
  const res = await fetch("/api/providers", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name, provider_type, credentials }),
  });
  if (res.ok) return res.json();
  return { error: (await res.text()) || `HTTP ${res.status}` };
}

export async function removeProvider(id: string): Promise<void> {
  await fetch(`/api/providers/${id}`, { method: "DELETE" });
}

/** Replace an existing provider's credentials (recovery after key rotation). */
export async function updateProviderCredentials(
  id: string,
  credentials: Record<string, string>,
): Promise<{ ok: true } | { error: string }> {
  const res = await fetch(`/api/providers/${id}/credentials`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ credentials }),
  });
  if (res.ok) return { ok: true };
  return { error: (await res.text()) || `HTTP ${res.status}` };
}

export interface Scene {
  id: string;
  name: string;
  created_at: string;
  lights: number;
}

export async function getScenes(): Promise<Scene[]> {
  const res = await fetch("/api/scenes");
  if (!res.ok) return [];
  return res.json();
}

export async function createScene(name: string): Promise<{ id: string; lights: number }> {
  const res = await fetch("/api/scenes", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name }),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

export async function activateScene(
  id: string,
  lightIds?: string[],
): Promise<{ applied: number; failed: number }> {
  const res = await fetch(`/api/scenes/${id}/activate`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(lightIds ? { light_ids: lightIds } : {}),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

export async function removeScene(id: string): Promise<void> {
  await fetch(`/api/scenes/${id}`, { method: "DELETE" });
}

export interface Group {
  id: string;
  name: string;
  light_ids: string[];
}

export async function getGroups(): Promise<Group[]> {
  const res = await fetch("/api/groups");
  if (!res.ok) return [];
  return res.json();
}

export async function createGroup(name: string, light_ids: string[]): Promise<{ id: string }> {
  const res = await fetch("/api/groups", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name, light_ids }),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

export async function removeGroup(id: string): Promise<void> {
  await fetch(`/api/groups/${id}`, { method: "DELETE" });
}

export async function setGroupMembers(id: string, light_ids: string[]): Promise<void> {
  await fetch(`/api/groups/${id}/lights`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ light_ids }),
  });
}

export async function setGroupState(
  id: string,
  state: LightState,
): Promise<{ applied: number; failed: number }> {
  const res = await fetch(`/api/groups/${id}/state`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(state),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

// ── Floor plans ──────────────────────────────────────────────────────────────

export type WallDir = "h" | "v";
export type Mount = "c" | "n" | "s" | "e" | "w";

export interface Wall {
  x: number;
  y: number;
  dir: WallDir;
}

export interface Placement {
  light_id: string;
  x: number;
  y: number;
  mount: Mount;
}

export interface PlanSummary {
  id: string;
  name: string;
  width: number;
  height: number;
  lights: number;
  created_at: string;
}

export interface PlanRoom {
  id: string;
  name: string;
  /** Auto-managed group mirroring the lights placed in this room (server-assigned). */
  group_id?: string;
  tiles: [number, number][];
}

export interface PlanDetail {
  id: string;
  name: string;
  width: number;
  height: number;
  tiles: [number, number][];
  walls: Wall[];
  lights: Placement[];
  rooms: PlanRoom[];
}

export async function getPlans(): Promise<PlanSummary[]> {
  const res = await fetch("/api/plans");
  if (!res.ok) return [];
  return res.json();
}

export async function createPlan(name: string, width: number, height: number): Promise<{ id: string }> {
  const res = await fetch("/api/plans", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name, width, height }),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
  return res.json();
}

export async function getPlan(id: string): Promise<PlanDetail> {
  const res = await fetch(`/api/plans/${id}`);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

export async function removePlan(id: string): Promise<void> {
  await fetch(`/api/plans/${id}`, { method: "DELETE" });
}

export async function putPlanLayout(id: string, tiles: [number, number][], walls: Wall[]): Promise<void> {
  const res = await fetch(`/api/plans/${id}/layout`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ tiles, walls }),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
}

export async function putPlanLights(id: string, placements: Placement[]): Promise<void> {
  const res = await fetch(`/api/plans/${id}/lights`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ placements }),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
}

export async function putPlanRooms(
  id: string,
  rooms: { id: string; name: string; tiles: [number, number][] }[],
): Promise<void> {
  const res = await fetch(`/api/plans/${id}/rooms`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ rooms }),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
}

/** Inverse of rgbToXy — CIE xy + Y brightness back to sRGB, for rendering. */
export function xyToRgb(x: number, y: number, brightness: number): [number, number, number] {
  if (y <= 0) return [0, 0, 0];
  const Y = brightness;
  const X = (Y / y) * x;
  const Z = (Y / y) * (1 - x - y);
  let r = X * 1.656492 - Y * 0.354851 - Z * 0.255038;
  let g = -X * 0.707196 + Y * 1.655397 + Z * 0.036152;
  let b = X * 0.051713 - Y * 0.121364 + Z * 1.01153;
  const gam = (c: number) => (c <= 0.0031308 ? 12.92 * c : 1.055 * Math.pow(c, 1 / 2.4) - 0.055);
  r = gam(Math.max(0, r));
  g = gam(Math.max(0, g));
  b = gam(Math.max(0, b));
  const m = Math.max(r, g, b, 1); // normalize overshoot instead of clipping hue
  return [Math.round((r / m) * 255), Math.round((g / m) * 255), Math.round((b / m) * 255)];
}

export type HuePairResult =
  | { app_key: string }
  | { error: "link_button_not_pressed" | "bridge_unreachable"; message: string };

/** One Hue link-button pairing attempt. 409 means the button wasn't pressed yet. */
export async function pairHueBridge(bridgeIp: string): Promise<HuePairResult> {
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
export function rgbToXy(r: number, g: number, b: number): { x: number; y: number; brightness: number } {
  const lin = (c: number) => {
    const v = c / 255;
    return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
  };
  const rl = lin(r), gl = lin(g), bl = lin(b);
  const X = rl * 0.664511 + gl * 0.154324 + bl * 0.162028;
  const Y = rl * 0.283881 + gl * 0.668433 + bl * 0.047685;
  const Z = rl * 0.000088 + gl * 0.07231 + bl * 0.986039;
  const sum = X + Y + Z;
  if (sum === 0) return { x: 0, y: 0, brightness: 0 };
  return { x: X / sum, y: Y / sum, brightness: Y };
}

export async function discoverLights(id: string): Promise<{ discovered: number }> {
  const res = await fetch(`/api/providers/${id}/discover`, { method: "POST" });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

/** Import the provider's native rooms/zones as local groups. */
export async function importProviderGroups(id: string): Promise<{ imported: number; found: number }> {
  const res = await fetch(`/api/providers/${id}/import-groups`, { method: "POST" });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}
