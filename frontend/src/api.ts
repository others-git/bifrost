// Typed wrappers for every REST endpoint Bifrost exposes.

export interface LightState {
  on: boolean;
  brightness?: number;
  color?: { x: number; y: number; brightness: number };
  color_temp_mirek?: number;
  /** Provider-reported reachability; undefined when the provider doesn't say. */
  reachable?: boolean;
  /** Active dynamic effect (a name from `capabilities.effects`, "no_effect" = none). */
  effect?: string;
  /** How the device is currently reached, for multi-transport providers (Govee):
   * "lan" = local network, "cloud" = cloud API. Undefined for single-transport. */
  transport?: string;
  /** The device's network address, when the provider knows it. */
  ip?: string;
}

/** Partial state carried by live events — absent fields are unchanged. */
export interface LightStatePatch {
  on?: boolean;
  brightness?: number;
  color?: { x: number; y: number; brightness: number };
  color_temp_mirek?: number;
  reachable?: boolean;
  effect?: string;
  transport?: string;
}

/** Merge a live-event patch into an existing state without losing fields. */
export function mergePatch(base: LightState | undefined, patch: LightStatePatch): LightState {
  return {
    on: patch.on ?? base?.on ?? false,
    brightness: patch.brightness ?? base?.brightness,
    color: patch.color ?? base?.color,
    color_temp_mirek: patch.color_temp_mirek ?? base?.color_temp_mirek,
    reachable: patch.reachable ?? base?.reachable,
    effect: patch.effect ?? base?.effect,
    transport: patch.transport ?? base?.transport,
  };
}

export interface LightCapabilities {
  dimmable: boolean;
  color_rgb: boolean;
  color_temperature: boolean;
  /** Dynamic effects this light supports (provider names; includes "no_effect"). */
  effects?: string[];
  /** Number of independently addressable colour segments (e.g. a Govee strip's
   * sections); absent when the light has none. */
  segments?: number;
}

/** One segment's target for a per-segment write: a colour (`rgb`, 24-bit
 * `0xRRGGBB`) and/or a brightness (0–100). Both optional — omitted = leave it. */
export interface SegmentColor {
  segment: number;
  rgb?: number;
  brightness?: number;
}

export interface Light {
  id: string;
  name: string;
  provider_id: string;
  device_id: string;
  capabilities: LightCapabilities;
  last_state?: LightState;
  /** Disabled devices keep their room membership but get no commands and are
   * hidden from room control. */
  enabled?: boolean;
  /** Optional glyph-name override; absent/null = use the type default. */
  glyph?: string | null;
  /** Normalized hardware id used for cross-provider de-dup; null if unknown. */
  hw_id?: string | null;
  /** When set, this device is a duplicate of (shadowed by) that device id —
   * hidden from control and collapsed in the inventory. */
  shadowed_by?: string | null;
  /** true if the shadow was set automatically by hw_id matching (native wins). */
  shadow_auto?: boolean;
  /** The room this device is directly assigned to (Devices-page assignment),
   * or null. Room links (synced provider groups) aren't reflected here. */
  room_id?: string | null;
  /** Room via a synced provider-group link, when there's no direct room_id. */
  inherited_room_id?: string | null;
}

/** Enable/disable a device of any domain (lights / audio / power). Disabled =
 * tracked, still in its room, but no commands and hidden from room control. */
export async function setLightEnabled(id: string, enabled: boolean): Promise<void> {
  await fetch(`/api/lights/${id}/enabled`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ enabled }),
  });
}
export async function setMediaEnabled(id: string, enabled: boolean): Promise<void> {
  await fetch(`/api/media/devices/${id}/enabled`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ enabled }),
  });
}
export async function setPowerEnabled(id: string, enabled: boolean): Promise<void> {
  await fetch(`/api/power/devices/${id}/enabled`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ enabled }),
  });
}

/** Override (or clear, with `null`) a device's glyph. Mirrors the enabled
 * setters: one per domain, same `{glyph}` body, type default when cleared. */
export async function setLightGlyph(id: string, glyph: string | null): Promise<void> {
  await fetch(`/api/lights/${id}/glyph`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ glyph }),
  });
}
export async function setMediaGlyph(id: string, glyph: string | null): Promise<void> {
  await fetch(`/api/media/devices/${id}/glyph`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ glyph }),
  });
}
export async function setPowerGlyph(id: string, glyph: string | null): Promise<void> {
  await fetch(`/api/power/devices/${id}/glyph`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ glyph }),
  });
}

/** Set a device's friendly name (`null`/empty reverts to the provider's name). */
async function setDeviceName(path: string, name: string | null): Promise<void> {
  await fetch(path, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name }),
  });
}
export const setLightName = (id: string, name: string | null) =>
  setDeviceName(`/api/lights/${id}/name`, name);
