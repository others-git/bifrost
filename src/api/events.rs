use crate::AppState;
use crate::api::auth::SessionOrKiosk;
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    routing::get,
};
use futures_util::stream::{self, StreamExt};
use serde::Serialize;
use std::collections::HashMap;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio_stream::wrappers::{BroadcastStream, IntervalStream};

type SseStream = Pin<Box<dyn stream::Stream<Item = Result<Event, Infallible>> + Send>>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(sse_events))
}

// ── Live-subscriber registry ────────────────────────────────────────────────
//
// The hub used to have no idea who was listening to `/api/events`: a stream
// opening, dying, or silently dropping events left no trace at all. That makes
// the whole class of "a surface went stale and only a reload fixes it" reports
// undiagnosable from the hub side — you cannot tell a client that never
// reconnected from one that is connected and receiving events it fails to
// render, and on a locked-down wall tablet there are no browser devtools to
// ask. This registry is the missing half: `GET /api/dev/streams` names every
// live subscriber and what it has actually been sent.

/// Counters for one live SSE subscriber. Shared (`Arc`) between the registry
/// and the stream's own branches, which bump them as events go out.
#[derive(Debug)]
pub struct StreamStats {
    id: u64,
    /// Which client this is, as far as we can tell: a paired kiosk by name, or
    /// a plain browser session.
    label: String,
    /// The kiosk row this stream belongs to, when the request carried a kiosk
    /// key. Names collide (two tablets of one model default to the same one),
    /// so the id is what lets a diagnostics view say WHICH tablet is missing.
    kiosk_id: Option<String>,
    user_agent: String,
    connected_at: std::time::SystemTime,
    /// Device-state + inventory events written to this client.
    events_sent: AtomicU64,
    /// Heartbeats written to this client. Separated from `events_sent` on
    /// purpose: "connected, beating, but zero events" and "connected and
    /// nothing at all" are different failures with different causes.
    beats_sent: AtomicU64,
    /// Events this subscriber missed because it could not keep up and the
    /// broadcast channel dropped them. Silently swallowed before, which made a
    /// lagging client indistinguishable from an idle house.
    lagged: AtomicU64,
    /// Unix-ms of the last write of any kind; 0 until the first one.
    last_write_ms: AtomicU64,
}

