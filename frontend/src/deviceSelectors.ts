// The ONE place that decides which devices a control surface may offer. Every
// picker/menu (automations, boards, rooms, aware-override, …) filters through
// these — never its own inline `enabled !== false && !shadowed_by` — so a
// device that must never be pickable (disabled, a hidden de-dup duplicate, a
// merged-in media companion) can't leak into one menu because another was
// updated and this one wasn't. The list endpoints stay full inventory (the
// Devices page needs disabled/shadowed rows to enable/unlink them); the
// filtering is a control-surface concern, applied here uniformly.
//
// The rule per domain: `enabled` (an unset flag counts as enabled — a freshly
// discovered device is on by default) AND not a de-dup shadow. Media adds
// `companion_of` (a device merged into a composite primary — controlled via
// the primary, hidden on its own). Remotes carry no shadow flag: their
// group-based de-dup is resolved server-side in `list_remotes`, so the list
// the client receives is already clean of duplicates — only `enabled` remains.

import type { Light, MediaDevice, PowerDevice, RemoteDevice, SensorDevice } from "./api";

const isEnabled = (d: { enabled?: boolean }) => d.enabled !== false;
const notShadowed = (d: { shadowed_by?: string | null }) => !d.shadowed_by;

/** Lights offerable as a control target. */
export const pickableLights = (lights: Light[]): Light[] =>
  lights.filter((l) => isEnabled(l) && notShadowed(l));

/** Media devices offerable as a control target (excludes merged-in companions). */
export const pickableMedia = (media: MediaDevice[]): MediaDevice[] =>
  media.filter((m) => isEnabled(m) && notShadowed(m) && !m.companion_of);

/** Power devices offerable as a control target. */
export const pickablePower = (power: PowerDevice[]): PowerDevice[] =>
  power.filter((p) => isEnabled(p) && notShadowed(p));

/** Sensors offerable as a trigger/gate subject. */
export const pickableSensors = (sensors: SensorDevice[]): SensorDevice[] =>
  sensors.filter((s) => isEnabled(s) && notShadowed(s));

/** Remotes offerable as a control target (server already de-dups by group). */
export const pickableRemotes = (remotes: RemoteDevice[]): RemoteDevice[] =>
  remotes.filter((r) => isEnabled(r));
