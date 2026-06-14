//! Hue bridge connection manager.
//!
//! Home Assistant is known to drop the Hue SSE event stream and not recover reliably.
//! This module addresses that with an explicit state machine, exponential-backoff
//! reconnect, polling fallback during outages, and periodic health checks.

use crate::models::audio::AudioEvent;
use crate::models::{LightCapabilities, LightState, LightStatePatch};
use crate::providers::hue::HueProvider;
use crate::providers::{AudioProvider, LightProvider};
use anyhow::Result;
use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, broadcast};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

const BACKOFF_BASE_MS: u64 = 1_000;
const BACKOFF_MAX_MS: u64 = 60_000;
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const POLL_INTERVAL_RECONNECTING: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected {
        since: Instant,
        last_event: Instant,
    },
    /// SSE dropped; actively attempting to reconnect with backoff.
    Reconnecting {
        attempt: u32,
        retry_at: Instant,
    },
    /// Unrecoverable (e.g. bridge removed/unconfigured).
    Failed {
        reason: String,
    },
}

impl ConnectionState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Connected { .. } => "connected",
            Self::Reconnecting { .. } => "reconnecting",
            Self::Failed { .. } => "failed",
        }
    }
}

/// A light-state update broadcast on the event pipeline. Carries a *patch*:
/// push sources (Hue SSE) only know the changed fields, while pollers send
/// full-state patches. Consumers merge into existing state.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LightEvent {
    pub device_id: String,
    pub patch: LightStatePatch,
    /// Full capabilities, present only on authoritative poll/discovery events.
    /// SSE partial events leave this `None` so stored capabilities are untouched.
    /// Lets stale capability flags (e.g. Hue `color_rgb`) self-heal on the next poll.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<LightCapabilities>,
}

pub struct HueConnectionManager {
    provider: Arc<HueProvider>,
    pub state: Arc<RwLock<ConnectionState>>,
    pub events: broadcast::Sender<LightEvent>,
}

impl HueConnectionManager {
    pub fn new(provider: HueProvider) -> (Self, broadcast::Receiver<LightEvent>) {
        let (tx, rx) = broadcast::channel(256);
        let mgr = Self {
            provider: Arc::new(provider),
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            events: tx,
        };
        (mgr, rx)
    }

    /// Spawn the background connection loop. Never returns unless the task is aborted.
    pub async fn run(self: Arc<Self>) {
        info!("hue connection manager starting");
        let mut attempt: u32 = 0;

        loop {
            *self.state.write().await = ConnectionState::Connecting;
            info!("hue: connecting (attempt {})", attempt + 1);

            match self.run_sse().await {
                Ok(()) => {
                    // SSE ended cleanly — treat as a drop and reconnect.
                    warn!("hue: SSE stream ended unexpectedly");
                }
                Err(e) => {
                    error!("hue: SSE error: {e:#}");
                }
            }

            let delay = backoff_delay(attempt);
            attempt += 1;

            *self.state.write().await = ConnectionState::Reconnecting {
                attempt,
                retry_at: Instant::now() + delay,
            };

            // Poll at coarse interval while waiting to reconnect so the UI isn't stale.
            let poll_ticks = delay.as_millis() / POLL_INTERVAL_RECONNECTING.as_millis();
            for _ in 0..poll_ticks {
                tokio::time::sleep(POLL_INTERVAL_RECONNECTING).await;
                if let Err(e) = self.poll_all_lights().await {
                    debug!("hue: poll fallback error: {e:#}");
                }
            }

            // Remaining delay after last poll tick.
            let remaining = delay.saturating_sub(POLL_INTERVAL_RECONNECTING * poll_ticks as u32);
            if !remaining.is_zero() {
                tokio::time::sleep(remaining).await;
            }
        }
    }

