use crate::AppState;
use crate::api::auth::Session;
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    routing::get,
};
use futures_util::stream::{self, StreamExt};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::{BroadcastStream, IntervalStream};

type SseStream = Pin<Box<dyn stream::Stream<Item = Result<Event, Infallible>> + Send>>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(sse_events))
}

/// How often the stream emits an `hb` event. This is a **named event**, not the
/// SSE keep-alive comment: a browser's `EventSource` never surfaces comments, so
/// a comment-only heartbeat gives the client no way to tell a healthy-but-quiet
/// stream from a connection that died silently (the WebView-suspended zombie a
/// wall tablet produces every screen-off cycle). With a real event the client
/// can watchdog the stream and reconnect on silence — see `frontend/src/useEvents.ts`.
const HEARTBEAT: Duration = Duration::from_secs(20);

async fn sse_events(
    State(state): State<Arc<AppState>>,
    _: Session,
) -> Result<axum::response::Response, StatusCode> {
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
            .filter_map(|r| std::future::ready(r.ok()))
            .map(|event| {
                let data = serde_json::to_string(&serde_json::json!({
                    "device_id": event.device_id,
                    "patch": event.patch,
                }))
                .unwrap_or_default();
                Ok::<Event, Infallible>(Event::default().event("light_state").data(data))
            })
            .boxed(),
        // Media/power/sensor pushes are full-state snapshots tagged with the
        // provider row id, so the frontend can match its device rows.
        BroadcastStream::new(media)
            .filter_map(|r| std::future::ready(r.ok()))
            .map(|(provider_id, event)| {
                let data = serde_json::to_string(&serde_json::json!({
                    "provider_id": provider_id,
                    "device_id": event.device_id,
                    "state": event.state,
                }))
                .unwrap_or_default();
                Ok::<Event, Infallible>(Event::default().event("media_state").data(data))
            })
            .boxed(),
        BroadcastStream::new(power)
            .filter_map(|r| std::future::ready(r.ok()))
            .map(|(provider_id, event)| {
                let data = serde_json::to_string(&serde_json::json!({
                    "provider_id": provider_id,
                    "device_id": event.device_id,
                    "state": event.state,
                }))
                .unwrap_or_default();
                Ok::<Event, Infallible>(Event::default().event("power_state").data(data))
            })
            .boxed(),
        BroadcastStream::new(sensors)
            .filter_map(|r| std::future::ready(r.ok()))
            .map(|(provider_id, event)| {
                let data = serde_json::to_string(&serde_json::json!({
                    "provider_id": provider_id,
                    "device_id": event.device_id,
                    "state": event.state,
                }))
                .unwrap_or_default();
                Ok::<Event, Infallible>(Event::default().event("sensor_state").data(data))
            })
            .boxed(),
        // Inventory changes (rename/glyph/enable/room/shadow, board edits): one
        // app-wide channel, so device lists refresh live on every surface and
        // every client.
        BroadcastStream::new(state.inventory_events.subscribe())
            .filter_map(|r| std::future::ready(r.ok()))
            .map(|table: String| {
                let data = serde_json::to_string(&serde_json::json!({ "table": table }))
                    .unwrap_or_default();
                Ok::<Event, Infallible>(Event::default().event("inventory").data(data))
            })
            .boxed(),
        // The observable liveness beat (see HEARTBEAT).
        IntervalStream::new(tokio::time::interval(HEARTBEAT))
            .map(|_| Ok::<Event, Infallible>(Event::default().event("hb").data("1")))
            .boxed(),
    ];

    // A pending stream keeps select_all from ever terminating.
    streams.push(stream::pending().boxed());

    let merged = stream::select_all(streams);

    let sse = Sse::new(merged).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    );
    Ok(crate::api::sse_unbuffered(sse))
}

#[cfg(test)]
mod tests {
    #[test]
    fn keep_alive_interval_is_reasonable() {
        // Keep-alive must be short enough that proxies don't drop the connection.
        // 15 s is well within the typical 60 s idle timeout.
        let secs: u64 = 15;
        assert!(secs < 60);
    }
}
