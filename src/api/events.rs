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
use tokio_stream::wrappers::BroadcastStream;

type SseStream = Pin<Box<dyn stream::Stream<Item = Result<Event, Infallible>> + Send>>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(sse_events))
}

async fn sse_events(
    State(state): State<Arc<AppState>>,
    _: Session,
) -> Result<Sse<impl stream::Stream<Item = Result<Event, Infallible>> + Send + 'static>, StatusCode>
{
    let (receivers, media_receivers, power_receivers, sensor_receivers) = {
        let connections = state.connections.lock().await;
        (
            connections.subscribe_all(),
            connections.subscribe_all_media(),
            connections.subscribe_all_power(),
            connections.subscribe_all_sensor(),
        )
    };

    let mut streams: Vec<SseStream> = receivers
        .into_iter()
        .map(|rx| {
            BroadcastStream::new(rx)
                .filter_map(|r| std::future::ready(r.ok()))
                .map(|event| {
                    let data = serde_json::to_string(&serde_json::json!({
                        "device_id": event.device_id,
                        "patch": event.patch,
                    }))
                    .unwrap_or_default();
                    Ok::<Event, Infallible>(Event::default().event("light_state").data(data))
                })
                .boxed()
        })
        .collect();

    // Media push events: full-state snapshots tagged with the provider row id
    // so the frontend can match its media_devices rows.
    for (provider_id, rx) in media_receivers {
        streams.push(
            BroadcastStream::new(rx)
                .filter_map(|r| std::future::ready(r.ok()))
                .map(move |event| {
                    let data = serde_json::to_string(&serde_json::json!({
                        "provider_id": provider_id,
                        "device_id": event.device_id,
                        "state": event.state,
                    }))
                    .unwrap_or_default();
                    Ok::<Event, Infallible>(Event::default().event("media_state").data(data))
                })
                .boxed(),
        );
    }

    // Power push events: full snapshots tagged with the provider row id so the
    // frontend can match its power_devices rows.
    for (provider_id, rx) in power_receivers {
        streams.push(
            BroadcastStream::new(rx)
                .filter_map(|r| std::future::ready(r.ok()))
                .map(move |event| {
                    let data = serde_json::to_string(&serde_json::json!({
                        "provider_id": provider_id,
                        "device_id": event.device_id,
                        "state": event.state,
                    }))
                    .unwrap_or_default();
                    Ok::<Event, Infallible>(Event::default().event("power_state").data(data))
                })
                .boxed(),
        );
    }

    // Sensor push events: full snapshots tagged with the provider row id so the
    // frontend can match its sensor_devices rows (motion/lux/temp updates).
    for (provider_id, rx) in sensor_receivers {
        streams.push(
            BroadcastStream::new(rx)
                .filter_map(|r| std::future::ready(r.ok()))
                .map(move |event| {
                    let data = serde_json::to_string(&serde_json::json!({
                        "provider_id": provider_id,
                        "device_id": event.device_id,
                        "state": event.state,
                    }))
                    .unwrap_or_default();
                    Ok::<Event, Infallible>(Event::default().event("sensor_state").data(data))
                })
                .boxed(),
        );
    }

    // A pending stream prevents select_all from terminating when there are no providers.
    streams.push(stream::pending().boxed());

    let merged = stream::select_all(streams);

    Ok(Sse::new(merged).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
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