export const setMediaName = (id: string, name: string | null) =>
  setDeviceName(`/api/media/devices/${id}/name`, name);
export const setPowerName = (id: string, name: string | null) =>
  setDeviceName(`/api/power/devices/${id}/name`, name);

/** Manually link a device as a duplicate of `shadowed_by` (or `null` to clear
 * the link → device becomes visible again). The de-dup auto-reconciler handles
 * exact hardware matches; this is the no-hw_id fallback and user override. */
export async function setLightShadow(id: string, shadowed_by: string | null): Promise<void> {
  await fetch(`/api/lights/${id}/shadow`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ shadowed_by }),
  });
}
export async function setMediaShadow(id: string, shadowed_by: string | null): Promise<void> {
  await fetch(`/api/media/devices/${id}/shadow`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ shadowed_by }),
  });
}
export async function setPowerShadow(id: string, shadowed_by: string | null): Promise<void> {
  await fetch(`/api/power/devices/${id}/shadow`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ shadowed_by }),
  });
}

/** M26 composite: merge an audio entity into `primary_id` (or `null` to unmerge).
 * Unlike shadowing, the companion's capabilities are routed/overlaid onto the
 * primary, not discarded. */
export async function setMediaCompanion(id: string, primary_id: string | null): Promise<void> {
  await fetch(`/api/media/devices/${id}/companion`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ primary_id }),
  });
}

/** Assign a device to a room from the device side (`null` removes it from its
 * room). Sets *direct* membership — room links (synced provider groups) are
 * managed on the Rooms page. */
export async function setLightRoom(id: string, room_id: string | null): Promise<void> {
  await fetch(`/api/lights/${id}/room`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ room_id }),
  });
}
export async function setMediaRoom(id: string, room_id: string | null): Promise<void> {
  await fetch(`/api/media/devices/${id}/room`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ room_id }),
  });
}
export async function setPowerRoom(id: string, room_id: string | null): Promise<void> {
  await fetch(`/api/power/devices/${id}/room`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ room_id }),
  });
}
/** M22: bind a source audio device to a receiver (volume/mute route to it), or
 * unbind with receiver_id = null. `receiver_source` is the receiver input to
 * select when the source becomes active. */
export async function setMediaReceiver(
  id: string,
  receiver_id: string | null,
  receiver_source: string | null,
): Promise<void> {
  await fetch(`/api/media/devices/${id}/receiver`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ receiver_id, receiver_source }),
  });
}

export interface Provider {
  id: string;
  provider_type: string;
  /** Human-facing type name, e.g. "Sonos". */
  type_name: string;
  /** UI category: a single device domain, or "integration" (e.g. Home Assistant). */
  domain: "light" | "media" | "integration";
  name: string;
  enabled: boolean;
  /** When set, discovering this provider removes devices it no longer reports. */
  prune: boolean;
  /** User-controlled sort position on the Devices page (ascending). */
  display_order: number;
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
  /** Human-facing name, e.g. "Philips Hue". */
  display_name: string;
  /**
   * UI grouping in the Add-provider menu. "light"/"media" are single-domain
   * device providers; "integration" is a higher-level platform adapter (e.g.
   * Home Assistant) that can surface many device kinds.
   */
  kind: "light" | "media" | "integration";
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

/** A device found on the LAN that isn't behind a configured provider yet. */
export interface FoundDevice {
  provider_type: string;
  type_name: string;
  host: string;
  label?: string | null;
  credentials: Record<string, unknown>;
}

/** Scan every discoverable provider type at once for unconfigured devices —
 * the "found devices" flow (no provider type to pick first). */
export async function discoverAllDevices(): Promise<FoundDevice[]> {
  const res = await fetch("/api/providers/discover-all");
  if (!res.ok) return [];
  return res.json();
}

// ── App settings ──────────────────────────────────────────────────────────────

export interface AppSettings {
  /** Extra private /24 subnets auto-detect should also sweep (Expanded-LAN). */
  expanded_lan_scan: string[];
  /** Developer mode — exposes contributor/dev-only surfaces (provider debug, the
   * `/api/dev` API). Omit on a partial PUT to leave it unchanged. */
  dev_mode?: boolean;
}

export async function getSettings(): Promise<AppSettings> {
  const res = await fetch("/api/settings");
  if (!res.ok) return { expanded_lan_scan: [] };
  return res.json();
}

/** Dev-mode only: a provider's debug diagnostics (raw upstream capabilities, the
 * ones we don't model yet, etc.). 404s when dev mode is off. */
export async function getDevProviderDebug(id: string): Promise<unknown | null> {
  const res = await fetch(`/api/dev/providers/${id}/debug`);
  if (!res.ok) return null;
  return res.json();
}

/** The raw upstream representation of one device (HA: state + attributes +
 * supported_features). Dev-mode only — 404s when off. */
/** One generic control primitive on a passthrough device. */
export interface GenericControl {
  type: "toggle" | "number" | "enum" | "button" | "readout";
  key: string;
  label: string;
  value?: unknown;
  min?: number;
  max?: number;
  step?: number;
  options?: string[];
  unit?: string;
}

/** A generic "passthrough" device — a source device Bifrost doesn't natively
 * model (climate, cover, lock, …), surfaced as a set of control primitives. */
export interface GenericDevice {
  provider_id: string;
  device_id: string;
  name: string;
  /** Source domain (climate, cover, …) — drives the glyph/label. */
  kind: string;
  controls: GenericControl[];
  hw_id?: string | null;
}

/** Live list of generic devices across every provider that serves them (HA). */
export async function getGenericDevices(): Promise<GenericDevice[]> {
  const res = await fetch("/api/generic/devices");
  if (!res.ok) return [];
  return res.json();
}

/** Apply a control write to a generic device. `value` is the primitive's new
 * value (bool / number / string; ignored for a button). */
export async function setGenericControl(
  providerId: string,
  deviceId: string,
  key: string,
  value: unknown,
): Promise<string | null> {
  const res = await fetch("/api/generic/devices/control", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ provider_id: providerId, device_id: deviceId, key, value }),
  });
  if (res.ok) return null;
  return (await res.text()) || `HTTP ${res.status}`;
}

