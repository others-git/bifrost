// Typed wrappers for every REST endpoint Bifrost exposes.

export interface LightState {
  on: boolean;
  brightness?: number;
  color?: { x: number; y: number; brightness: number };
  color_temp_mirek?: number;
  /** Provider-reported reachability; undefined when the provider doesn't say. */
  reachable?: boolean;
}

/** Partial state carried by live events — absent fields are unchanged. */
export interface LightStatePatch {
  on?: boolean;
  brightness?: number;
  color?: { x: number; y: number; brightness: number };
  color_temp_mirek?: number;
  reachable?: boolean;
}

/** Merge a live-event patch into an existing state without losing fields. */
export function mergePatch(base: LightState | undefined, patch: LightStatePatch): LightState {
  return {
    on: patch.on ?? base?.on ?? false,
    brightness: patch.brightness ?? base?.brightness,
    color: patch.color ?? base?.color,
    color_temp_mirek: patch.color_temp_mirek ?? base?.color_temp_mirek,
    reachable: patch.reachable ?? base?.reachable,
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
  kind: "light" | "audio";
  /** Whether the UI should offer a "Scan network" button for this type. */
  supports_discovery: boolean;
  schema: CredentialField[];
}

/** A device found by a provider's network scan. */
export interface DiscoveredDevice {
  host: string;
  label?: string | null;
  /** Credential fields pre-shaped for the add-provider form. */
  credentials: Record<string, unknown>;
}

/** Scan the LAN for devices of a provider type that supports auto-detect. */
export async function scanForDevices(providerType: string): Promise<DiscoveredDevice[]> {
  const res = await fetch(`/api/providers/scan/${providerType}`, { method: "POST" });
  if (!res.ok) return [];
  return res.json();
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

// ── Audio devices ────────────────────────────────────────────────────────────

export interface NowPlaying {
  title?: string;
  artist?: string;
  album?: string;
  play_state?: "playing" | "paused" | "stopped";
}

export interface AudioState {
  power: boolean;
  volume: number;
  mute: boolean;
  source?: string;
  now_playing?: NowPlaying;
  reachable?: boolean;
}

export interface AudioCapabilities {
  sources: boolean;
  transport: boolean;
  now_playing: boolean;
}

export interface AudioDevice {
  id: string;
  provider_id: string;
  /** Provider-native id (e.g. "main") — matches audio_state push events. */
  device_id: string;
  name: string;
  kind: "receiver" | "speaker" | "zone";
  capabilities: AudioCapabilities;
  state: AudioState;
  last_seen?: string;
}

/** Sparse command — only the fields present are applied. */
export interface AudioCommand {
  power?: boolean;
  volume?: number;
  mute?: boolean;
  source?: string;
  transport?: "play" | "pause" | "stop" | "next" | "previous" | "toggle";
}

export async function getAudioDevices(): Promise<AudioDevice[]> {
  const res = await fetch("/api/audio/devices");
  if (!res.ok) return [];
  return res.json();
}

/** Live read — round-trips to the device and refreshes the cache. */
export async function getAudioDevice(id: string): Promise<AudioDevice | null> {
  const res = await fetch(`/api/audio/devices/${id}`);
  if (!res.ok) return null;
  return res.json();
}

export async function setAudioState(id: string, cmd: AudioCommand): Promise<string | null> {
  const res = await fetch(`/api/audio/devices/${id}/state`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(cmd),
  });
  if (res.ok) return null;
  return (await res.text()) || `HTTP ${res.status}`;
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

// ── Rooms ────────────────────────────────────────────────────────────────────
// A Room aggregates links to provider-group mirrors plus direct lights;
// effective membership is the union.

export interface RoomLink {
  provider_group_id: string;
  name: string;
  provider_id: string;
}

export interface Room {
  id: string;
  name: string;
  /** Effective members: linked provider-group lights ∪ direct lights. */
  light_ids: string[];
  direct_light_ids: string[];
  links: RoomLink[];
  /** Linked audio device (volume/mute on the room's controls), if any. */
  audio_device_id?: string | null;
}

/** Link (or, with null, unlink) an audio device to a room. */
export async function setRoomAudio(roomId: string, audioDeviceId: string | null): Promise<void> {
  await fetch(`/api/rooms/${roomId}/audio`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ audio_device_id: audioDeviceId }),
  });
}

export interface ProviderGroupInfo {
  id: string;
  provider_id: string;
  provider_group_id: string;
  name: string;
  light_ids: string[];
}

