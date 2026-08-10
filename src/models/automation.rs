//! Automations — the "when this, then that" layer. An automation's **trigger
//! input** is a tagged enum ([`AutomationTrigger`]) so new input kinds can
//! join without reshaping storage: a sensor event, a Room's occupancy, a
//! device's power, a manual macro, or a painted 24-hour **schedule**
//! ([`AutomationTrigger::Schedule`] — the kiosk display plan's timeline,
//! shared here as [`PlanMode`]). Event inputs are edge-triggered: they fire on
//! a state *transition* (motion appearing, a value crossing a threshold),
//! never on a level, so they can't re-fire every report — and the schedule
//! evaluator detects the plan's own transitions, keeping that property.
//! Conditions gate the fire (time window, other sensors' current readings);
//! actions replay through the **shared service layer** (rooms / lights / power
//! / scenes) — the same fns behind session, `/api/v1`, and MCP — never a
//! parallel control path.

use super::sensor::SensorReading;
use super::{LightState, is_clear_effect};
use serde::{Deserialize, Serialize};

/// What a rule listens for on its sensor. Boolean triggers suit motion /
/// occupancy / contact; threshold triggers suit numeric sensors (lux, °C, %).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SensorTrigger {
    /// The reading flipped to `true` (motion detected, contact opened).
    BecameTrue,
    /// The reading flipped to `false` (motion cleared, contact closed).
    BecameFalse,
    /// The reading has stayed `false` for `secs` — the "no motion for N
    /// minutes" trigger. Armed by the falling edge, fired by the engine's
    /// clock, cancelled by any `true` in between.
    ClearFor { secs: u32 },
    /// The reading has stayed `true` for `secs` — the mirror of [`Self::ClearFor`]
    /// ("door left open ten minutes", "occupied for an hour"). Same timer
    /// machinery, watching the opposite state.
    HeldFor { secs: u32 },
    /// The numeric reading crossed **up** through `value`.
    RoseAbove { value: f64 },
    /// The numeric reading crossed **down** through `value`.
    DroppedBelow { value: f64 },
}

impl SensorTrigger {
    /// Whether the `prev → now` transition fires this trigger. Edge-triggered:
    /// an unknown `prev` (first observation after startup) never fires, so a
    /// restart can't replay actions for a state that was already true.
    /// The stay triggers (`ClearFor`/`HeldFor`) never fire here — an edge only
    /// *arms* their timer ([`Self::arms_stay_timer`]); the engine's clock fires it.
    pub fn fires(&self, prev: Option<SensorReading>, now: SensorReading) -> bool {
        match self {
            SensorTrigger::BecameTrue => {
                now.as_bool() == Some(true) && prev.and_then(SensorReading::as_bool) == Some(false)
            }
            SensorTrigger::BecameFalse => {
                now.as_bool() == Some(false) && prev.and_then(SensorReading::as_bool) == Some(true)
            }
            SensorTrigger::ClearFor { .. } | SensorTrigger::HeldFor { .. } => false,
            SensorTrigger::RoseAbove { value } => {
                now.as_number().is_some_and(|n| n > *value)
                    && prev
                        .and_then(SensorReading::as_number)
                        .is_some_and(|p| p <= *value)
            }
            SensorTrigger::DroppedBelow { value } => {
                now.as_number().is_some_and(|n| n < *value)
                    && prev
                        .and_then(SensorReading::as_number)
                        .is_some_and(|p| p >= *value)
            }
        }
    }

    /// The stayed-state timer this trigger describes: the boolean state it
    /// watches and how long the reading must hold it. `ClearFor` watches
    /// `false`, `HeldFor` watches `true`; edge triggers have none.
    pub fn stay_watch(&self) -> Option<(bool, u32)> {
        match self {
            SensorTrigger::ClearFor { secs } => Some((false, *secs)),
            SensorTrigger::HeldFor { secs } => Some((true, *secs)),
            _ => None,
        }
    }

    /// Whether this transition arms the stay timer: the reading just entered
    /// the watched state (or, with unknown `prev`, is first observed in it).
    /// Unlike [`Self::fires`], an unknown `prev` **does** arm: after a restart
    /// with the room already empty, "off after N minutes empty" should still
    /// eventually run (the actions are idempotent). Returns the watched state
    /// and the wait.
    pub fn arms_stay_timer(
        &self,
        prev: Option<SensorReading>,
        now: SensorReading,
    ) -> Option<(bool, u32)> {
        let (watched, secs) = self.stay_watch()?;
        (now.as_bool() == Some(watched) && prev.and_then(SensorReading::as_bool) != Some(watched))
            .then_some((watched, secs))
    }
}

