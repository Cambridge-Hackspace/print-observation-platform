//! Live updates to every screen, over Server-Sent Events (ADR 0001).
//!
//! Updates only flow from the server to screens; actions are ordinary HTTP
//! requests. A screen that connects, or reconnects, is sent a snapshot first,
//! so it catches up rather than rebuilding its state. One that falls so far
//! behind that it misses events is sent a fresh snapshot in their place,
//! rather than carrying on with a gap it cannot see.

use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use futures_util::{Stream, StreamExt, stream};
use tokio::sync::{broadcast, watch};
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};

use crate::{AppState, api::ServerEvent};

/// How many events a slow screen may fall behind before it is sent a fresh
/// snapshot instead.
const BACKLOG: usize = 256;

/// The live-update hub: publish an event once, and every connected screen
/// receives it.
#[derive(Clone)]
pub struct Events {
    tx: broadcast::Sender<(u64, ServerEvent)>,
    next_id: Arc<AtomicU64>,
    closed: Arc<watch::Sender<bool>>,
}

impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}

impl Events {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BACKLOG);
        Self {
            tx,
            next_id: Arc::new(AtomicU64::new(1)),
            closed: Arc::new(watch::Sender::new(false)),
        }
    }

    /// Send `event` to every connected screen.
    pub fn publish(&self, event: ServerEvent) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // No receivers means no screens are connected, which is not an error.
        let _ = self.tx.send((id, event));
    }

    /// End every stream, and any opened from now on, so the server can shut
    /// down. A stream never ends on its own, and a graceful shutdown waits for
    /// every connection to close: without this, one open kiosk holds the server
    /// up until its supervisor gives up and kills it. Screens reconnect to the
    /// next server and are sent a snapshot.
    pub fn close(&self) {
        self.closed.send_replace(true);
    }

    fn subscribe(&self) -> broadcast::Receiver<(u64, ServerEvent)> {
        self.tx.subscribe()
    }

    /// The id of the newest event published, for a snapshot to carry.
    fn current_id(&self) -> u64 {
        self.next_id.load(Ordering::Relaxed).saturating_sub(1)
    }
}

/// `GET /api/events`.
pub async fn stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribe before taking the snapshot, so nothing published in between
    // is lost: at worst a screen sees an event its snapshot already includes.
    let rx = state.events.subscribe();
    let mut closed = state.events.closed.subscribe();
    let ended = async move {
        let _ = closed.wait_for(|c| *c).await;
    };
    let first = snapshot(&state).await;
    let rest = BroadcastStream::new(rx).then(move |item| {
        let state = state.clone();
        async move {
            match item {
                Ok((id, event)) => frame(id, &event),
                Err(BroadcastStreamRecvError::Lagged(_)) => snapshot(&state).await,
            }
        }
    });
    Sse::new(stream::once(async { first }).chain(rest).take_until(ended))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

async fn snapshot(state: &AppState) -> Result<Event, Infallible> {
    let id = state.events.current_id();
    frame(id, &ServerEvent::Status(state.status().await))
}

fn frame(id: u64, event: &ServerEvent) -> Result<Event, Infallible> {
    // A ServerEvent is plain data with string keys, so serializing it cannot
    // fail; the fallback keeps a stream alive rather than panicking if that
    // ever stops being true.
    let data = serde_json::to_string(event)
        .unwrap_or_else(|e| format!(r#"{{"type":"error","data":"{e}"}}"#));
    Ok(Event::default().id(id.to_string()).data(data))
}