export interface DeviceRaw {
  provider_type: string;
  entity_id?: string;
  domain?: string;
  state?: unknown;
  supported_features?: number | null;
  attributes?: Record<string, unknown>;
  /** How a generic passthrough device would model this entity (dev preview). */
  generic_preview?: GenericControl[];
  note?: string;
}

export async function getDeviceRaw(providerId: string, deviceId: string): Promise<DeviceRaw | null> {
  const res = await fetch(`/api/dev/devices/${providerId}/${encodeURIComponent(deviceId)}/raw`);
  if (!res.ok) return null;
  return res.json();
}

export interface DevProvider {
  id: string;
  name: string;
  provider_type: string;
  enabled: boolean;
  /** Whether this provider exposes `debug_info` (light providers, for now). */
  has_debug: boolean;
}

/** Dev-mode only: server build/runtime info (version, build profile, counts). */
export async function getDevInfo(): Promise<Record<string, unknown> | null> {
  const res = await fetch("/api/dev/info");
  if (!res.ok) return null;
  return res.json();
}

/** Dev-mode only: the provider list with debug-availability flags. */
export async function getDevProviders(): Promise<DevProvider[]> {
  const res = await fetch("/api/dev/providers");
  if (!res.ok) return [];
  const body = await res.json();
  return body.providers ?? [];
}

/** Save settings — a **partial** update; omitted fields keep their stored value
 * (a dev-mode toggle won't clobber the subnet list, and vice-versa). Returns the
 * normalised settings, or an error message. */
export async function updateSettings(
  settings: Partial<AppSettings>,
): Promise<AppSettings | { error: string }> {
  const res = await fetch("/api/settings", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(settings),
  });
  if (res.ok) return res.json();
  return { error: (await res.text()) || `HTTP ${res.status}` };
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

/** Per-process build nonce — changes on every server restart/redeploy. */
export async function getInstance(): Promise<{ instance_id: string; version: string } | null> {
  const res = await fetch(`/api/instance?_=${Date.now()}`, { cache: "no-store" });
  if (!res.ok) return null;
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

/** Trade a paired kiosk's `bfr_key` cookie (set by the kiosk app) for a dashboard
 * session, so an authorized wall fixture skips the password login. */
export async function kioskLogin(): Promise<boolean> {
  const res = await fetch("/api/auth/kiosk", { method: "POST" });
  return res.ok;
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

/** Set per-segment colours on a strip (write-only; not reflected in light state). */
export async function setLightSegments(id: string, segments: SegmentColor[]): Promise<void> {
  await fetch(`/api/lights/${id}/segments`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ segments }),
  });
}

// ── Audio devices ────────────────────────────────────────────────────────────

export interface NowPlaying {
  title?: string;
  artist?: string;
  album?: string;
  play_state?: "playing" | "paused" | "stopped";
}