/// An optional gate checked at fire time (not at arm time).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleCondition {
    /// Only fire between `start` and `end` (`"HH:MM"`, server-local). An
    /// overnight window (`start > end`, e.g. 21:00–06:00) wraps midnight.
    /// `days` limits it to weekdays (ISO: 0 = Monday … 6 = Sunday); `None` or
    /// empty = every day. An overnight window belongs to the day it **starts**.
    TimeWindow {
        start: String,
        end: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        days: Option<Vec<u8>>,
    },
    /// The Room's aggregate occupancy currently reads `occupied` — the room
    /// analog of [`Self::SensorIs`] ("only while the office is occupied").
    RoomIs { room_id: String, occupied: bool },
    /// Another sensor's current numeric reading is above `value`.
    SensorAbove { sensor_id: String, value: f64 },
    /// Another sensor's current numeric reading is below `value` (the classic
    /// "only when dark" gate on a lux sensor).
    SensorBelow { sensor_id: String, value: f64 },
    /// Another boolean sensor currently reads `on`.
    SensorIs { sensor_id: String, on: bool },
    /// A device's power boolean currently reads `on` — the gate analog of the
    /// device trigger. With `on: false` it's the natural **"unless"** clause:
    /// "…unless the TV is on" gates the rule on the TV being off.
    DeviceIs {
        domain: TriggerDeviceDomain,
        device_id: String,
        on: bool,
    },
}

/// One hour's mode in a painted 24-hour plan — the shared vocabulary of the
/// kiosk display plan and the automation schedule trigger: **On** (forced
/// active: screen awake / actions applied), **Off** (forced inactive: screen
/// asleep / things put back — beats an occupied room), **Aware** (follows a
/// room's presence verdict).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PlanMode {
    On,
    Off,
    Aware,
}

/// The plan's mode for a local hour. `None` for a malformed plan or an
/// out-of-range hour — callers fall back to whatever legacy policy they have.
pub fn plan_mode(hour_modes: &str, hour: usize) -> Option<PlanMode> {
    match hour_modes.as_bytes().get(hour)? {
        b'W' => Some(PlanMode::On),
        b'S' => Some(PlanMode::Off),
        b'A' => Some(PlanMode::Aware),
        _ => None,
    }
}

/// The desired active state for a plan hour. On/Off are absolute; an Aware
/// hour follows the room's presence verdict and governs nothing when the room
/// has no presence input (`None` — leave things alone).
pub fn plan_desired(mode: PlanMode, present: Option<bool>) -> Option<bool> {
    match mode {
        PlanMode::On => Some(true),
        PlanMode::Off => Some(false),
        PlanMode::Aware => present,
    }
}

/// Whether a stored plan is exactly 24 mode characters ('W'/'S'/'A') — the
/// shared write-side validation for `kiosks.hour_modes` and the schedule
/// trigger's `hour_modes`.
pub fn is_valid_hour_plan(hour_modes: &str) -> bool {
    hour_modes.len() == 24 && hour_modes.bytes().all(|b| matches!(b, b'W' | b'S' | b'A'))
}

/// Parse `"HH:MM"` into minutes-since-midnight (0..=1439). Tolerant of
/// surrounding whitespace; `None` for malformed or out-of-range input.
/// The one shared clock parser — kiosk schedules use it too.
pub fn parse_hhmm(s: &str) -> Option<u16> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u16 = h.parse().ok()?;
    let m: u16 = m.parse().ok()?;
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Whether `now_min` (minutes since midnight) falls inside the window,
/// wrapping midnight when `start > end`. An equal start/end means "always"
/// (a zero-length window would be useless, and the UI can't express it).
pub fn time_window_contains(start: u16, end: u16, now_min: u16) -> bool {
    use std::cmp::Ordering;
    match start.cmp(&end) {
        Ordering::Equal => true,
        Ordering::Less => (start..end).contains(&now_min),
        Ordering::Greater => now_min >= start || now_min < end,
    }
}

