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

use std::{collections::BTreeMap, time::Duration};

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use pop_integrations::go2rtc::{Go2rtc, Go2rtcError, Picture};
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