export interface MediaState {
  power: boolean;
  volume: number;
  mute: boolean;
  source?: string;
  /** Selectable sources/inputs — receiver inputs, or a smart TV's apps (HA
   * `source_list`). Switch to one by sending it as `source`. */
  source_list?: string[];
  now_playing?: NowPlaying;
  reachable?: boolean;
  /** When this device is in a live multi-speaker sync group, the provider id of
   * the group's coordinator. Devices sharing this are playing in sync; the UI
   * derives a single grouped control from them. null/absent = standalone. */
  group_coordinator?: string | null;
  /** The device's network address, when the provider knows it. */
  ip?: string;
}

export interface MediaCapabilities {
  sources: boolean;
  transport: boolean;
  now_playing: boolean;
  /** Device exposes saved favorites the user can start playing (Sonos). */
  favorites?: boolean;
  /** Device can be grouped/ungrouped with other speakers to play in sync
   * (provider-native grouping, e.g. Sonos), independent of Bifrost Rooms. */
  grouping?: boolean;
}

/** A saved favorite/preset (e.g. a Sonos Favorite) playable by reference. */
export interface MediaFavorite {
  id: string;
  title: string;
  subtitle?: string;
}

export interface MediaDevice {
  id: string;
  provider_id: string;
  /** Provider-native id (e.g. "main") — matches audio_state push events. */
  device_id: string;
  name: string;
  kind: "receiver" | "speaker" | "tv" | "zone";
  capabilities: MediaCapabilities;
  state: MediaState;
  last_seen?: string;
  enabled?: boolean;
  /** Optional glyph-name override; absent/null = use the type default. */
  glyph?: string | null;
  /** Normalized hardware id used for cross-provider de-dup; null if unknown. */
  hw_id?: string | null;
  /** When set, this device is a duplicate of (shadowed by) that device id —
   * hidden from control and collapsed in the inventory. */
  shadowed_by?: string | null;
  /** true if the shadow was set automatically by hw_id matching (native wins). */
  shadow_auto?: boolean;
  /** M26 composite: the primary device this entity is merged into (a companion),
   * or null. A companion is hidden from control; its state/controls merge into
   * the primary (unlike shadowed_by, which discards them). */
  companion_of?: string | null;
  /** The room this device is directly assigned to (Devices-page assignment),
   * or null. Room links (synced provider groups) aren't reflected here. */
  room_id?: string | null;
  /** Room via a synced provider-group link, when there's no direct room_id. */
  inherited_room_id?: string | null;
  /** M22: the receiver this source's volume/mute routes to; null = unbound. */
  receiver_id?: string | null;
  /** The receiver input to select when this source becomes active; null = none. */
  receiver_source?: string | null;
  /** M24 composite: the paired remote's id when this TV's media_player shares
   * hardware with an enabled remote — resolved server-side so the unified TV
   * control renders without a separate remote lookup. null = no paired remote. */
  remote_id?: string | null;
}

/** Sparse command — only the fields present are applied. */
export interface MediaCommand {
  power?: boolean;
  volume?: number;
  mute?: boolean;
  source?: string;
  transport?: "play" | "pause" | "stop" | "next" | "previous" | "toggle";
}

export async function getMediaDevices(): Promise<MediaDevice[]> {
  const res = await fetch("/api/media/devices");
  if (!res.ok) return [];
  return res.json();
}

/** Live read — round-trips to the device and refreshes the cache. */
export async function getMediaDevice(id: string): Promise<MediaDevice | null> {
  const res = await fetch(`/api/media/devices/${id}`);
  if (!res.ok) return null;
  return res.json();
}

export async function setMediaState(id: string, cmd: MediaCommand): Promise<string | null> {
  const res = await fetch(`/api/media/devices/${id}/state`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(cmd),
  });
  if (res.ok) return null;
  return (await res.text()) || `HTTP ${res.status}`;
}

/** List a device's saved favorites (live read from the provider). */
export async function getMediaFavorites(id: string): Promise<MediaFavorite[]> {
  const res = await fetch(`/api/media/devices/${id}/favorites`);
  if (!res.ok) return [];
  return res.json();
}

/** Start playing a favorite by its provider-native id. */
export async function playMediaFavorite(
  id: string,
  favoriteId: string,
): Promise<string | null> {
  const res = await fetch(`/api/media/devices/${id}/favorites/play`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ favorite_id: favoriteId }),
  });
  if (res.ok) return null;
  return (await res.text()) || `HTTP ${res.status}`;
}

/** Join `id` into the synced playback group coordinated by `coordinatorId`
 * (provider-native speaker grouping, e.g. Sonos). */