impl RuleCondition {
    /// Evaluate against the clock (`now_min` minutes past midnight, `now_day`
    /// ISO weekday 0 = Monday) and lookups for the other sensors' cached
    /// readings and Rooms' occupancy. A condition whose sensor/room has no
    /// reading is **false** (fail closed — better to skip an automation than
    /// fire it blind); a malformed time window is **true** (fail open — don't
    /// let a typo silently disable the whole rule).
    pub fn holds(
        &self,
        now_min: u16,
        now_day: u8,
        reading_of: impl Fn(&str) -> Option<SensorReading>,
        occupancy_of: impl Fn(&str) -> Option<bool>,
        device_on_of: impl Fn(TriggerDeviceDomain, &str) -> Option<bool>,
    ) -> bool {
        match self {
            RuleCondition::TimeWindow { start, end, days } => {
                match (parse_hhmm(start), parse_hhmm(end)) {
                    (Some(s), Some(e)) => {
                        // An overnight window past midnight still belongs to the
                        // weekday it started on.
                        let started_yesterday = s > e && now_min < e;
                        let effective_day = if started_yesterday {
                            (now_day + 6) % 7
                        } else {
                            now_day
                        };
                        let day_ok = days
                            .as_ref()
                            .is_none_or(|d| d.is_empty() || d.contains(&effective_day));
                        day_ok && time_window_contains(s, e, now_min)
                    }
                    _ => true,
                }
            }
            RuleCondition::RoomIs { room_id, occupied } => {
                occupancy_of(room_id).is_some_and(|o| o == *occupied)
            }
            RuleCondition::SensorAbove { sensor_id, value } => reading_of(sensor_id)
                .and_then(SensorReading::as_number)
                .is_some_and(|n| n > *value),
            RuleCondition::SensorBelow { sensor_id, value } => reading_of(sensor_id)
                .and_then(SensorReading::as_number)
                .is_some_and(|n| n < *value),
            RuleCondition::SensorIs { sensor_id, on } => reading_of(sensor_id)
                .and_then(SensorReading::as_bool)
                .is_some_and(|b| b == *on),
            RuleCondition::DeviceIs {
                domain,
                device_id,
                on,
            } => device_on_of(*domain, device_id).is_some_and(|b| b == *on),
        }
    }
}

/// One thing a rule does when it fires. Every variant maps to a shared
/// service-layer fn — a rule is just another caller, like session/v1/MCP.
/// (No `PartialEq`: `LightState` deliberately isn't comparable.)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleAction {
    /// Drive a whole room (`apply_room_state`): `{on:false}` is a pure power
    /// command fanning to lights + switches + speakers; a state with light
    /// attributes (brightness, colour) touches only the lights.
    Room { room_id: String, state: LightState },
    /// Drive one light (`apply_light_state`).
    Light { light_id: String, state: LightState },
    /// Switch one power device (`apply_power_state`).
    Power { device_id: String, on: bool },
    /// Apply a scene (`apply_scene_entries`).
    Scene { scene_id: String },
    /// Launch an app on a TV/streamer's remote (`apply_remote_command` with
    /// `LaunchApp` — the same shared path the remote UI and voice use, so
    /// recents recording and per-vendor launch routing come along free). `app`
    /// is whatever the remote's catalog launches with: a vendor launch URI, a
    /// bare package (the Android TV adapter wraps it), or a deep link.
    App { remote_id: String, app: String },
    /// **Toggle** one device's power — read its cached on-state and apply the
    /// inverse through the domain's shared apply fn. Unlike the absolute
    /// `Light`/`Power` actions this is *relative*, which is what a physical
    /// macro button wants ("press = flip"). A device whose state is unknown is
    /// skipped rather than blind-guessed.
    Toggle {
        domain: TriggerDeviceDomain,
        device_id: String,
    },
}

impl RuleAction {
    /// Normalize a stored action: clear-effect tokens in embedded light states
    /// become `None`, mirroring what the light service does on write.
    pub fn normalized(mut self) -> Self {
        if let RuleAction::Room { state, .. } | RuleAction::Light { state, .. } = &mut self
            && state.effect.as_deref().is_some_and(is_clear_effect)
        {
            state.effect = None;
        }
        self
    }
}

/// Which device table a device trigger watches. The boolean each domain
/// contributes: a light's `on`, a media device's `power`, a power device's `on`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerDeviceDomain {
    Light,
    Media,
    Power,
}