    /// Connect to the SSE event stream and process events until the stream drops.
    async fn run_sse(&self) -> Result<()> {
        use futures_util::StreamExt;

        let stream = self.provider.event_stream().await?;

        *self.state.write().await = ConnectionState::Connected {
            since: Instant::now(),
            last_event: Instant::now(),
        };
        info!("hue: SSE connected");

        // Resync once on (re)connect: anything that changed while the stream
        // was down (app restart, bridge reboot) produced no events, so stored
        // state may be stale until the next change otherwise.
        if let Err(e) = self.poll_all_lights().await {
            warn!("hue: initial resync after connect failed: {e:#}");
        }

        let mut health_tick = tokio::time::interval(HEALTH_CHECK_INTERVAL);

        tokio::pin!(stream);

        loop {
            tokio::select! {
                event = stream.next() => {
                    match event {
                        Some(Ok(ev)) => {
                            self.handle_sse_event(&ev.data).await;
                            // Read since, drop the read lock, then write — avoids double-lock.
                            let since = {
                                let s = self.state.read().await;
                                if let ConnectionState::Connected { since, .. } = &*s { Some(*since) } else { None }
                            };
                            if let Some(since) = since {
                                *self.state.write().await = ConnectionState::Connected {
                                    since,
                                    last_event: Instant::now(),
                                };
                            }
                        }
                        Some(Err(e)) => {
                            return Err(anyhow::anyhow!("SSE stream error: {e}"));
                        }
                        None => return Ok(()),
                    }
                }
                _ = health_tick.tick() => {
                    if let Err(e) = self.health_check().await {
                        return Err(anyhow::anyhow!("health check failed: {e}"));
                    }
                }
            }
        }
    }

    async fn handle_sse_event(&self, data: &str) {
        // Hue SSE events are JSON arrays of resource updates.
        let events: serde_json::Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return,
        };
        let Some(arr) = events.as_array() else { return };

        for event in arr {
            let Some(data_arr) = event.get("data").and_then(|d| d.as_array()) else {
                continue;
            };
            for item in data_arr {
                let Some(id) = item.get("id").and_then(|v| v.as_str()) else {
                    continue;
                };
                let patch = crate::providers::hue::parse_patch_from_event(item);
                if patch.is_empty() {
                    continue; // unrelated resource update (scene, geofence, …)
                }
                let _ = self.events.send(LightEvent {
                    device_id: id.to_string(),
                    patch,
                    capabilities: None,
                });
            }
        }
    }

    async fn health_check(&self) -> Result<()> {
        // A lightweight GET to confirm the bridge is still responding.
        self.provider.ping().await
    }

    async fn poll_all_lights(&self) -> Result<()> {
        let lights = self.provider.discover().await?;
        for light in lights {
            let _ = self.events.send(LightEvent {
                patch: LightStatePatch::from_full(&light.state),
                capabilities: Some(light.capabilities),
                device_id: light.provider_id,
            });
        }
        Ok(())
    }
}

// ── Connection registry ─────────────────────────────────────────────────────

/// Serializable snapshot of a provider's connection state for the status API.
#[derive(Debug, Serialize)]
pub struct ConnectionStatus {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ConnectionStatus {
    pub fn from_state(cs: &ConnectionState) -> Self {
        let now = Instant::now();
        match cs {
            ConnectionState::Disconnected => Self {
                state: "disconnected",
                since_secs: None,
                last_event_secs: None,
                reason: None,
            },
            ConnectionState::Connecting => Self {
                state: "connecting",
                since_secs: None,
                last_event_secs: None,
                reason: None,
            },
            ConnectionState::Connected { since, last_event } => Self {
                state: "connected",
                since_secs: Some(now.saturating_duration_since(*since).as_secs()),
                last_event_secs: Some(now.saturating_duration_since(*last_event).as_secs()),
                reason: None,
            },
            ConnectionState::Reconnecting { attempt, retry_at } => Self {
                state: "reconnecting",
                since_secs: None,
                last_event_secs: None,
                reason: Some(format!(
                    "attempt {}, retry in {}s",
                    attempt,
                    retry_at.saturating_duration_since(now).as_secs()
                )),
            },
            ConnectionState::Failed { reason } => Self {
                state: "failed",
                since_secs: None,
                last_event_secs: None,
                reason: Some(reason.clone()),
            },
        }
    }
}

// ── Polling manager ─────────────────────────────────────────────────────────

/// Keeps state fresh for providers without a push channel (Govee, WLED, …) by
/// polling on an interval. Broadcasts `LightEvent`s on the same pipeline as
/// the Hue SSE manager, so the frontend event stream works identically.
pub struct PollingManager {
    provider: Box<dyn LightProvider>,
    pub state: Arc<RwLock<ConnectionState>>,
    pub events: broadcast::Sender<LightEvent>,
    interval: Duration,
}

impl PollingManager {
    pub fn new(
        provider: Box<dyn LightProvider>,
        interval: Duration,
    ) -> (Self, broadcast::Receiver<LightEvent>) {
        let (tx, rx) = broadcast::channel(256);
        let mgr = Self {
            provider,
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            events: tx,
            interval,
        };
        (mgr, rx)
    }

