# Composite devices

A **composite device** is a single Bifrost control surface that aggregates more
than one underlying entity for **one physical device**. The canonical case is a
smart TV: the same box can show up as several Bifrost rows with *complementary*
capabilities — a `media_player` that carries now-playing + volume, a second
`media_player` that carries the app list, a virtual `remote.*` (D-pad + app
launch), and an AV receiver that actually owns the volume. Bifrost merges those
into **one effective device** so the user sees and controls a single thing, and
every action is routed to whichever underlying entity can actually perform it.

This page is the source of truth for how composites are assembled and routed. It
exists because the pieces are spread across several files and are easy to step
over — when you touch any composite seam, read this first and keep it current.

---

## TL;DR

- A composite is **one primary `media_device` row** with up to three kinds of
  attachment overlaid onto it **server-side**:
  1. **Companions** — other `media_device` rows for the same physical box,
     merged lossless-ly via `companion_of`.
  2. **A paired remote** — a `remote_device` linked via `paired_media_id`,
     surfaced on the primary as `remote_id`.
  3. **A bound receiver** — a receiver `media_device` that owns this source's
     volume/mute, via `receiver_id` / `receiver_source`.
- The merged result is the **effective device**: the API returns the primary row
  with companions' state/capabilities folded in, the receiver's volume overlaid,
  the paired remote's id attached, and power/reachability resolved across the
  whole set. Clients never assemble a composite themselves.
- **Reads merge; writes route.** Each command field (power / volume / transport /
  source / favorites / cast) goes to the backing that owns it, not blindly to one
  entity.
- Resolution is **direction-independent** and **whole-composite**: it considers
  `device ∪ companions`, so which row is the primary doesn't change what controls
  surface (fixed in 0.27.2).

---

## Why not just de-dup (shadow)?

Bifrost already has cross-provider **de-dup** (`shadowed_by`): when the *same*
device is reachable two ways (native Hue/Sonos/Bravia **and** via Home Assistant),
the integration copy is **shadowed** — hidden and discarded, native wins. See
[Rooms & devices → de-duplication](rooms-and-devices.md).

That is the wrong tool when the two rows are **not** equivalent duplicates but
**complementary halves** of one device (TV `media_player` #1 has volume, #2 has
apps; or a TV plus its receiver plus its remote). Shadowing one would *lose* the
capability it uniquely carries. A **companion** link instead **merges** the
secondary into a primary so the **union** of capabilities lives on one surface.

| | `shadowed_by` (de-dup) | `companion_of` (composite) |
|---|---|---|
| Relationship | same device seen twice | complementary parts of one device |
| Secondary row | hidden **and discarded** | hidden, **state/caps overlaid onto primary** |
| Capabilities | only the native copy's | **union** of both |
| Set by | auto hw_id match (`reconcile_duplicates`) + manual | manual merge (Devices page) |

A row is **never both** shadowed and a companion.

---

## The three ingredients

### 1. Companions — `companion_of`

`media_devices.companion_of` holds the id of the **primary** `media_device` this
row merges into. `NULL` = a standalone surface. The primary carries no marker
itself; it's a normal row that companions point at.

On read, each companion's complementary state is overlaid onto its primary by
`merge_companion_into` (`src/api/media.rs`), filling only what the primary lacks
and **unioning** the capability flags:

- `now_playing`, `source`, `group_coordinator` — filled if the primary has none.
- `source_list` — union (appends entries the primary doesn't already have).
- `volume` / `mute` — if the primary reads volume `0` and a companion carries a
  real (non-zero) volume, take the companion's (one TV view reads 0 while a
  Cast/soundbar view carries the real level).
- `receiver_id` / `receiver_source` — adopted from a companion if the primary is
  unbound (so a companion's receiver binding surfaces the receiver's volume on the
  merged card).
- `capabilities.{transport,sources,favorites,now_playing,grouping}` — OR-unioned.

Companions are **hidden from control** and **collapsed in the inventory**, and
are **excluded from room membership** (`effective_media_members` filters
`companion_of IS NULL`, `src/api/rooms.rs`). They remain in the device list,
marked, so the Devices page can show/un-merge them.

### 2. Paired remote — `paired_media_id` → `remote_id`

A virtual remote (`remote_devices` — D-pad keys, text, app launch) is **paired**
to its TV's `media_device` when they share a hardware id.
`remote_devices.paired_media_id` is set idempotently after discovery by
`reconcile_remote_pairings`
(`src/api/remote.rs`): for each remote with an `hw_id`, find a non-shadowed
`media_device` with the same `hw_id`, preferring `kind = 'tv'`.