/// What starts an automation — the **trigger input**, tagged so more input
/// kinds (schedules, device state, …) can be added without a schema change.
/// Today: a sensor event, or a Room's aggregate occupancy making the same
/// boolean transitions (provider-agnostic — every presence sensor in the room
/// feeds it, so "room empty for 15 minutes" is one rule however many sensors
/// the room has).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationTrigger {
    /// A sensor's reading makes the given edge transition.
    Sensor {
        sensor_id: String,
        event: SensorTrigger,
    },
    /// A Room's occupancy (see `rooms::room_occupancy`) makes the given
    /// **boolean** transition — occupied (`became_true`), empty
    /// (`became_false`), stays empty (`clear_for`), stays occupied (`held_for`).
    Room {
        room_id: String,
        event: SensorTrigger,
    },
    /// A device's power makes the given **boolean** transition — "the TV turns
    /// on", "the fan has been on for an hour". Watches the same push streams
    /// that keep the UI live, so a change made on the device itself (the TV's
    /// own remote) triggers too.
    Device {
        domain: TriggerDeviceDomain,
        device_id: String,
        event: SensorTrigger,
    },
    /// No event input at all — a **macro**: the rule only runs on demand
    /// (`POST /api/automations/{id}/run` — an AIO board button, voice, MCP).
    /// The engine never event-fires it, and `run` skips conditions like any
    /// hand-run rule, so a manual rule is purely "a named list of actions".
    Manual {},
    /// A **timer**: the kiosk display plan's paintable 24-hour timeline
    /// ([`plan_mode`]/[`plan_desired`]), restricted to On/Off hours (no
    /// Aware) and to **pure power** actions ([`is_power_only`]) over rooms,
    /// lights, and switches. An On hour beginning powers the targets on — a
    /// power-only write, so a light comes back in whatever colour it last
    /// wore — and an Off hour beginning powers the same targets off. The
    /// engine detects the plan's own transitions, so this still fires like an
    /// edge — never re-applies every tick, and a manual change mid-hour
    /// sticks until the next boundary.
    Schedule { hour_modes: String },
}

impl AutomationTrigger {
    /// The sensor this trigger listens to, if it's a sensor input — the
    /// denormalized `automations.sensor_id` lookup column (`None` for other
    /// input kinds, which the engine matches in code).
    pub fn sensor_id(&self) -> Option<&str> {
        match self {
            AutomationTrigger::Sensor { sensor_id, .. } => Some(sensor_id),
            _ => None,
        }
    }

    /// The room this trigger listens to, if it's a room-occupancy input.
    pub fn room_id(&self) -> Option<&str> {
        match self {
            AutomationTrigger::Room { room_id, .. } => Some(room_id),
            _ => None,
        }
    }

    /// The device this trigger watches, if it's a device-state input.
    pub fn device(&self) -> Option<(TriggerDeviceDomain, &str)> {
        match self {
            AutomationTrigger::Device {
                domain, device_id, ..
            } => Some((*domain, device_id)),
            _ => None,
        }
    }

    /// The transition event — `None` for a manual (macro) rule or a schedule,
    /// which have no event input to match.
    pub fn event(&self) -> Option<&SensorTrigger> {
        match self {
            AutomationTrigger::Sensor { event, .. }
            | AutomationTrigger::Room { event, .. }
            | AutomationTrigger::Device { event, .. } => Some(event),
            AutomationTrigger::Manual {} | AutomationTrigger::Schedule { .. } => None,
        }
    }

    /// The painted plan, if this is a schedule (timer) input.
    pub fn schedule(&self) -> Option<&str> {
        match self {
            AutomationTrigger::Schedule { hour_modes } => Some(hour_modes),
            _ => None,
        }
    }
}

/// Whether an embedded light state is a **pure power** command — no
/// brightness/colour/temperature/effect clause. All a schedule (timer) rule's
/// room/light actions may carry: the timer only switches power, so a light
/// keeps (and comes back in) whatever look it last had.
pub fn is_power_only(state: &LightState) -> bool {
    state.brightness.is_none()
        && state.color.is_none()
        && state.color_temp_mirek.is_none()
        && state.effect.is_none()
}

/// One device's pre-fire state, captured for a timed hold ("put things back
/// after N minutes"). Restores replay through the same shared service fns as
/// actions. Media members aren't captured — resuming playback is a different
/// problem than re-applying a state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RestoreEntry {
    Light { light_id: String, state: LightState },
    Power { device_id: String, on: bool },
}

