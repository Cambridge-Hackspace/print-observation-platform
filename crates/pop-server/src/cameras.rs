//! The printers' cameras, through go2rtc (#66).
//!
//! go2rtc runs as its own service (ADR 0001). POP keeps it serving one stream
//! per tower printer that has a camera in BAM!, named `pop_<BAM! id>`, and
//! looks at each to say whether the camera works. It looks again whenever the
//! printers change, and every half-minute: go2rtc keeps streams added through
//! its API in its own configuration file, but one started from a fresh file,
//! or edited by hand, has lost or changed them, and the next look puts them
//! right.
//!
//! Streams whose names do not start `pop_` are someone else's, and POP leaves
//! them alone. A `pop_` stream no printer wants is removed: go2rtc would keep
//! it, and its camera's address, forever.
//!
//! A camera's address can hold its password, so it is never logged or shown;
//! admins see each camera by its printer and stream.
//!
//! Watching (#67): members watch a tower printer's camera through POP, never
//! go2rtc directly, as MSE over a WebSocket (H.264 cameras) or as MJPEG (JPEG
//! cameras). Read-only by construction: a viewer names a tower printer, never
//! a go2rtc stream, and the only thing a viewer's messages can carry to
//! go2rtc is a list of codecs, checked first. go2rtc's own error text,
//! which can name the camera's address, never reaches a viewer.

use std::{collections::BTreeMap, time::Duration};

use axum::{
    Json,
    body::Body,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use pop_integrations::go2rtc::{Go2rtc, Go2rtcError, Mjpeg, Mse, Picture};
use tokio::{sync::watch, task::JoinHandle};

use crate::{
    AppState,
    api::{AdminCamera, ApiError, CameraHealth},
    health::{CheckFuture, HealthCheck},
};

/// The start of every stream name POP owns.
pub const PREFIX: &str = "pop_";

/// A printer's camera, as BAM! gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Camera {
    /// The printer's name.
    pub name: String,
    /// The camera's address: rtsp, rtsps, http or https.
    pub source: String,
}

/// The cameras go2rtc should serve, by the printer's BAM! id.
pub type Wanted = BTreeMap<String, Camera>;

/// The stream go2rtc serves a printer's camera as.
pub fn stream_name(bam_id: &str) -> String {
    let safe: String = bam_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{PREFIX}{safe}")
}

/// How often to look again with nothing changed.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub every: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            every: Duration::from_secs(30),
        }
    }
}

/// Keep go2rtc serving `wanted`. The receiver has each camera's health, as
/// admins see it.
pub fn spawn(
    client: Go2rtc,
    wanted: watch::Receiver<Wanted>,
    opts: Options,
) -> (watch::Receiver<Vec<AdminCamera>>, JoinHandle<()>) {
    let (tx, rx) = watch::channel(Vec::new());
    (rx, tokio::spawn(run(client, wanted, opts, tx)))
}

async fn run(
    client: Go2rtc,
    mut wanted: watch::Receiver<Wanted>,
    opts: Options,
    tx: watch::Sender<Vec<AdminCamera>>,
) {
    loop {
        let want = wanted.borrow_and_update().clone();
        let synced = sync(&client, &want).await;
        if let Err(e) = &synced {
            tracing::warn!("cameras: {e}");
        }
        let report = look(&client, &want, &synced).await;
        tx.send_if_modified(|r| {
            let changed = *r != report;
            *r = report;
            changed
        });
        tokio::select! {
            changed = wanted.changed() => if changed.is_err() {
                return;
            },
            () = tokio::time::sleep(opts.every) => {}
        }
    }
}

/// Make go2rtc's `pop_` streams exactly `want`. A camera go2rtc refuses is
/// said, by its printer's id; the rest still go in.
async fn sync(client: &Go2rtc, want: &Wanted) -> Result<BTreeMap<String, String>, Go2rtcError> {
    let have = client.streams().await?;
    let names: BTreeMap<String, &str> = want
        .keys()
        .map(|id| (stream_name(id), id.as_str()))
        .collect();
    for name in have.keys().filter(|n| n.starts_with(PREFIX)) {
        if !names.contains_key(name) {
            client.delete(name).await?;
            tracing::info!("cameras: removed {name}, which no printer has now");
        }
    }
    let mut refused = BTreeMap::new();
    for (id, cam) in want {
        let name = stream_name(id);
        if have.get(&name).is_some_and(|s| *s == [cam.source.as_str()]) {
            continue;
        }
        match client.put(&name, &cam.source).await {
            Ok(()) => tracing::info!("cameras: serving {}'s camera as {name}", cam.name),
            Err(Go2rtcError::Refused(why)) => {
                tracing::warn!(
                    "cameras: go2rtc will not serve {}'s camera: {why}",
                    cam.name
                );
                refused.insert(
                    id.clone(),
                    format!("go2rtc will not serve this camera: {why}"),
                );
            }
            Err(e) => return Err(e),
        }
    }
    Ok(refused)
}

