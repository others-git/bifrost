// Typed wrappers for every REST endpoint Bifrost exposes.

export interface LightState {
  on: boolean;
  brightness?: number;
  color?: { x: number; y: number; brightness: number };
  color_temp_mirek?: number;
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

export async function discoverLights(id: string): Promise<{ discovered: number }> {
  const res = await fetch(`/api/providers/${id}/discover`, { method: "POST" });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}
