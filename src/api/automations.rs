//! Sensor automations — CRUD for [`SensorRule`]s and the background engine
//! that runs them.
//!
//! The engine subscribes to the same sensor push channels the SSE endpoint
//! uses (Hue SSE, HA WebSocket — no polling), keeps each sensor's previous
//! reading in memory to detect **edges**, and holds a pending-timer map for
//! `clear_for` ("no motion for N minutes") rules. Rules are re-read from the
//! DB per event, so an edit takes effect immediately; subscriptions are
//! rebuilt periodically so providers added later join without a restart.
//! Schedule (timer) rules ride the same loop: [`evaluate_schedules`] turns
//! each painted hour plan into an active verdict and acts on ITS edges —
//! powering the rule's targets on when an On hour begins and off when an Off
//! hour begins (pure power both ways, so a light keeps its look).
//! Actions replay through the shared service layer (`apply_room_state`,
//! `apply_light_state`, `apply_power_state`, `apply_scene_entries`) — an
//! automation is just another caller, like session/v1/MCP.
//!
//! Debug logging is on the `bifrost::automation` target.

use crate::AppState;
use crate::api::auth::Session;
use crate::connection::SensorEvent;
use crate::models::automation::{
    ActionStep, Automation, AutomationTrigger, RestoreEntry, RuleAction, RuleCondition,
    SensorTrigger, TriggerDeviceDomain, is_valid_hour_plan, plan_desired, plan_mode,
};

/// The table a device-trigger domain lives in (fixed identifiers — injection-free).
pub(crate) fn device_trigger_table(domain: TriggerDeviceDomain) -> &'static str {
    match domain {
        TriggerDeviceDomain::Light => "lights",
        TriggerDeviceDomain::Media => "media_devices",
        TriggerDeviceDomain::Power => "power_devices",
    }
}
use crate::models::sensor::{SensorReading, SensorState};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, put},
};
use serde::Deserialize;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_handler).post(create_handler))
        .route("/{id}", put(update_handler).delete(delete_handler))
        .route("/{id}/run", axum::routing::post(run_handler))
}

// ── CRUD ─────────────────────────────────────────────────────────────────────

fn row_to_automation(r: sqlx::sqlite::SqliteRow) -> Automation {
    Automation {
        id: r.get("id"),
        name: r.get("name"),
        enabled: r.get::<i64, _>("enabled") != 0,
        // A row whose trigger can't parse (shouldn't happen — writes are typed)
        // degrades to a never-firing sensor input rather than poisoning the list.
        trigger: serde_json::from_str(&r.get::<String, _>("trigger_json")).unwrap_or(
            AutomationTrigger::Sensor {
                sensor_id: r.get::<Option<String>, _>("sensor_id").unwrap_or_default(),
                event: SensorTrigger::BecameTrue,
            },
        ),
        conditions: serde_json::from_str(&r.get::<String, _>("conditions_json"))
            .unwrap_or_default(),
        steps: serde_json::from_str(&r.get::<String, _>("actions_json")).unwrap_or_default(),
        cooldown_secs: r.get::<i64, _>("cooldown_secs") as u32,
        restore_secs: r.get::<Option<i64>, _>("restore_secs").map(|v| v as u32),
        last_fired_at: r.get("last_fired_at"),
    }
}

/// Every stored rule, newest last. Shared by the session router (and any
/// future v1/MCP surface).
pub(crate) async fn list_automations(state: &AppState) -> Result<Vec<Automation>, ()> {
    sqlx::query("SELECT * FROM automations ORDER BY created_at, id")
        .fetch_all(&state.db)
        .await
        .map(|rows| rows.into_iter().map(row_to_automation).collect())
        .map_err(|e| tracing::error!("db error listing sensor rules: {e}"))
}

/// A create/update request: everything but the id / last-fired stamp.
#[derive(Debug, Deserialize)]
pub(crate) struct AutomationBody {
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub trigger: AutomationTrigger,
    #[serde(default)]
    pub conditions: Vec<RuleCondition>,
    pub steps: Vec<ActionStep>,
    #[serde(default)]
    pub cooldown_secs: u32,
    /// Timed hold: put everything back after this many seconds.
    #[serde(default)]
    pub restore_secs: Option<u32>,
}

fn default_true() -> bool {
    true
}

pub(crate) enum SaveRuleOutcome {
    Ok(Automation),
    NotFound,
    BadRequest(String),
    Db,
}

/// Validate an automation body against its trigger input: the subject must
/// exist, its event must suit the subject's reading type (a threshold on a
/// motion sensor — or on a room — would never fire; reject it loudly instead),
/// and there must be at least one action.
async fn validate_body(state: &AppState, body: &AutomationBody) -> Result<(), SaveRuleOutcome> {
    if body.steps.iter().all(|s| s.actions.is_empty()) {
        return Err(SaveRuleOutcome::BadRequest(
            "an automation needs at least one action".into(),
        ));
    }
    let event = body.trigger.event();
    let trigger_numeric = matches!(
        event,
        Some(SensorTrigger::RoseAbove { .. } | SensorTrigger::DroppedBelow { .. })
    );
    match &body.trigger {
        // A macro has no event input — nothing to validate on the trigger side.
        AutomationTrigger::Manual {} => {}
        AutomationTrigger::Sensor { sensor_id, .. } => {
            let kind: Option<String> =
                sqlx::query_scalar("SELECT kind FROM sensor_devices WHERE id = ?")
                    .bind(sensor_id)
                    .fetch_optional(&state.db)
                    .await
                    .map_err(|e| {
                        tracing::error!("db error validating automation sensor: {e}");
                        SaveRuleOutcome::Db
                    })?;
            let Some(kind) = kind else {
                return Err(SaveRuleOutcome::BadRequest("unknown sensor".into()));
            };
            let numeric = matches!(kind.as_str(), "illuminance" | "temperature" | "humidity");
            if numeric != trigger_numeric {
                return Err(SaveRuleOutcome::BadRequest(if numeric {
                    "a numeric sensor needs a threshold trigger (rises above / drops below)".into()
                } else {
                    "a motion/contact sensor needs a detected/cleared trigger".into()
                }));
            }
        }
        AutomationTrigger::Device {
            domain, device_id, ..
        } => {
            if trigger_numeric {
                return Err(SaveRuleOutcome::BadRequest(
                    "a device's power is on/off — pick a turns-on/off trigger".into(),
                ));
            }
            let exists: Option<i64> = sqlx::query_scalar(&format!(
                "SELECT 1 FROM {} WHERE id = ?",
                device_trigger_table(*domain)
            ))
            .bind(device_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| {
                tracing::error!("db error validating automation device: {e}");
                SaveRuleOutcome::Db
            })?;
            if exists.is_none() {
                return Err(SaveRuleOutcome::BadRequest("unknown device".into()));
            }
        }
        AutomationTrigger::Room { room_id, .. } => {
            if trigger_numeric {
                return Err(SaveRuleOutcome::BadRequest(
                    "a room's occupancy is occupied/empty — pick an occupancy trigger".into(),
                ));
            }
            // A room with no presence sensors has unknown occupancy and would
            // never fire (`room_occupancy` = None covers unknown rooms too).
            if crate::api::rooms::room_occupancy(state, room_id)
                .await
                .is_none()
            {
                return Err(SaveRuleOutcome::BadRequest(
                    "that room has no motion or occupancy sensors, so it can't trigger".into(),
                ));
            }
        }
        AutomationTrigger::Schedule { hour_modes } => {
            // Timers paint On/Off only — Aware ('A') is a kiosk display mode,
            // not an automation one.
            if !is_valid_hour_plan(hour_modes) || hour_modes.contains('A') {
                return Err(SaveRuleOutcome::BadRequest(
                    "a timer plan must be exactly 24 hours of on / off".into(),
                ));
            }
            // A timer only switches power: On hours power its targets on, Off
            // hours power them off. Attribute clauses (brightness, colour,
            // scenes, app launches) would be re-imposed at every boundary,
            // clobbering whatever look the light had — reject them loudly.
            for action in body.steps.iter().flat_map(|s| s.actions.iter()) {
                let power_only = match action {
                    RuleAction::Power { .. } => true,
                    RuleAction::Light { state, .. } | RuleAction::Room { state, .. } => {
                        crate::models::automation::is_power_only(state)
                    }
                    RuleAction::Scene { .. }
                    | RuleAction::App { .. }
                    | RuleAction::Toggle { .. } => false,
                };
                if !power_only {
                    return Err(SaveRuleOutcome::BadRequest(
                        "a timer only switches power — pick rooms, lights, or switches".into(),
                    ));
                }
            }
            // The plan already brings its own off direction; a timed hold on
            // top would fight it.
            if body.restore_secs.is_some() {
                return Err(SaveRuleOutcome::BadRequest(
                    "a timer rule already turns its devices off when its off hours begin".into(),
                ));
            }
            // A timer fires at most once an hour by construction; a cooldown
            // could only silently swallow a later On window's turn-on while
            // its Off edge still ran.
            if body.cooldown_secs != 0 {
                return Err(SaveRuleOutcome::BadRequest(
                    "a timer fires at most once an hour — cooldown doesn't apply".into(),
                ));
            }
        }
    }
    if let Some((_, secs)) = event.and_then(|e| e.stay_watch())
        && !(30..=24 * 3600).contains(&secs)
    {
        return Err(SaveRuleOutcome::BadRequest(
            "a stays-for duration must be between 30 seconds and 24 hours".into(),
        ));
    }
    if let Some(secs) = body.restore_secs
        && !(30..=24 * 3600).contains(&secs)
    {
        return Err(SaveRuleOutcome::BadRequest(
            "the put-things-back delay must be between 30 seconds and 24 hours".into(),
        ));
    }
    Ok(())
}

/// Serialize the step list for the `actions_json` column, normalizing each
/// action's embedded light state (clear-effect tokens → `None`).
fn steps_json(steps: &[ActionStep]) -> String {
    let normalized: Vec<ActionStep> = steps
        .iter()
        .map(|s| ActionStep {
            conditions: s.conditions.clone(),
            actions: s
                .actions
                .iter()
                .cloned()
                .map(RuleAction::normalized)
                .collect(),
        })
        .collect();
    serde_json::to_string(&normalized).unwrap_or_else(|_| "[]".into())
}

pub(crate) async fn create_rule(state: &AppState, body: AutomationBody) -> SaveRuleOutcome {
    if let Err(out) = validate_body(state, &body).await {
        return out;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let res = sqlx::query(
        "INSERT INTO automations (id, sensor_id, name, enabled, trigger_json, conditions_json, actions_json, cooldown_secs, restore_secs)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(body.trigger.sensor_id())
    .bind(body.name.trim())
    .bind(body.enabled as i64)
    .bind(serde_json::to_string(&body.trigger).unwrap_or_default())
    .bind(serde_json::to_string(&body.conditions).unwrap_or_else(|_| "[]".into()))
    .bind(steps_json(&body.steps))
    .bind(body.cooldown_secs as i64)
    .bind(body.restore_secs.map(|v| v as i64))
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => {
            state.automations_changed.notify_waiters();
            fetch_rule(state, &id).await
        }
        Err(e) => {
            tracing::error!("db error creating sensor rule: {e}");
            SaveRuleOutcome::Db
        }
    }
}

pub(crate) async fn update_rule(
    state: &AppState,
    id: &str,
    body: AutomationBody,
) -> SaveRuleOutcome {
    if let Err(out) = validate_body(state, &body).await {
        return out;
    }
    let res = sqlx::query(
        "UPDATE automations SET sensor_id = ?, name = ?, enabled = ?, trigger_json = ?,
                conditions_json = ?, actions_json = ?, cooldown_secs = ?, restore_secs = ? WHERE id = ?",
    )
    .bind(body.trigger.sensor_id())
    .bind(body.name.trim())
    .bind(body.enabled as i64)
    .bind(serde_json::to_string(&body.trigger).unwrap_or_default())
    .bind(serde_json::to_string(&body.conditions).unwrap_or_else(|_| "[]".into()))
    .bind(steps_json(&body.steps))
    .bind(body.cooldown_secs as i64)
    .bind(body.restore_secs.map(|v| v as i64))
    .bind(id)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() > 0 => {
            state.automations_changed.notify_waiters();
            fetch_rule(state, id).await
        }
        Ok(_) => SaveRuleOutcome::NotFound,
        Err(e) => {
            tracing::error!("db error updating sensor rule: {e}");
            SaveRuleOutcome::Db
        }
    }
}

async fn fetch_rule(state: &AppState, id: &str) -> SaveRuleOutcome {
    match sqlx::query("SELECT * FROM automations WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(r)) => SaveRuleOutcome::Ok(row_to_automation(r)),
        Ok(None) => SaveRuleOutcome::NotFound,
        Err(e) => {
            tracing::error!("db error fetching sensor rule: {e}");
            SaveRuleOutcome::Db
        }
    }
}