export async function getRooms(): Promise<Room[]> {
  const res = await fetch("/api/rooms");
  if (!res.ok) return [];
  return res.json();
}

export async function getProviderGroups(): Promise<ProviderGroupInfo[]> {
  const res = await fetch("/api/provider-groups");
  if (!res.ok) return [];
  return res.json();
}

export async function createRoom(name: string, light_ids: string[]): Promise<{ id: string }> {
  const res = await fetch("/api/rooms", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name, light_ids }),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
  return res.json();
}

/** Merge `sourceRoomId` into `targetRoomId` (links, lights, scenes, plan regions move; source is deleted). */
export async function mergeRooms(targetRoomId: string, sourceRoomId: string): Promise<void> {
  const res = await fetch(`/api/rooms/${targetRoomId}/merge`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ source_room_id: sourceRoomId }),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
}

export async function removeRoom(id: string): Promise<void> {
  await fetch(`/api/rooms/${id}`, { method: "DELETE" });
}

/** Replace the room's DIRECT lights (linked members are unaffected). */
export async function setRoomDirectLights(id: string, light_ids: string[]): Promise<void> {
  await fetch(`/api/rooms/${id}/lights`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ light_ids }),
  });
}

export async function setRoomLinks(id: string, provider_group_ids: string[]): Promise<void> {
  await fetch(`/api/rooms/${id}/links`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ provider_group_ids }),
  });
}

export async function setRoomState(
  id: string,
  state: LightState,
): Promise<{ applied: number; failed: number }> {
  const res = await fetch(`/api/rooms/${id}/state`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(state),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

// ── Palette scenes (global presets, applied to any room) ─────────────────────

export interface PaletteScene {
  id: string;
  name: string;
  brightness?: number;
  palette: string[];
}

export async function getPaletteScenes(): Promise<PaletteScene[]> {
  const res = await fetch("/api/palette-scenes");
  if (!res.ok) return [];
  return res.json();
}

export async function createPaletteScene(scene: {
  name: string;
  brightness?: number;
  palette: string[];
}): Promise<{ id: string }> {
  const res = await fetch("/api/palette-scenes", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(scene),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
  return res.json();
}

/** Capture a room's currently-lit colors and average brightness as a new scene. */
export async function savePaletteSceneFromRoom(
  roomId: string,
  name: string,
): Promise<{ id: string }> {
  const res = await fetch(`/api/palette-scenes/from-room/${roomId}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name }),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
  return res.json();
}

export async function deletePaletteScene(sceneId: string): Promise<void> {
  await fetch(`/api/palette-scenes/${sceneId}`, { method: "DELETE" });
}

/** Apply a global scene to a specific room. */
export async function applySceneToRoom(
  roomId: string,
  sceneId: string,
): Promise<{ applied: number; failed: number }> {
  const res = await fetch(`/api/rooms/${roomId}/scenes/${sceneId}/apply`, { method: "POST" });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

// ── Client API keys (public /api/v1 access) ──────────────────────────────────

export interface ApiKey {
  id: string;
  name: string;
  prefix: string;
  created_at: string;
  last_used?: string;
}

export async function getApiKeys(): Promise<ApiKey[]> {
  const res = await fetch("/api/api-keys");
  if (!res.ok) return [];
  return res.json();
}

/** Create a key. The full `key` is returned exactly once — show it immediately. */
export async function createApiKey(
  name: string,
): Promise<{ id: string; name: string; key: string; prefix: string }> {
  const res = await fetch("/api/api-keys", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name }),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
  return res.json();
}

export async function revokeApiKey(id: string): Promise<void> {
  await fetch(`/api/api-keys/${id}`, { method: "DELETE" });
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
  /** Vertices an LED strip passes through after its start tile (corners
   * supported); absent for a point light. */
  points?: [number, number][];
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
  /** The Bifrost Room this region is bound to (server-assigned). */
  room_id?: string;
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

/** Resize the plan grid. The server prunes content outside the new bounds. */
export async function putPlanSize(id: string, width: number, height: number): Promise<void> {
  const res = await fetch(`/api/plans/${id}/size`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ width, height }),
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

export function rgbToHex(r: number, g: number, b: number): string {
  return "#" + [r, g, b].map((v) => v.toString(16).padStart(2, "0")).join("");
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

/** Sync the provider's rooms/zones into mirrors and keep Rooms in step. */
export async function syncProviderGroups(
  id: string,
): Promise<{ synced: number; rooms_created: number; rooms_linked: number }> {
  const res = await fetch(`/api/providers/${id}/sync-groups`, { method: "POST" });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}