/// One **step** of a rule's "then": a list of actions, optionally gated by the
/// step's own conditions. This is what lets a single rule branch — "dim the
/// lights always, but only open Hulu after 18:00" is two steps, the second
/// carrying a time-window condition; two steps with opposite conditions on one
/// trigger are an if/else. A step with empty `conditions` always runs (given
/// the rule-level gate passed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionStep {
    /// This step's own gate, checked at fire time (in addition to the
    /// rule-level conditions). Empty = the step always runs.
    #[serde(default)]
    pub conditions: Vec<RuleCondition>,
    pub actions: Vec<RuleAction>,
}

/// A stored automation, as served to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Automation {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub trigger: AutomationTrigger,
    /// Rule-level gate: checked once at fire time; if it fails, nothing runs.
    /// Per-step conditions gate individual steps on top of this.
    pub conditions: Vec<RuleCondition>,
    /// The "then": one or more steps, each optionally condition-gated. (Stored
    /// in the `actions_json` column, whose content is now a step list.)
    pub steps: Vec<ActionStep>,
    pub cooldown_secs: u32,
    /// Timed hold: put everything the actions touched back to its pre-fire
    /// state after this many seconds. `None` = the changes stick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_secs: Option<u32>,
    /// When the automation last ran (`datetime('now')` UTC), for the UI readout.
    pub last_fired_at: Option<String>,
}

