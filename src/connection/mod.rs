//! Hue bridge connection manager.
//!
//! Home Assistant is known to drop the Hue SSE event stream and not recover reliably.
//! This module addresses that with an explicit state machine, exponential-backoff
//! reconnect, polling fallback during outages, and periodic health checks.

use crate::models::LightState;
use crate::providers::LightProvider;
use crate::providers::hue::HueProvider;
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

/// A light-state update broadcast to all WebSocket clients.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LightEvent {
    pub device_id: String,
    pub state: LightState,
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
                let state = crate::providers::hue::parse_light_state_from_event(item);
                let _ = self.events.send(LightEvent {
                    device_id: id.to_string(),
                    state,
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
                device_id: light.provider_id,
                state: light.state,
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

struct ConnectionEntry {
    manager: Arc<HueConnectionManager>,
    _sse_task: JoinHandle<()>,
    _db_task: JoinHandle<()>,
}

impl Drop for ConnectionEntry {
    fn drop(&mut self) {
        self._sse_task.abort();
        self._db_task.abort();
    }
}

/// Owns one `HueConnectionManager` per provider. Thread-safe via `Mutex<ConnectionRegistry>`.
pub struct ConnectionRegistry {
    entries: HashMap<String, ConnectionEntry>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Spawn the SSE loop and a DB writer task for the given Hue provider.
    pub fn start(&mut self, provider_id: String, provider: HueProvider, db: SqlitePool) {
        let (mgr, rx) = HueConnectionManager::new(provider);
        let mgr = Arc::new(mgr);
        let sse_task = tokio::spawn(Arc::clone(&mgr).run());
        let db_task = tokio::spawn(db_writer_task(rx, db));
        self.entries.insert(
            provider_id,
            ConnectionEntry {
                manager: mgr,
                _sse_task: sse_task,
                _db_task: db_task,
            },
        );
    }

    /// Abort tasks for the given provider. No-op if not managed.
    pub fn stop(&mut self, provider_id: &str) {
        self.entries.remove(provider_id); // Drop aborts both tasks.
    }

    /// Return a shared handle to the manager's state lock (for the status endpoint).
    pub fn get_state_lock(&self, provider_id: &str) -> Option<Arc<RwLock<ConnectionState>>> {
        self.entries
            .get(provider_id)
            .map(|e| Arc::clone(&e.manager.state))
    }

    /// Subscribe to all managed Hue managers. Each returned receiver gets every
    /// `LightEvent` broadcast by its manager. Used by the SSE endpoint.
    pub fn subscribe_all(&self) -> Vec<broadcast::Receiver<LightEvent>> {
        self.entries
            .values()
            .map(|e| e.manager.events.subscribe())
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
            Ok(event) => {
                let state_json = serde_json::to_string(&event.state).unwrap_or_default();
                let _ = sqlx::query(
                    "UPDATE lights SET last_state = ?, last_seen = datetime('now') WHERE device_id = ?",
                )
                .bind(&state_json)
                .bind(&event.device_id)
                .execute(&db)
                .await;
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("light event db writer lagged by {n} events");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
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