    /// Poll forever. Successful polls mark the provider Connected; failures
    /// transition to Reconnecting with exponential backoff.
    pub async fn run(self: Arc<Self>) {
        info!("polling manager starting ({})", self.provider.name());
        let mut attempt: u32 = 0;
        let mut connected_since: Option<Instant> = None;

        loop {
            match self.poll_once().await {
                Ok(n) => {
                    attempt = 0;
                    let since = *connected_since.get_or_insert_with(Instant::now);
                    *self.state.write().await = ConnectionState::Connected {
                        since,
                        last_event: Instant::now(),
                    };
                    debug!("{}: polled {n} lights", self.provider.name());
                    tokio::time::sleep(self.interval).await;
                }
                Err(e) => {
                    warn!("{}: poll failed: {e:#}", self.provider.name());
                    connected_since = None;
                    let delay = backoff_delay(attempt);
                    attempt += 1;
                    *self.state.write().await = ConnectionState::Reconnecting {
                        attempt,
                        retry_at: Instant::now() + delay,
                    };
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    /// One polling pass: enumerate devices, fetch each device's live state,
    /// broadcast the result. Falls back to the discovery snapshot when a
    /// per-device state fetch fails (e.g. transient cloud error).
    async fn poll_once(&self) -> Result<usize> {
        let lights = self.provider.discover().await?;
        let mut count = 0;
        for light in lights {
            let state = match self.provider.get_state(&light.provider_id).await {
                Ok(s) => s,
                Err(e) => {
                    debug!(
                        "{}: get_state({}) failed, using discovery snapshot: {e:#}",
                        self.provider.name(),
                        light.provider_id
                    );
                    light.state
                }
            };
            let _ = self.events.send(LightEvent {
                patch: LightStatePatch::from_full(&state),
                capabilities: Some(light.capabilities),
                device_id: light.provider_id,
            });
            count += 1;
        }
        Ok(count)
    }
}

// ── Audio push manager ───────────────────────────────────────────────────────

/// Keeps a persistent push channel open to an audio device (Onkyo eISCP).
/// The provider's `event_stream` yields full-state events until the socket
/// drops; this manager owns reconnection with the shared backoff and is the
/// only code that re-opens the stream.
pub struct AudioPushManager {
    provider: Box<dyn AudioProvider>,
    pub state: Arc<RwLock<ConnectionState>>,
    pub events: broadcast::Sender<AudioEvent>,
}

impl AudioPushManager {
    pub fn new(provider: Box<dyn AudioProvider>) -> (Self, broadcast::Receiver<AudioEvent>) {
        let (tx, rx) = broadcast::channel(256);
        let mgr = Self {
            provider,
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            events: tx,
        };
        (mgr, rx)
    }

    pub async fn run(self: Arc<Self>) {
        info!("audio push manager starting ({})", self.provider.name());
        let mut attempt: u32 = 0;

        loop {
            *self.state.write().await = ConnectionState::Connecting;
            match self.provider.event_stream().await {
                Ok(mut rx) => {
                    attempt = 0;
                    let since = Instant::now();
                    *self.state.write().await = ConnectionState::Connected {
                        since,
                        last_event: since,
                    };
                    info!("{}: push channel connected", self.provider.name());
                    while let Some(event) = rx.recv().await {
                        let _ = self.events.send(event);
                        *self.state.write().await = ConnectionState::Connected {
                            since,
                            last_event: Instant::now(),
                        };
                    }
                    warn!("{}: push channel closed", self.provider.name());
                }
                Err(e) => {
                    warn!("{}: push connect failed: {e:#}", self.provider.name());
                }
            }

            let delay = backoff_delay(attempt);
            attempt += 1;
            *self.state.write().await = ConnectionState::Reconnecting {
                attempt,
                retry_at: Instant::now() + delay,
            };
            tokio::time::sleep(delay).await;
        }
    }
}

/// Writes pushed audio events to `audio_devices.last_state`. Push events are
/// full snapshots, so this replaces rather than merges.
async fn audio_db_writer_task(
    mut rx: broadcast::Receiver<AudioEvent>,
    provider_row_id: String,
    db: SqlitePool,
) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                let state_json = serde_json::to_string(&event.state).unwrap_or_default();
                let _ = sqlx::query(
                    "UPDATE audio_devices SET last_state = ?, last_seen = datetime('now')
                     WHERE provider_id = ? AND device_id = ?",
                )
                .bind(&state_json)
                .bind(&provider_row_id)
                .bind(&event.device_id)
                .execute(&db)
                .await;
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("audio event db writer lagged by {n} events");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

// ── Connection registry ─────────────────────────────────────────────────────

/// One managed connection (SSE, polling, or audio push): its observable state,
/// its event channel(s), and the background tasks that keep it alive.
struct ConnectionEntry {
    state: Arc<RwLock<ConnectionState>>,
    events: broadcast::Sender<LightEvent>,
    /// Present only for audio push managers.
    audio_events: Option<broadcast::Sender<AudioEvent>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for ConnectionEntry {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Owns one connection manager per provider. Thread-safe via `Mutex<ConnectionRegistry>`.
pub struct ConnectionRegistry {
    entries: HashMap<String, ConnectionEntry>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Spawn the Hue SSE loop and a DB writer task for the given provider.
    pub fn start_sse(&mut self, provider_id: String, provider: HueProvider, db: SqlitePool) {
        let (mgr, rx) = HueConnectionManager::new(provider);
        let mgr = Arc::new(mgr);
        let state = Arc::clone(&mgr.state);
        let events = mgr.events.clone();
        let sse_task = tokio::spawn(Arc::clone(&mgr).run());
        let db_task = tokio::spawn(db_writer_task(rx, db));
        self.entries.insert(
            provider_id,
            ConnectionEntry {
                state,
                events,
                audio_events: None,
                tasks: vec![sse_task, db_task],
            },
        );
    }

    /// Spawn a polling loop and a DB writer task for any provider.
    pub fn start_polling(
        &mut self,
        provider_id: String,
        provider: Box<dyn LightProvider>,
        interval: Duration,
        db: SqlitePool,
    ) {
        let (mgr, rx) = PollingManager::new(provider, interval);
        let mgr = Arc::new(mgr);
        let state = Arc::clone(&mgr.state);
        let events = mgr.events.clone();
        let poll_task = tokio::spawn(Arc::clone(&mgr).run());
        let db_task = tokio::spawn(db_writer_task(rx, db));
        self.entries.insert(
            provider_id,
            ConnectionEntry {
                state,
                events,
                audio_events: None,
                tasks: vec![poll_task, db_task],
            },
        );
    }

    /// Spawn an audio push loop (persistent device-initiated updates) and its
    /// DB writer for the given provider.
    pub fn start_audio_push(
        &mut self,
        provider_id: String,
        provider: Box<dyn AudioProvider>,
        db: SqlitePool,
    ) {
        let (mgr, rx) = AudioPushManager::new(provider);
        let mgr = Arc::new(mgr);
        let state = Arc::clone(&mgr.state);
        let audio_events = mgr.events.clone();
        let push_task = tokio::spawn(Arc::clone(&mgr).run());
        let db_task = tokio::spawn(audio_db_writer_task(rx, provider_id.clone(), db));
        // A dummy light channel keeps the entry shape uniform; nothing sends on it.
        let (events, _) = broadcast::channel(1);
        self.entries.insert(
            provider_id,
            ConnectionEntry {
                state,
                events,
                audio_events: Some(audio_events),
                tasks: vec![push_task, db_task],
            },
        );
    }

    /// Abort tasks for the given provider. No-op if not managed.
    pub fn stop(&mut self, provider_id: &str) {
        self.entries.remove(provider_id); // Drop aborts the tasks.
    }

    /// Return a shared handle to the manager's state lock (for the status endpoint).
    pub fn get_state_lock(&self, provider_id: &str) -> Option<Arc<RwLock<ConnectionState>>> {
        self.entries.get(provider_id).map(|e| Arc::clone(&e.state))
    }

    /// Subscribe to every managed connection. Each returned receiver gets every
    /// `LightEvent` broadcast by its manager. Used by the SSE endpoint.
    pub fn subscribe_all(&self) -> Vec<broadcast::Receiver<LightEvent>> {
        self.entries
            .values()
            .map(|e| e.events.subscribe())
            .collect()
    }

    /// Subscribe to every audio push channel, tagged with its provider row id
    /// (so SSE consumers can match `audio_devices` rows). Used by the SSE endpoint.
    pub fn subscribe_all_audio(&self) -> Vec<(String, broadcast::Receiver<AudioEvent>)> {
        self.entries
            .iter()
            .filter_map(|(id, e)| {
                e.audio_events
                    .as_ref()
                    .map(|tx| (id.clone(), tx.subscribe()))
            })
            .collect()
    }
}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Writes incoming light events to the DB. Runs as a background task alongside each manager.
async fn db_writer_task(mut rx: broadcast::Receiver<LightEvent>, db: SqlitePool) {
    loop {
        match rx.recv().await {
            Ok(event) => apply_event_to_db(&event, &db).await,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("light event db writer lagged by {n} events");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Merge an event's patch into the stored `last_state`. Events are partial —
/// overwriting would reset fields the event didn't mention.
async fn apply_event_to_db(event: &LightEvent, db: &SqlitePool) {
    use sqlx::Row;

    let row = sqlx::query("SELECT last_state FROM lights WHERE device_id = ?")
        .bind(&event.device_id)
        .fetch_optional(db)
        .await;

    let Ok(Some(row)) = row else { return }; // unknown device or db error

    let mut state: LightState = row
        .get::<Option<String>, _>("last_state")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    event.patch.apply_to(&mut state);

    let state_json = serde_json::to_string(&state).unwrap_or_default();

    // Authoritative poll/discovery events carry full capabilities; refresh them so
    // capability flags (e.g. Hue `color_rgb`) self-heal. SSE patches carry no
    // capabilities, so the stored value is left untouched.
    if let Some(caps) = &event.capabilities {
        let caps_json = serde_json::to_string(caps).unwrap_or_default();
        let _ = sqlx::query(
            "UPDATE lights SET last_state = ?, capabilities = ?, last_seen = datetime('now') WHERE device_id = ?",
        )
        .bind(&state_json)
        .bind(&caps_json)
        .bind(&event.device_id)
        .execute(db)
        .await;
    } else {
        let _ = sqlx::query(
            "UPDATE lights SET last_state = ?, last_seen = datetime('now') WHERE device_id = ?",
        )
        .bind(&state_json)
        .bind(&event.device_id)
        .execute(db)
        .await;
    }
}

// ── Backoff ─────────────────────────────────────────────────────────────────

fn backoff_delay(attempt: u32) -> Duration {
    let ms = BACKOFF_BASE_MS
        .saturating_mul(1u64.checked_shl(attempt.min(10)).unwrap_or(u64::MAX))
        .min(BACKOFF_MAX_MS);
    // Add ±20% jitter to avoid thundering-herd on restart.
    let jitter = (ms as f64 * 0.2 * (rand::random::<f64>() - 0.5)) as i64;
    Duration::from_millis((ms as i64 + jitter).max(100) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Light, LightCapabilities};
    use chrono::Utc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use uuid::Uuid;

    // ── Polling manager ──────────────────────────────────────────────────────

    /// Scripted provider: serves `lights`, per-device state via `get_state`,
    /// and fails discovery after `fail_after` calls (u32::MAX = never).
    struct ScriptedProvider {
        lights: Vec<(String, LightState)>,
        discover_calls: AtomicU32,
        fail_discover_after: u32,
        fail_get_state: bool,
    }

    impl ScriptedProvider {
        fn new(lights: Vec<(String, LightState)>) -> Self {
            Self {
                lights,
                discover_calls: AtomicU32::new(0),
                fail_discover_after: u32::MAX,
                fail_get_state: false,
            }
        }
    }

    #[async_trait::async_trait]
    impl LightProvider for ScriptedProvider {
        fn name(&self) -> &str {
            "scripted"
        }

        async fn discover(&self) -> Result<Vec<Light>> {
            let n = self.discover_calls.fetch_add(1, Ordering::SeqCst);
            if n >= self.fail_discover_after {
                anyhow::bail!("scripted discover failure");
            }
            Ok(self
                .lights
                .iter()
                .map(|(id, _)| Light {
                    id: Uuid::new_v4(),
                    provider_id: id.clone(),
                    provider: crate::models::Provider::Govee,
                    name: format!("Light {id}"),
                    // Discovery snapshot is default (Govee's device list has no state).
                    state: LightState::default(),
                    capabilities: LightCapabilities::default(),
                    last_seen: Utc::now(),
                    hw_id: None,
                })
                .collect())
        }

        async fn set_state(&self, _id: &str, _s: &LightState) -> Result<()> {
            Ok(())
        }

        async fn get_state(&self, id: &str) -> Result<LightState> {
            if self.fail_get_state {
                anyhow::bail!("scripted get_state failure");
            }
            self.lights
                .iter()
                .find(|(lid, _)| lid == id)
                .map(|(_, s)| s.clone())
                .ok_or_else(|| anyhow::anyhow!("unknown device"))
        }
    }

    fn on_at(brightness: f32) -> LightState {
        LightState {
            on: true,
            brightness: Some(brightness),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn poll_broadcasts_live_state_for_each_device() {
        let provider = ScriptedProvider::new(vec![
            ("dev-1".into(), on_at(75.0)),
            ("dev-2".into(), on_at(30.0)),
        ]);
        let (mgr, mut rx) = PollingManager::new(Box::new(provider), Duration::from_secs(60));

        let n = mgr.poll_once().await.unwrap();
        assert_eq!(n, 2);

        let e1 = rx.recv().await.unwrap();
        let e2 = rx.recv().await.unwrap();
        let mut got: Vec<_> = vec![
            (e1.device_id, e1.patch.brightness),
            (e2.device_id, e2.patch.brightness),
        ];
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            got,
            vec![
                ("dev-1".to_string(), Some(75.0)),
                ("dev-2".to_string(), Some(30.0))
            ]
        );
    }

    #[tokio::test]
    async fn poll_falls_back_to_discovery_snapshot_when_get_state_fails() {
        let mut provider = ScriptedProvider::new(vec![("dev-1".into(), on_at(75.0))]);
        provider.fail_get_state = true;
        let (mgr, mut rx) = PollingManager::new(Box::new(provider), Duration::from_secs(60));

        mgr.poll_once().await.unwrap();

        // get_state failed → falls back to the (default) discovery snapshot.
        let event = rx.recv().await.unwrap();
        assert_eq!(event.device_id, "dev-1");
        assert_eq!(event.patch.on, Some(false));
        assert_eq!(event.patch.brightness, None);
    }

    #[tokio::test]
    async fn poll_run_marks_connected_then_reconnecting_on_failure() {
        let mut provider = ScriptedProvider::new(vec![("dev-1".into(), on_at(50.0))]);
        provider.fail_discover_after = 1; // first poll succeeds, second fails
        let (mgr, _rx) = PollingManager::new(Box::new(provider), Duration::from_millis(10));
        let mgr = Arc::new(mgr);
        let state = Arc::clone(&mgr.state);

        let task = tokio::spawn(Arc::clone(&mgr).run());

        // After the first successful poll: Connected.
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(state.read().await.label(), "connected");

        // After the second (failing) poll: Reconnecting with backoff.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(state.read().await.label(), "reconnecting");

        task.abort();
    }

    #[tokio::test]
    async fn poll_discover_failure_propagates_as_err() {
        let mut provider = ScriptedProvider::new(vec![]);
        provider.fail_discover_after = 0;
        let (mgr, _rx) = PollingManager::new(Box::new(provider), Duration::from_secs(60));
        assert!(mgr.poll_once().await.is_err());
    }

    // ── DB writer merging ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn partial_event_merges_into_stored_state() {
        // Regression for the Hue out-of-sync bug: a brightness-only event must
        // not reset on/color in the stored state.
        use sqlx::Row;
        use sqlx::sqlite::SqliteConnectOptions;
        use std::str::FromStr;

        let opts = SqliteConnectOptions::from_str(":memory:")
            .unwrap()
            .foreign_keys(true);
        let db = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("./migrations").run(&db).await.unwrap();

        sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES ('p1','hue','H','enc')")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state)
             VALUES ('l1','p1','dev-1','L','{}', ?)",
        )
        .bind(r#"{"on":true,"brightness":80.0,"color":null,"color_temp_mirek":null}"#)
        .execute(&db)
        .await
        .unwrap();

        let event = LightEvent {
            device_id: "dev-1".into(),
            patch: LightStatePatch {
                brightness: Some(42.0),
                ..Default::default()
            },
            capabilities: None,
        };
        apply_event_to_db(&event, &db).await;

        let row =
            sqlx::query("SELECT last_state, capabilities FROM lights WHERE device_id = 'dev-1'")
                .fetch_one(&db)
                .await
                .unwrap();
        let state: LightState = serde_json::from_str(&row.get::<String, _>("last_state")).unwrap();
        assert!(state.on, "brightness event must not flip the light off");
        assert_eq!(state.brightness, Some(42.0));
        // SSE patch carries no capabilities: the stored value is left untouched.
        assert_eq!(row.get::<String, _>("capabilities"), "{}");
    }

    #[tokio::test]
    async fn poll_event_refreshes_stale_capabilities() {
        // Regression: a Hue color bulb discovered before the color_rgb fix had a
        // stale `color_rgb: false` capability that never self-healed, because only
        // the initial provider sync wrote capabilities. Authoritative poll events
        // now carry full capabilities and refresh the stored row.
        use sqlx::Row;
        use sqlx::sqlite::SqliteConnectOptions;
        use std::str::FromStr;

        let opts = SqliteConnectOptions::from_str(":memory:")
            .unwrap()
            .foreign_keys(true);
        let db = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("./migrations").run(&db).await.unwrap();

        sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES ('p1','hue','H','enc')")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state)
             VALUES ('l1','p1','dev-1','L', ?, '{}')",
        )
        .bind(r#"{"dimmable":true,"color_rgb":false,"color_temperature":true,"hue_gamut":null}"#)
        .execute(&db)
        .await
        .unwrap();

        let event = LightEvent {
            device_id: "dev-1".into(),
            patch: LightStatePatch {
                on: Some(true),
                ..Default::default()
            },
            capabilities: Some(LightCapabilities {
                dimmable: true,
                color_rgb: true,
                color_temperature: true,
                hue_gamut: Some(crate::models::HueGamut::C),
            }),
        };
        apply_event_to_db(&event, &db).await;

        let row = sqlx::query("SELECT capabilities FROM lights WHERE device_id = 'dev-1'")
            .fetch_one(&db)
            .await
            .unwrap();
        let caps: LightCapabilities =
            serde_json::from_str(&row.get::<String, _>("capabilities")).unwrap();
        assert!(caps.color_rgb, "poll must refresh stale color_rgb to true");
        assert_eq!(caps.hue_gamut, Some(crate::models::HueGamut::C));
    }

    // ── Audio push manager ───────────────────────────────────────────────────

    use crate::models::audio::{AudioCommand, AudioDevice, AudioState};

    /// Scripted push provider: each `event_stream` call yields the scripted
    /// events then closes (simulating a dropped socket). Connect fails when
    /// `fail_connect` is set.
    struct ScriptedAudioProvider {
        events: Vec<AudioEvent>,
        fail_connect: bool,
    }

    #[async_trait::async_trait]
    impl AudioProvider for ScriptedAudioProvider {
        fn name(&self) -> &str {
            "scripted-audio"
        }
        async fn discover(&self) -> Result<Vec<AudioDevice>> {
            Ok(vec![])
        }
        async fn get_state(&self, _id: &str) -> Result<AudioState> {
            Ok(AudioState::default())
        }
        async fn set_state(&self, _id: &str, _cmd: &AudioCommand) -> Result<()> {
            Ok(())
        }
        async fn event_stream(&self) -> Result<tokio::sync::mpsc::Receiver<AudioEvent>> {
            if self.fail_connect {
                anyhow::bail!("scripted connect failure");
            }
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            let events = self.events.clone();
            tokio::spawn(async move {
                for e in events {
                    if tx.send(e).await.is_err() {
                        return;
                    }
                }
                // Hold the channel open briefly so the manager stays Connected.
                tokio::time::sleep(Duration::from_millis(200)).await;
            });
            Ok(rx)
        }
    }

    fn audio_event(volume: u8) -> AudioEvent {
        AudioEvent {
            device_id: "main".into(),
            state: AudioState {
                power: true,
                volume,
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn audio_push_manager_broadcasts_events_and_marks_connected() {
        let provider = ScriptedAudioProvider {
            events: vec![audio_event(30), audio_event(45)],
            fail_connect: false,
        };
        let (mgr, mut rx) = AudioPushManager::new(Box::new(provider));
        let mgr = Arc::new(mgr);
        let state = Arc::clone(&mgr.state);
        let task = tokio::spawn(Arc::clone(&mgr).run());

        let e1 = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("first event in time")
            .unwrap();
        assert_eq!(e1.state.volume, 30);
        let e2 = rx.recv().await.unwrap();
        assert_eq!(e2.state.volume, 45);
        assert_eq!(state.read().await.label(), "connected");

        task.abort();
    }

    #[tokio::test]
    async fn audio_push_manager_backs_off_when_connect_fails() {
        let provider = ScriptedAudioProvider {
            events: vec![],
            fail_connect: true,
        };
        let (mgr, _rx) = AudioPushManager::new(Box::new(provider));
        let mgr = Arc::new(mgr);
        let state = Arc::clone(&mgr.state);
        let task = tokio::spawn(Arc::clone(&mgr).run());

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(state.read().await.label(), "reconnecting");

        task.abort();
    }

    #[tokio::test]
    async fn audio_db_writer_replaces_last_state() {
        use sqlx::Row;
        use sqlx::sqlite::SqliteConnectOptions;
        use std::str::FromStr;

        let opts = SqliteConnectOptions::from_str(":memory:")
            .unwrap()
            .foreign_keys(true);
        let db = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("./migrations").run(&db).await.unwrap();

        sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES ('p1','onkyo','AV','enc')")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO audio_devices (id, provider_id, device_id, name, kind, capabilities, last_state)
             VALUES ('a1','p1','main','Receiver','receiver','{}', ?)",
        )
        .bind(r#"{"power":false,"volume":0,"mute":false}"#)
        .execute(&db)
        .await
        .unwrap();

        let (tx, rx) = broadcast::channel(8);
        let writer = tokio::spawn(audio_db_writer_task(rx, "p1".to_string(), db.clone()));
        tx.send(audio_event(62)).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let row = sqlx::query("SELECT last_state FROM audio_devices WHERE id = 'a1'")
            .fetch_one(&db)
            .await
            .unwrap();
        let state: AudioState = serde_json::from_str(&row.get::<String, _>("last_state")).unwrap();
        assert!(state.power);
        assert_eq!(state.volume, 62);

        writer.abort();
    }

    // ── Backoff ──────────────────────────────────────────────────────────────

    #[test]
    fn delay_is_always_positive() {
        for attempt in 0..=15 {
            let d = backoff_delay(attempt);
            assert!(d.as_millis() >= 100, "attempt {attempt}: delay was {:?}", d);
        }
    }

    #[test]
    fn delay_increases_with_early_attempts() {
        // Run several times to smooth out jitter.
        let samples = 20;
        let avg = |attempt: u32| -> u128 {
            (0..samples)
                .map(|_| backoff_delay(attempt).as_millis())
                .sum::<u128>()
                / samples
        };
        assert!(avg(0) < avg(2), "delay should grow between attempt 0 and 2");
        assert!(avg(2) < avg(5), "delay should grow between attempt 2 and 5");
    }

    #[test]
    fn delay_caps_near_max() {
        // At attempt 10+, delay must be at most MAX + 20% jitter.
        for attempt in 10..=20 {
            let d = backoff_delay(attempt);
            let ceiling = (BACKOFF_MAX_MS as f64 * 1.11) as u128; // max + 11% headroom
            assert!(
                d.as_millis() <= ceiling,
                "attempt {attempt}: delay {d:?} exceeded ceiling {ceiling}ms"
            );
        }
    }

    #[test]
    fn jitter_produces_variation() {
        // Same attempt should not always yield the exact same duration.
        let delays: Vec<_> = (0..20).map(|_| backoff_delay(3).as_millis()).collect();
        let unique: std::collections::HashSet<_> = delays.iter().collect();
        assert!(unique.len() > 1, "backoff has no jitter");
    }
}