impl Automation {
    /// Every action across all steps, ignoring step conditions — for the
    /// restore snapshot (over-capturing a device a failing-condition step
    /// won't touch is a harmless no-op restore).
    pub fn all_actions(&self) -> impl Iterator<Item = &RuleAction> {
        self.steps.iter().flat_map(|s| s.actions.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(v: bool) -> SensorReading {
        SensorReading::Bool(v)
    }
    fn n(v: f64) -> SensorReading {
        SensorReading::Number(v)
    }

    #[test]
    fn became_true_fires_only_on_the_rising_edge() {
        let t = SensorTrigger::BecameTrue;
        assert!(t.fires(Some(b(false)), b(true)));
        assert!(!t.fires(Some(b(true)), b(true))); // level, not edge
        assert!(!t.fires(None, b(true))); // unknown prev: no startup replay
        assert!(!t.fires(Some(b(false)), b(false)));
        assert!(!t.fires(Some(n(1.0)), b(true))); // numeric prev is not "false"
    }

    #[test]
    fn became_false_fires_only_on_the_falling_edge() {
        let t = SensorTrigger::BecameFalse;
        assert!(t.fires(Some(b(true)), b(false)));
        assert!(!t.fires(Some(b(false)), b(false)));
        assert!(!t.fires(None, b(false)));
    }

    #[test]
    fn threshold_triggers_fire_on_the_crossing_not_the_level() {
        let up = SensorTrigger::RoseAbove { value: 25.0 };
        assert!(up.fires(Some(n(24.0)), n(26.0)));
        assert!(up.fires(Some(n(25.0)), n(25.1))); // from exactly-at counts as below
        assert!(!up.fires(Some(n(26.0)), n(27.0))); // already above
        assert!(!up.fires(None, n(30.0)));
        let down = SensorTrigger::DroppedBelow { value: 20.0 };
        assert!(down.fires(Some(n(21.0)), n(19.0)));
        assert!(!down.fires(Some(n(19.0)), n(18.0)));
    }

    #[test]
    fn stay_triggers_arm_on_entering_their_watched_state() {
        let clear = SensorTrigger::ClearFor { secs: 600 };
        assert_eq!(
            clear.arms_stay_timer(Some(b(true)), b(false)),
            Some((false, 600))
        );
        assert_eq!(clear.arms_stay_timer(None, b(false)), Some((false, 600))); // startup seed arms
        assert_eq!(clear.arms_stay_timer(Some(b(false)), b(false)), None); // already armed
        assert_eq!(clear.arms_stay_timer(Some(b(false)), b(true)), None);
        assert!(!clear.fires(Some(b(true)), b(false))); // never fires on the edge itself

        // HeldFor is the exact mirror: it watches `true`.
        let held = SensorTrigger::HeldFor { secs: 300 };
        assert_eq!(
            held.arms_stay_timer(Some(b(false)), b(true)),
            Some((true, 300))
        );
        assert_eq!(held.arms_stay_timer(None, b(true)), Some((true, 300)));
        assert_eq!(held.arms_stay_timer(Some(b(true)), b(true)), None);
        assert_eq!(held.arms_stay_timer(Some(b(true)), b(false)), None);
        assert!(!held.fires(Some(b(false)), b(true)));

        // Edge triggers describe no stay timer.
        assert_eq!(SensorTrigger::BecameTrue.stay_watch(), None);
    }

    #[test]
    fn time_window_handles_day_overnight_and_always() {
        // Daytime window 08:00–17:00.
        assert!(time_window_contains(480, 1020, 600));
        assert!(!time_window_contains(480, 1020, 1200));
        // Overnight window 21:00–06:00 wraps midnight.
        assert!(time_window_contains(1260, 360, 1380)); // 23:00
        assert!(time_window_contains(1260, 360, 120)); // 02:00
        assert!(!time_window_contains(1260, 360, 720)); // noon
        // Equal start/end = always.
        assert!(time_window_contains(300, 300, 0));
    }

    #[test]
    fn parse_hhmm_accepts_valid_and_rejects_garbage() {
        assert_eq!(parse_hhmm("00:00"), Some(0));
        assert_eq!(parse_hhmm("06:30"), Some(390));
        assert_eq!(parse_hhmm("23:59"), Some(1439));
        assert_eq!(parse_hhmm(" 9:05 "), Some(545)); // trimmed, single-digit hour
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);
        assert_eq!(parse_hhmm("6"), None);
        assert_eq!(parse_hhmm("aa:bb"), None);
        assert_eq!(parse_hhmm(""), None);
    }

    #[test]
    fn conditions_fail_closed_on_missing_sensor_readings() {
        let cond = RuleCondition::SensorBelow {
            sensor_id: "lux".into(),
            value: 20.0,
        };
        assert!(cond.holds(0, 0, |_| Some(n(5.0)), |_| None, |_, _| None));
        assert!(!cond.holds(0, 0, |_| Some(n(50.0)), |_| None, |_, _| None));
        assert!(!cond.holds(0, 0, |_| None, |_| None, |_, _| None)); // unknown reading never satisfies
        let is = RuleCondition::SensorIs {
            sensor_id: "door".into(),
            on: true,
        };
        assert!(is.holds(0, 0, |_| Some(b(true)), |_| None, |_, _| None));
        assert!(!is.holds(0, 0, |_| Some(n(3.0)), |_| None, |_, _| None)); // numeric is not a boolean
    }

    #[test]
    fn room_is_condition_reads_occupancy_and_fails_closed() {
        let cond = RuleCondition::RoomIs {
            room_id: "r1".into(),
            occupied: false,
        };
        assert!(cond.holds(0, 0, |_| None, |_| Some(false), |_, _| None));
        assert!(!cond.holds(0, 0, |_| None, |_| Some(true), |_, _| None));
        assert!(!cond.holds(0, 0, |_| None, |_| None, |_, _| None)); // no presence sensors → unknown
    }

    #[test]
    fn device_is_gates_on_the_power_boolean_and_fails_closed() {
        let unless_tv_on = RuleCondition::DeviceIs {
            domain: TriggerDeviceDomain::Media,
            device_id: "tv1".into(),
            on: false,
        };
        // "…unless the TV is on": holds while the TV is off…
        assert!(unless_tv_on.holds(0, 0, |_| None, |_| None, |_, _| Some(false)));
        // …blocks while it's on…
        assert!(!unless_tv_on.holds(0, 0, |_| None, |_| None, |_, _| Some(true)));
        // …and an unknown device fails closed.
        assert!(!unless_tv_on.holds(0, 0, |_| None, |_| None, |_, _| None));

        let only_if_on = RuleCondition::DeviceIs {
            domain: TriggerDeviceDomain::Light,
            device_id: "l1".into(),
            on: true,
        };
        assert!(only_if_on.holds(0, 0, |_| None, |_| None, |_, _| Some(true)));
        assert!(!only_if_on.holds(0, 0, |_| None, |_| None, |_, _| Some(false)));
    }

    #[test]
    fn device_is_roundtrips_serde() {
        let c = RuleCondition::DeviceIs {
            domain: TriggerDeviceDomain::Media,
            device_id: "tv1".into(),
            on: false,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("device_is"), "{json}");
        let back: RuleCondition = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(back, RuleCondition::DeviceIs { ref device_id, on: false, domain: TriggerDeviceDomain::Media } if device_id == "tv1")
        );
    }