/// Each camera's health: a picture from each, all at once.
async fn look(
    client: &Go2rtc,
    want: &Wanted,
    synced: &Result<BTreeMap<String, String>, Go2rtcError>,
) -> Vec<AdminCamera> {
    let mut asked = tokio::task::JoinSet::new();
    for id in want.keys() {
        let client = client.clone();
        let id = id.clone();
        asked.spawn(async move {
            let picture = client.picture(&stream_name(&id)).await;
            (id, picture)
        });
    }
    let mut pictures = BTreeMap::new();
    while let Some(Ok((id, p))) = asked.join_next().await {
        pictures.insert(id, p);
    }
    let mut out: Vec<AdminCamera> = want
        .iter()
        .map(|(id, cam)| {
            let (health, detail) = match (synced, pictures.remove(id)) {
                (Err(e), _) => (CameraHealth::Unknown, Some(e.to_string())),
                (Ok(refused), _) if refused.contains_key(id) => {
                    (CameraHealth::Unknown, refused.get(id).cloned())
                }
                (_, Some(Ok(Picture::Jpeg(_)))) => (CameraHealth::Live, None),
                (_, Some(Ok(Picture::Nothing))) => (CameraHealth::NoPicture, None),
                (_, Some(Ok(Picture::NoStream))) => (CameraHealth::NotInGo2rtc, None),
                (_, Some(Err(e))) => (CameraHealth::Unknown, Some(e.to_string())),
                (_, None) => (CameraHealth::Unknown, None),
            };
            AdminCamera {
                bam_id: id.clone(),
                name: cam.name.clone(),
                stream: stream_name(id),
                health,
                detail,
            }
        })
        .collect();
    out.sort_by_key(|c| c.name.to_lowercase());
    out
}

/// `GET /api/admin/cameras`: each tower printer's camera and whether it works.
pub async fn list(State(state): State<AppState>) -> Response {
    let Some(cameras) = state.cameras.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                error: "POP has no go2rtc to serve cameras: set [go2rtc] url".into(),
            }),
        )
            .into_response();
    };
    Json(cameras.borrow().clone()).into_response()
}

/// go2rtc answers, and lets POP in.
pub struct Go2rtcHealth(pub Go2rtc);

impl HealthCheck for Go2rtcHealth {
    fn name(&self) -> &'static str {
        "go2rtc"
    }

    fn check(&self) -> CheckFuture<'_> {
        Box::pin(async move {
            self.0.version().await.map(|_| ()).map_err(|e| {
                tracing::error!("health: {e}");
                match e {
                    Go2rtcError::Auth => "go2rtc refuses POP's user name and password".to_owned(),
                    _ => "go2rtc does not answer".to_owned(),
                }
            })
        })
    }
}

/// How many may watch at once, across every camera: the tower's connection
/// carries each viewer's video separately.
pub const MAX_VIEWERS: usize = 16;

fn refuse(code: StatusCode, msg: &str) -> Response {
    (
        code,
        Json(ApiError {
            error: msg.to_owned(),
        }),
    )
        .into_response()
}

/// The go2rtc stream of a tower printer's camera, if it has one.
fn camera_stream(state: &AppState, bam_id: &str) -> Option<String> {
    state
        .cameras
        .as_ref()?
        .borrow()
        .iter()
        .find(|c| c.bam_id == bam_id)
        .map(|c| c.stream.clone())
}

/// A viewer's place, if there is room.
fn a_place(state: &AppState) -> Option<tokio::sync::OwnedSemaphorePermit> {
    state.viewers.clone().try_acquire_owned().ok()
}

fn no_room() -> Response {
    refuse(
        StatusCode::SERVICE_UNAVAILABLE,
        "too many people are watching; try again in a while",
    )
}