export async function groupMediaDevice(
  id: string,
  coordinatorId: string,
): Promise<string | null> {
  const res = await fetch(`/api/media/devices/${id}/group`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ coordinator_id: coordinatorId }),
  });
  if (res.ok) return null;
  return (await res.text()) || `HTTP ${res.status}`;
}

/** Remove `id` from any playback group, returning it to standalone playback. */
export async function ungroupMediaDevice(id: string): Promise<string | null> {
  const res = await fetch(`/api/media/devices/${id}/ungroup`, { method: "POST" });
  if (res.ok) return null;
  return (await res.text()) || `HTTP ${res.status}`;
}

// ── Remote control (M24) ──────────────────────────────────────────────────────

export interface RemoteState {
  on: boolean;
  /** Foreground app's package id (e.g. `com.netflix.ninja`), if reported. */
  current_app?: string;
  reachable?: boolean;
  /** The device's network address, when the provider knows it. */
  ip?: string;
}

export interface RemoteDevice {
  id: string;
  provider_id: string;
  device_id: string;
  name: string;
  state: RemoteState;
  last_seen: string | null;
  enabled: boolean;
  glyph: string | null;
  hw_id: string | null;
  /** The paired TV audio device id, if this remote controls a known TV. */
  paired_media_id: string | null;
}

/** The canonical keys a remote understands (snake_case, mirrors the backend). */
export type RemoteKey =
  | "up" | "down" | "left" | "right" | "select"
  | "back" | "home" | "menu"
  | "volume_up" | "volume_down" | "mute"
  | "play_pause" | "next" | "previous"
  | "power";

/** One action sent to a remote — a tagged union (exactly one variant). */
export type RemoteCommand =
  | { key: { key: RemoteKey; hold_secs?: number } }
  | { text: { text: string } }
  | { launch_app: { activity: string } }
  | { power: { on: boolean } }
  | { native: { token: string } };

/** One entry in a remote's expanded command catalogue (Bravia IRCC list, …). */
export interface RemoteCommandInfo {
  name: string;
  token: string;
}

/** The remote's expanded (native) command catalogue — empty when it has none. */
export async function getRemoteCommands(id: string): Promise<RemoteCommandInfo[]> {
  const res = await fetch(`/api/remote/devices/${id}/commands`);
  if (!res.ok) return [];
  return res.json();
}

/** A launchable app on a remote's TV (pinned ∪ recents). */
export interface RemoteApp {
  package: string;
  name: string;
  pinned: boolean;
  last_seen: string | null;
}

export async function getRemoteDevices(): Promise<RemoteDevice[]> {
  const res = await fetch("/api/remote/devices");
  if (!res.ok) return [];
  return res.json();
}

/** Live read — round-trips to the device for fresh power / current-app. */
export async function getRemoteState(id: string): Promise<RemoteState | null> {
  const res = await fetch(`/api/remote/devices/${id}`);
  if (!res.ok) return null;
  return res.json();
}

export async function sendRemoteCommand(id: string, cmd: RemoteCommand): Promise<string | null> {
  const res = await fetch(`/api/remote/devices/${id}/command`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(cmd),
  });
  if (res.ok) return null;
  return (await res.text()) || `HTTP ${res.status}`;
}

/** A remote's launchable apps (pinned first, then recents). */
export async function getRemoteApps(id: string): Promise<RemoteApp[]> {
  const res = await fetch(`/api/remote/devices/${id}/apps`);
  if (!res.ok) return [];
  return res.json();
}

export async function setRemoteAppPin(
  id: string,
  pkg: string,
  pinned: boolean,
): Promise<string | null> {
  const res = await fetch(`/api/remote/devices/${id}/apps/pin`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ package: pkg, pinned }),
  });
  if (res.ok) return null;
  return (await res.text()) || `HTTP ${res.status}`;
}

// ── Power devices ─────────────────────────────────────────────────────────────

/** Glyph-bearing flavour of a strictly-on/off device. */
export type PowerKind = "switch" | "outlet" | "fan" | "toggle" | "generic";

export interface PowerState {
  on: boolean;
  reachable?: boolean;
}

/** A strictly on/off device (switch, plug, fan, boolean helper). Its only state
 * is `on`; `kind` drives the glyph, not behaviour. */
export interface PowerDevice {
  id: string;
  provider_id: string;
  /** Provider-native id (e.g. an HA entity_id). */
  device_id: string;
  name: string;
  kind: PowerKind;
  state: PowerState;
  last_seen?: string;
  enabled?: boolean;
  /** Optional glyph-name override; absent/null = use the type default. */
  glyph?: string | null;
  /** Normalized hardware id used for cross-provider de-dup; null if unknown. */
  hw_id?: string | null;
  /** When set, this device is a duplicate of (shadowed by) that device id —
   * hidden from control and collapsed in the inventory. */
  shadowed_by?: string | null;
  /** true if the shadow was set automatically by hw_id matching (native wins). */
  shadow_auto?: boolean;
  /** The room this device is directly assigned to (Devices-page assignment),
   * or null. Room links (synced provider groups) aren't reflected here. */
  room_id?: string | null;
  /** Room via a synced provider-group link, when there's no direct room_id. */
  inherited_room_id?: string | null;
}