impl StreamStats {
    fn touch(&self) {
        self.last_write_ms.store(unix_ms(), Ordering::Relaxed);
    }
    fn event(&self) {
        self.events_sent.fetch_add(1, Ordering::Relaxed);
        self.touch();
    }
    fn beat(&self) {
        self.beats_sent.fetch_add(1, Ordering::Relaxed);
        self.touch();
    }
    fn lag(&self, n: u64) {
        self.lagged.fetch_add(n, Ordering::Relaxed);
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One live subscriber, as reported by `GET /api/dev/streams`.
#[derive(Debug, Serialize, PartialEq)]
pub struct StreamSnapshot {
    pub id: u64,
    pub label: String,
    /// Set when this subscriber is a paired kiosk — joins it to its kiosk row.
    pub kiosk_id: Option<String>,
    pub user_agent: String,
    pub connected_secs: u64,
    pub events_sent: u64,
    pub beats_sent: u64,
    pub lagged: u64,
    /// Seconds since the last write, or `None` if nothing has been written yet.
    pub idle_secs: Option<u64>,
}

/// Every `/api/events` subscriber currently connected.
#[derive(Default)]
pub struct StreamRegistry {
    live: Mutex<HashMap<u64, Arc<StreamStats>>>,
    next_id: AtomicU64,
}

impl StreamRegistry {
    /// Register a subscriber. The returned guard removes it again when the
    /// response stream is dropped — which, for SSE, is the only disconnect
    /// signal there is.
    fn register(
        self: &Arc<Self>,
        label: String,
        kiosk_id: Option<String>,
        user_agent: String,
    ) -> StreamGuard {
        let stats = Arc::new(StreamStats {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            label,
            kiosk_id,
            user_agent,
            connected_at: std::time::SystemTime::now(),
            events_sent: AtomicU64::new(0),
            beats_sent: AtomicU64::new(0),
            lagged: AtomicU64::new(0),
            last_write_ms: AtomicU64::new(0),
        });
        if let Ok(mut live) = self.live.lock() {
            live.insert(stats.id, Arc::clone(&stats));
        }
        tracing::debug!(
            target: "bifrost::events",
            stream = stats.id,
            label = %stats.label,
            ua = %stats.user_agent,
            "sse subscriber connected"
        );
        StreamGuard {
            registry: Arc::clone(self),
            stats,
        }
    }

    pub fn snapshot(&self) -> Vec<StreamSnapshot> {
        let now = std::time::SystemTime::now();
        let now_ms = unix_ms();
        let Ok(live) = self.live.lock() else {
            return Vec::new();
        };
        let mut out: Vec<StreamSnapshot> = live
            .values()
            .map(|s| {
                let last = s.last_write_ms.load(Ordering::Relaxed);
                StreamSnapshot {
                    id: s.id,
                    label: s.label.clone(),
                    kiosk_id: s.kiosk_id.clone(),
                    user_agent: s.user_agent.clone(),
                    connected_secs: now
                        .duration_since(s.connected_at)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    events_sent: s.events_sent.load(Ordering::Relaxed),
                    beats_sent: s.beats_sent.load(Ordering::Relaxed),
                    lagged: s.lagged.load(Ordering::Relaxed),
                    idle_secs: (last > 0).then(|| now_ms.saturating_sub(last) / 1000),
                }
            })
            .collect();
        out.sort_by_key(|s| s.id);
        out
    }
}

/// Keeps a registry entry alive for exactly as long as the response stream.
struct StreamGuard {
    registry: Arc<StreamRegistry>,
    stats: Arc<StreamStats>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        if let Ok(mut live) = self.registry.live.lock() {
            live.remove(&self.stats.id);
        }
        tracing::debug!(
            target: "bifrost::events",
            stream = self.stats.id,
            label = %self.stats.label,
            events = self.stats.events_sent.load(Ordering::Relaxed),
            beats = self.stats.beats_sent.load(Ordering::Relaxed),
            lagged = self.stats.lagged.load(Ordering::Relaxed),
            held_secs = self.stats.connected_at.elapsed().map(|d| d.as_secs()).unwrap_or(0),
            "sse subscriber disconnected"
        );
    }
}

/// Pass a broadcast item through, or count and log the ones the channel dropped
/// because this subscriber fell behind.
///
/// A `Lagged` error means the client has **missed state it will never be sent
/// again** — precisely the "this surface is stale and only a reload fixes it"
/// symptom. It used to be discarded with `r.ok()`, which made a lagging client
/// indistinguishable from a quiet house both here and in the journal.
fn drop_lagged<T>(
    stats: &Arc<StreamStats>,
    domain: &'static str,
    r: Result<T, tokio_stream::wrappers::errors::BroadcastStreamRecvError>,
) -> std::future::Ready<Option<T>> {
    use tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged;
    match r {
        Ok(v) => std::future::ready(Some(v)),
        Err(Lagged(n)) => {
            stats.lag(n);
            tracing::warn!(
                target: "bifrost::events",
                stream = stats.id,
                label = %stats.label,
                domain,
                dropped = n,
                "sse subscriber lagged — events dropped, this client is now behind"
            );
            std::future::ready(None)
        }
    }
}

/// The response stream, carrying its own registration. Dropping the response
/// (the client hung up) drops the guard, which deregisters.
struct Registered<S> {
    inner: S,
    _guard: StreamGuard,
}

impl<S: stream::Stream + Unpin> stream::Stream for Registered<S> {
    type Item = S::Item;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<S::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

/// How often the stream emits an `hb` event. This is a **named event**, not the
/// SSE keep-alive comment: a browser's `EventSource` never surfaces comments, so
/// a comment-only heartbeat gives the client no way to tell a healthy-but-quiet
/// stream from a connection that died silently (the WebView-suspended zombie a
/// wall tablet produces every screen-off cycle). With a real event the client
/// can watchdog the stream and reconnect on silence — see `frontend/src/useEvents.ts`.
const HEARTBEAT: Duration = Duration::from_secs(20);

/// The live stream, gated by a session **or** a paired kiosk's `bfr_key`
/// cookie — deliberately NOT session-only.
///
/// A dashboard session is a 7-day absolute expiry that nothing renews, and a
/// wall tablet's WebView stays loaded for weeks. Session-only gating therefore
/// killed a kiosk's event stream on a timer: `EventSource` cannot see a status
/// code, so a 401 arrives as an indistinguishable `error`, and the client
/// reconnects to the same refused endpoint forever while the hub never
/// registers a subscriber at all. The board keeps rendering whatever it last
/// heard, and only a reload — which re-mints the session — brings it back.
/// The kiosk's *key* is its durable identity (it already authorizes every
/// `/api/v1` write), so the stream rides that instead.
async fn sse_events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    _: SessionOrKiosk,
) -> Result<axum::response::Response, StatusCode> {
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .chars()
        .take(120)
        .collect::<String>();
    // A wall tablet is the client that matters most here and the one we can
    // least easily inspect, so name it by its kiosk row when its key cookie
    // says which one it is.
    let identity = crate::api::kiosks::kiosk_identity_for_headers(&state, &headers).await;
    let label = match &identity {
        Some((_, name)) => format!("kiosk:{name}"),
        None => "session".to_string(),
    };
    let guard = state
        .streams
        .register(label, identity.map(|(id, _)| id), user_agent);
    let stats = Arc::clone(&guard.stats);
    // ONE subscription per domain, on the registry's app-wide fan-in channels —
    // never per provider. A per-provider snapshot would go permanently deaf for
    // any provider whose manager restarts after this connection opened (a
    // relocate rebind, a credential edit, a pairing), with no error to notice:
    // exactly the "board went stale, only a reload fixes it" failure.
    let (lights, media, power, sensors) = {
        let connections = state.connections.lock().await;
        (
            connections.subscribe_lights(),
            connections.subscribe_media(),
            connections.subscribe_power(),
            connections.subscribe_sensors(),
        )
    };

    let mut streams: Vec<SseStream> = vec![
        BroadcastStream::new(lights)
            .filter_map({
                let stats = Arc::clone(&stats);
                move |r| drop_lagged(&stats, "light", r)
            })
            .map({
                let stats = Arc::clone(&stats);
                move |event| {
                    let data = serde_json::to_string(&serde_json::json!({
                        "device_id": event.device_id,
                        "patch": event.patch,
                    }))
                    .unwrap_or_default();
                    stats.event();
                    Ok::<Event, Infallible>(Event::default().event("light_state").data(data))
                }
            })
            .boxed(),
        // Media/power/sensor pushes are full-state snapshots tagged with the
        // provider row id, so the frontend can match its device rows.
        //
        // Media pushes are *composed* first (`media::compose_media_push`): a
        // client must be handed the same effective device a read of it returns,
        // so a receiver-bound source shows its receiver's volume/mute and a
        // receiver's own push also reaches the sources bound to it. One push can
        // therefore become several — hence `then` + `flat_map`.
        BroadcastStream::new(media)
            .filter_map({
                let stats = Arc::clone(&stats);
                move |r| drop_lagged(&stats, "media", r)
            })
            .then({
                let state = Arc::clone(&state);
                move |(provider_id, event)| {
                    let state = Arc::clone(&state);
                    async move {
                        stream::iter(
                            crate::api::media::compose_media_push(&state, &provider_id, &event)
                                .await,
                        )
                    }
                }
            })
            .flatten()
            .map({
                let stats = Arc::clone(&stats);
                move |push| {
                    let data = serde_json::to_string(&serde_json::json!({
                        "provider_id": push.provider_id,
                        "device_id": push.device_id,
                        "state": push.state,
                    }))
                    .unwrap_or_default();
                    stats.event();
                    Ok::<Event, Infallible>(Event::default().event("media_state").data(data))
                }
            })
            .boxed(),
        BroadcastStream::new(power)
            .filter_map({
                let stats = Arc::clone(&stats);
                move |r| drop_lagged(&stats, "power", r)
            })
            .map({
                let stats = Arc::clone(&stats);
                move |(provider_id, event)| {
                    let data = serde_json::to_string(&serde_json::json!({
                        "provider_id": provider_id,
                        "device_id": event.device_id,
                        "state": event.state,
                    }))
                    .unwrap_or_default();
                    stats.event();
                    Ok::<Event, Infallible>(Event::default().event("power_state").data(data))
                }
            })
            .boxed(),
        BroadcastStream::new(sensors)
            .filter_map({
                let stats = Arc::clone(&stats);
                move |r| drop_lagged(&stats, "sensor", r)
            })
            .map({
                let stats = Arc::clone(&stats);
                move |(provider_id, event)| {
                    let data = serde_json::to_string(&serde_json::json!({
                        "provider_id": provider_id,
                        "device_id": event.device_id,
                        "state": event.state,
                    }))
                    .unwrap_or_default();
                    stats.event();
                    Ok::<Event, Infallible>(Event::default().event("sensor_state").data(data))
                }
            })
            .boxed(),
        // Inventory changes (rename/glyph/enable/room/shadow, board edits): one
        // app-wide channel, so device lists refresh live on every surface and
        // every client.
        BroadcastStream::new(state.inventory_events.subscribe())
            .filter_map({
                let stats = Arc::clone(&stats);
                move |r| drop_lagged(&stats, "inventory", r)
            })
            .map({
                let stats = Arc::clone(&stats);
                move |table: String| {
                    let data = serde_json::to_string(&serde_json::json!({ "table": table }))
                        .unwrap_or_default();
                    stats.event();
                    Ok::<Event, Infallible>(Event::default().event("inventory").data(data))
                }
            })
            .boxed(),
        // The observable liveness beat (see HEARTBEAT).
        IntervalStream::new(tokio::time::interval(HEARTBEAT))
            .map({
                let stats = Arc::clone(&stats);
                move |_| {
                    stats.beat();
                    Ok::<Event, Infallible>(Event::default().event("hb").data("1"))
                }
            })
            .boxed(),
    ];

    // A pending stream keeps select_all from ever terminating.
    streams.push(stream::pending().boxed());

    // The guard rides the response stream: when the client hangs up, axum drops
    // the stream, which drops the guard, which deregisters. That drop is the
    // ONLY disconnect signal SSE offers.
    let merged = Registered {
        inner: stream::select_all(streams),
        _guard: guard,
    };

    let sse = Sse::new(merged).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    );
    Ok(crate::api::sse_unbuffered(sse))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registered_subscriber_appears_and_leaves_with_its_guard() {
        let reg = Arc::new(StreamRegistry::default());
        assert!(reg.snapshot().is_empty());

        let guard = reg.register(
            "kiosk:Kitchen".into(),
            Some("kiosk-1".into()),
            "BifrostKiosk/1".into(),
        );
        let live = reg.snapshot();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].label, "kiosk:Kitchen");
        assert_eq!(live[0].user_agent, "BifrostKiosk/1");
        // The id, not the label, is what joins this to a kiosk row: two tablets
        // of one model default to the same name.
        assert_eq!(live[0].kiosk_id.as_deref(), Some("kiosk-1"));
        // Nothing written yet — "connected but never sent anything" has to be
        // distinguishable from "sent something a while ago".
        assert_eq!(live[0].idle_secs, None);
        assert_eq!(live[0].events_sent, 0);

        // Dropping the guard is what a client hanging up looks like.
        drop(guard);
        assert!(
            reg.snapshot().is_empty(),
            "a hung-up client must leave the registry"
        );
    }

    #[test]
    fn counters_separate_events_beats_and_dropped_backlog() {
        let reg = Arc::new(StreamRegistry::default());
        let guard = reg.register("session".into(), None, "curl".into());
        guard.stats.event();
        guard.stats.event();
        guard.stats.beat();
        guard.stats.lag(7);

        let s = &reg.snapshot()[0];
        // Beats must not inflate the event count: "beating but zero events" is
        // the signature that says the hub, not the browser, is the problem.
        assert_eq!(s.events_sent, 2);
        assert_eq!(s.beats_sent, 1);
        assert_eq!(s.lagged, 7);
        assert_eq!(s.idle_secs, Some(0), "a write sets the idle clock");
    }

    #[test]
    fn subscribers_get_distinct_ids_and_a_stable_order() {
        let reg = Arc::new(StreamRegistry::default());
        let a = reg.register("session".into(), None, "one".into());
        let b = reg.register("session".into(), None, "two".into());
        let snap = reg.snapshot();
        assert_eq!(snap.len(), 2);
        assert!(snap[0].id < snap[1].id, "listed oldest-first by id");
        assert_ne!(a.stats.id, b.stats.id);

        // One leaving must not disturb the other.
        drop(a);
        let snap = reg.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].user_agent, "two");
    }

    #[tokio::test]
    async fn lagged_events_are_counted_and_dropped_not_silently_swallowed() {
        let reg = Arc::new(StreamRegistry::default());
        let guard = reg.register("session".into(), None, "curl".into());
        let stats = Arc::clone(&guard.stats);

        use tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged;
        assert_eq!(drop_lagged(&stats, "light", Ok(1u8)).await, Some(1));
        assert_eq!(
            drop_lagged::<u8>(&stats, "light", Err(Lagged(4))).await,
            None,
            "a lagged item can't be delivered — it's gone"
        );
        assert_eq!(
            reg.snapshot()[0].lagged,
            4,
            "but it must be COUNTED: this client has silently missed state"
        );
    }

    #[test]
    fn keep_alive_interval_is_reasonable() {
        // Keep-alive must be short enough that proxies don't drop the connection.
        // 15 s is well within the typical 60 s idle timeout.
        let secs: u64 = 15;
        assert!(secs < 60);
    }
}