    #[test]
    fn malformed_time_window_fails_open() {
        let cond = RuleCondition::TimeWindow {
            start: "9am".into(),
            end: "17:00".into(),
            days: None,
        };
        assert!(cond.holds(0, 0, |_| None, |_| None, |_, _| None));
    }

    #[test]
    fn time_window_weekday_filter_tracks_the_starting_day_overnight() {
        let weekend_evening = RuleCondition::TimeWindow {
            start: "21:00".into(),
            end: "06:00".into(),
            days: Some(vec![5, 6]), // Sat, Sun
        };
        // Saturday 23:00 — inside.
        assert!(weekend_evening.holds(23 * 60, 5, |_| None, |_| None, |_, _| None));
        // Sunday 02:00 — still Saturday's window (it started yesterday).
        assert!(weekend_evening.holds(2 * 60, 6, |_| None, |_| None, |_, _| None));
        // Monday 02:00 — Sunday's window, still allowed (Sunday is listed).
        assert!(weekend_evening.holds(2 * 60, 0, |_| None, |_| None, |_, _| None));
        // Wednesday 23:00 — right time, wrong day.
        assert!(!weekend_evening.holds(23 * 60, 2, |_| None, |_| None, |_, _| None));
        // An empty day list means every day.
        let daily = RuleCondition::TimeWindow {
            start: "08:00".into(),
            end: "17:00".into(),
            days: Some(vec![]),
        };
        assert!(daily.holds(9 * 60, 3, |_| None, |_| None, |_, _| None));
    }

