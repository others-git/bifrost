# Automations

Automations are Bifrost's "when this, then that" layer: a **trigger** fires on
a change in your home, optional **conditions** gate it, and one or more
**actions** run — server-side, off the same live device feeds the dashboards
use, so nothing needs a browser open.

Every rule reads as a sentence, and is built as one in a single editor:

> **When** *Office* **stays empty for** *15 min* — **only** *between 21:00 and
> 06:00* — **then** *Office off*.

Manage them on the **Automations** page (each rule grouped under its trigger),
or from a sensor's detail panel on the **Devices** page.

---

## Triggers

A trigger watches one **subject** and fires on a **transition** — never on a
level, so a sensor re-reporting "motion" can't re-run your actions, and a
restart never replays state that was already true.

### Rooms (occupancy)

The most powerful subject: a Room's **aggregate occupancy**, fed by every
presence sensor in the room (Hue motion, MotionAware areas, Home Assistant
occupancy — interchangeable). "Office empty for 15 minutes → off" is one rule
no matter how many sensors the room has, and a sensor added to the room later
joins the aggregate automatically.

Events: **becomes occupied** · **becomes empty** · **stays empty for…** ·
**stays occupied for…**

### Sensors

A single sensor's reading. Events adapt to what the sensor measures:

- **Motion / occupancy** — detects motion · clears · stays clear for… · stays
  detected for…
- **Contact** (door/window) — opens · closes · stays open for… ("the fridge
  door has been open ten minutes") · stays closed for…
- **Illuminance / temperature / humidity** — rises above · drops below a
  threshold. These fire on the **crossing**, not while the value sits past it.

### Devices (power)

A device's power state: **turns on** · **turns off** · **stays on for…** ·
**stays off for…** — for lights, switches, and TVs/speakers. The classic:

> **When** *BRAVIA* **turns on**, **then** *Hallway off*, *Living Room to 20%*.

Device triggers watch the same live feeds as the dashboards, so a change made
*outside* Bifrost (the TV's own remote, a physical switch) triggers too. A
smart TV that only answers when asked gets polled tightly **while a rule
watches it** — within seconds of the TV coming on — and isn't touched at all
once no rule does.

---

## Conditions ("only if")

Conditions are checked at the moment the trigger fires; all must hold.

- **Time window** — `21:00`–`06:00` style, server-local. Overnight windows
  wrap midnight, and an optional **weekday filter** applies to the day the
  window *starts* (a Saturday-night window still covers Sunday 2 am).
- **Another sensor's reading** — "only when the lux sensor is below 20",
  "only while the door sensor is closed". An unknown reading never satisfies
  a condition: a rule would rather skip than fire blind.
- **Another room's occupancy** — "only while the living room is empty".

---

## Actions ("then")

Each action drives the same shared control paths as the UI and voice:

- **A room** — on, off, or on with a brightness/colour. Room on/off is the
  *pure power* command: it also stops the room's speakers and switches; a
  brightness or colour clause touches only the lights.
- **A single light** — on, off, or on with a brightness/colour.
- **A power device** — on or off.
- **A scene** — apply any saved scene.

An action step reads as a sentence: a verb (**turn on**, **turn off**, or
**apply scene**), then its targets — pick several at once from one searchable,
room-grouped checklist. "Turn on" can chain **"and…" clauses** — *and set
brightness to 40%*, *and set colour to ▮* — which compose into **one command
per target**, not separate writes. Clauses only exist on "turn on": a light
won't change colour while off, so "set colour" without the power verb would
silently do nothing — the sentence always says it. Switches sharing a clause
step simply turn on. A rule can carry several steps; they run in order,
best-effort (one unreachable device doesn't stop the rest).

**Cooldown** ("don't re-run within N minutes") stops rapid re-fires — useful
for anything triggered by motion in a busy hallway.

**Put things back** — a timed hold: when the rule fires, Bifrost first
snapshots the current state of everything the actions touch (each light's full
colour/brightness, each switch's power — room and scene targets expand to
their member devices), applies the actions, and restores the snapshot after
the configured time. "Someone's at the door → porch to 100% — put things back
after 5 minutes" returns the porch to whatever it was, not just "off". A
re-fire during the hold extends the timer but keeps the original snapshot, and
holds survive a restart. When **two rules'** holds overlap on the same device,
the later one inherits the earlier capture — whichever timer runs last, the
device returns to its true pre-automation state, never to another rule's
output. (Speakers aren't resumed — playback isn't a state.)

**Manual override wins.** The hold exists to undo what the *automation* did —
so once anything else changes a held device (the Bifrost UI, the Hue app, a
wall switch: they all arrive on the same live push streams), that device is
released from the hold and keeps your change; the rest of the snapshot still
restores on time. "Motion turned the hall on at 30%, I bumped it to full" —
the hall stays at full, it doesn't snap off ten minutes later. Detection is
tolerant of device-side rounding and ignores reachability blips, so only a
genuine change counts as yours.

---

## Running, testing, and pausing

Every rule row has:

- **Run** (▶) — execute the actions right now, skipping the trigger and
  conditions. The way to test a rule without walking past a sensor.
- An **enable switch** — pause a rule without deleting it.
- **Duplicate** — copies the rule *disabled*, ready to retune.
- A **last ran** readout ("ran 5m ago").

Rule edits take effect immediately — the engine re-reads rules per event, and
a rule created mid-flight baselines its subject at creation, so the very next
transition fires it.

---

## Debugging: why didn't it fire?

Enable **developer mode** (Settings → Developer) and open the **event log**:
it streams the server's own records live — every device state push, every
rule fire, and every *skip reason* (in cooldown, condition not met), plus the
voice pipeline and discovery. Filter to **Automations** or **Device state**,
act on the device, and watch the decision happen.

---

## Worked examples

| Goal | Rule |
|---|---|
| Hall light on motion, at night only | When *Hall motion* **detects motion** · only *21:00–06:30* · then *Hallway on at 30%* |
| Room off when everyone leaves | When *Office* **stays empty for 15 min** · then *Office off* |
| Movie lighting when the TV wakes | When *BRAVIA* **turns on** · then *Living Room to 20%*, *Hallway off* |
| Fridge door alarm | When *Fridge contact* **stays open for 5 min** · then *Kitchen light on* (blink your attention) |
| Dark-day lamp | When *Office lux* **drops below 15** · only *Mon–Fri* *08:00–17:00* · then *Desk lamp on* |
| Doorbell spotlight | When *Porch motion* **detects motion** · then *Porch at 100%* · **put things back after 5 min** |

---

## Notes

- Triggers are **edge-only**: a rule fires when the state *changes* into place,
  not continuously while it holds.
- "Stays … for" timers cancel the moment the state breaks (motion cancels an
  "empty for 15 min" countdown) and survive restarts.
- A Room trigger needs at least one presence sensor in the room — Bifrost
  refuses to save a rule that could never fire.
- Automations are configured per hub and run entirely locally.
