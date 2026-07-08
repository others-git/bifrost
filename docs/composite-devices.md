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

- A composite is a **flat group of member rows** sharing one cross-domain
  `group_id` (on both `media_devices` and `remote_devices`) — **there is no stored
  primary**. Members can be:
  1. Several `media_device` rows for the same physical box (a TV's two
     `media_player`s, …).
  2. One or more `remote_device`s (D-pad / app launch).
  3. *(separate relationship)* a **bound receiver** that owns a source's
     volume/mute, via `receiver_id` / `receiver_source` — a *shared* device
     (many sources → one receiver), so it's referenced, not a group member.
- The group's representative — the **surface** — is **derived** at read time
  (highest authority, then kind, then smallest id), never stored, so it can't
  drift from the members. The API still exposes a derived `companion_of` on each
  non-surface member (= the surface's id) for client compatibility.
- The merged result is the **effective device**: the API returns the surface row
  with the other members' state/capabilities folded in, the receiver's volume
  overlaid, the richest paired remote's id attached, and power/reachability
  resolved across the whole group. Clients never assemble a composite themselves.
- **Reads merge; writes route.** Each command field (power / volume / transport /
  source / favorites / cast) goes to the member that owns it, not blindly to one
  entity.
- Resolution is **direction-independent** and **whole-group**: it considers every
  member sharing the `group_id`, so there's no "which row is primary" to get
  wrong. The **freshest/most-capable member wins**: power is on if any reachable
  view is, now-playing is the richest view, and the surfaced remote is the native
  (richest-catalogue) one.
- **Merging is a union** (`set_media_companion`): merging device A into B folds
  A's whole group into B's, so combining two composites of the same physical
  device (e.g. a TV that surfaced under two MACs) is one clean operation.

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

| | `shadowed_by` (de-dup) | `group_id` (composite) |
|---|---|---|
| Relationship | same device seen twice | complementary parts of one device |
| Secondary row | hidden **and discarded** | hidden, **state/caps overlaid onto primary** |
| Capabilities | only the native copy's | **union** of both |
| Set by | auto hw_id match (`reconcile_duplicates`) + manual | manual merge (Devices page) |

A row is **never both** shadowed and a companion.

---

## The three ingredients

### 1. Group members — `group_id`

`media_devices.group_id` (and `remote_devices.group_id`) holds the composite a
row belongs to. `NULL` = standalone. There's no stored primary; the
representative **surface** is derived by [`group_surfaces`] (authority, then kind,
then smallest id) and each non-surface member gets a derived `companion_of`
(= the surface id) for client compatibility.

On read, each companion's complementary state is overlaid onto its primary by
`merge_companion_into` (`src/api/media.rs`), filling only what the primary lacks
and **unioning** the capability flags:

- `now_playing` — the **richest** snapshot across the composite wins, scored by
  `now_playing_score` (a member actually playing — has a title/artist and an active
  `play_state` — beats one that's idle/stopped/empty, which beats `None`). So an
  idle `media_player` view never masks the companion (a Cast entity, the native TV)
  that knows what's on, irrespective of which row is the primary.
- `source`, `group_coordinator` — filled if the primary has none.
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
collapsed by `group_surfaces`, `src/api/rooms.rs`). They remain in the device list,
marked, so the Devices page can show/un-merge them.

A **disabled or shadowed member contributes nothing**: it neither merges state,
nor feeds a power signal, nor qualifies as a routing backing, nor can be elected
the surface — disabled means "out of control", and that includes a member's
stale cache steering the composite. It stays marked (`companion_of`) so the
inventory keeps it collapsed under the surface.

### 2. Paired remote — `remote_devices.group_id` → `remote_id`

A virtual remote (`remote_devices` — D-pad keys, text, app launch) **joins its
TV's group** when they share a hardware id: `remote_devices.group_id` is set
idempotently after discovery by `reconcile_remote_pairings` (`src/api/remote.rs`)
— for each *unpaired* remote with an `hw_id`, find a non-shadowed `media_device`
with the same `hw_id` (preferring `kind = 'tv'`), ensure it has a group, and join
it. `group_id IS NULL`-only means a manual merge is never clobbered by discovery.

On the media read path the **inverse** is surfaced: the effective device carries
`remote_id` (the paired, enabled remote's id), resolved against the **whole
composite** — every remote sharing the composite's `group_id`, via
`load_paired_remotes`.

A composite can carry **several** paired remotes for the same TV — e.g. a native
vendor remote (carrying the full IRCC/native catalogue) *and* an HA `remote.*`
copy (an empty catalogue). `best_remote_per_group` surfaces the
**highest-authority** one (native over Integration; ties break on the smaller id,
so the choice is stable), so the richer "Full remote" catalogue is **never masked
by a leaner integration copy regardless of which device was merged into which**.
Every paired remote's power signal still feeds the composite power resolution
(below) — only the *surfaced* one is deduplicated, not the signals.

The frontend reads `device.remote_id` directly to render the unified **AIO TV
control** (keypad + apps fly-out) with no separate remote lookup.

### 3. Bound receiver — `receiver_id` / `receiver_source`

A **source** device (TV / streamer / console) can be **bound to a receiver** that
owns its volume. Stored on the *source* (`media_devices.receiver_id` +
`receiver_source`); many sources → one receiver. Detail in
[Rooms & devices → receiver binding](rooms-and-devices.md). Within a composite:

- **Read:** the effective device shows the **receiver's** volume/mute (what the
  source's volume slider actually controls). Uses the receiver's *cached* state,
  not a live read — push-mode receivers (Onkyo) allow only one connection. That
  cache is kept honest by the provider's **liveness heartbeat** (the Onkyo link
  re-queries on a timer and reconnects a silently half-open socket), so a frozen
  link can't surface a stale bound-source volume.
- **Write:** `volume`/`mute` route to the receiver; `power`/`source`/`transport`
  stay on the source. The receiver **mirrors the source's power**: on wakes it
  and switches it to `receiver_source`, off takes it down too — the bound pair
  behaves as one appliance, and commands are absolute (never toggles) so a
  receiver already in the desired state is a no-op.
  (`MediaCommand::split_for_receiver` / `apply_with_receiver`.)
- **In a composite, the binding may live on a different member than power.**
  Power routes to the most authoritative member (the native TV), while the
  receiver binding often sits on its HA twin. The power mirror still fires:
  `apply_routed` mirrors composite power to a receiver bound to **any** member
  (after the source wakes, so the receiver selects an active input), skipping
  the case where the power member carries the binding itself (its own split
  already mirrors). The bound member is never sent its own power command — it's
  the same physical device the surface already drives.

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

A composite's power is genuinely ambiguous: one `media_player` view of a TV can
read `off`/`unavailable` (or go stale) while a sibling entity for the *same* box
(a Cast `media_player`, the native row) reads `on` and playing. A stale or lean
view must not mask the one that knows the truth — otherwise a playing TV shows
**off** and the Dashboard hides its now-playing. `resolve_composite_power` treats
`on` as a **positive, order-independent** signal over two tiers:

- **Media views** (primary + companion `media_player`s — all the same physical
  device) are weighed **symmetrically**: if **any reachable** media view reports
  `on`, the composite is `on`; `reachable` if any is. No view's `off` can veto a
  sibling's `on`, and the result never depends on which row is the primary.
- **Remotes are the standby-wake fallback** — used **only when no media view is
  reachable** (a cold TV whose `media_player` is `unavailable`): the paired
  remote's `(reachable, on)` then rescues the composite. A remote is **not**
  allowed to override a reachable media view, so a stale remote-`on` can't force a
  powered-off-but-reachable TV on.
- With **nothing reachable**, the primary is returned as-is (truly offline) — the
  client still offers a Wake-on-LAN power-on, which can reach a fully-down NIC that
  a live read can't.

Both tiers are modelled as interchangeable `PowerSignal { reachable, on }`
collections. **This is the single seam a new TV control surface plugs into** — a
native Bravia/Sony power read joins the composite by contributing a `PowerSignal`
to the media tier, with no special-casing in the resolver. `apply_composite_power`
then writes the resolved `(reachable, on)` back onto the effective device (it folds
the primary in, so a stale primary is corrected by a fresher member).

---

## Command routing (write path)

`apply_media_command` (`src/api/media.rs`) — the shared service fn behind
session / `/api/v1` / MCP — drives the composite. It loads the **backings**
(`load_composite_backings`: the primary first, then companions, each with its
capability flags, receiver-bound flag, current volume, now-playing flag, and a
priority — only **controllable** members qualify: a disabled/shadowed member
must not attract a command it would then refuse, failing the whole command)
and routes per field via `route_across_backings`:

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

When more than one backing can do the same thing, the **highest-authority** one
wins. `backing_authority` is a deliberately **binary** "native wins": a native
single-domain provider (Bravia, Onkyo, Sonos) outranks a multi-domain
**Integration** copy (Home Assistant) — `registry.ui_domain(type) == Integration`
→ `0`, else `1` — mirroring de-dup's rule. Within a composite the members are the
*same physical device* surfaced more than once, so native-vs-Integration is the
only authority distinction that carries information: every genuinely contested
control is decided either by this binary rank or by physical routing, so a finer
grade (receiver > TV API > …) would be false precision. So merging a native TV
into an HA device lets the TV take precedence for power/source/transport while the
union of capabilities is still offered. Ties keep primary-first order. The same `backing_authority`
also picks the surfaced remote (the **Paired remote** ingredient above). Two
**physical-routing overrides** beat authority: volume follows a receiver binding,
and transport follows the backing actually playing.

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

- **Mutual exclusion** — a row is never both `shadowed_by` (de-dup) and a member
  of a composite `group_id`. Shadowing discards; grouping merges. **Enforced**,
  not just conventional: `set_media_companion` rejects merging a shadowed row,
  and whenever a grouped media row gets shadowed (the de-dup reconciler or a
  manual shadow), `enforce_shadow_group_exclusion` migrates its group —
  companions *and* paired remotes — onto the canonical (shadowing) row before
  detaching it, so a composite is never stranded on a hidden row (the classic
  sequence: an HA TV carries the group + remote, then the native provider
  arrives and de-dup shadows the HA row).
- **Flat groups, no primary** — membership is a single `group_id`; there's no
  stored primary and no chains to reject. The surface (representative) is derived
  by [`group_surfaces`], so merging is symmetric and a **union**
  (`set_media_companion` folds one group into another; rejects only self-merge and
  a shadowed/unknown target).
- **Direction independence** — resolution considers every row sharing the
  `group_id`, so the surfaced controls never depend on how the group was assembled.
- **Members are hidden, not deleted** — a non-surface member is excluded from
  control and room membership (collapsed by `group_surfaces` in `rooms`/`voice`),
  but still listed (with a derived `companion_of`) so it can be un-merged.
- **Overlay order is fixed** — merge members → resolve power → overlay receiver
  volume.
- **One service layer** — composite assembly/routing lives in `api::media`
  service fns; session, `/api/v1`, and MCP all delegate there. Never fork a
  composite control path per surface.

---

## Frontend

- **Control:** `MediaEditor` / `DeviceControl` read the effective device. When
  `device.remote_id` is set they render the AIO TV control (Remote keypad + Apps,
  with now-playing) instead of fetching remotes separately. The **Full remote**
  panel (`ExpandedRemote`) lists the surfaced remote's native catalogue: pinned
  commands form an always-visible **favourites strip**, the rest live behind a
  height-capped, scrollable sheet (so a long catalogue never runs off-screen), and
  each button has a ★ to pin/unpin (`setRemoteCommandPin`). Pins persist per remote
  in `remote_command_pins`; the catalogue is fetched live and the pin flag overlaid
  server-side (`overlay_pins`), so favourites survive a remote's catalogue changing.
- **Configuration (Devices page, `frontend/src/pages/Devices.tsx`):**
  - **Merge** — `MergePicker` lets you merge an media entity into another
    same-physical-device primary (`PUT …/media/devices/{id}/companion`
    `{primary_id}`). A merged entity collapses to a `MergedCompanion` row with an
    **Unmerge** action (`{primary_id: null}`).
  - **Composite diagnostic (dev mode only)** — `buildComposites` derives a
    read-only view of each composite and its members (Primary / Companion /
    Remote / Volume → receiver) for inspection. It's anchored on whatever media
    row is the surface, so new composite shapes show up automatically. Each card
    also shows a **Control precedence** panel (`CompositeRouting`, fed by
    `GET /api/dev/media/{id}/routing`): which member device each control resolves
    to and why — computed from the real routing helpers, so it can't drift.

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
| Composite membership (cross-domain) | `media_devices.group_id`, `remote_devices.group_id` (mig 0049) |
| Derived surface (representative) | `group_surfaces`, `derive_companions` — `src/api/media.rs` |
| Receiver binding columns | `media_devices.receiver_id` / `receiver_source` |
| Remote pairing column | `remote_devices.group_id` (shared with the TV's media rows) |
| Companion state merge | `merge_companion_into`, `now_playing_score` — `src/api/media.rs` |
| Effective-device assembly | `list_all_devices`, `get_device_live` — `src/api/media.rs` |
| Power/reachability resolve | `PowerSignal`, `resolve_composite_power`, `apply_composite_power` — `src/api/media.rs` |
| Surfaced remote (native wins) | `load_paired_remotes`, `best_remote_per_group` (keyed by `group_id`) — `src/api/media.rs` |
| Command routing | `apply_media_command`, `route_across_backings`, `load_composite_backings`, `capable_backing` — `src/api/media.rs` |
| Per-field routing helpers (shared by write + diagnostic) | `pick_best`, `route_volume`/`route_transport`/`route_source`/`route_power` — `src/api/media.rs` |
| Precedence diagnostic (which member wins each control + why) | `composite_routing` → `GET /api/dev/media/{id}/routing` (dev-mode) — `src/api/media.rs`, `src/api/dev.rs` |
| Authority (native wins) | `backing_authority` — `src/api/media.rs` |
| Power-on remote fan-out | `wake_paired_remote` — `src/api/media.rs` |
| Cast across backings | `cast_to_device` / `cast_one` — `src/api/media.rs` |
| Set/clear companion | `set_media_companion` — `src/api/media.rs` |
| Shadow/group mutual exclusion (group migration) | `enforce_shadow_group_exclusion` — `src/api/media.rs`, run by `reconcile_duplicates` + `set_device_shadow` |
| Remote pairing reconcile | `reconcile_remote_pairings` — `src/api/remote.rs` |
| Remote command favourites | `set_command_pin`, `overlay_pins`, `remote_command_pins` table — `src/api/remote.rs` |
| Receiver-link liveness | `onkyo_link_actor` heartbeat (`HEARTBEAT`) — `src/providers/onkyo/mod.rs` |
| Room exclusion | `effective_media_members` (collapses each group to its surface) — `src/api/rooms.rs` |
| Frontend merge UI + diagnostic | `MergePicker`, `MergedCompanion`, `buildComposites` — `frontend/src/pages/Devices.tsx` |
| Frontend full-remote + favourites | `ExpandedRemote`, `CommandButton` — `frontend/src/components/BifrostRemote.tsx` |

Debug logging is on the `bifrost::composite` target.

---

## Extending it

- **A new native TV surface (Bravia, …) joining a composite's power resolution:**
  contribute a `PowerSignal` to the **media tier** (a companion `media_player`) —
  no resolver changes; "any reachable view on ⇒ on" already covers it.
- **A new routable command field:** add a branch to `route_across_backings`
  choosing the owning backing by capability/physical routing, and union its
  capability flag in `merge_companion_into` so it surfaces.
- **A new "no single owner" action (like cast):** try it across
  `load_composite_backings` (primary first) rather than assuming the primary.

When you add or change any of these, **update this page and the
[Rooms & devices](rooms-and-devices.md) page** in the same change.
