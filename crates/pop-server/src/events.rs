//! Live updates to every screen, over Server-Sent Events (ADR 0001, #6).
//!
//! Updates only flow from the server to screens; actions are ordinary HTTP
//! requests. Every event has an id, `<epoch>.<n>`, where `n` increases and
//! the epoch is this process's, so an id from before a restart is never
//! mistaken for one after it.
//!
//! - A screen that connects is sent a snapshot: everything it shows, at once.
//! - A screen that reconnects says the last id it saw (the `Last-Event-ID`
//!   header, which browsers send on their own, or `?last_event_id=` from a
//!   screen that had to open a new connection itself). If that id is still in
//!   the replay buffer, it is sent exactly the events it missed. If not, or it
//!   is from another epoch, it is sent a snapshot instead.
//! - A screen that falls so far behind that the stream drops events for it is
//!   sent a snapshot in their place, rather than carrying on with a gap.
//!
//! Publishing, and a screen subscribing and reading the replay buffer, happen
//! under one lock, so a reconnecting screen gets every event once: those
//! before it subscribed from the buffer, those after from the stream. A
//! snapshot carries the id it was taken at, and the subscription after it
//! starts at the next one.
//!
//! The stream is public (#7), so what it names is decided per connection
//! (#134): a screen signed in, or on the space's own network (the kiosk,
//! until kiosk sessions, #12), sees whose print is whose. Anyone else sees
//! owners' first names only when `[public] owner_names` is on, and otherwise
//! nobody named at all, in a printer's job or in a notice.
//! `tests/public_stream.rs` pins every field the stream carries.

use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Extension,
    extract::{ConnectInfo, Query, State},
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
};
use futures_util::{Stream, StreamExt, stream};
use tokio::sync::{broadcast, watch};
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};

use crate::{
    AppState,
    api::{ServerEvent, Snapshot},
};

/// How many events a reconnecting screen can catch up on, and how many a slow
/// one may fall behind, before it is sent a snapshot instead.
pub const REPLAY: usize = 256;

#[derive(Clone)]
pub struct Events {
    inner: Arc<Inner>,
}

struct Inner {
    epoch: String,
    log: Mutex<Log>,
    tx: broadcast::Sender<(u64, ServerEvent)>,
    closed: watch::Sender<bool>,
}

struct Log {
    /// The id the next event gets.
    next: u64,
    /// The newest events, oldest first, at most REPLAY of them.
    recent: VecDeque<(u64, ServerEvent)>,
}

impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}

impl Events {
    pub fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Self::with_epoch(format!("{nanos:x}{:x}", std::process::id()))
    }

    /// An event hub whose ids carry `epoch`, for a test that needs to know it.
    pub fn with_epoch(epoch: String) -> Self {
        let (tx, _) = broadcast::channel(REPLAY);
        Self {
            inner: Arc::new(Inner {
                epoch,
                log: Mutex::new(Log {
                    next: 1,
                    recent: VecDeque::with_capacity(REPLAY),
                }),
                tx,
                closed: watch::Sender::new(false),
            }),
        }
    }

    pub fn epoch(&self) -> &str {
        &self.inner.epoch
    }

    /// Send `event` to every connected screen, and keep it for any that
    /// reconnect.
    pub fn publish(&self, event: ServerEvent) {
        let mut log = self.lock();
        let id = log.next;
        log.next += 1;
        if log.recent.len() == REPLAY {
            log.recent.pop_front();
        }
        log.recent.push_back((id, event.clone()));
        // No receivers means no screens are connected, which is not an error.
        let _ = self.inner.tx.send((id, event));
    }

    /// End every stream, and any opened from now on, so the server can shut
    /// down. A stream never ends on its own, and a graceful shutdown waits for
    /// every connection to close: without this, one open kiosk holds the server
    /// up until its supervisor gives up and kills it. Screens reconnect to the
    /// next server and are sent a snapshot.
    pub fn close(&self) {
        self.inner.closed.send_replace(true);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Log> {
        // A panic while holding this lock leaves the log whole (every write
        // to it is a single push or pop), so carrying on is safe.
        self.inner.log.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A fresh subscription for a screen that fell behind, and the id of the
    /// newest event, which its snapshot will include. Taken under the lock, so
    /// the stream picks up exactly after that id, and the stale backlog of the
    /// old subscription, all of it older, is dropped rather than replayed.
    fn rejoin(&self) -> (broadcast::Receiver<(u64, ServerEvent)>, u64) {
        let log = self.lock();
        (self.inner.tx.subscribe(), log.next - 1)
    }

    /// Subscribe, and decide how a screen that last saw `last` catches up:
    /// with the events it missed, or (`None`) with a snapshot.
    fn join(&self, last: Option<&str>) -> Join {
        let log = self.lock();
        let rx = self.inner.tx.subscribe();
        let current = log.next - 1;
        let replay = last.and_then(|l| self.parse(l)).and_then(|seen| {
            // Resumable if nothing it missed has left the buffer: the oldest
            // kept event follows directly on what it saw, or it missed none.
            let oldest = log.recent.front().map_or(log.next, |(id, _)| *id);
            (seen <= current && seen + 1 >= oldest).then(|| {
                log.recent
                    .iter()
                    .filter(|(id, _)| *id > seen)
                    .cloned()
                    .collect::<Vec<_>>()
            })
        });
        Join {
            rx,
            current,
            replay,
        }
    }

    /// `n` from an id of this epoch.
    fn parse(&self, id: &str) -> Option<u64> {
        let (epoch, n) = id.rsplit_once('.')?;
        (epoch == self.inner.epoch)
            .then(|| n.parse().ok())
            .flatten()
    }

    fn id(&self, n: u64) -> String {
        format!("{}.{n}", self.inner.epoch)
    }
}

struct Join {
    rx: broadcast::Receiver<(u64, ServerEvent)>,
    current: u64,
    replay: Option<Vec<(u64, ServerEvent)>>,
}

/// Whose names a connection to the stream sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Names {
    /// Whole names: signed in, or at the space.
    All,
    /// Owners' first names: the public, with `[public] owner_names` on.
    First,
    /// Nobody's.
    None,
}