On the media read path the **inverse** is surfaced: the effective device carries
`remote_id` (the paired, enabled remote's id), resolved against the **whole
composite** — `cm.id = <surface> OR cm.companion_of = <surface>` — so a remote
paired to *any* companion still surfaces on the primary. The frontend reads
`device.remote_id` directly to render the unified **AIO TV control** (keypad +
apps fly-out) with no separate remote lookup.

### 3. Bound receiver — `receiver_id` / `receiver_source`

A **source** device (TV / streamer / console) can be **bound to a receiver** that
owns its volume. Stored on the *source* (`media_devices.receiver_id` +
`receiver_source`); many sources → one receiver. Detail in
[Rooms & devices → receiver binding](rooms-and-devices.md). Within a composite:

- **Read:** the effective device shows the **receiver's** volume/mute (what the
  source's volume slider actually controls). Uses the receiver's *cached* state,
  not a live read — push-mode receivers (Onkyo) allow only one connection.
- **Write:** `volume`/`mute` route to the receiver; `power`/`source`/`transport`
  stay on the source. Powering the source **on** also wakes the receiver and
  switches it to `receiver_source`. (`MediaCommand::split_for_receiver` /
  `apply_with_receiver`.)

---

## The effective device (read path)

`list_all_devices` and `get_device_live` (`src/api/media.rs`) both assemble the
effective device. **The overlay order is load-bearing — do not reorder it:**

1. **Load rows** with their direct/inherited room and `remote_id` subqueries.
2. **Merge companions** (`merge_companion_into`) — fold each companion's state +
   capabilities into its primary.
3. **Resolve composite power/reachability** (`apply_composite_power`) — using the
   companions' and the paired remote's power signals.
4. **Overlay the receiver's volume/mute** — last, so a bound source (whether the
   binding came from the primary or was adopted from a companion in step 2) shows
   the receiver's level.

Companions are skipped as surfaces in steps 3–4 (`if companion_of.is_some()
{ continue }`). `get_device_live` does the same for a single device, loading its
companions and one paired-remote state on demand.

### Power & reachability resolution

A standby TV often reports its `media_player` as `unavailable`, even though the
box is reachable via its remote or a sibling entity. `resolve_composite_power`
fixes the effective on/reachable state:

- While the **primary is reachable**, it is authoritative (its `power` is the rich
  media state) — nothing overrides it.
- When the **primary is unreachable**, fall back to members
  (companions + paired remote): `reachable` if **any** member is, `on` if any
  **reachable** member reports on.
- With **no reachable member**, leave it as-is (truly offline) — the client still
  offers a Wake-on-LAN power-on, which can reach a fully-down NIC that a live read
  can't.

Members are modelled as an interchangeable `PowerSignal { reachable, on }`. **This
is the single seam a new TV control surface plugs into** — a native Bravia/Sony
power read joins the composite by contributing a `PowerSignal`, with no
special-casing in the resolver.

---

## Command routing (write path)

`apply_media_command` (`src/api/media.rs`) — the shared service fn behind
session / `/api/v1` / MCP — drives the composite. It loads the **backings**
(`load_composite_backings`: the primary first, then companions, each with its
capability flags, receiver-bound flag, current volume, now-playing flag, and a
priority) and routes per field via `route_across_backings`:

| Command field | Routes to |
|---|---|
| `volume` / `mute` | a **receiver-bound** backing (physical routing wins), else the backing **actually carrying audio** (non-zero volume), else the primary |
| `transport` | the backing **actually playing** (`now_playing`), else the highest-priority `transport`-capable backing, else the primary |
| `source` / app | the highest-priority `sources`-capable backing, else the primary |
| `power` | the **highest-priority** backing (a native TV over an HA copy) |

Each routed sub-command is then applied through `apply_with_receiver`, which does
*that backing's own* receiver split. A single-backing device (no companions)
skips routing and is driven directly.

### Priority — native wins (capability arbitration)

When more than one backing can do the same thing, the **highest-priority** one
wins. `backing_priority` ranks native single-domain providers (Bravia, Onkyo,
Sonos) **above** a multi-domain **Integration** copy (Home Assistant), mirroring
de-dup's "native wins" rule (`registry.ui_domain(type) == Integration` → `0`, else
`1`). So merging a native TV into an HA device lets the TV take precedence for
power/source/transport while the union of capabilities is still offered. Ties keep
primary-first order. Two **physical-routing overrides** beat priority: volume
follows a receiver binding, and transport follows the backing actually playing.

### Power-on fans out to the paired remote

For `power: true`, `apply_media_command` fires the media command **and** wakes the
paired remote **concurrently** (`wake_paired_remote` → `apply_remote_command`
`Power{on:true}`, which does a Wake-on-LAN nudge before the provider `turn_on`).
Concurrency matters: a standby `media_player` can hang to a timeout, and the WoL
nudge must not wait behind it. The command reports **success if either path
worked**. Power-**off** stays on the `media_player` only (same box; no WoL needed).

> General principle (applies to every composite action): **route each action to
> the capability that can actually perform it for the device's current state**,
> with sensible fallback ordering — never blindly send everything to one entity.
> Power-on → WoL + remote/`turn_on`; volume → bound receiver; transport →
> the player that's playing; app-launch → remote; now-playing → the player.

### Favorites, grouping, cast

These have no single owner either, so they resolve across the composite:

