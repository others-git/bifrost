//! Sensor automations — CRUD for [`SensorRule`]s and the background engine
//! that runs them.
//!
//! The engine subscribes to the same sensor push channels the SSE endpoint
//! uses (Hue SSE, HA WebSocket — no polling), keeps each sensor's previous
//! reading in memory to detect **edges**, and holds a pending-timer map for
//! `clear_for` ("no motion for N minutes") rules. Rules are re-read from the
//! DB per event, so an edit takes effect immediately; subscriptions are
//! rebuilt periodically so providers added later join without a restart.
//! Actions replay through the shared service layer (`apply_room_state`,
//! `apply_light_state`, `apply_power_state`, `apply_scene_entries`) — an
//! automation is just another caller, like session/v1/MCP.
//!
//! Debug logging is on the `bifrost::automation` target.

use crate::AppState;
use crate::api::auth::Session;
use crate::connection::SensorEvent;
use crate::models::automation::{
    Automation, AutomationTrigger, RuleAction, RuleCondition, SensorTrigger, TriggerDeviceDomain,
};

/// The table a device-trigger domain lives in (fixed identifiers — injection-free).
fn device_trigger_table(domain: TriggerDeviceDomain) -> &'static str {
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
        actions: serde_json::from_str(&r.get::<String, _>("actions_json")).unwrap_or_default(),
        cooldown_secs: r.get::<i64, _>("cooldown_secs") as u32,
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
    pub actions: Vec<RuleAction>,
    #[serde(default)]
    pub cooldown_secs: u32,
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
    if body.actions.is_empty() {
        return Err(SaveRuleOutcome::BadRequest(
            "an automation needs at least one action".into(),
        ));
    }
    let event = body.trigger.event();
    let trigger_numeric = matches!(
        event,
        SensorTrigger::RoseAbove { .. } | SensorTrigger::DroppedBelow { .. }
    );
    match &body.trigger {
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
    }
    if let Some((_, secs)) = event.stay_watch()
        && !(30..=24 * 3600).contains(&secs)
    {
        return Err(SaveRuleOutcome::BadRequest(
            "a stays-for duration must be between 30 seconds and 24 hours".into(),
        ));
    }
    Ok(())
}

fn actions_json(actions: &[RuleAction]) -> String {
    let normalized: Vec<RuleAction> = actions
        .iter()
        .cloned()
        .map(RuleAction::normalized)
        .collect();
    serde_json::to_string(&normalized).unwrap_or_else(|_| "[]".into())
}