/// Whose names this request may see.
async fn names_for(state: &AppState, peer: Option<std::net::IpAddr>, headers: &HeaderMap) -> Names {
    if crate::access::is_local(&state.access, peer, headers) {
        return Names::All;
    }
    if let Some(token) = crate::access::session_token(headers)
        && let Ok(Some(_)) = crate::users::session_user(&state.db, &token).await
    {
        return Names::All;
    }
    if state.public.owner_names {
        Names::First
    } else {
        Names::None
    }
}

/// An event as a connection that sees `names` is sent it.
pub fn named(event: &ServerEvent, names: Names) -> ServerEvent {
    let printers = |ps: &[crate::api::TowerPrinter]| -> Vec<crate::api::TowerPrinter> {
        ps.iter()
            .cloned()
            .map(|mut p| {
                if let Some(j) = p.state.job.as_mut() {
                    j.owner = match names {
                        Names::All => j.owner.take(),
                        Names::First => j
                            .owner
                            .as_deref()
                            .and_then(crate::notices::first_name)
                            .map(str::to_owned),
                        Names::None => None,
                    };
                }
                p
            })
            .collect()
    };
    let told = |ts: &[crate::api::GroupNotice]| -> Vec<crate::api::GroupNotice> {
        ts.iter()
            .cloned()
            .map(|mut t| {
                // What was posted names the owner by first name only.
                if names == Names::None {
                    t.text = t.without_owner.clone();
                }
                t
            })
            .collect()
    };
    match event {
        ServerEvent::Snapshot(s) => ServerEvent::Snapshot(Snapshot {
            status: s.status.clone(),
            printers: printers(&s.printers),
            told: told(&s.told),
        }),
        ServerEvent::Printers(ps) => ServerEvent::Printers(printers(ps)),
        ServerEvent::Told(ts) => ServerEvent::Told(told(ts)),
        ServerEvent::Status(_) => event.clone(),
    }
}

/// `GET /api/events`.
pub async fn stream(
    State(state): State<AppState>,
    connected: Option<Extension<ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let peer = connected.map(|Extension(ConnectInfo(a))| a.ip());
    let names = names_for(&state, peer, &headers).await;
    let last = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| query.get("last_event_id").cloned());
    let mut closed = state.events.inner.closed.subscribe();
    let ended = async move {
        let _ = closed.wait_for(|c| *c).await;
    };
    let Join {
        rx,
        current,
        replay,
    } = state.events.join(last.as_deref());

    // What the screen is sent first: what it missed, or a snapshot. Either
    // way the subscription, taken under the same lock, starts right after it.
    let first = match replay {
        Some(missed) => missed
            .iter()
            .map(|(n, e)| frame(&state.events, *n, &named(e, names)))
            .collect::<Vec<_>>(),
        None => vec![snapshot(&state, current, names).await],
    };

    let rest = stream::unfold(
        (BroadcastStream::new(rx), state),
        move |(mut rx, state)| async move {
            match rx.next().await? {
                Ok((n, e)) => Some((frame(&state.events, n, &named(&e, names)), (rx, state))),
                Err(BroadcastStreamRecvError::Lagged(_)) => {
                    // Its backlog is older than the snapshot it is about to
                    // be sent: drop it with the old subscription.
                    let (fresh, at) = state.events.rejoin();
                    let s = snapshot(&state, at, names).await;
                    Some((s, (BroadcastStream::new(fresh), state)))
                }
            }
        },
    );
    Sse::new(stream::iter(first).chain(rest).take_until(ended))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

/// Everything a screen shows, as of event `at`. Taken after `at` was read, so
/// it includes at least everything up to it.
async fn snapshot(state: &AppState, at: u64, names: Names) -> Result<Event, Infallible> {
    let snap = ServerEvent::Snapshot(Snapshot {
        status: state.status().await,
        printers: state.tower.borrow().clone(),
        told: state.told.borrow().clone(),
    });
    frame(&state.events, at, &named(&snap, names))
}

fn frame(events: &Events, n: u64, event: &ServerEvent) -> Result<Event, Infallible> {
    // A ServerEvent is plain data with string keys, so serializing it cannot
    // fail; the fallback keeps a stream alive rather than panicking if that
    // ever stops being true.
    let data = serde_json::to_string(event)
        .unwrap_or_else(|e| format!(r#"{{"type":"error","data":"{e}"}}"#));
    Ok(Event::default().id(events.id(n)).data(data))
}