- **Favorites** — `capable_backing(id, |c| c.favorites)` picks the
  highest-priority backing advertising favorites (e.g. a Sonos companion merged
  into a TV), not necessarily the primary.
- **Grouping** — the `grouping` capability is unioned onto the primary so the
  control surfaces; the group/ungroup call itself targets the speaker row.
- **Cast** (`play_media`) — has no capability flag, so cast is **tried across all
  backings** (primary first) until one succeeds: a TV's native row may not cast
  while its HA companion does, and which row is primary must not matter.

---

## Invariants

Keep these true whenever you touch the composite code:

- **Mutual exclusion** — a row is never both `shadowed_by` and `companion_of`.
- **No chains** — a companion can't itself have companions, and you can't merge
  into a companion or a shadowed row. Enforced in `set_media_companion`
  (rejects self-merge, an unknown/companion/shadowed primary, and merging a row
  that already has companions).
- **Direction independence** — resolution always considers `device ∪ companions`
  (`m.id = ? OR m.companion_of = ?`, `COALESCE(companion_of, id)`), so the
  surfaced controls don't depend on which row was merged into which (0.27.2).
- **Companions are hidden, not deleted** — excluded from control and room
  membership (`companion_of IS NULL`), collapsed in the inventory, but still
  listed (marked) so they can be un-merged.
- **Overlay order is fixed** — merge companions → resolve power → overlay
  receiver volume.
- **One service layer** — composite assembly/routing lives in `api::media`
  service fns; session, `/api/v1`, and MCP all delegate there. Never fork a
  composite control path per surface.

---

## Frontend

- **Control:** `MediaEditor` / `DeviceControl` read the effective device. When
  `device.remote_id` is set they render the AIO TV control (Remote keypad + Apps,
  with now-playing) instead of fetching remotes separately.
- **Configuration (Devices page, `frontend/src/pages/Devices.tsx`):**
  - **Merge** — `MergePicker` lets you merge an media entity into another
    same-physical-device primary (`PUT …/media/devices/{id}/companion`
    `{primary_id}`). A merged entity collapses to a `MergedCompanion` row with an
    **Unmerge** action (`{primary_id: null}`).
  - **Composite diagnostic (dev mode only)** — `buildComposites` derives a
    read-only view of each composite and its members (Primary / Companion /
    Remote / Volume → receiver) for inspection. It's anchored on whatever media
    row is the surface, so new composite shapes show up automatically.

---

## API

Companion link (session + `v1`), shared service `set_media_companion`:

```
PUT /api/media/devices/{id}/companion        { "primary_id": "<id>" | null }
PUT /api/v1/media/devices/{id}/companion     { "primary_id": "<id>" | null }
```

Receiver binding: `PUT /api/{,v1/}media/devices/{id}/receiver`
(`set_media_receiver`). Remote pairing is **automatic** (`reconcile_remote_pairings`
after discovery), not a route. All composite behaviour is otherwise transparent —
reads of `…/media/devices` and commands to `…/media/devices/{id}/state` already
return/act on the effective device.

---

## Code map

| Concern | Location |
|---|---|
| Companion column | `media_devices.companion_of` |
| Receiver binding columns | `media_devices.receiver_id` / `receiver_source` |
| Remote pairing column | `remote_devices.paired_media_id` |
| Companion state merge | `merge_companion_into` — `src/api/media.rs` |
| Effective-device assembly | `list_all_devices`, `get_device_live` — `src/api/media.rs` |
| Power/reachability resolve | `PowerSignal`, `resolve_composite_power`, `apply_composite_power` — `src/api/media.rs` |
| Command routing | `apply_media_command`, `route_across_backings`, `load_composite_backings`, `capable_backing` — `src/api/media.rs` |
| Priority (native wins) | `backing_priority` — `src/api/media.rs` |
| Power-on remote fan-out | `wake_paired_remote` — `src/api/media.rs` |
| Cast across backings | `cast_to_device` / `cast_one` — `src/api/media.rs` |
| Set/clear companion | `set_media_companion` — `src/api/media.rs` |
| Remote pairing reconcile | `reconcile_remote_pairings` — `src/api/remote.rs` |
| Room exclusion | `effective_media_members` (`companion_of IS NULL`) — `src/api/rooms.rs` |
| Frontend merge UI + diagnostic | `MergePicker`, `MergedCompanion`, `buildComposites` — `frontend/src/pages/Devices.tsx` |

Debug logging is on the `bifrost::composite` target.

---

## Extending it

- **A new native TV surface (Bravia, …) joining a composite's power resolution:**
  contribute a `PowerSignal` — no resolver changes.
- **A new routable command field:** add a branch to `route_across_backings`
  choosing the owning backing by capability/physical routing, and union its
  capability flag in `merge_companion_into` so it surfaces.
- **A new "no single owner" action (like cast):** try it across
  `load_composite_backings` (primary first) rather than assuming the primary.

When you add or change any of these, **update this page and the
[Rooms & devices](rooms-and-devices.md) page** in the same change.