pub(crate) async fn create_rule(state: &AppState, body: AutomationBody) -> SaveRuleOutcome {
    if let Err(out) = validate_body(state, &body).await {
        return out;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let res = sqlx::query(
        "INSERT INTO automations (id, sensor_id, name, enabled, trigger_json, conditions_json, actions_json, cooldown_secs)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(body.trigger.sensor_id())
    .bind(body.name.trim())
    .bind(body.enabled as i64)
    .bind(serde_json::to_string(&body.trigger).unwrap_or_default())
    .bind(serde_json::to_string(&body.conditions).unwrap_or_else(|_| "[]".into()))
    .bind(actions_json(&body.actions))
    .bind(body.cooldown_secs as i64)
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
                conditions_json = ?, actions_json = ?, cooldown_secs = ? WHERE id = ?",
    )
    .bind(body.trigger.sensor_id())
    .bind(body.name.trim())
    .bind(body.enabled as i64)
    .bind(serde_json::to_string(&body.trigger).unwrap_or_default())
    .bind(serde_json::to_string(&body.conditions).unwrap_or_else(|_| "[]".into()))
    .bind(actions_json(&body.actions))
    .bind(body.cooldown_secs as i64)
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
/// dozens, not thousands.
async fn non_sensor_rules(state: &AppState) -> Vec<Automation> {
    sqlx::query("SELECT * FROM automations WHERE sensor_id IS NULL AND enabled = 1")
        .fetch_all(&state.db)
        .await
        .map_err(|e| tracing::error!("db error loading non-sensor rules: {e}"))
        .unwrap_or_default()
        .into_iter()
        .map(row_to_automation)
        .collect()
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
/// cached state — the fire-time re-check for device stay timers.
async fn cached_device_on(
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

/// Evaluate a rule's conditions right now. Fails closed on unknown readings.
async fn conditions_hold(state: &AppState, rule: &Automation) -> bool {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    let now_min = (now.hour() * 60 + now.minute()) as u16;
    let now_day = now.weekday().num_days_from_monday() as u8;
    for cond in &rule.conditions {
        // Cross-subject gates need the other subject's current state; resolve
        // it before the sync `holds` call.
        let reading = match cond {
            RuleCondition::SensorAbove { sensor_id, .. }
            | RuleCondition::SensorBelow { sensor_id, .. }
            | RuleCondition::SensorIs { sensor_id, .. } => cached_reading(state, sensor_id).await,
            RuleCondition::TimeWindow { .. } | RuleCondition::RoomIs { .. } => None,
        };
        let occupancy = match cond {
            RuleCondition::RoomIs { room_id, .. } => {
                crate::api::rooms::room_occupancy(state, room_id).await
            }
            _ => None,
        };
        if !cond.holds(now_min, now_day, |_| reading, |_| occupancy) {
            tracing::debug!(target: "bifrost::automation", rule = %rule.id, ?cond, "rule skipped: condition not met");
            return false;
        }
    }
    true
}

/// Execute a rule's actions through the shared service layer. Best-effort per
/// action: one failing action is logged and the rest still run.
pub(crate) async fn execute_rule(state: &AppState, rule: &Automation) {
    tracing::debug!(
        target: "bifrost::automation",
        rule = %rule.id,
        name = %rule.name,
        actions = rule.actions.len(),
        "rule fired",
    );
    for action in &rule.actions {
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
                let applied = crate::api::scenes::apply_scene_entries(state, scene_id, None).await;
                tracing::debug!(target: "bifrost::automation", rule = %rule.id, scene = %scene_id, ok = applied.is_some(), "action: scene");
            }
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
    if !conditions_hold(state, rule).await {
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
        let event = rule.trigger.event();
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
    let members = crate::api::rooms::room_presence_readings(state, room_id).await;
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
    if let Some((watched, secs)) = rule.trigger.event().stay_watch()
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

/// The background engine loop: merge every provider's sensor push stream,
/// process events as they arrive, tick armed timers, and rebuild the
/// subscription set periodically (providers added later join then).
pub async fn run_engine(state: Arc<AppState>) {
    use futures_util::StreamExt;
    use tokio_stream::wrappers::BroadcastStream;

    let mut engine = EngineState::default();
    seed_engine(&state, &mut engine).await;

    /// One event off any of the pipelines the engine watches.
    enum EngineEvent {
        Sensor(String, crate::connection::SensorEvent),
        Light(crate::connection::LightEvent),
        Media(String, crate::models::media::MediaEvent),
        Power(String, crate::connection::PowerEvent),
    }

    loop {
        let (sensor_rx, light_rx, media_rx, power_rx) = {
            let connections = state.connections.lock().await;
            (
                connections.subscribe_all_sensor(),
                connections.subscribe_all(),
                connections.subscribe_all_media(),
                connections.subscribe_all_power(),
            )
        };
        if sensor_rx.is_empty() && light_rx.is_empty() && media_rx.is_empty() && power_rx.is_empty()
        {
            // No connected providers yet; check again shortly.
            tokio::time::sleep(Duration::from_secs(30)).await;
            continue;
        }
        let mut streams: Vec<futures_util::stream::BoxStream<'static, EngineEvent>> = Vec::new();
        for (provider_id, rx) in sensor_rx {
            streams.push(
                BroadcastStream::new(rx)
                    .filter_map(|r| std::future::ready(r.ok()))
                    .map(move |ev| EngineEvent::Sensor(provider_id.clone(), ev))
                    .boxed(),
            );
        }
        for rx in light_rx {
            streams.push(
                BroadcastStream::new(rx)
                    .filter_map(|r| std::future::ready(r.ok()))
                    .map(EngineEvent::Light)
                    .boxed(),
            );
        }
        for (provider_id, rx) in media_rx {
            streams.push(
                BroadcastStream::new(rx)
                    .filter_map(|r| std::future::ready(r.ok()))
                    .map(move |ev| EngineEvent::Media(provider_id.clone(), ev))
                    .boxed(),
            );
        }
        for (provider_id, rx) in power_rx {
            streams.push(
                BroadcastStream::new(rx)
                    .filter_map(|r| std::future::ready(r.ok()))
                    .map(move |ev| EngineEvent::Power(provider_id.clone(), ev))
                    .boxed(),
            );
        }
        let mut merged = futures_util::stream::select_all(streams);
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        let resubscribe = tokio::time::sleep(Duration::from_secs(60));
        tokio::pin!(resubscribe);

        loop {
            tokio::select! {
                ev = merged.next() => match ev {
                    Some(EngineEvent::Sensor(provider_id, event)) => {
                        process_sensor_event(&state, &mut engine, &provider_id, &event).await;
                    }
                    Some(EngineEvent::Light(event)) => {
                        // A patch without `on` is an attribute change, not a
                        // power edge. Light events aren't provider-tagged (the
                        // SSE feed matches them the same way).
                        if let Some(on) = event.patch.on {
                            let row: Option<String> = sqlx::query_scalar(
                                "SELECT id FROM lights WHERE device_id = ? AND enabled = 1 AND shadowed_by IS NULL",
                            )
                            .bind(&event.device_id)
                            .fetch_optional(&state.db)
                            .await
                            .ok()
                            .flatten();
                            if let Some(id) = row {
                                process_device_event(&state, &mut engine, TriggerDeviceDomain::Light, &id, on).await;
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
                            process_device_event(&state, &mut engine, TriggerDeviceDomain::Power, &id, event.state.on).await;
                        }
                    }
                    None => break, // every stream closed — resubscribe
                },
                _ = tick.tick() => fire_due_timers(&state, &mut engine).await,
                // A rule was created/edited: baseline its subject now (fill-only,
                // so live readings and armed timers are untouched) — a rule made
                // mid-flight must fire on the next edge, not after a restart.
                _ = state.automations_changed.notified() => seed_engine(&state, &mut engine).await,
                _ = &mut resubscribe => break,
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
             VALUES (?, ?, ?, ?, '[{\"kind\":\"power\",\"device_id\":\"nope\",\"on\":true}]')",
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
             VALUES (?, ?, '[{\"kind\":\"power\",\"device_id\":\"nope\",\"on\":true}]')",
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
        tokio::time::sleep(Duration::from_millis(1100)).await;
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
        tokio::time::sleep(Duration::from_millis(1100)).await;
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
                     '[{\"kind\":\"power\",\"device_id\":\"nope\",\"on\":true}]')",
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
                     '[{\"kind\":\"power\",\"device_id\":\"nope\",\"on\":true}]')",
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
            actions: vec![],
            cooldown_secs: cooldown,
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
}