pub(crate) async fn delete_rule(state: &AppState, id: &str) -> StatusCode {
    match sqlx::query("DELETE FROM automations WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            state.automations_changed.notify_waiters();
            StatusCode::NO_CONTENT
        }
        Ok(_) => StatusCode::NOT_FOUND,
        Err(e) => {
            tracing::error!("db error deleting sensor rule: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

fn save_response(out: SaveRuleOutcome) -> axum::response::Response {
    match out {
        SaveRuleOutcome::Ok(rule) => Json(rule).into_response(),
        SaveRuleOutcome::NotFound => StatusCode::NOT_FOUND.into_response(),
        SaveRuleOutcome::BadRequest(m) => (StatusCode::UNPROCESSABLE_ENTITY, m).into_response(),
        SaveRuleOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn list_handler(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    match list_automations(&state).await {
        Ok(rules) => Json(rules).into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn create_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Json(body): Json<AutomationBody>,
) -> impl IntoResponse {
    save_response(create_rule(&state, body).await)
}

async fn update_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(body): Json<AutomationBody>,
) -> impl IntoResponse {
    save_response(update_rule(&state, &id, body).await)
}

/// Run an automation's actions immediately, skipping its trigger and
/// conditions — the "test this rule" button. Works on disabled rules too (the
/// point is trying the actions), and stamps `last_fired_at` like a real fire.
pub(crate) async fn run_automation(state: &AppState, id: &str) -> StatusCode {
    let rule = match sqlx::query("SELECT * FROM automations WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(r)) => row_to_automation(r),
        Ok(None) => return StatusCode::NOT_FOUND,
        Err(e) => {
            tracing::error!("db error loading automation to run: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };
    tracing::debug!(target: "bifrost::automation", rule = %id, "manual run");
    execute_rule(state, &rule).await;
    StatusCode::NO_CONTENT
}

async fn run_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    run_automation(&state, &id).await
}

async fn delete_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    delete_rule(&state, &id).await
}

// ── Engine ───────────────────────────────────────────────────────────────────

/// What a stay timer is watching — the subject whose boolean state must hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StaySubject {
    /// A sensor's reading (by sensor row id).
    Sensor(String),
    /// A Room's aggregate occupancy (by room id).
    Room(String),
    /// A device's power boolean (by domain + row id).
    Device(TriggerDeviceDomain, String),
}

/// An armed stays-for timer: fire when `fire_at` passes, provided the subject
/// still reads `watched`.
pub(crate) struct PendingStay {
    subject: StaySubject,
    watched: bool,
    fire_at: Instant,
}

/// In-memory engine state: previous readings for edge detection (sensors and
/// room occupancy) and the armed stay timers. Sized by sensors/rules — tiny.
#[derive(Default)]
pub(crate) struct EngineState {
    /// sensor row id → last seen reading.
    pub prev: HashMap<String, SensorReading>,
    /// room id → last derived occupancy.
    pub room_prev: HashMap<String, bool>,
    /// (domain, device row id) → last seen power boolean.
    pub device_prev: HashMap<(TriggerDeviceDomain, String), bool>,
    /// rule id → armed stay timer.
    pub pending: HashMap<String, PendingStay>,
    /// schedule (timer) rule id → last computed plan-active verdict, for the
    /// plan's own edge detection.
    pub schedule_prev: HashMap<String, bool>,
    /// The local hour the schedule evaluator last completed a pass for — plan
    /// edges only happen on hour changes, so the ordinary 5s ticks in between
    /// skip the rule-table read entirely.
    pub schedule_hour: Option<usize>,
}

/// A sensor row as the engine needs it.
struct SensorRow {
    id: String,
    enabled: bool,
    presence: bool,
}

/// Resolve the sensor row a push event belongs to.
async fn sensor_for_event(
    state: &AppState,
    provider_row_id: &str,
    device_id: &str,
) -> Option<SensorRow> {
    sqlx::query(
        "SELECT id, enabled, kind FROM sensor_devices
         WHERE provider_id = ? AND device_id = ? AND shadowed_by IS NULL",
    )
    .bind(provider_row_id)
    .bind(device_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tracing::error!("db error resolving sensor for event: {e}"))
    .ok()
    .flatten()
    .map(|r| SensorRow {
        id: r.get("id"),
        enabled: r.get::<i64, _>("enabled") != 0,
        presence: crate::api::sensors::parse_kind(&r.get::<String, _>("kind")).is_presence(),
    })
}

/// The cached reading of any sensor (for cross-sensor conditions).
async fn cached_reading(state: &AppState, sensor_id: &str) -> Option<SensorReading> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT last_state FROM sensor_devices WHERE id = ?")
            .bind(sensor_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    serde_json::from_str::<SensorState>(&raw?).ok()?.reading
}

/// The rules listening to one sensor (via the denormalized lookup column).
async fn rules_for_sensor(state: &AppState, sensor_id: &str) -> Vec<Automation> {
    sqlx::query("SELECT * FROM automations WHERE sensor_id = ? AND enabled = 1")
        .bind(sensor_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| tracing::error!("db error loading rules for sensor: {e}"))
        .unwrap_or_default()
        .into_iter()
        .map(row_to_automation)
        .collect()
}

/// Every enabled non-sensor rule. Non-sensor triggers have no lookup column
/// (`sensor_id IS NULL`), so callers match in code — rules number in the
/// dozens, not thousands. `Err` when the read failed: a caller that mutates
/// engine state on a rule's *absence* (the schedule evaluator's verdict map)
/// must never mistake a failed read for "no rules".
async fn non_sensor_rules_checked(state: &AppState) -> Result<Vec<Automation>, ()> {
    sqlx::query("SELECT * FROM automations WHERE sensor_id IS NULL AND enabled = 1")
        .fetch_all(&state.db)
        .await
        .map(|rows| rows.into_iter().map(row_to_automation).collect())
        .map_err(|e| tracing::error!("db error loading non-sensor rules: {e}"))
}

/// [`non_sensor_rules_checked`] for the per-event lookups, where a failed read
/// just means this event matches nothing (no state is wiped by emptiness).
async fn non_sensor_rules(state: &AppState) -> Vec<Automation> {
    non_sensor_rules_checked(state).await.unwrap_or_default()
}

/// The rules listening to one room's occupancy.
async fn rules_for_room(state: &AppState, room_id: &str) -> Vec<Automation> {
    non_sensor_rules(state)
        .await
        .into_iter()
        .filter(|rule| rule.trigger.room_id() == Some(room_id))
        .collect()
}

/// The rules watching one device's power.
async fn rules_for_device(
    state: &AppState,
    domain: TriggerDeviceDomain,
    device_row_id: &str,
) -> Vec<Automation> {
    non_sensor_rules(state)
        .await
        .into_iter()
        .filter(|rule| rule.trigger.device() == Some((domain, device_row_id)))
        .collect()
}

/// The stored power boolean of a device-trigger subject, from its table's
/// cached state — the fire-time re-check for device stay timers, and the
/// kiosk scheduler's "aware override" read (`api::kiosks::scheduler_tick`).
pub(crate) async fn cached_device_on(
    state: &AppState,
    domain: TriggerDeviceDomain,
    device_row_id: &str,
) -> Option<bool> {
    let raw: Option<String> = sqlx::query_scalar(&format!(
        "SELECT last_state FROM {} WHERE id = ?",
        device_trigger_table(domain)
    ))
    .bind(device_row_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let v: serde_json::Value = serde_json::from_str(&raw?).ok()?;
    match domain {
        TriggerDeviceDomain::Light | TriggerDeviceDomain::Power => v.get("on")?.as_bool(),
        TriggerDeviceDomain::Media => v.get("power")?.as_bool(),
    }
}

/// Detect a device power edge and run its rules through the shared edge path.
pub(crate) async fn process_device_event(
    state: &AppState,
    engine: &mut EngineState,
    domain: TriggerDeviceDomain,
    device_row_id: &str,
    on: bool,
) {
    let key = (domain, device_row_id.to_string());
    let prev = engine.device_prev.insert(key, on);
    if prev == Some(on) {
        return; // no edge (full-state pushes repeat unchanged snapshots)
    }
    let rules = rules_for_device(state, domain, device_row_id).await;
    if rules.is_empty() && prev.is_none() {
        return;
    }
    apply_edge(
        state,
        engine,
        &StaySubject::Device(domain, device_row_id.to_string()),
        rules,
        prev.map(SensorReading::Bool),
        SensorReading::Bool(on),
    )
    .await;
}

/// Whether a rule is inside its cooldown (last_fired_at within cooldown_secs).
fn in_cooldown(rule: &Automation, now_utc: chrono::DateTime<chrono::Utc>) -> bool {
    if rule.cooldown_secs == 0 {
        return false;
    }
    let Some(last) = rule
        .last_fired_at
        .as_deref()
        .and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok())
    else {
        return false;
    };
    let last_utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(last, chrono::Utc);
    now_utc.signed_duration_since(last_utc).num_seconds() < rule.cooldown_secs as i64
}

/// Evaluate a set of conditions right now. Fails closed on unknown readings.
/// Shared by the rule-level gate and each step's own gate.
async fn conditions_hold(state: &AppState, conditions: &[RuleCondition]) -> bool {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    let now_min = (now.hour() * 60 + now.minute()) as u16;
    let now_day = now.weekday().num_days_from_monday() as u8;
    for cond in conditions {
        // Cross-subject gates need the other subject's current state; resolve
        // it before the sync `holds` call.
        let reading = match cond {
            RuleCondition::SensorAbove { sensor_id, .. }
            | RuleCondition::SensorBelow { sensor_id, .. }
            | RuleCondition::SensorIs { sensor_id, .. } => cached_reading(state, sensor_id).await,
            RuleCondition::TimeWindow { .. }
            | RuleCondition::RoomIs { .. }
            | RuleCondition::DeviceIs { .. } => None,
        };
        let occupancy = match cond {
            RuleCondition::RoomIs { room_id, .. } => {
                crate::api::rooms::room_occupancy(state, room_id).await
            }
            _ => None,
        };
        // A device gate ("…unless the TV is on") reads the same cached power
        // boolean the device triggers watch — fail closed on an unknown device.
        let device_on = match cond {
            RuleCondition::DeviceIs {
                domain, device_id, ..
            } => cached_device_on(state, *domain, device_id).await,
            _ => None,
        };
        if !cond.holds(
            now_min,
            now_day,
            |_| reading,
            |_| occupancy,
            |_, _| device_on,
        ) {
            tracing::debug!(target: "bifrost::automation", ?cond, "condition not met");
            return false;
        }
    }
    true
}

/// The pre-fire snapshot for a timed hold: the current cached state of every
/// device the rule's actions will touch — room actions expand to the room's
/// effective light + power members, scene actions to the devices the scene
/// drives. Deduped (first capture of a device wins). Media members are not
/// captured: resuming playback isn't re-applying a state.
async fn snapshot_targets<'a>(
    state: &AppState,
    actions: impl Iterator<Item = &'a RuleAction>,
) -> Vec<RestoreEntry> {
    let mut seen: std::collections::HashSet<(bool, String)> = std::collections::HashSet::new();
    let mut light_ids: Vec<String> = Vec::new();
    let mut power_ids: Vec<String> = Vec::new();
    let mut want = |is_light: bool, id: String| {
        if seen.insert((is_light, id.clone())) {
            if is_light {
                light_ids.push(id);
            } else {
                power_ids.push(id);
            }
        }
    };
    for action in actions {
        match action {
            RuleAction::Light { light_id, .. } => want(true, light_id.clone()),
            RuleAction::Power { device_id, .. } => want(false, device_id.clone()),
            RuleAction::Room { room_id, .. } => {
                for id in crate::api::rooms::effective_member_ids(state, room_id).await {
                    want(true, id);
                }
                for id in crate::api::rooms::effective_power_member_ids(state, room_id).await {
                    want(false, id);
                }
            }
            RuleAction::Scene { scene_id } => {
                let lights: Vec<String> =
                    sqlx::query_scalar("SELECT light_id FROM scene_entries WHERE scene_id = ?")
                        .bind(scene_id)
                        .fetch_all(&state.db)
                        .await
                        .unwrap_or_default();
                for id in lights {
                    want(true, id);
                }
                let power: Vec<String> = sqlx::query_scalar(
                    "SELECT power_device_id FROM scene_power_entries WHERE scene_id = ?",
                )
                .bind(scene_id)
                .fetch_all(&state.db)
                .await
                .unwrap_or_default();
                for id in power {
                    want(false, id);
                }
            }
            // An app launch has no restorable "previous state" — nothing to
            // snapshot (you can't un-launch Hulu).
            RuleAction::App { .. } => {}
            // A toggle touches one device's power; snapshot it so a timed hold
            // can restore it (media isn't in the light/power restore model,
            // same as scenes/rooms — only light + power are captured).
            RuleAction::Toggle { domain, device_id } => match domain {
                TriggerDeviceDomain::Light => want(true, device_id.clone()),
                TriggerDeviceDomain::Power => want(false, device_id.clone()),
                TriggerDeviceDomain::Media => {}
            },
        }
    }

    let mut entries = Vec::new();
    for id in light_ids {
        let raw: Option<String> = sqlx::query_scalar("SELECT last_state FROM lights WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
        if let Some(st) =
            raw.and_then(|s| serde_json::from_str::<crate::models::LightState>(&s).ok())
        {
            entries.push(RestoreEntry::Light {
                light_id: id,
                state: st,
            });
        }
    }
    for id in power_ids {
        let raw: Option<String> =
            sqlx::query_scalar("SELECT last_state FROM power_devices WHERE id = ?")
                .bind(&id)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();
        if let Some(st) =
            raw.and_then(|s| serde_json::from_str::<crate::models::power::PowerState>(&s).ok())
        {
            entries.push(RestoreEntry::Power {
                device_id: id,
                on: st.on,
            });
        }
    }
    entries
}

/// The device states already captured by **other rules' pending holds**,
/// keyed by device. When two holds overlap on a device, only the first
/// capture saw the true pre-automation state — every later hold must inherit
/// it, or the final restore replays a mid-automation state (which rule fires
/// first is arbitrary, so both must converge on the same original).
async fn pending_hold_entries(
    state: &AppState,
) -> std::collections::HashMap<(bool, String), RestoreEntry> {
    let rows: Vec<String> = sqlx::query_scalar("SELECT snapshot_json FROM automation_restores")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let mut map = std::collections::HashMap::new();
    for raw in rows {
        for entry in serde_json::from_str::<Vec<RestoreEntry>>(&raw).unwrap_or_default() {
            let key = match &entry {
                RestoreEntry::Light { light_id, .. } => (true, light_id.clone()),
                RestoreEntry::Power { device_id, .. } => (false, device_id.clone()),
            };
            // With inheritance in place every pending copy of a device is the
            // same original, so first-seen is as good as any.
            map.entry(key).or_insert(entry);
        }
    }
    map
}

// ── Manual-override drop-out ─────────────────────────────────────────────────
//
// A timed hold exists to undo what the AUTOMATION did. Once something else —
// a human in the Bifrost UI, the Hue app, a wall switch — changes a held
// device, the automation's write is no longer the last word, and replaying
// the stale snapshot at restore time would destroy newer intent. The engine
// therefore watches every held device on the same push pipelines that keep
// the UI live and, on a genuine divergence, releases that device from every
// pending snapshot (the rest of the hold still restores on time).
//
// "Genuine" is the hard part: the rule's own writes echo back on those same
// streams (Hue SSE within a second, poll providers a full snapshot every
// cycle), so each held device carries a **reference state** — what the device
// should look like while held, seeded from the cache the shared apply path
// just wrote. A short grace window after each hold-affecting write lets the
// provider echo settle into the reference (device-side rounding); after it,
// only a toleranced divergence counts as manual. Dimensions the reference has
// never seen adopt fill-only, so a slow first poll can't false-positive.

/// How long provider echoes of a hold-affecting write may keep settling the
/// reference instead of counting as a manual change.
const HOLD_GRACE: Duration = Duration::from_secs(15);

enum HoldRef {
    Light(crate::models::LightState),
    Power(bool),
}

struct WatchedHold {
    grace_until: Instant,
    reference: HoldRef,
}

/// The in-memory watch over devices held by pending timed holds, keyed like
/// snapshot entries: `(is_light, device_row_id)`. Lives on [`AppState`]; the
/// references are rebuilt from the state cache on restart ([`seed_hold_watch`]).
#[derive(Default)]
pub struct HoldWatch {
    inner: tokio::sync::Mutex<HashMap<(bool, String), WatchedHold>>,
}

/// (Re)register every device of a snapshot with a fresh reference from the
/// state cache — call **after** the hold-affecting write (rule actions, a
/// restore), when the shared apply path has just updated that cache.
async fn watch_hold_devices(state: &AppState, entries: &[RestoreEntry]) {
    let grace_until = Instant::now() + HOLD_GRACE;
    for entry in entries {
        let (key, reference) = match entry {
            RestoreEntry::Light { light_id, .. } => {
                let raw: Option<String> =
                    sqlx::query_scalar("SELECT last_state FROM lights WHERE id = ?")
                        .bind(light_id)
                        .fetch_optional(&state.db)
                        .await
                        .ok()
                        .flatten();
                let Some(st) =
                    raw.and_then(|s| serde_json::from_str::<crate::models::LightState>(&s).ok())
                else {
                    continue;
                };
                ((true, light_id.clone()), HoldRef::Light(st))
            }
            RestoreEntry::Power { device_id, .. } => {
                let raw: Option<String> =
                    sqlx::query_scalar("SELECT last_state FROM power_devices WHERE id = ?")
                        .bind(device_id)
                        .fetch_optional(&state.db)
                        .await
                        .ok()
                        .flatten();
                let Some(st) = raw.and_then(|s| {
                    serde_json::from_str::<crate::models::power::PowerState>(&s).ok()
                }) else {
                    continue;
                };
                ((false, device_id.clone()), HoldRef::Power(st.on))
            }
        };
        state.hold_watch.inner.lock().await.insert(
            key,
            WatchedHold {
                grace_until,
                reference,
            },
        );
    }
}

/// Rebuild the watch from the persisted pending holds (references from the
/// state cache) — holds survive a restart, so their watches must too.
pub(crate) async fn seed_hold_watch(state: &AppState) {
    let rows: Vec<String> = sqlx::query_scalar("SELECT snapshot_json FROM automation_restores")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    for raw in rows {
        let entries: Vec<RestoreEntry> = serde_json::from_str(&raw).unwrap_or_default();
        watch_hold_devices(state, &entries).await;
    }
}

/// Whether a pushed light patch genuinely diverges from a held reference —
/// toleranced so provider rounding (Govee RGB⇄xy, fractional brightness)
/// can't read as a manual change. A reachability drop is a device going
/// dark, never human intent, and an off light's attribute echoes are stale
/// by definition, so only its power edge counts.
fn light_patch_diverges(
    reference: &crate::models::LightState,
    patch: &crate::models::LightStatePatch,
) -> bool {
    if patch.reachable == Some(false) {
        return false;
    }
    if let Some(on) = patch.on
        && on != reference.on
    {
        return true;
    }
    if !reference.on {
        return false;
    }
    if let (Some(b), Some(rb)) = (patch.brightness, reference.brightness)
        && (b - rb).abs() > 2.0
    {
        return true;
    }
    if let (Some(c), Some(rc)) = (patch.color.as_ref(), reference.color.as_ref())
        && ((c.x - rc.x).abs() > 0.02 || (c.y - rc.y).abs() > 0.02)
    {
        return true;
    }
    if let (Some(ct), Some(rct)) = (patch.color_temp_mirek, reference.color_temp_mirek)
        && ct.abs_diff(rct) > 10
    {
        return true;
    }
    if let (Some(e), Some(re)) = (patch.effect.as_deref(), reference.effect.as_deref()) {
        let (e_clear, re_clear) = (
            crate::models::is_clear_effect(e),
            crate::models::is_clear_effect(re),
        );
        if e_clear != re_clear || (!e_clear && e != re) {
            return true;
        }
    }
    false
}

/// Adopt patch dimensions the reference hasn't seen yet (fill-only) — a push
/// provider only echoes the dimensions a write touched, so the first manual
/// change on an untouched dimension needs a baseline to diverge FROM next time.
fn fill_reference(
    reference: &mut crate::models::LightState,
    patch: &crate::models::LightStatePatch,
) {
    if reference.brightness.is_none() {
        reference.brightness = patch.brightness;
    }
    if reference.color.is_none() {
        reference.color = patch.color.clone();
    }
    if reference.color_temp_mirek.is_none() {
        reference.color_temp_mirek = patch.color_temp_mirek;
    }
    if reference.effect.is_none() {
        reference.effect = patch.effect.clone();
    }
}

/// Remove one device from every pending snapshot (a snapshot left empty is
/// deleted whole). The manual-change release path.
async fn release_held_device(state: &AppState, is_light: bool, id: &str) {
    let rows = sqlx::query("SELECT automation_id, snapshot_json FROM automation_restores")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    for row in rows {
        let rule_id: String = row.get("automation_id");
        let entries: Vec<RestoreEntry> =
            serde_json::from_str(&row.get::<String, _>("snapshot_json")).unwrap_or_default();
        let kept: Vec<&RestoreEntry> = entries
            .iter()
            .filter(|e| match e {
                RestoreEntry::Light { light_id, .. } => !(is_light && light_id == id),
                RestoreEntry::Power { device_id, .. } => is_light || device_id != id,
            })
            .collect();
        if kept.len() == entries.len() {
            continue;
        }
        if kept.is_empty() {
            let _ = sqlx::query("DELETE FROM automation_restores WHERE automation_id = ?")
                .bind(&rule_id)
                .execute(&state.db)
                .await;
        } else {
            let _ = sqlx::query(
                "UPDATE automation_restores SET snapshot_json = ? WHERE automation_id = ?",
            )
            .bind(serde_json::to_string(&kept).unwrap_or_else(|_| "[]".into()))
            .bind(&rule_id)
            .execute(&state.db)
            .await;
        }
        tracing::debug!(
            target: "bifrost::automation",
            rule = %rule_id,
            device = %id,
            light = is_light,
            "manual change: device released from pending hold",
        );
    }
}

/// A pushed light event vs the hold watch: inside grace it settles the
/// reference (provider echo); after it, a divergence releases the device.
pub(crate) async fn observe_hold_light(
    state: &AppState,
    light_row_id: &str,
    patch: &crate::models::LightStatePatch,
) {
    observe_hold_light_at(state, light_row_id, patch, Instant::now()).await;
}

async fn observe_hold_light_at(
    state: &AppState,
    light_row_id: &str,
    patch: &crate::models::LightStatePatch,
    now: Instant,
) {
    {
        let mut watch = state.hold_watch.inner.lock().await;
        let key = (true, light_row_id.to_string());
        let Some(held) = watch.get_mut(&key) else {
            return;
        };
        let HoldRef::Light(reference) = &mut held.reference else {
            return;
        };
        if patch.reachable == Some(false) {
            return; // a device going dark is never a manual change
        }
        if now < held.grace_until {
            patch.apply_to(reference);
            return;
        }
        if !light_patch_diverges(reference, patch) {
            fill_reference(reference, patch);
            return;
        }
        watch.remove(&key);
    }
    release_held_device(state, true, light_row_id).await;
}

/// A pushed power event vs the hold watch (power is a plain boolean — exact).
pub(crate) async fn observe_hold_power(state: &AppState, device_row_id: &str, on: bool) {
    observe_hold_power_at(state, device_row_id, on, Instant::now()).await;
}

async fn observe_hold_power_at(state: &AppState, device_row_id: &str, on: bool, now: Instant) {
    {
        let mut watch = state.hold_watch.inner.lock().await;
        let key = (false, device_row_id.to_string());
        let Some(held) = watch.get_mut(&key) else {
            return;
        };
        let HoldRef::Power(reference) = &mut held.reference else {
            return;
        };
        if now < held.grace_until {
            *reference = on;
            return;
        }
        if on == *reference {
            return;
        }
        watch.remove(&key);
    }
    release_held_device(state, false, device_row_id).await;
}

/// Schedule (or extend) a rule's timed hold. A re-fire during the hold pushes
/// `restore_at` out but keeps the **original** snapshot — re-capturing would
/// snapshot the rule's own triggered state, making the "restore" a no-op.
async fn schedule_restore(state: &AppState, rule: &Automation, secs: u32) {
    let modifier = format!("+{secs} seconds");
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM automation_restores WHERE automation_id = ?")
            .bind(&rule.id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    if existing.is_some() {
        let _ = sqlx::query(
            "UPDATE automation_restores SET restore_at = datetime('now', ?) WHERE automation_id = ?",
        )
        .bind(&modifier)
        .bind(&rule.id)
        .execute(&state.db)
        .await;
        tracing::debug!(target: "bifrost::automation", rule = %rule.id, secs, "hold extended (original snapshot kept)");
        return;
    }
    let mut snapshot = snapshot_targets(state, rule.all_actions()).await;
    if snapshot.is_empty() {
        return; // nothing capturable — nothing to put back
    }
    // Overlapping holds: a device another rule's pending hold already captured
    // keeps that (true pre-automation) state — this rule may only be seeing
    // the other rule's output.
    let inherited = pending_hold_entries(state).await;
    for entry in &mut snapshot {
        let key = match &*entry {
            RestoreEntry::Light { light_id, .. } => (true, light_id.clone()),
            RestoreEntry::Power { device_id, .. } => (false, device_id.clone()),
        };
        if let Some(original) = inherited.get(&key) {
            *entry = original.clone();
        }
    }
    let _ = sqlx::query(
        "INSERT INTO automation_restores (automation_id, restore_at, snapshot_json)
         VALUES (?, datetime('now', ?), ?)",
    )
    .bind(&rule.id)
    .bind(&modifier)
    .bind(serde_json::to_string(&snapshot).unwrap_or_else(|_| "[]".into()))
    .execute(&state.db)
    .await;
    tracing::debug!(target: "bifrost::automation", rule = %rule.id, secs, devices = snapshot.len(), "hold scheduled: state snapshotted");
}

/// Apply and clear every due timed hold. Runs on the engine tick; persisted in
/// the DB, so a hold survives a restart.
pub(crate) async fn apply_due_restores(state: &AppState) {
    let rows = sqlx::query(
        "SELECT automation_id, snapshot_json FROM automation_restores
         WHERE restore_at <= datetime('now')",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    for row in rows {
        let rule_id: String = row.get("automation_id");
        let entries: Vec<RestoreEntry> =
            serde_json::from_str(&row.get::<String, _>("snapshot_json")).unwrap_or_default();
        tracing::debug!(target: "bifrost::automation", rule = %rule_id, devices = entries.len(), "hold ended: putting things back");
        for entry in &entries {
            match entry {
                RestoreEntry::Light {
                    light_id,
                    state: st,
                } => {
                    let _ = crate::api::lights::apply_light_state(state, light_id, st).await;
                }
                RestoreEntry::Power { device_id, on } => {
                    let _ = crate::api::power::apply_power_state(state, device_id, *on).await;
                }
            }
        }
        let _ = sqlx::query("DELETE FROM automation_restores WHERE automation_id = ?")
            .bind(&rule_id)
            .execute(&state.db)
            .await;
        // The restore's own writes echo on the push streams like anything
        // else: devices another rule still holds (overlap inheritance) get a
        // fresh reference + grace; the rest leave the watch.
        let still_held = pending_hold_entries(state).await;
        let mut done = Vec::new();
        let mut refresh = Vec::new();
        for entry in entries {
            let key = match &entry {
                RestoreEntry::Light { light_id, .. } => (true, light_id.clone()),
                RestoreEntry::Power { device_id, .. } => (false, device_id.clone()),
            };
            if still_held.contains_key(&key) {
                refresh.push(entry);
            } else {
                done.push(key);
            }
        }
        if !done.is_empty() {
            let mut watch = state.hold_watch.inner.lock().await;
            for key in done {
                watch.remove(&key);
            }
        }
        if !refresh.is_empty() {
            watch_hold_devices(state, &refresh).await;
        }
    }
}

/// Execute a rule's actions through the shared service layer. Best-effort per
/// action: one failing action is logged and the rest still run.
pub(crate) async fn execute_rule(state: &AppState, rule: &Automation) {
    tracing::debug!(
        target: "bifrost::automation",
        rule = %rule.id,
        name = %rule.name,
        steps = rule.steps.len(),
        "rule fired",
    );
    if let Some(secs) = rule.restore_secs {
        schedule_restore(state, rule, secs).await;
    }
    for (i, step) in rule.steps.iter().enumerate() {
        // A step runs only if its own conditions hold (the rule-level gate was
        // already checked by `try_fire`). An empty condition list always runs.
        if !step.conditions.is_empty() && !conditions_hold(state, &step.conditions).await {
            tracing::debug!(target: "bifrost::automation", rule = %rule.id, step = i, "step skipped: conditions not met");
            continue;
        }
        for action in &step.actions {
            match action {
                RuleAction::Room { room_id, state: st } => {
                    let members = crate::api::rooms::effective_members(state, room_id).await;
                    let (applied, failed) =
                        crate::api::rooms::apply_room_state(state, room_id, st, members).await;
                    tracing::debug!(target: "bifrost::automation", rule = %rule.id, room = %room_id, applied, failed, "action: room state");
                }
                RuleAction::Light {
                    light_id,
                    state: st,
                } => {
                    let outcome = crate::api::lights::apply_light_state(state, light_id, st).await;
                    tracing::debug!(target: "bifrost::automation", rule = %rule.id, light = %light_id, ok = matches!(outcome, crate::api::lights::SetLightOutcome::Ok), "action: light state");
                }
                RuleAction::Power { device_id, on } => {
                    let outcome = crate::api::power::apply_power_state(state, device_id, *on).await;
                    tracing::debug!(target: "bifrost::automation", rule = %rule.id, device = %device_id, on, ok = matches!(outcome, crate::api::power::SetPowerOutcome::Ok), "action: power");
                }
                RuleAction::Scene { scene_id } => {
                    let applied =
                        crate::api::scenes::apply_scene_entries(state, scene_id, None).await;
                    tracing::debug!(target: "bifrost::automation", rule = %rule.id, scene = %scene_id, ok = applied.is_some(), "action: scene");
                }
                RuleAction::App { remote_id, app } => {
                    let outcome = crate::api::remote::apply_remote_command(
                        state,
                        remote_id,
                        &crate::models::remote::RemoteCommand::LaunchApp {
                            activity: app.clone(),
                        },
                    )
                    .await;
                    tracing::debug!(target: "bifrost::automation", rule = %rule.id, remote = %remote_id, %app, outcome = ?outcome, "action: app launch");
                }
                RuleAction::Toggle { domain, device_id } => {
                    match cached_device_on(state, *domain, device_id).await {
                        Some(cur) => {
                            let next = !cur;
                            match domain {
                                TriggerDeviceDomain::Light => {
                                    crate::api::lights::apply_light_state(
                                        state,
                                        device_id,
                                        &crate::models::LightState {
                                            on: next,
                                            ..Default::default()
                                        },
                                    )
                                    .await;
                                }
                                TriggerDeviceDomain::Power => {
                                    crate::api::power::apply_power_state(state, device_id, next)
                                        .await;
                                }
                                TriggerDeviceDomain::Media => {
                                    crate::api::media::apply_media_command(
                                        state,
                                        device_id,
                                        &crate::models::media::MediaCommand {
                                            power: Some(next),
                                            ..Default::default()
                                        },
                                    )
                                    .await;
                                }
                            }
                            tracing::debug!(target: "bifrost::automation", rule = %rule.id, ?domain, device = %device_id, from = cur, to = next, "action: toggle");
                        }
                        None => {
                            tracing::debug!(target: "bifrost::automation", rule = %rule.id, ?domain, device = %device_id, "action: toggle skipped — state unknown");
                        }
                    }
                }
            }
        }
    }
    // Watch the held devices now the shared apply path has written the cache:
    // the reference is "what the rule left behind", and any later genuine
    // divergence is a manual change that releases the device from the hold.
    // (Re-fires re-register — the rule just re-applied its output.)
    if rule.restore_secs.is_some() {
        let raw: Option<String> = sqlx::query_scalar(
            "SELECT snapshot_json FROM automation_restores WHERE automation_id = ?",
        )
        .bind(&rule.id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        if let Some(entries) = raw.and_then(|s| serde_json::from_str::<Vec<RestoreEntry>>(&s).ok())
        {
            watch_hold_devices(state, &entries).await;
        }
    }
    let _ = sqlx::query("UPDATE automations SET last_fired_at = datetime('now') WHERE id = ?")
        .bind(&rule.id)
        .execute(&state.db)
        .await;
}

/// Fire a rule if its gates (cooldown + conditions) pass.
async fn try_fire(state: &AppState, rule: &Automation) {
    if in_cooldown(rule, chrono::Utc::now()) {
        tracing::debug!(target: "bifrost::automation", rule = %rule.id, "rule skipped: in cooldown");
        return;
    }
    if !conditions_hold(state, &rule.conditions).await {
        return;
    }
    execute_rule(state, rule).await;
}

/// Run one subject's transition against its rules: cancel the stay timers the
/// transition breaks, arm the ones it starts, and fire matching edge triggers.
/// The single edge path shared by sensor events and room-occupancy changes.
async fn apply_edge(
    state: &AppState,
    engine: &mut EngineState,
    subject: &StaySubject,
    rules: Vec<Automation>,
    prev: Option<SensorReading>,
    now: SensorReading,
) {
    // Leaving a watched state cancels that timer (motion cancels "empty for…",
    // clearing cancels "detected for…").
    engine
        .pending
        .retain(|_, p| !(p.subject == *subject && now.as_bool() == Some(!p.watched)));
    for rule in rules {
        // A manual (macro) rule has no event input — it can't event-fire.
        let Some(event) = rule.trigger.event() else {
            continue;
        };
        if let Some((watched, secs)) = event.arms_stay_timer(prev, now) {
            tracing::debug!(target: "bifrost::automation", rule = %rule.id, watched, secs, "stay timer armed");
            engine.pending.insert(
                rule.id.clone(),
                PendingStay {
                    subject: subject.clone(),
                    watched,
                    fire_at: Instant::now() + Duration::from_secs(secs as u64),
                },
            );
            continue;
        }
        if event.fires(prev, now) {
            try_fire(state, &rule).await;
        }
    }
}

/// Rooms whose occupancy could change with this sensor: every enabled room the
/// sensor is an effective member of (direct or via a synced-group link).
/// Over-inclusive is fine — occupancy is recomputed properly per room.
async fn rooms_containing_sensor(state: &AppState, sensor_id: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT DISTINCT r.id FROM rooms r
         WHERE r.enabled = 1 AND (
             EXISTS (SELECT 1 FROM room_sensor_devices rs
                      WHERE rs.room_id = r.id AND rs.sensor_device_id = ?1)
             OR EXISTS (SELECT 1 FROM room_links rl
                        JOIN provider_group_sensor_devices pgs
                          ON pgs.provider_group_id = rl.provider_group_id
                        WHERE rl.room_id = r.id AND pgs.sensor_device_id = ?1)
         )",
    )
    .bind(sensor_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| tracing::error!("db error resolving rooms for sensor: {e}"))
    .unwrap_or_default()
}

/// A room's occupancy as the engine sees it *right now*: the DB member list
/// overlaid with the engine's own in-memory readings — the DB cache is written
/// by a separate task and can trail the push event being processed.
async fn engine_room_occupancy(
    state: &AppState,
    engine: &EngineState,
    room_id: &str,
) -> Option<bool> {
    let members = crate::api::rooms::room_presence_readings_db(&state.db, room_id).await;
    (!members.is_empty()).then(|| {
        members.iter().any(|(id, db_detecting)| {
            engine
                .prev
                .get(id)
                .and_then(|r| r.as_bool())
                .unwrap_or(*db_detecting)
        })
    })
}

/// Process one sensor push event: detect the sensor's own edge, then any room
/// occupancy edges it causes, running both through the shared edge path.
pub(crate) async fn process_sensor_event(
    state: &AppState,
    engine: &mut EngineState,
    provider_row_id: &str,
    event: &SensorEvent,
) {
    let Some(reading) = event.state.reading else {
        return; // unreachable/empty report — keep prev intact
    };
    let Some(sensor) = sensor_for_event(state, provider_row_id, &event.device_id).await else {
        return;
    };
    let prev = engine.prev.get(&sensor.id).copied();
    engine.prev.insert(sensor.id.clone(), reading);
    if !sensor.enabled {
        return; // disabled sensors keep their prev current but trigger nothing
    }

    let rules = rules_for_sensor(state, &sensor.id).await;
    apply_edge(
        state,
        engine,
        &StaySubject::Sensor(sensor.id.clone()),
        rules,
        prev,
        reading,
    )
    .await;

    // Room phase: a presence reading can flip its rooms' aggregate occupancy.
    if !sensor.presence {
        return;
    }
    for room_id in rooms_containing_sensor(state, &sensor.id).await {
        let Some(occ) = engine_room_occupancy(state, engine, &room_id).await else {
            continue;
        };
        let room_prev = engine.room_prev.insert(room_id.clone(), occ);
        if room_prev == Some(occ) {
            continue; // no room-level edge
        }
        let rules = rules_for_room(state, &room_id).await;
        apply_edge(
            state,
            engine,
            &StaySubject::Room(room_id.clone()),
            rules,
            room_prev.map(SensorReading::Bool),
            SensorReading::Bool(occ),
        )
        .await;
    }
}

// ── Schedule (timer) rules ───────────────────────────────────────────────────

/// The Off-edge pass: power every target of the rule's actions **off**,
/// through the same shared service fns the On fire uses. Pure power — a light
/// keeps its colour/brightness for the next On hour — and unconditional (step
/// conditions gate the On direction; off is the safety direction). Non-power
/// actions can't be stored on a timer (writes validate), so they're skipped.
async fn execute_schedule_off(state: &AppState, rule: &Automation) {
    tracing::debug!(target: "bifrost::automation", rule = %rule.id, name = %rule.name, "timer off: powering targets down");
    let off = crate::models::LightState::default(); // on:false, no attributes
    for action in rule.all_actions() {
        match action {
            RuleAction::Room { room_id, .. } => {
                let members = crate::api::rooms::effective_members(state, room_id).await;
                let (applied, failed) =
                    crate::api::rooms::apply_room_state(state, room_id, &off, members).await;
                tracing::debug!(target: "bifrost::automation", rule = %rule.id, room = %room_id, applied, failed, "timer off: room");
            }
            RuleAction::Light { light_id, .. } => {
                let outcome = crate::api::lights::apply_light_state(state, light_id, &off).await;
                tracing::debug!(target: "bifrost::automation", rule = %rule.id, light = %light_id, ok = matches!(outcome, crate::api::lights::SetLightOutcome::Ok), "timer off: light");
            }
            RuleAction::Power { device_id, .. } => {
                let outcome = crate::api::power::apply_power_state(state, device_id, false).await;
                tracing::debug!(target: "bifrost::automation", rule = %rule.id, device = %device_id, ok = matches!(outcome, crate::api::power::SetPowerOutcome::Ok), "timer off: power");
            }
            RuleAction::Scene { .. } | RuleAction::App { .. } | RuleAction::Toggle { .. } => {}
        }
    }
}

/// Evaluate every schedule (timer) rule at the current local hour. `force`
/// evaluates even within an already-seen hour — the rule-edit path, where the
/// plan set itself may have changed.
pub(crate) async fn evaluate_schedules(state: &AppState, engine: &mut EngineState, force: bool) {
    use chrono::Timelike;
    evaluate_schedules_at(state, engine, chrono::Local::now().hour() as usize, force).await;
}

/// Evaluate schedule rules at `hour` and act on the plan's **edges**: going
/// active (an On hour begins) fires the rule through the normal gate
/// (conditions, cooldown) — its power-only actions turn the targets on;
/// going inactive powers the same targets off ([`execute_schedule_off`]).
/// A first observation while active reconciles like the kiosk scheduler (the
/// porch light still comes on after an evening restart); a first observation
/// while inactive does **nothing** — forcing everything off at every boot
/// would stomp whatever a restart interrupted, and the plan's next real Off
/// edge (tomorrow at the latest) squares things anyway. A rule that leaves
/// the schedule set (disabled, deleted, retargeted) just stops being managed.
pub(crate) async fn evaluate_schedules_at(
    state: &AppState,
    engine: &mut EngineState,
    hour: usize,
    force: bool,
) {
    if !force && engine.schedule_hour == Some(hour) {
        return; // no boundary since the last pass — nothing can have edged
    }
    // A failed read is NOT "no rules": clearing the verdict map on emptiness
    // would make the next tick treat every active timer as a first
    // observation and re-fire it mid-hour, stomping manual overrides. Leave
    // all engine state untouched and let a later pass retry.
    let Ok(rules) = non_sensor_rules_checked(state).await else {
        return;
    };
    engine.schedule_hour = Some(hour);
    let rules: Vec<Automation> = rules
        .into_iter()
        .filter(|r| r.trigger.schedule().is_some())
        .collect();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for rule in &rules {
        let Some(hour_modes) = rule.trigger.schedule() else {
            continue;
        };
        seen.insert(&rule.id);
        // Off-only semantics for a stray 'A' (unreachable — writes validate).
        let Some(active) = plan_mode(hour_modes, hour).and_then(|mode| plan_desired(mode, None))
        else {
            continue;
        };
        let prev = engine.schedule_prev.insert(rule.id.clone(), active);
        if prev == Some(active) {
            continue; // no plan edge
        }
        if active {
            // Timers skip the cooldown gate: they fire at most once an hour by
            // construction, and a cooldown spanning two On windows would
            // silently kill the second window's turn-on while its Off edge
            // still ran. Conditions still gate (and writes reject a nonzero
            // cooldown on timers anyway).
            tracing::debug!(target: "bifrost::automation", rule = %rule.id, hour, "timer on-edge: firing");
            if conditions_hold(state, &rule.conditions).await {
                execute_rule(state, rule).await;
            }
        } else if prev.is_some() {
            execute_schedule_off(state, rule).await;
        }
    }
    // Rules gone from the schedule set stop being tracked (and managed).
    engine
        .schedule_prev
        .retain(|id, _| seen.contains(id.as_str()));
}

/// Fire any due stay timers. The subject must still hold the watched state (a
/// missed cancel must not fire a stale timer) and the rule must still exist
/// and be enabled (edits/deletes since arming win).
pub(crate) async fn fire_due_timers(state: &AppState, engine: &mut EngineState) {
    let now = Instant::now();
    let due: Vec<(String, StaySubject, bool)> = engine
        .pending
        .iter()
        .filter(|(_, p)| p.fire_at <= now)
        .map(|(rule_id, p)| (rule_id.clone(), p.subject.clone(), p.watched))
        .collect();
    for (rule_id, subject, watched) in due {
        engine.pending.remove(&rule_id);
        let still_held = match &subject {
            StaySubject::Sensor(id) => {
                cached_reading(state, id)
                    .await
                    .and_then(SensorReading::as_bool)
                    == Some(watched)
            }
            // By fire time (≥30s after arming) the DB cache has settled, so the
            // shared room_occupancy read is authoritative.
            StaySubject::Room(id) => {
                crate::api::rooms::room_occupancy(state, id).await == Some(watched)
            }
            StaySubject::Device(domain, id) => {
                cached_device_on(state, *domain, id).await == Some(watched)
            }
        };
        if !still_held {
            tracing::debug!(target: "bifrost::automation", rule = %rule_id, "stay timer lapsed: subject no longer holds the watched state");
            continue;
        }
        let rule = match sqlx::query("SELECT * FROM automations WHERE id = ? AND enabled = 1")
            .bind(&rule_id)
            .fetch_optional(&state.db)
            .await
        {
            Ok(Some(r)) => row_to_automation(r),
            _ => continue, // deleted or disabled since arming
        };
        try_fire(state, &rule).await;
    }
}

/// Arm the stay timer for one rule whose subject is already in the watched
/// state — the restart/new-rule recovery path (idempotent actions make this
/// safe). Never re-arms: a rule whose timer is already running keeps its
/// original deadline (a re-seed must not reset "empty for 15 minutes").
fn seed_stay(engine: &mut EngineState, rule: &Automation, subject: StaySubject, current: bool) {
    if engine.pending.contains_key(&rule.id) {
        return;
    }
    if let Some((watched, secs)) = rule.trigger.event().and_then(|e| e.stay_watch())
        && current == watched
    {
        engine.pending.insert(
            rule.id.clone(),
            PendingStay {
                subject,
                watched,
                fire_at: Instant::now() + Duration::from_secs(secs as u64),
            },
        );
    }
}

/// Seed the previous-reading maps (and re-arm stay timers) from the DB cache,
/// so a restart doesn't lose "empty since" state.
async fn seed_engine(state: &AppState, engine: &mut EngineState) {
    // Sensors: previous readings + stay timers for already-held states.
    let rows = sqlx::query("SELECT id, last_state FROM sensor_devices WHERE shadowed_by IS NULL")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    for r in rows {
        let id: String = r.get("id");
        let reading = r
            .get::<Option<String>, _>("last_state")
            .and_then(|s| serde_json::from_str::<SensorState>(&s).ok())
            .and_then(|s| s.reading);
        if let Some(reading) = reading {
            // Fill-only: a re-seed (rules changed mid-flight) must not clobber
            // a fresher in-memory reading with the DB cache.
            let effective = *engine.prev.entry(id.clone()).or_insert(reading);
            if let Some(current) = effective.as_bool() {
                for rule in rules_for_sensor(state, &id).await {
                    seed_stay(engine, &rule, StaySubject::Sensor(id.clone()), current);
                }
            }
        }
    }

    // Rooms with room-triggered rules: occupancy baseline + stay timers.
    let room_rules: Vec<Automation> =
        sqlx::query("SELECT * FROM automations WHERE sensor_id IS NULL AND enabled = 1")
            .fetch_all(&state.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(row_to_automation)
            .collect();
    for rule in &room_rules {
        // Device rules: baseline the power boolean and re-arm stay timers.
        if let Some((domain, device_id)) = rule.trigger.device() {
            if let Some(on) = cached_device_on(state, domain, device_id).await {
                let effective = *engine
                    .device_prev
                    .entry((domain, device_id.to_string()))
                    .or_insert(on);
                seed_stay(
                    engine,
                    rule,
                    StaySubject::Device(domain, device_id.to_string()),
                    effective,
                );
            }
            continue;
        }
        let Some(room_id) = rule.trigger.room_id() else {
            continue;
        };
        let occ = match engine.room_prev.get(room_id) {
            Some(o) => Some(*o),
            None => {
                let o = crate::api::rooms::room_occupancy(state, room_id).await;
                if let Some(o) = o {
                    engine.room_prev.insert(room_id.to_string(), o);
                }
                o
            }
        };
        if let Some(occ) = occ {
            seed_stay(engine, rule, StaySubject::Room(room_id.to_string()), occ);
        }
    }
}

/// The background engine loop: merge every provider's push streams, process
/// events as they arrive, and tick armed timers.
pub async fn run_engine(state: Arc<AppState>) {
    use futures_util::StreamExt;
    use tokio_stream::wrappers::BroadcastStream;

    let mut engine = EngineState::default();
    seed_engine(&state, &mut engine).await;
    seed_hold_watch(&state).await;

    /// One event off any of the pipelines the engine watches.
    enum EngineEvent {
        Sensor(String, crate::connection::SensorEvent),
        Light(crate::connection::LightEvent),
        Media(String, crate::models::media::MediaEvent),
        Power(String, crate::connection::PowerEvent),
    }

    // Subscribed ONCE, on the registry's app-wide fan-in channels: they outlive
    // every manager restart and carry providers added later, so the engine can
    // never go deaf for a provider (see `ConnectionRegistry`).
    let (sensor_rx, light_rx, media_rx, power_rx) = {
        let connections = state.connections.lock().await;
        (
            connections.subscribe_sensors(),
            connections.subscribe_lights(),
            connections.subscribe_media(),
            connections.subscribe_power(),
        )
    };
    let streams: Vec<futures_util::stream::BoxStream<'static, EngineEvent>> = vec![
        BroadcastStream::new(sensor_rx)
            .filter_map(|r| std::future::ready(r.ok()))
            .map(|(provider_id, ev)| EngineEvent::Sensor(provider_id, ev))
            .boxed(),
        BroadcastStream::new(light_rx)
            .filter_map(|r| std::future::ready(r.ok()))
            .map(EngineEvent::Light)
            .boxed(),
        BroadcastStream::new(media_rx)
            .filter_map(|r| std::future::ready(r.ok()))
            .map(|(provider_id, ev)| EngineEvent::Media(provider_id, ev))
            .boxed(),
        BroadcastStream::new(power_rx)
            .filter_map(|r| std::future::ready(r.ok()))
            .map(|(provider_id, ev)| EngineEvent::Power(provider_id, ev))
            .boxed(),
        // Never terminate: the fan-ins are alive for the process's life, but a
        // pending arm keeps select_all honest if one is ever dropped.
        futures_util::stream::pending().boxed(),
    ];
    let mut merged = futures_util::stream::select_all(streams);
    let mut tick = tokio::time::interval(Duration::from_secs(5));

    loop {
        tokio::select! {
            ev = merged.next() => match ev {
                Some(EngineEvent::Sensor(provider_id, event)) => {
                    process_sensor_event(&state, &mut engine, &provider_id, &event).await;
                }
                Some(EngineEvent::Light(event)) => {
                    // Light events aren't provider-tagged (the SSE feed
                    // matches them the same way). Resolve the row once for
                    // both consumers: the hold watch sees every patch (a
                    // manual brightness/colour nudge has no `on`), the
                    // device-trigger path only power edges. Skip the
                    // lookup when neither can care.
                    let watching = !state.hold_watch.inner.lock().await.is_empty();
                    if event.patch.on.is_some() || watching {
                        let row: Option<String> = sqlx::query_scalar(
                            "SELECT id FROM lights WHERE device_id = ? AND enabled = 1 AND shadowed_by IS NULL",
                        )
                        .bind(&event.device_id)
                        .fetch_optional(&state.db)
                        .await
                        .ok()
                        .flatten();
                        if let Some(id) = row {
                            if watching {
                                observe_hold_light(&state, &id, &event.patch).await;
                            }
                            if let Some(on) = event.patch.on {
                                process_device_event(&state, &mut engine, TriggerDeviceDomain::Light, &id, on).await;
                            }
                        }
                    }
                }
                Some(EngineEvent::Media(provider_id, event)) => {
                    let row: Option<String> = sqlx::query_scalar(
                        "SELECT id FROM media_devices WHERE provider_id = ? AND device_id = ? AND enabled = 1 AND shadowed_by IS NULL",
                    )
                    .bind(&provider_id)
                    .bind(&event.device_id)
                    .fetch_optional(&state.db)
                    .await
                    .ok()
                    .flatten();
                    if let Some(id) = row {
                        process_device_event(&state, &mut engine, TriggerDeviceDomain::Media, &id, event.state.power).await;
                    }
                }
                Some(EngineEvent::Power(provider_id, event)) => {
                    let row: Option<String> = sqlx::query_scalar(
                        "SELECT id FROM power_devices WHERE provider_id = ? AND device_id = ? AND enabled = 1 AND shadowed_by IS NULL",
                    )
                    .bind(&provider_id)
                    .bind(&event.device_id)
                    .fetch_optional(&state.db)
                    .await
                    .ok()
                    .flatten();
                    if let Some(id) = row {
                        observe_hold_power(&state, &id, event.state.on).await;
                        process_device_event(&state, &mut engine, TriggerDeviceDomain::Power, &id, event.state.on).await;
                    }
                }
                None => break, // unreachable: the pending arm never ends
            },
            _ = tick.tick() => {
                evaluate_schedules(&state, &mut engine, false).await;
                fire_due_timers(&state, &mut engine).await;
                apply_due_restores(&state).await;
            }
            // A rule was created/edited: baseline its subject now (fill-only,
            // so live readings and armed timers are untouched) — a rule made
            // mid-flight must fire on the next edge, not after a restart.
            // Schedule rules reconcile immediately: a timer painted active
            // for the current hour takes effect now, not at the next tick.
            _ = state.automations_changed.notified() => {
                seed_engine(&state, &mut engine).await;
                evaluate_schedules(&state, &mut engine, true).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::default_registry;
    use sqlx::SqlitePool;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    async fn test_state() -> Arc<AppState> {
        let opts = SqliteConnectOptions::from_str(":memory:")
            .unwrap()
            .foreign_keys(true);
        let db = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("./migrations").run(&db).await.unwrap();
        let state = Arc::new(AppState::new(
            db,
            "test-secret-key-32-bytes-exactly",
            default_registry(),
        ));
        sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES ('p','ha','HA','x')")
            .execute(&state.db)
            .await
            .unwrap();
        state
    }

    async fn seed_sensor(state: &AppState, id: &str, kind: &str, last_state: &str) {
        sqlx::query(
            "INSERT INTO sensor_devices (id, provider_id, device_id, name, kind, last_state)
             VALUES (?, 'p', ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(id)
        .bind(id)
        .bind(kind)
        .bind(last_state)
        .execute(&state.db)
        .await
        .unwrap();
    }

    /// Insert a rule directly (bypassing create_rule validation) so tests can
    /// use short clear-for timers. Actions target a nonexistent power device —
    /// best-effort execution still stamps `last_fired_at`, which is the
    /// observable "it fired".
    async fn seed_rule(state: &AppState, id: &str, sensor: &str, event: &str, conditions: &str) {
        let trigger = format!(r#"{{"kind":"sensor","sensor_id":"{sensor}","event":{event}}}"#);
        sqlx::query(
            "INSERT INTO automations (id, sensor_id, trigger_json, conditions_json, actions_json)
             VALUES (?, ?, ?, ?, '[{\"conditions\":[],\"actions\":[{\"kind\":\"power\",\"device_id\":\"nope\",\"on\":true}]}]')",
        )
        .bind(id)
        .bind(sensor)
        .bind(&trigger)
        .bind(conditions)
        .execute(&state.db)
        .await
        .unwrap();
    }

    /// Insert a room-triggered rule (no sensor lookup column — matched in code).
    async fn seed_room_rule(state: &AppState, id: &str, room: &str, event: &str) {
        let trigger = format!(r#"{{"kind":"room","room_id":"{room}","event":{event}}}"#);
        sqlx::query(
            "INSERT INTO automations (id, trigger_json, actions_json)
             VALUES (?, ?, '[{\"conditions\":[],\"actions\":[{\"kind\":\"power\",\"device_id\":\"nope\",\"on\":true}]}]')",
        )
        .bind(id)
        .bind(&trigger)
        .execute(&state.db)
        .await
        .unwrap();
    }

    /// A room with the given sensors as direct members.
    async fn seed_room(state: &AppState, room: &str, sensor_ids: &[&str]) {
        sqlx::query("INSERT INTO rooms (id, name) VALUES (?, ?)")
            .bind(room)
            .bind(room)
            .execute(&state.db)
            .await
            .unwrap();
        for sid in sensor_ids {
            sqlx::query(
                "INSERT INTO room_sensor_devices (room_id, sensor_device_id) VALUES (?, ?)",
            )
            .bind(room)
            .bind(sid)
            .execute(&state.db)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn unless_device_gate_blocks_while_the_device_is_on() {
        // "if motion detected … unless the TV is on": a device_is{on:false}
        // condition holds only while the TV is off — the "unless" clause.
        let state = test_state().await;
        seed_sensor(
            &state,
            "s1",
            "motion",
            r#"{"reading":{"bool":false},"reachable":true}"#,
        )
        .await;
        sqlx::query(
            r#"INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state)
               VALUES ('tv1', 'p', 'tv-1', 'TV', 'tv', '{}', '{"power":true,"volume":0,"mute":false}')"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        seed_rule(
            &state,
            "r1",
            "s1",
            r#"{"kind":"became_true"}"#,
            r#"[{"kind":"device_is","domain":"media","device_id":"tv1","on":false}]"#,
        )
        .await;
        let mut engine = EngineState::default();
        seed_engine(&state, &mut engine).await;

        // Motion while the TV is ON → the unless clause blocks the fire.
        let ev = |b: bool| SensorEvent {
            device_id: "s1".into(),
            state: crate::models::sensor::SensorState {
                reading: Some(crate::models::sensor::SensorReading::Bool(b)),
                reachable: Some(true),
                changed_at: None,
            },
        };
        process_sensor_event(&state, &mut engine, "p", &ev(true)).await;
        assert!(
            !fired(&state, "r1").await,
            "TV on — the unless clause must block"
        );

        // TV goes off; the next motion edge fires.
        sqlx::query(r#"UPDATE media_devices SET last_state = '{"power":false,"volume":0,"mute":false}' WHERE id = 'tv1'"#)
            .execute(&state.db)
            .await
            .unwrap();
        process_sensor_event(&state, &mut engine, "p", &ev(false)).await;
        process_sensor_event(&state, &mut engine, "p", &ev(true)).await;
        assert!(
            fired(&state, "r1").await,
            "TV off — the rule fires normally"
        );
    }

    async fn fired(state: &AppState, rule: &str) -> bool {
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT last_fired_at FROM automations WHERE id = ?",
        )
        .bind(rule)
        .fetch_one(&state.db)
        .await
        .unwrap()
        .is_some()
    }

    fn motion_event(device_id: &str, detected: bool) -> SensorEvent {
        SensorEvent {
            device_id: device_id.into(),
            state: SensorState::boolean(detected),
        }
    }

    #[tokio::test]
    async fn rising_edge_fires_the_rule_but_a_level_does_not() {
        let state = test_state().await;
        seed_sensor(&state, "m1", "motion", r#"{"reading":{"bool":false}}"#).await;
        seed_rule(&state, "r1", "m1", r#"{"kind":"became_true"}"#, "[]").await;
        let mut engine = EngineState::default();
        seed_engine(&state, &mut engine).await;

        process_sensor_event(&state, &mut engine, "p", &motion_event("m1", true)).await;
        assert!(fired(&state, "r1").await, "rising edge must fire");

        // Reset the stamp; a repeated `true` (level, no edge) must not re-fire.
        sqlx::query("UPDATE automations SET last_fired_at = NULL WHERE id = 'r1'")
            .execute(&state.db)
            .await
            .unwrap();
        process_sensor_event(&state, &mut engine, "p", &motion_event("m1", true)).await;
        assert!(!fired(&state, "r1").await, "a level must not re-fire");
    }

    #[tokio::test]
    async fn disabled_sensor_and_disabled_rule_never_fire() {
        let state = test_state().await;
        seed_sensor(&state, "m1", "motion", r#"{"reading":{"bool":false}}"#).await;
        seed_rule(&state, "r1", "m1", r#"{"kind":"became_true"}"#, "[]").await;
        sqlx::query("UPDATE sensor_devices SET enabled = 0 WHERE id = 'm1'")
            .execute(&state.db)
            .await
            .unwrap();
        let mut engine = EngineState::default();
        seed_engine(&state, &mut engine).await;
        process_sensor_event(&state, &mut engine, "p", &motion_event("m1", true)).await;
        assert!(
            !fired(&state, "r1").await,
            "disabled sensor must not trigger"
        );

        sqlx::query("UPDATE sensor_devices SET enabled = 1 WHERE id = 'm1'")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("UPDATE automations SET enabled = 0 WHERE id = 'r1'")
            .execute(&state.db)
            .await
            .unwrap();
        process_sensor_event(&state, &mut engine, "p", &motion_event("m1", false)).await;
        process_sensor_event(&state, &mut engine, "p", &motion_event("m1", true)).await;
        assert!(!fired(&state, "r1").await, "disabled rule must not fire");
    }

    #[tokio::test]
    async fn unmet_sensor_condition_gates_the_fire() {
        let state = test_state().await;
        seed_sensor(&state, "m1", "motion", r#"{"reading":{"bool":false}}"#).await;
        // Gate on a lux sensor currently reading bright (50 lx).
        seed_sensor(
            &state,
            "lux",
            "illuminance",
            r#"{"reading":{"number":50.0}}"#,
        )
        .await;
        seed_rule(
            &state,
            "r1",
            "m1",
            r#"{"kind":"became_true"}"#,
            r#"[{"kind":"sensor_below","sensor_id":"lux","value":20.0}]"#,
        )
        .await;
        let mut engine = EngineState::default();
        seed_engine(&state, &mut engine).await;
        process_sensor_event(&state, &mut engine, "p", &motion_event("m1", true)).await;
        assert!(!fired(&state, "r1").await, "bright room must gate the rule");

        // Darkness satisfies the gate on the next edge.
        sqlx::query(
            r#"UPDATE sensor_devices SET last_state = '{"reading":{"number":5.0}}' WHERE id = 'lux'"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        process_sensor_event(&state, &mut engine, "p", &motion_event("m1", false)).await;
        process_sensor_event(&state, &mut engine, "p", &motion_event("m1", true)).await;
        assert!(
            fired(&state, "r1").await,
            "dark room must let the rule fire"
        );
    }

    #[tokio::test]
    async fn clear_for_arms_fires_after_the_wait_and_cancels_on_motion() {
        let state = test_state().await;
        seed_sensor(&state, "m1", "motion", r#"{"reading":{"bool":true}}"#).await;
        seed_rule(&state, "r1", "m1", r#"{"kind":"clear_for","secs":1}"#, "[]").await;
        let mut engine = EngineState::default();
        seed_engine(&state, &mut engine).await;

        // Falling edge arms the timer; motion again cancels it.
        process_sensor_event(&state, &mut engine, "p", &motion_event("m1", false)).await;
        assert!(engine.pending.contains_key("r1"));
        process_sensor_event(&state, &mut engine, "p", &motion_event("m1", true)).await;
        assert!(engine.pending.is_empty(), "motion must cancel the timer");

        // Arm again; after the wait the timer fires (sensor still clear —
        // process_sensor_event also updated the DB-adjacent cache used by the
        // fire check via the sensor_db_writer in prod; emulate it here).
        process_sensor_event(&state, &mut engine, "p", &motion_event("m1", false)).await;
        sqlx::query(
            r#"UPDATE sensor_devices SET last_state = '{"reading":{"bool":false},"reachable":true}' WHERE id = 'm1'"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        // Backdate the armed timer instead of sleeping past it in real time.
        engine.pending.get_mut("r1").unwrap().fire_at = Instant::now();
        fire_due_timers(&state, &mut engine).await;
        assert!(fired(&state, "r1").await, "due timer must fire");
        assert!(engine.pending.is_empty());
    }

    #[tokio::test]
    async fn startup_seed_arms_clear_for_on_an_already_clear_sensor() {
        let state = test_state().await;
        seed_sensor(&state, "m1", "motion", r#"{"reading":{"bool":false}}"#).await;
        seed_rule(
            &state,
            "r1",
            "m1",
            r#"{"kind":"clear_for","secs":600}"#,
            "[]",
        )
        .await;
        let mut engine = EngineState::default();
        seed_engine(&state, &mut engine).await;
        assert!(
            engine.pending.contains_key("r1"),
            "restart with an empty room must re-arm the off-timer"
        );
    }

    #[tokio::test]
    async fn held_for_arms_on_rising_edge_and_fires_while_still_held() {
        let state = test_state().await;
        seed_sensor(&state, "door", "contact", r#"{"reading":{"bool":false}}"#).await;
        seed_rule(
            &state,
            "r1",
            "door",
            r#"{"kind":"held_for","secs":1}"#,
            "[]",
        )
        .await;
        let mut engine = EngineState::default();
        seed_engine(&state, &mut engine).await;

        // Rising edge arms; closing again cancels.
        process_sensor_event(&state, &mut engine, "p", &motion_event("door", true)).await;
        assert!(engine.pending.contains_key("r1"));
        process_sensor_event(&state, &mut engine, "p", &motion_event("door", false)).await;
        assert!(engine.pending.is_empty(), "closing must cancel the timer");

        // Arm again and let it fire — the reading must still be `true`.
        process_sensor_event(&state, &mut engine, "p", &motion_event("door", true)).await;
        sqlx::query(
            r#"UPDATE sensor_devices SET last_state = '{"reading":{"bool":true},"reachable":true}' WHERE id = 'door'"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        // Backdate the armed timer instead of sleeping past it in real time.
        engine.pending.get_mut("r1").unwrap().fire_at = Instant::now();
        fire_due_timers(&state, &mut engine).await;
        assert!(
            fired(&state, "r1").await,
            "held timer must fire while still open"
        );
    }

    #[tokio::test]
    async fn room_occupancy_edge_fires_room_rules_not_per_sensor_noise() {
        let state = test_state().await;
        // Two presence sensors in one room; the room is occupied while EITHER
        // detects, so only the aggregate edge may fire the rule.
        seed_sensor(&state, "m1", "motion", r#"{"reading":{"bool":false}}"#).await;
        seed_sensor(&state, "m2", "motion", r#"{"reading":{"bool":true}}"#).await;
        seed_room(&state, "office", &["m1", "m2"]).await;
        seed_room_rule(&state, "r-occ", "office", r#"{"kind":"became_false"}"#).await;
        let mut engine = EngineState::default();
        seed_engine(&state, &mut engine).await;

        // m1 flips true then false while m2 still detects: room stays occupied,
        // no aggregate edge, no fire.
        process_sensor_event(&state, &mut engine, "p", &motion_event("m1", true)).await;
        process_sensor_event(&state, &mut engine, "p", &motion_event("m1", false)).await;
        assert!(!fired(&state, "r-occ").await, "room never became empty");

        // m2 clears too → the room's occupancy falls → the room rule fires.
        sqlx::query(
            r#"UPDATE sensor_devices SET last_state = '{"reading":{"bool":false}}' WHERE id = 'm1'"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        process_sensor_event(&state, &mut engine, "p", &motion_event("m2", false)).await;
        assert!(
            fired(&state, "r-occ").await,
            "the aggregate empty edge must fire"
        );
    }

    #[tokio::test]
    async fn device_power_edge_fires_device_rules_and_stays_baselined() {
        let state = test_state().await;
        sqlx::query(
            "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state)
             VALUES ('tv1', 'p', 'media_player.tv', 'TV', 'tv', '{}', '{\"power\":false,\"volume\":0,\"mute\":false}')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO automations (id, trigger_json, actions_json)
             VALUES ('r1', '{\"kind\":\"device\",\"domain\":\"media\",\"device_id\":\"tv1\",\"event\":{\"kind\":\"became_true\"}}',
                     '[{\"conditions\":[],\"actions\":[{\"kind\":\"power\",\"device_id\":\"nope\",\"on\":true}]}]')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        let mut engine = EngineState::default();
        seed_engine(&state, &mut engine).await;
        // Startup baselined the TV as off.
        assert_eq!(
            engine
                .device_prev
                .get(&(TriggerDeviceDomain::Media, "tv1".to_string())),
            Some(&false)
        );

        // The TV coming on (e.g. via its own remote, seen by the demand poller)
        // fires the rule; a repeated on-snapshot doesn't re-fire.
        process_device_event(&state, &mut engine, TriggerDeviceDomain::Media, "tv1", true).await;
        assert!(fired(&state, "r1").await, "power edge must fire");
        sqlx::query("UPDATE automations SET last_fired_at = NULL WHERE id = 'r1'")
            .execute(&state.db)
            .await
            .unwrap();
        process_device_event(&state, &mut engine, TriggerDeviceDomain::Media, "tv1", true).await;
        assert!(
            !fired(&state, "r1").await,
            "an unchanged snapshot must not re-fire"
        );
    }

    #[tokio::test]
    async fn rule_created_mid_flight_baselines_on_reseed_and_fires_on_next_edge() {
        // The dev-instance scenario: the engine has been running (no rules),
        // a device rule is created, and the device's first event after that is
        // already "on" — the notify-driven re-seed must have baselined "off"
        // from the DB cache so that first event IS the edge.
        let state = test_state().await;
        sqlx::query(
            "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state)
             VALUES ('tv1', 'p', 'media_player.tv', 'TV', 'tv', '{}', '{\"power\":false,\"volume\":0,\"mute\":false}')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        let mut engine = EngineState::default();
        seed_engine(&state, &mut engine).await; // boot: no rules yet
        assert!(engine.device_prev.is_empty());

        // Rule arrives mid-flight; the run loop re-seeds on the notify.
        sqlx::query(
            "INSERT INTO automations (id, trigger_json, actions_json)
             VALUES ('r1', '{\"kind\":\"device\",\"domain\":\"media\",\"device_id\":\"tv1\",\"event\":{\"kind\":\"became_true\"}}',
                     '[{\"conditions\":[],\"actions\":[{\"kind\":\"power\",\"device_id\":\"nope\",\"on\":true}]}]')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        seed_engine(&state, &mut engine).await;
        assert_eq!(
            engine
                .device_prev
                .get(&(TriggerDeviceDomain::Media, "tv1".to_string())),
            Some(&false),
            "re-seed must baseline the new rule's device from the DB cache"
        );

        // The demand poller's first emission (TV already on) is now an edge.
        process_device_event(&state, &mut engine, TriggerDeviceDomain::Media, "tv1", true).await;
        assert!(fired(&state, "r1").await);
    }

    #[tokio::test]
    async fn timed_hold_snapshots_before_actions_and_restores_after() {
        let state = test_state().await;
        sqlx::query(
            r#"INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state)
               VALUES ('l1', 'p', 'bulb-1', 'Desk lamp', '{}', '{"on":true,"brightness":80.0}')"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO automations (id, trigger_json, actions_json, restore_secs)
             VALUES ('r1', '{\"kind\":\"sensor\",\"sensor_id\":\"x\",\"event\":{\"kind\":\"became_true\"}}',
                     '[{\"conditions\":[],\"actions\":[{\"kind\":\"light\",\"light_id\":\"l1\",\"state\":{\"on\":false}}]}]', 600)",
        )
        .execute(&state.db)
        .await
        .unwrap();
        let rule = match fetch_rule(&state, "r1").await {
            SaveRuleOutcome::Ok(r) => r,
            _ => unreachable!(),
        };

        execute_rule(&state, &rule).await;
        // The pre-fire state (on at 80%) was captured before the off action.
        let snap: String = sqlx::query_scalar(
            "SELECT snapshot_json FROM automation_restores WHERE automation_id = 'r1'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert!(
            snap.contains("80"),
            "snapshot must hold the pre-fire brightness: {snap}"
        );

        // A re-fire during the hold keeps the ORIGINAL snapshot (re-capturing
        // would snapshot the rule's own triggered state).
        sqlx::query(r#"UPDATE lights SET last_state = '{"on":false}' WHERE id = 'l1'"#)
            .execute(&state.db)
            .await
            .unwrap();
        execute_rule(&state, &rule).await;
        let snap2: String = sqlx::query_scalar(
            "SELECT snapshot_json FROM automation_restores WHERE automation_id = 'r1'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert_eq!(snap, snap2, "re-fire must not re-snapshot");

        // Force the hold due; the tick applies it and clears the row.
        sqlx::query(
            "UPDATE automation_restores SET restore_at = datetime('now', '-1 seconds') WHERE automation_id = 'r1'",
        )
        .execute(&state.db)
        .await
        .unwrap();
        apply_due_restores(&state).await;
        let remaining: Option<String> = sqlx::query_scalar(
            "SELECT snapshot_json FROM automation_restores WHERE automation_id = 'r1'",
        )
        .fetch_optional(&state.db)
        .await
        .unwrap();
        assert!(remaining.is_none(), "an applied hold must be cleared");
    }

    #[tokio::test]
    async fn overlapping_holds_inherit_the_first_captured_original() {
        // Two rules with holds touch the same light. Whichever fires second
        // only sees the first rule's OUTPUT — it must inherit the first
        // capture, so the final restore (in either timer order) is the true
        // pre-automation state.
        let state = test_state().await;
        sqlx::query(
            r#"INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state)
               VALUES ('l1', 'p', 'bulb-1', 'Lamp', '{}', '{"on":false}')"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        for (id, action) in [
            (
                "rA",
                r#"[{"conditions":[],"actions":[{"kind":"light","light_id":"l1","state":{"on":true}}]}]"#,
            ),
            (
                "rB",
                r#"[{"conditions":[],"actions":[{"kind":"light","light_id":"l1","state":{"on":true,"brightness":100.0}}]}]"#,
            ),
        ] {
            sqlx::query(
                "INSERT INTO automations (id, trigger_json, actions_json, restore_secs)
                 VALUES (?, '{\"kind\":\"sensor\",\"sensor_id\":\"x\",\"event\":{\"kind\":\"became_true\"}}', ?, 600)",
            )
            .bind(id)
            .bind(action)
            .execute(&state.db)
            .await
            .unwrap();
        }
        let rule = |id: &str| {
            let state = &state;
            let id = id.to_string();
            async move {
                match fetch_rule(state, &id).await {
                    SaveRuleOutcome::Ok(r) => r,
                    _ => unreachable!(),
                }
            }
        };

        // Rule A fires against the true original (off), then its action lands.
        execute_rule(&state, &rule("rA").await).await;
        sqlx::query(
            r#"UPDATE lights SET last_state = '{"on":true,"brightness":40.0}' WHERE id = 'l1'"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        // Rule B fires while A's hold is pending — it sees "on at 40" but must
        // inherit A's captured "off".
        execute_rule(&state, &rule("rB").await).await;
        let snap_b: String = sqlx::query_scalar(
            "SELECT snapshot_json FROM automation_restores WHERE automation_id = 'rB'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
        let entries: Vec<RestoreEntry> = serde_json::from_str(&snap_b).unwrap();
        assert!(
            matches!(&entries[0], RestoreEntry::Light { state, .. } if !state.on),
            "rule B must inherit the true original (off), got: {snap_b}"
        );
    }

    fn lstate(json: &str) -> crate::models::LightState {
        serde_json::from_str(json).unwrap()
    }
    fn lpatch(json: &str) -> crate::models::LightStatePatch {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn light_patch_divergence_is_toleranced() {
        let held = lstate(r#"{"on":true,"brightness":50.0,"color_temp_mirek":300}"#);
        // A power flip always diverges; a matching power echo never does.
        assert!(light_patch_diverges(&held, &lpatch(r#"{"on":false}"#)));
        assert!(!light_patch_diverges(&held, &lpatch(r#"{"on":true}"#)));
        // Brightness: device rounding stays, a real nudge diverges.
        assert!(!light_patch_diverges(
            &held,
            &lpatch(r#"{"brightness":51.5}"#)
        ));
        assert!(light_patch_diverges(
            &held,
            &lpatch(r#"{"brightness":80.0}"#)
        ));
        // Colour temperature.
        assert!(!light_patch_diverges(
            &held,
            &lpatch(r#"{"color_temp_mirek":305}"#)
        ));
        assert!(light_patch_diverges(
            &held,
            &lpatch(r#"{"color_temp_mirek":400}"#)
        ));
        // A dimension the reference has never seen can't diverge (fill-only).
        assert!(!light_patch_diverges(
            &held,
            &lpatch(r#"{"color":{"x":0.5,"y":0.4,"brightness":0.8}}"#)
        ));
        // A reachability drop is a device going dark, not a manual change.
        assert!(!light_patch_diverges(
            &held,
            &lpatch(r#"{"on":false,"reachable":false}"#)
        ));
        // An off light's attribute echoes are stale — only its power edge counts.
        let held_off = lstate(r#"{"on":false,"brightness":50.0}"#);
        assert!(!light_patch_diverges(
            &held_off,
            &lpatch(r#"{"brightness":100.0}"#)
        ));
        assert!(light_patch_diverges(&held_off, &lpatch(r#"{"on":true}"#)));
    }

    #[test]
    fn colour_divergence_is_toleranced() {
        let held = lstate(r#"{"on":true,"color":{"x":0.5,"y":0.4,"brightness":0.8}}"#);
        assert!(!light_patch_diverges(
            &held,
            &lpatch(r#"{"color":{"x":0.51,"y":0.41,"brightness":0.8}}"#)
        ));
        assert!(light_patch_diverges(
            &held,
            &lpatch(r#"{"color":{"x":0.2,"y":0.7,"brightness":0.8}}"#)
        ));
    }

    /// Seed one held light: a pending restore row + its watch entry
    /// (reference = the cached `last_state`).
    async fn seed_held_light(state: &AppState, reference: &str) {
        sqlx::query(
            "INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state)
               VALUES ('l1', 'p', 'bulb-1', 'Lamp', '{}', ?)",
        )
        .bind(reference)
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO automations (id, trigger_json, actions_json, restore_secs)
             VALUES ('r1', '{\"kind\":\"sensor\",\"sensor_id\":\"x\",\"event\":{\"kind\":\"became_true\"}}',
                     '[]', 600)",
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO automation_restores (automation_id, restore_at, snapshot_json)
               VALUES ('r1', datetime('now', '+600 seconds'),
                       '[{"kind":"light","light_id":"l1","state":{"on":false}}]')"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        watch_hold_devices(
            state,
            &[RestoreEntry::Light {
                light_id: "l1".into(),
                state: lstate(r#"{"on":false}"#),
            }],
        )
        .await;
    }

    async fn hold_pending(state: &AppState, rule: &str) -> bool {
        sqlx::query_scalar::<_, String>(
            "SELECT snapshot_json FROM automation_restores WHERE automation_id = ?",
        )
        .bind(rule)
        .fetch_optional(&state.db)
        .await
        .unwrap()
        .is_some()
    }

    #[tokio::test]
    async fn manual_change_releases_a_held_light() {
        let state = test_state().await;
        seed_held_light(&state, r#"{"on":true,"brightness":50.0}"#).await;
        let after_grace = Instant::now() + Duration::from_secs(60);

        // The rule's own poll echo (identical state) keeps the hold.
        observe_hold_light_at(
            &state,
            "l1",
            &lpatch(r#"{"on":true,"brightness":50.0}"#),
            after_grace,
        )
        .await;
        assert!(
            hold_pending(&state, "r1").await,
            "an echo must not release the hold"
        );

        // A manual brightness nudge releases the device — and here it was the
        // snapshot's only entry, so the whole row goes.
        observe_hold_light_at(&state, "l1", &lpatch(r#"{"brightness":90.0}"#), after_grace).await;
        assert!(
            !hold_pending(&state, "r1").await,
            "a manual change must release the device from the hold"
        );
        assert!(
            state.hold_watch.inner.lock().await.is_empty(),
            "a released device must leave the watch"
        );
    }

    #[tokio::test]
    async fn echo_within_grace_settles_the_reference() {
        let state = test_state().await;
        seed_held_light(&state, r#"{"on":true,"brightness":50.0}"#).await;

        // Inside grace the device's own rounding (49 ≠ 50 beyond tolerance
        // would be a false release later) merges into the reference…
        observe_hold_light_at(
            &state,
            "l1",
            &lpatch(r#"{"on":true,"brightness":45.0}"#),
            Instant::now(),
        )
        .await;
        assert!(
            hold_pending(&state, "r1").await,
            "a grace-window echo must never release"
        );

        // …so the settled value no longer reads as manual after grace.
        let after_grace = Instant::now() + Duration::from_secs(60);
        observe_hold_light_at(&state, "l1", &lpatch(r#"{"brightness":45.0}"#), after_grace).await;
        assert!(
            hold_pending(&state, "r1").await,
            "the settled echo value is the reference"
        );
    }

    #[tokio::test]
    async fn unseen_dimension_adopts_then_diverges() {
        let state = test_state().await;
        // Reference knows nothing about colour (the rule only touched power).
        seed_held_light(&state, r#"{"on":true}"#).await;
        let after_grace = Instant::now() + Duration::from_secs(60);

        // First colour observation is baseline, not a manual change…
        observe_hold_light_at(
            &state,
            "l1",
            &lpatch(r#"{"color":{"x":0.5,"y":0.4,"brightness":0.8}}"#),
            after_grace,
        )
        .await;
        assert!(
            hold_pending(&state, "r1").await,
            "first sight of a dimension must adopt"
        );

        // …a later different colour is one.
        observe_hold_light_at(
            &state,
            "l1",
            &lpatch(r#"{"color":{"x":0.2,"y":0.7,"brightness":0.8}}"#),
            after_grace,
        )
        .await;
        assert!(
            !hold_pending(&state, "r1").await,
            "a colour change past baseline must release"
        );
    }

    #[tokio::test]
    async fn manual_power_flip_releases_only_that_device() {
        let state = test_state().await;
        sqlx::query(
            r#"INSERT INTO power_devices (id, provider_id, device_id, name, kind, last_state)
               VALUES ('p1', 'p', 'plug-1', 'Plug', 'plug', '{"on":true}')"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO automations (id, trigger_json, actions_json, restore_secs)
             VALUES ('r1', '{\"kind\":\"sensor\",\"sensor_id\":\"x\",\"event\":{\"kind\":\"became_true\"}}',
                     '[]', 600)",
        )
        .execute(&state.db)
        .await
        .unwrap();
        // The snapshot holds a light AND the plug — only the plug is touched.
        sqlx::query(
            r#"INSERT INTO automation_restores (automation_id, restore_at, snapshot_json)
               VALUES ('r1', datetime('now', '+600 seconds'),
                       '[{"kind":"light","light_id":"l1","state":{"on":false}},
                         {"kind":"power","device_id":"p1","on":false}]')"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        watch_hold_devices(
            &state,
            &[RestoreEntry::Power {
                device_id: "p1".into(),
                on: false,
            }],
        )
        .await;
        let after_grace = Instant::now() + Duration::from_secs(60);

        observe_hold_power_at(&state, "p1", true, after_grace).await; // echo of cached on:true
        assert!(hold_pending(&state, "r1").await);

        observe_hold_power_at(&state, "p1", false, after_grace).await; // manual flip
        let snap: String = sqlx::query_scalar(
            "SELECT snapshot_json FROM automation_restores WHERE automation_id = 'r1'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert!(
            !snap.contains("p1"),
            "the flipped plug must leave the snapshot: {snap}"
        );
        assert!(
            snap.contains("l1"),
            "the untouched light must stay held: {snap}"
        );
    }

    #[tokio::test]
    async fn restore_clears_the_devices_watch() {
        let state = test_state().await;
        seed_held_light(&state, r#"{"on":true,"brightness":50.0}"#).await;
        sqlx::query("UPDATE automation_restores SET restore_at = datetime('now', '-1 seconds')")
            .execute(&state.db)
            .await
            .unwrap();
        apply_due_restores(&state).await;
        assert!(
            state.hold_watch.inner.lock().await.is_empty(),
            "a completed restore must stop watching its devices"
        );
    }

    #[tokio::test]
    async fn room_actions_snapshot_the_rooms_members() {
        let state = test_state().await;
        sqlx::query("INSERT INTO rooms (id, name) VALUES ('room1', 'Office')")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query(
            r#"INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state)
               VALUES ('l1', 'p', 'bulb-1', 'Lamp', '{}', '{"on":true,"brightness":40.0}')"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO room_lights (room_id, light_id) VALUES ('room1', 'l1')")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query(
            r#"INSERT INTO power_devices (id, provider_id, device_id, name, kind, last_state)
               VALUES ('p1', 'p', 'switch-1', 'Fan', 'switch', '{"on":true}')"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO room_power_devices (room_id, power_device_id) VALUES ('room1', 'p1')",
        )
        .execute(&state.db)
        .await
        .unwrap();

        let actions = [RuleAction::Room {
            room_id: "room1".into(),
            state: crate::models::LightState {
                on: false,
                ..Default::default()
            },
        }];
        let entries = snapshot_targets(&state, actions.iter()).await;
        assert_eq!(
            entries.len(),
            2,
            "room expands to its light + power members"
        );
        assert!(
            entries
                .iter()
                .any(|e| matches!(e, RestoreEntry::Light { light_id, .. } if light_id == "l1"))
        );
        assert!(entries.iter().any(
            |e| matches!(e, RestoreEntry::Power { device_id, on: true } if device_id == "p1")
        ));
    }

    #[test]
    fn cooldown_compares_against_the_utc_stamp() {
        let mk = |last: Option<&str>, cooldown| Automation {
            id: "r".into(),
            name: String::new(),
            enabled: true,
            trigger: AutomationTrigger::Sensor {
                sensor_id: "s".into(),
                event: SensorTrigger::BecameTrue,
            },
            conditions: vec![],
            steps: vec![],
            cooldown_secs: cooldown,
            restore_secs: None,
            last_fired_at: last.map(str::to_string),
        };
        let now = chrono::Utc::now();
        let recent = (now - chrono::Duration::seconds(10))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let old = (now - chrono::Duration::seconds(500))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        assert!(in_cooldown(&mk(Some(&recent), 60), now));
        assert!(!in_cooldown(&mk(Some(&old), 60), now));
        assert!(!in_cooldown(&mk(None, 60), now)); // never fired
        assert!(!in_cooldown(&mk(Some(&recent), 0), now)); // no cooldown configured
    }

    // ── Schedule (timer) rules ───────────────────────────────────────────────

    /// A 24-hour plan that is `mode` at `hour` and 'S' (off) everywhere else.
    fn plan_only(hour: usize, mode: char) -> String {
        let mut plan: Vec<u8> = vec![b'S'; 24];
        plan[hour] = mode as u8;
        String::from_utf8(plan).unwrap()
    }

    /// Point the test provider at a wiremock HA that accepts every power and
    /// light service call, so the timer's on/off passes really run end to end
    /// and land in the state cache.
    async fn mock_ha(state: &AppState) -> wiremock::MockServer {
        use wiremock::matchers::{method, path_regex};
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(method("POST"))
            .and(path_regex(
                r"^/api/services/(homeassistant|light)/turn_(on|off)$",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        let creds = state
            .encrypt_credentials(&format!(r#"{{"base_url":"{}","token":"t"}}"#, server.uri()))
            .unwrap();
        sqlx::query("UPDATE providers SET credentials = ? WHERE id = 'p'")
            .bind(&creds)
            .execute(&state.db)
            .await
            .unwrap();
        server
    }

    async fn seed_power_device(state: &AppState, id: &str, on: bool) {
        sqlx::query(
            "INSERT INTO power_devices (id, provider_id, device_id, name, kind, last_state)
             VALUES (?, 'p', ?, ?, 'switch', ?)",
        )
        .bind(id)
        .bind(id)
        .bind(id)
        .bind(format!(r#"{{"on":{on}}}"#))
        .execute(&state.db)
        .await
        .unwrap();
    }

    /// The cached power boolean the shared apply path writes on success.
    async fn power_on(state: &AppState, id: &str) -> Option<bool> {
        let raw: Option<String> =
            sqlx::query_scalar("SELECT last_state FROM power_devices WHERE id = ?")
                .bind(id)
                .fetch_one(&state.db)
                .await
                .unwrap();
        serde_json::from_str::<serde_json::Value>(&raw?)
            .ok()?
            .get("on")?
            .as_bool()
    }

    /// A schedule rule powering `pw1` on (inserted directly, like the other
    /// engine tests — validation is covered separately via `create_rule`).
    async fn seed_schedule_rule(state: &AppState, id: &str, plan: &str) {
        let trigger = format!(r#"{{"kind":"schedule","hour_modes":"{plan}"}}"#);
        sqlx::query(
            "INSERT INTO automations (id, trigger_json, actions_json)
             VALUES (?, ?, '[{\"conditions\":[],\"actions\":[{\"kind\":\"power\",\"device_id\":\"pw1\",\"on\":true}]}]')",
        )
        .bind(id)
        .bind(&trigger)
        .execute(&state.db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn timer_on_edge_powers_on_and_off_edge_powers_off() {
        let state = test_state().await;
        let _server = mock_ha(&state).await;
        seed_power_device(&state, "pw1", false).await;
        seed_schedule_rule(&state, "r1", &plan_only(9, 'W')).await;
        let mut engine = EngineState::default();

        // 09:00 — first observation while active reconciles: fire → power on.
        evaluate_schedules_at(&state, &mut engine, 9, false).await;
        assert!(fired(&state, "r1").await, "an On hour must fire the rule");
        assert_eq!(power_on(&state, "pw1").await, Some(true));

        // Re-evaluating the same hour is a level, not an edge — no re-fire.
        // (Forced, so the edge logic itself is exercised, not the hour gate.)
        sqlx::query("UPDATE automations SET last_fired_at = NULL WHERE id = 'r1'")
            .execute(&state.db)
            .await
            .unwrap();
        evaluate_schedules_at(&state, &mut engine, 9, true).await;
        assert!(!fired(&state, "r1").await, "a held level must not re-fire");

        // 10:00 (Off) — the same targets power off. Pure power both ways: the
        // write carries no attribute clauses, so a coloured light would come
        // back in its own colour next On hour.
        evaluate_schedules_at(&state, &mut engine, 10, false).await;
        assert_eq!(
            power_on(&state, "pw1").await,
            Some(false),
            "the Off edge must power the targets down"
        );
    }

    #[tokio::test]
    async fn timer_first_observation_while_off_does_not_act() {
        let state = test_state().await;
        let _server = mock_ha(&state).await;
        // Someone left the plug on before a restart.
        seed_power_device(&state, "pw1", true).await;
        seed_schedule_rule(&state, "r1", &"S".repeat(24)).await;
        let mut engine = EngineState::default();

        evaluate_schedules_at(&state, &mut engine, 10, false).await;
        assert_eq!(
            power_on(&state, "pw1").await,
            Some(true),
            "a boot inside Off hours must not stomp existing state"
        );
        assert!(!fired(&state, "r1").await, "an Off hour never fires");
        // …but the verdict is recorded, so the NEXT On edge still fires.
        evaluate_schedules_at(&state, &mut engine, 10, true).await;
        assert_eq!(engine.schedule_prev.get("r1"), Some(&false));
    }

    #[tokio::test]
    async fn disabling_a_timer_stops_managing_its_devices() {
        let state = test_state().await;
        let _server = mock_ha(&state).await;
        seed_power_device(&state, "pw1", false).await;
        seed_schedule_rule(&state, "r1", &"W".repeat(24)).await;
        let mut engine = EngineState::default();
        evaluate_schedules_at(&state, &mut engine, 0, false).await;
        assert_eq!(power_on(&state, "pw1").await, Some(true));

        sqlx::query("UPDATE automations SET enabled = 0 WHERE id = 'r1'")
            .execute(&state.db)
            .await
            .unwrap();
        // Forced, mirroring the automations_changed poke a real disable fires.
        evaluate_schedules_at(&state, &mut engine, 0, true).await;
        assert_eq!(
            power_on(&state, "pw1").await,
            Some(true),
            "disabling stops managing — it doesn't power anything off"
        );
        assert!(
            !engine.schedule_prev.contains_key("r1"),
            "a dropped rule leaves the verdict map"
        );
    }

    #[tokio::test]
    async fn create_validates_the_timer_plan_and_power_only_actions() {
        let state = test_state().await;
        let power_step = |actions: Vec<RuleAction>| {
            vec![ActionStep {
                conditions: vec![],
                actions,
            }]
        };
        let body =
            |hour_modes: &str, steps: Vec<ActionStep>, restore: Option<u32>| AutomationBody {
                name: String::new(),
                enabled: true,
                trigger: AutomationTrigger::Schedule {
                    hour_modes: hour_modes.into(),
                },
                conditions: vec![],
                steps,
                cooldown_secs: 0,
                restore_secs: restore,
            };
        let power_on_action = || RuleAction::Power {
            device_id: "nope".into(),
            on: true,
        };
        let with_cooldown = |mut b: AutomationBody, secs: u32| {
            b.cooldown_secs = secs;
            b
        };

        // Malformed plans are rejected loudly…
        assert!(matches!(
            create_rule(
                &state,
                body("WS", power_step(vec![power_on_action()]), None)
            )
            .await,
            SaveRuleOutcome::BadRequest(_)
        ));
        // …and so is Aware — a kiosk display mode, not a timer one.
        assert!(matches!(
            create_rule(
                &state,
                body(&"A".repeat(24), power_step(vec![power_on_action()]), None)
            )
            .await,
            SaveRuleOutcome::BadRequest(_)
        ));
        // A timer only switches power: attribute clauses and scenes are out.
        assert!(matches!(
            create_rule(
                &state,
                body(
                    &"W".repeat(24),
                    power_step(vec![RuleAction::Light {
                        light_id: "l1".into(),
                        state: crate::models::LightState {
                            on: true,
                            brightness: Some(40.0),
                            ..Default::default()
                        },
                    }]),
                    None
                )
            )
            .await,
            SaveRuleOutcome::BadRequest(_)
        ));
        assert!(matches!(
            create_rule(
                &state,
                body(
                    &"W".repeat(24),
                    power_step(vec![RuleAction::Scene {
                        scene_id: "s1".into()
                    }]),
                    None
                )
            )
            .await,
            SaveRuleOutcome::BadRequest(_)
        ));
        // The plan brings its own off direction — no timed hold on top.
        assert!(matches!(
            create_rule(
                &state,
                body(
                    &"W".repeat(24),
                    power_step(vec![power_on_action()]),
                    Some(600)
                )
            )
            .await,
            SaveRuleOutcome::BadRequest(_)
        ));
        // Cooldown doesn't apply either: it could only swallow a later On
        // window's turn-on while its Off edge still ran.
        assert!(matches!(
            create_rule(
                &state,
                with_cooldown(
                    body(&"W".repeat(24), power_step(vec![power_on_action()]), None),
                    600
                )
            )
            .await,
            SaveRuleOutcome::BadRequest(_)
        ));
        // Pure power over a light + a switch saves.
        assert!(matches!(
            create_rule(
                &state,
                body(
                    &format!("{}{}", "W".repeat(12), "S".repeat(12)),
                    power_step(vec![
                        power_on_action(),
                        RuleAction::Light {
                            light_id: "l1".into(),
                            state: crate::models::LightState {
                                on: true,
                                ..Default::default()
                            },
                        },
                    ]),
                    None
                )
            )
            .await,
            SaveRuleOutcome::Ok(_)
        ));
    }

    /// The cached light state the shared apply path writes on success.
    async fn light_state(state: &AppState, id: &str) -> serde_json::Value {
        let raw: String = sqlx::query_scalar("SELECT last_state FROM lights WHERE id = ?")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    #[tokio::test]
    async fn timer_edges_power_room_members_and_lights_pure_power() {
        let state = test_state().await;
        let _server = mock_ha(&state).await;
        // room1 carries a light + a switch; l2 is a standalone blue lamp.
        sqlx::query("INSERT INTO rooms (id, name) VALUES ('room1', 'Den')")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query(
            r#"INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state)
               VALUES ('l1', 'p', 'light.l1', 'Lamp', '{}', '{"on":false}'),
                      ('l2', 'p', 'light.l2', 'Blue lamp', '{}',
                       '{"on":false,"color":{"x":0.2,"y":0.2,"brightness":0.5}}')"#,
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO room_lights (room_id, light_id) VALUES ('room1', 'l1')")
            .execute(&state.db)
            .await
            .unwrap();
        seed_power_device(&state, "pw1", false).await;
        sqlx::query(
            "INSERT INTO room_power_devices (room_id, power_device_id) VALUES ('room1', 'pw1')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO automations (id, trigger_json, actions_json)
               VALUES ('r1', ?, '[{"conditions":[],"actions":[
                   {"kind":"room","room_id":"room1","state":{"on":true}},
                   {"kind":"light","light_id":"l2","state":{"on":true}}]}]')"#,
        )
        .bind(format!(
            r#"{{"kind":"schedule","hour_modes":"{}"}}"#,
            plan_only(9, 'W')
        ))
        .execute(&state.db)
        .await
        .unwrap();
        let mut engine = EngineState::default();

        // On edge: the room fans to its light + switch members; the standalone
        // lamp powers on too, keeping its colour (pure-power write).
        evaluate_schedules_at(&state, &mut engine, 9, false).await;
        assert_eq!(light_state(&state, "l1").await["on"], true);
        assert_eq!(power_on(&state, "pw1").await, Some(true));
        let l2 = light_state(&state, "l2").await;
        assert_eq!(l2["on"], true);
        assert!(
            l2["color"].is_object(),
            "power-on must not strip colour: {l2}"
        );

        // Off edge: the same targets power down — and the blue lamp is still
        // blue for the next On hour.
        evaluate_schedules_at(&state, &mut engine, 10, false).await;
        assert_eq!(light_state(&state, "l1").await["on"], false);
        assert_eq!(power_on(&state, "pw1").await, Some(false));
        let l2 = light_state(&state, "l2").await;
        assert_eq!(l2["on"], false);
        assert!(
            l2["color"].is_object(),
            "power-off must not strip colour: {l2}"
        );
    }

    #[tokio::test]
    async fn hour_gate_and_failed_reads_leave_the_verdict_map_alone() {
        let state = test_state().await;
        let _server = mock_ha(&state).await;
        seed_power_device(&state, "pw1", false).await;
        seed_schedule_rule(&state, "r1", &"W".repeat(24)).await;
        let mut engine = EngineState::default();
        evaluate_schedules_at(&state, &mut engine, 9, false).await;
        assert_eq!(engine.schedule_prev.get("r1"), Some(&true));

        // Same hour, unforced: the pass skips outright (no rule-table read) —
        // even a deleted rule stays tracked until a boundary or an edit poke.
        sqlx::query("DELETE FROM automations WHERE id = 'r1'")
            .execute(&state.db)
            .await
            .unwrap();
        evaluate_schedules_at(&state, &mut engine, 9, false).await;
        assert_eq!(
            engine.schedule_prev.get("r1"),
            Some(&true),
            "a same-hour tick must skip the pass"
        );

        // A failed rules read must leave the verdict map untouched — wiping it
        // would make the next tick re-fire every active timer as a first
        // observation, stomping manual overrides.
        state.db.close().await;
        evaluate_schedules_at(&state, &mut engine, 10, false).await;
        assert_eq!(
            engine.schedule_prev.get("r1"),
            Some(&true),
            "a failed read must not wipe verdicts"
        );
    }
}