export async function getPowerDevices(): Promise<PowerDevice[]> {
  const res = await fetch("/api/power/devices");
  if (!res.ok) return [];
  return res.json();
}

/** Live read — round-trips to the device and refreshes the cache. */
export async function getPowerDevice(id: string): Promise<PowerDevice | null> {
  const res = await fetch(`/api/power/devices/${id}`);
  if (!res.ok) return null;
  return res.json();
}

export async function setPowerState(id: string, on: boolean): Promise<string | null> {
  const res = await fetch(`/api/power/devices/${id}/state`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ on }),
  });
  if (res.ok) return null;
  return (await res.text()) || `HTTP ${res.status}`;
}

/** Re-run a provider's device discovery (lights or audio). */
/** Discover a provider's devices. `prune` omitted → uses the provider's stored
 * flag; `true`/`false` forces it for this run (removes / keeps stale devices). */
export async function discoverProvider(
  id: string,
  opts?: { prune?: boolean },
): Promise<{ discovered: number; pruned: number }> {
  const q = opts?.prune === undefined ? "" : `?prune=${opts.prune}`;
  const res = await fetch(`/api/providers/${id}/discover${q}`, { method: "POST" });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

/** Set whether discovering this provider prunes devices it no longer reports. */
export async function setProviderPrune(id: string, prune: boolean): Promise<void> {
  await fetch(`/api/providers/${id}/prune`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ prune }),
  });
}

/** Persist the Devices-page ordering of provider groups (full id list, top → bottom). */
export async function setProviderOrder(order: string[]): Promise<void> {
  await fetch("/api/providers/order", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ order }),
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

/** A provider's current non-secret configuration, used to prefill the edit form. */
export interface ProviderConfig {
  name: string;
  provider_type: string;
  /** Current values for non-secret fields (e.g. host/IP). Secrets are omitted. */
  values: Record<string, unknown>;
  /** False when stored credentials can't be decrypted — re-enter everything. */
  decryptable: boolean;
}

export async function getProviderConfig(id: string): Promise<ProviderConfig | null> {
  const res = await fetch(`/api/providers/${id}/config`);
  if (!res.ok) return null;
  return res.json();
}

/**
 * Update a provider's IP/credentials. Submitted fields are merged over the
 * stored ones, so a blank secret field keeps its current value.
 */
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

/** A scene: a full-state snapshot of lights (color/temp/effect) + power devices,
 * restored verbatim. `room_id == null` = a whole-home scene (one can be the
 * `is_default` "Restore Home" preset); otherwise it's scoped to that room's
 * members (a Room Scene). */
export interface Scene {
  id: string;
  name: string;
  created_at: string;
  lights: number;
  power: number;
  is_default: boolean;
  room_id: string | null;
  room_name: string | null;
}

export async function getScenes(): Promise<Scene[]> {
  const res = await fetch("/api/scenes");
  if (!res.ok) return [];
  return res.json();
}

/** Mark (or clear) a home scene as the single "Restore Home" default. */
export async function setDefaultScene(id: string, isDefault: boolean): Promise<void> {
  await fetch(`/api/scenes/${id}/default`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ default: isDefault }),
  });
}

/** One-tap "Restore Home": apply the default home scene. Throws if none is set. */
export async function restoreDefaultHome(): Promise<{ applied: number; failed: number }> {
  const res = await fetch("/api/scenes/restore-default", { method: "POST" });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
  return res.json();
}

/** Capture a new scene. Omit `roomId` for a whole-home snapshot; pass a room id
 * to scope it to that room's members (a Room Scene). */
export async function createScene(
  name: string,
  roomId?: string,
): Promise<{ id: string; lights: number; power: number }> {
  const res = await fetch("/api/scenes", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name, room_id: roomId ?? null }),
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
  /** Which domain the linked provider room/zone belongs to. */
  domain: "light" | "media";
}

export interface Room {
  id: string;
  name: string;
  /** Effective members: linked provider-group lights ∪ direct lights. */
  light_ids: string[];
  direct_light_ids: string[];
  links: RoomLink[];
  /** Audio devices this room controls (volume/mute fans out to all), each with
   * its per-room volume offset. */
  media_devices: RoomMediaMember[];
  /** Power devices (switches/plugs/fans) the room contains. */
  power_device_ids: string[];
  /** User-configured quick-control buttons on the room's Control card. */
  controls: RoomControl[];
  /** Disabled rooms are hidden from the Dashboard/Floor Plan; managed in Settings. */
  enabled: boolean;
}