/// `GET /api/printers/{bam_id}/camera/mjpeg`: a JPEG camera's pictures.
pub async fn watch_mjpeg(State(state): State<AppState>, Path(bam_id): Path<String>) -> Response {
    let (Some(go2rtc), Some(stream)) = (state.go2rtc.clone(), camera_stream(&state, &bam_id))
    else {
        return refuse(
            StatusCode::NOT_FOUND,
            "that printer has no camera on the tower",
        );
    };
    let Some(place) = a_place(&state) else {
        return no_room();
    };
    match go2rtc.mjpeg(&stream).await {
        Ok(Mjpeg::Playing { content_type, body }) => {
            let pictures =
                futures_util::stream::unfold((body, place), |(mut body, place)| async move {
                    Mjpeg::chunk(&mut body)
                        .await
                        .map(|c| (Ok::<_, std::convert::Infallible>(c), (body, place)))
                });
            (
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CACHE_CONTROL, "no-store".to_owned()),
                ],
                Body::from_stream(pictures),
            )
                .into_response()
        }
        Ok(Mjpeg::NoPicture | Mjpeg::NoStream) => refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "the camera is not giving pictures as MJPEG; try the other way of watching, or check the camera",
        ),
        Err(e) => {
            tracing::warn!("cameras: watching {stream}: {e}");
            refuse(
                StatusCode::BAD_GATEWAY,
                "the camera cannot be reached through POP just now",
            )
        }
    }
}

/// Whether `codecs` is a list of codec names, as a browser says them:
/// `avc1.640029,mp4a.40.2`.
fn is_codec_list(codecs: &str) -> bool {
    !codecs.is_empty()
        && codecs.len() <= 512
        && codecs
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == ',')
}

/// `GET /api/printers/{bam_id}/camera/mse`: an H.264 camera, as MSE over a
/// WebSocket. The viewer sends `{"type":"mse","value":"<codecs>"}`; POP
/// answers `{"type":"mse","value":"<MIME type>"}` and the video, or
/// `{"type":"error","value":"unsupported"}` (watch it as MJPEG instead),
/// `"no_picture"` or `"bad_request"`.
pub async fn watch_mse(
    State(state): State<AppState>,
    Path(bam_id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    let (Some(go2rtc), Some(stream)) = (state.go2rtc.clone(), camera_stream(&state, &bam_id))
    else {
        return refuse(
            StatusCode::NOT_FOUND,
            "that printer has no camera on the tower",
        );
    };
    let Some(place) = a_place(&state) else {
        return no_room();
    };
    ws.on_upgrade(move |socket| relay(go2rtc, stream, socket, place))
}

async fn relay(
    go2rtc: Go2rtc,
    stream: String,
    mut viewer: WebSocket,
    _place: tokio::sync::OwnedSemaphorePermit,
) {
    let say = |kind: &str, value: &str| {
        Message::Text(
            serde_json::json!({"type": kind, "value": value})
                .to_string()
                .into(),
        )
    };
    let asked = tokio::time::timeout(Duration::from_secs(10), viewer.recv()).await;
    let codecs = match asked {
        Ok(Some(Ok(Message::Text(t)))) => serde_json::from_str::<serde_json::Value>(&t)
            .ok()
            .filter(|v| v["type"] == "mse")
            .and_then(|v| v["value"].as_str().map(str::to_owned))
            .filter(|c| is_codec_list(c)),
        _ => None,
    };
    let Some(codecs) = codecs else {
        let _ = viewer.send(say("error", "bad_request")).await;
        return;
    };
    let media = match go2rtc.mse(&stream, &codecs).await {
        Ok(Mse::Playing { mime, media }) => {
            if viewer.send(say("mse", &mime)).await.is_err() {
                return;
            }
            media
        }
        Ok(Mse::Unsupported) => {
            let _ = viewer.send(say("error", "unsupported")).await;
            return;
        }
        Ok(Mse::NoPicture) => {
            let _ = viewer.send(say("error", "no_picture")).await;
            return;
        }
        Err(e) => {
            tracing::warn!("cameras: watching {stream}: {e}");
            let _ = viewer.send(say("error", "no_picture")).await;
            return;
        }
    };
    let mut media = media;
    loop {
        tokio::select! {
            fragment = media.next() => match fragment {
                Some(f) => if viewer.send(Message::Binary(f.into())).await.is_err() {
                    return;
                },
                None => {
                    let _ = viewer.send(say("error", "no_picture")).await;
                    return;
                }
            },
            // Nothing the viewer sends now goes anywhere; a close ends it.
            m = viewer.recv() => match m {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
        }
    }
}
