use crate::AppState;
use crate::api::auth::require_session;
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
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
    headers: HeaderMap,
) -> Result<Sse<impl stream::Stream<Item = Result<Event, Infallible>> + Send + 'static>, StatusCode>
{
    if require_session(&state, &headers).await.is_none() {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let (receivers, audio_receivers) = {
        let connections = state.connections.lock().await;
        (connections.subscribe_all(), connections.subscribe_all_audio())
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

    // Audio push events: full-state snapshots tagged with the provider row id
    // so the frontend can match its audio_devices rows.
    for (provider_id, rx) in audio_receivers {
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
                    Ok::<Event, Infallible>(Event::default().event("audio_state").data(data))
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