export type ControlKind = "power" | "volume" | "brightness" | "scene";

/** One device a configured control acts on. */
export interface ControlTarget {
  domain: "light" | "media" | "power";
  id: string;
}

/** A configured quick-control button (see migration 0034). `power` toggles its
 * targets; `volume`/`brightness` open a scoped control; `scene` applies a scene. */
export interface RoomControl {
  /** Server-assigned; empty/omitted on create. */
  id?: string;
  kind: ControlKind;
  /** A Glyph registry name (`components/glyphs`). */
  glyph: string;
  label?: string | null;
  targets: ControlTarget[];
  scene_id?: string | null;
}

/** Replace the room's configured quick-control buttons (add/edit/remove/reorder). */
export async function setRoomControls(roomId: string, controls: RoomControl[]): Promise<void> {
  const res = await fetch(`/api/rooms/${roomId}/controls`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ controls }),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
}

export interface RoomMediaMember {
  media_device_id: string;
  /** Signed %, added to the room volume then clamped 0–100 for this device. */
  volume_offset: number;
}

/** Replace a room's explicit audio devices + per-device volume offsets. */
export async function setRoomMediaDevices(
  roomId: string,
  devices: RoomMediaMember[],
): Promise<void> {
  await fetch(`/api/rooms/${roomId}/media`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ devices }),
  });
}

/** Fan a volume/mute command out to every audio device in the room (offsets applied). */
export async function setRoomMediaState(
  roomId: string,
  cmd: { volume?: number; mute?: boolean },
): Promise<void> {
  await fetch(`/api/rooms/${roomId}/media/state`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(cmd),
  });
}

export interface ProviderGroupInfo {
  id: string;
  provider_id: string;
  provider_group_id: string;
  name: string;
  /** The group's primary domain label; an area can still mix domains (see the
   * `*_ids` lists). */
  domain: "light" | "media";
  light_ids: string[];
  media_device_ids: string[];
  /** Member power devices (switches/plugs/fans). */
  power_device_ids: string[];
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

/** Enable or disable a room (disabled rooms are hidden from Dashboard/Floor Plan). */
export async function setRoomEnabled(id: string, enabled: boolean): Promise<void> {
  await fetch(`/api/rooms/${id}/enabled`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ enabled }),
  });
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

/** Replace the room's power-device membership (switches/plugs/fans). */
export async function setRoomPowerDevices(id: string, power_device_ids: string[]): Promise<void> {
  await fetch(`/api/rooms/${id}/power`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ power_device_ids }),
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

/**
 * Mint a short-lived device-pairing token. The dashboard renders it as a QR; a
 * headless device (the wall tablet) scans it and POSTs to `/api/enrollment/redeem`
 * to receive a real API key — no key typed on a touchscreen.
 */
export async function createEnrollmentToken(): Promise<{
  token: string;
  expires_at: string;
  expires_in_secs: number;
}> {
  const res = await fetch("/api/enrollment", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: "{}",
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
  return res.json();
}

// ── AI model endpoints (voice: transcription / chat / tts) ───────────────────

export type AiRole = "transcription" | "chat" | "tts";

export interface AiEndpoint {
  role: AiRole;
  base_url: string;
  model: string;
  /** A key is stored (never returned). Omit `api_key` on save to keep it. */
  has_key: boolean;
  enabled: boolean;
}

export async function getAiEndpoints(): Promise<AiEndpoint[]> {
  const res = await fetch("/api/ai-endpoints");
  if (!res.ok) return [];
  return res.json();
}

export async function putAiEndpoint(
  role: AiRole,
  body: { base_url: string; model: string; api_key?: string | null; enabled: boolean },
): Promise<void> {
  const res = await fetch(`/api/ai-endpoints/${role}`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
}

export async function deleteAiEndpoint(role: AiRole): Promise<void> {
  await fetch(`/api/ai-endpoints/${role}`, { method: "DELETE" });
}

/** Probe an endpoint's reachability (`GET {base_url}/models`). */
export async function testAiEndpoint(role: AiRole): Promise<{ ok: boolean; message: string }> {
  const res = await fetch(`/api/ai-endpoints/${role}/test`, { method: "POST" });
  if (!res.ok) return { ok: false, message: `HTTP ${res.status}` };
  return res.json();
}

// ── Kiosks (wall-tablet companion apps) ──────────────────────────────────────

export interface Kiosk {
  id: string;
  name: string;
  app_version: string | null;
  screen_on: boolean | null;
  last_seen: string | null;
  online: boolean;
  pending_command: string | null;
  /** false once de-authed (key revoked) — must re-pair. */
  authorized: boolean;
  /** Assigned Room id (the kiosk's voice context / location), or null. */
  room_id: string | null;
  /** Battery / power telemetry from the last check-in (null on older apps). */
  battery_level: number | null;
  battery_charging: boolean | null;
  battery_voltage_mv: number | null;
  /** Signed micro-amps (+ = charging into the battery). */
  battery_current_ua: number | null;
  /** Deci-celsius (245 = 24.5°C). */
  battery_temp_dc: number | null;
  /** ac | usb | wireless | none */
  power_source: string | null;
}

export async function getKiosks(): Promise<Kiosk[]> {
  const res = await fetch("/api/kiosks");
  if (!res.ok) return [];
  return res.json();
}

export type KioskCommandVerb = "sleep" | "wake" | "lock" | "update";

/** Queue a command the kiosk performs on its next check-in. */
export async function kioskCommand(id: string, command: KioskCommandVerb): Promise<void> {
  const res = await fetch(`/api/kiosks/${id}/command`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ command }),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
}

/** Assign the kiosk to a Room (its voice context), or clear with null. */
export async function setKioskRoom(id: string, room_id: string | null): Promise<void> {
  const res = await fetch(`/api/kiosks/${id}/room`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ room_id }),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
}