    #[test]
    fn automation_trigger_tags_the_input_kind() {
        let t: AutomationTrigger = serde_json::from_str(
            r#"{"kind":"sensor","sensor_id":"s1","event":{"kind":"became_true"}}"#,
        )
        .unwrap();
        assert_eq!(t.sensor_id(), Some("s1"));
        assert_eq!(t.room_id(), None);
        assert_eq!(t.event(), Some(&SensorTrigger::BecameTrue));
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains(r#""kind":"sensor""#));

        let t: AutomationTrigger = serde_json::from_str(
            r#"{"kind":"room","room_id":"r1","event":{"kind":"clear_for","secs":900}}"#,
        )
        .unwrap();
        assert_eq!(t.sensor_id(), None); // room rules have no sensor lookup key
        assert_eq!(t.room_id(), Some("r1"));
        assert_eq!(t.event().and_then(|e| e.stay_watch()), Some((false, 900)));

        let t: AutomationTrigger = serde_json::from_str(
            r#"{"kind":"device","domain":"media","device_id":"tv1","event":{"kind":"became_true"}}"#,
        )
        .unwrap();
        assert_eq!(t.sensor_id(), None);
        assert_eq!(t.device(), Some((TriggerDeviceDomain::Media, "tv1")));
        assert_eq!(t.event(), Some(&SensorTrigger::BecameTrue));
        assert!(
            serde_json::to_string(&t)
                .unwrap()
                .contains(r#""domain":"media""#)
        );
    }

    #[test]
    fn trigger_and_action_json_round_trips_snake_case_tags() {
        let t: SensorTrigger = serde_json::from_str(r#"{"kind":"clear_for","secs":300}"#).unwrap();
        assert_eq!(t, SensorTrigger::ClearFor { secs: 300 });
        let a: RuleAction =
            serde_json::from_str(r#"{"kind":"power","device_id":"p1","on":false}"#).unwrap();
        assert!(matches!(a, RuleAction::Power { ref device_id, on: false } if device_id == "p1"));
        let s = serde_json::to_string(&SensorTrigger::RoseAbove { value: 25.5 }).unwrap();
        assert!(s.contains(r#""kind":"rose_above""#));
    }

    #[test]
    fn plan_mode_reads_the_painted_hour_and_rejects_garbage() {
        let plan = format!("{}{}{}", "S".repeat(8), "A".repeat(10), "W".repeat(6));
        assert_eq!(plan_mode(&plan, 0), Some(PlanMode::Off));
        assert_eq!(plan_mode(&plan, 8), Some(PlanMode::Aware));
        assert_eq!(plan_mode(&plan, 23), Some(PlanMode::On));
        assert_eq!(plan_mode(&plan, 24), None); // out of range
        assert_eq!(plan_mode("XXXX", 1), None); // not a mode char
        assert_eq!(plan_mode("", 0), None);
    }

    #[test]
    fn plan_desired_is_absolute_except_aware_which_follows_presence() {
        assert_eq!(plan_desired(PlanMode::On, None), Some(true));
        assert_eq!(plan_desired(PlanMode::On, Some(false)), Some(true));
        assert_eq!(plan_desired(PlanMode::Off, Some(true)), Some(false));
        assert_eq!(plan_desired(PlanMode::Aware, Some(true)), Some(true));
        assert_eq!(plan_desired(PlanMode::Aware, Some(false)), Some(false));
        assert_eq!(plan_desired(PlanMode::Aware, None), None); // nothing governs
    }

    #[test]
    fn hour_plan_validation_requires_exactly_24_mode_chars() {
        assert!(is_valid_hour_plan(&"W".repeat(24)));
        assert!(is_valid_hour_plan(&format!(
            "{}{}",
            "S".repeat(12),
            "A".repeat(12)
        )));
        assert!(!is_valid_hour_plan(&"W".repeat(23)));
        assert!(!is_valid_hour_plan(&"W".repeat(25)));
        assert!(!is_valid_hour_plan(&format!("{}x", "W".repeat(23))));
        assert!(!is_valid_hour_plan(""));
    }

    #[test]
    fn schedule_trigger_round_trips_and_has_no_event_input() {
        let t: AutomationTrigger = serde_json::from_str(&format!(
            r#"{{"kind":"schedule","hour_modes":"{}"}}"#,
            "W".repeat(24)
        ))
        .unwrap();
        assert_eq!(t.event(), None); // a plan transition, not a sensor edge
        assert_eq!(t.sensor_id(), None);
        assert_eq!(t.room_id(), None);
        assert_eq!(t.device(), None);
        assert_eq!(t.schedule().map(str::len), Some(24));
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains(r#""kind":"schedule""#));
    }

    #[test]
    fn is_power_only_admits_bare_power_and_rejects_attribute_clauses() {
        let power = LightState {
            on: true,
            ..Default::default()
        };
        assert!(is_power_only(&power));
        assert!(is_power_only(&LightState {
            on: false,
            ..Default::default()
        }));
        assert!(!is_power_only(&LightState {
            on: true,
            brightness: Some(40.0),
            ..Default::default()
        }));
        assert!(!is_power_only(&LightState {
            on: true,
            effect: Some("candle".into()),
            ..Default::default()
        }));
    }

    #[test]
    fn manual_trigger_is_a_macro_with_no_event_input() {
        let t: AutomationTrigger = serde_json::from_str(r#"{"kind":"manual"}"#).unwrap();
        assert!(matches!(t, AutomationTrigger::Manual {}));
        // No event, no lookup keys — the engine can never event-fire it.
        assert_eq!(t.event(), None);
        assert_eq!(t.sensor_id(), None);
        assert_eq!(t.room_id(), None);
        assert_eq!(t.device(), None);
        assert!(
            serde_json::to_string(&t)
                .unwrap()
                .contains(r#""kind":"manual""#)
        );
    }

    #[test]
    fn app_action_round_trips() {
        let a: RuleAction = serde_json::from_str(
            r#"{"kind":"app","remote_id":"r1","app":"com.hulu.livingroomplus"}"#,
        )
        .unwrap();
        assert!(
            matches!(a, RuleAction::App { ref remote_id, ref app } if remote_id == "r1" && app == "com.hulu.livingroomplus")
        );
        let s = serde_json::to_string(&a).unwrap();
        assert!(s.contains(r#""kind":"app""#));
    }

    #[test]
    fn toggle_action_round_trips() {
        let a: RuleAction =
            serde_json::from_str(r#"{"kind":"toggle","domain":"power","device_id":"fan1"}"#)
                .unwrap();
        assert!(
            matches!(a, RuleAction::Toggle { domain: TriggerDeviceDomain::Power, ref device_id } if device_id == "fan1")
        );
        assert!(
            serde_json::to_string(&a)
                .unwrap()
                .contains(r#""kind":"toggle""#)
        );
    }

    #[test]
    fn normalized_clears_effect_tokens_in_embedded_light_states() {
        let a = RuleAction::Light {
            light_id: "l1".into(),
            state: LightState {
                on: true,
                effect: Some("no_effect".into()),
                ..Default::default()
            },
        };
        match a.normalized() {
            RuleAction::Light { state, .. } => assert_eq!(state.effect, None),
            _ => unreachable!(),
        }
    }
}