/** Revoke the kiosk's key — it must re-enroll via QR. */
export async function kioskDeauth(id: string): Promise<void> {
  await fetch(`/api/kiosks/${id}/deauth`, { method: "POST" });
}

export async function forgetKiosk(id: string): Promise<void> {
  await fetch(`/api/kiosks/${id}`, { method: "DELETE" });
}

// ── Kiosk OTA update source (the hub relays GitHub releases to LAN kiosks) ────

/** Where the hub pulls kiosk APKs from — editable from the dashboard. */
export interface KioskUpdateConfig {
  repo: string;
  asset: string;
}

/** The cached APK's identity (what a kiosk compares its installed version to). */
export interface KioskUpdateManifest {
  version_code: number;
  version_name: string;
  size: number;
  sha256: string;
}

export async function getKioskUpdateConfig(): Promise<KioskUpdateConfig> {
  const res = await fetch("/api/kiosks/update/config");
  if (!res.ok) return { repo: "", asset: "" };
  return res.json();
}

export async function setKioskUpdateConfig(
  repo: string,
  asset: string,
): Promise<KioskUpdateConfig | { error: string }> {
  const res = await fetch("/api/kiosks/update/config", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ repo, asset }),
  });
  if (res.ok) return res.json();
  return { error: (await res.text()) || `HTTP ${res.status}` };
}

/** The currently-cached APK manifest on the hub (null until a fetch is run). */
export async function getKioskUpdateStatus(): Promise<{ cached: KioskUpdateManifest | null }> {
  const res = await fetch("/api/kiosks/update");
  if (!res.ok) return { cached: null };
  return res.json();
}

/** Fetch the latest release from GitHub and cache its APK on the hub. */
export async function refreshKioskUpdate(): Promise<
  { manifest: KioskUpdateManifest; downloaded: boolean } | { error: string }
> {
  const res = await fetch("/api/kiosks/update", { method: "POST" });
  if (res.ok) return res.json();
  return { error: (await res.text()) || `HTTP ${res.status}` };
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

/** An audio device placed on the plan — point only (no LED-strip points). */
export interface MediaPlacement {
  media_device_id: string;
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
  media: MediaPlacement[];
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

export async function putPlanAudio(id: string, placements: MediaPlacement[]): Promise<void> {
  const res = await fetch(`/api/plans/${id}/media`, {
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

export type SmartTvPairResult =
  | { status: "pin_displayed"; message: string }
  | { status: "paired"; auth: string }
  | { error: string; message: string };

/** Smart-TV (Bravia) PIN pairing. Call with no `pin` to make the TV show a PIN,
 * then again with that `pin` to receive the `auth` token. */
export async function pairSmartTv(host: string, pin?: string): Promise<SmartTvPairResult> {
  const res = await fetch("/api/providers/smarttv/pair", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ host, pin: pin || undefined }),
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

/** Sync the provider's rooms/zones into mirrors and keep Rooms in step. */
export async function syncProviderGroups(
  id: string,
): Promise<{ synced: number; rooms_created: number; rooms_linked: number }> {
  const res = await fetch(`/api/providers/${id}/sync-groups`, { method: "POST" });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}
