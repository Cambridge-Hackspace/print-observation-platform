//! The printers' cameras, through go2rtc (#66), and Bambu printers' own (#33).
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
//! A Bambu printer's own camera (#33) is one of two kinds:
//!
//! - RTSPS (X1, P2S, H2): go2rtc plays it, but through POP's relay
//!   (`pop_printers::bambu_camera::Relay`), which checks the printer's pinned
//!   certificate; go2rtc checks none for a camera addressed by IP. go2rtc's
//!   source is the relay's loopback port, signed in with the access code.
//! - JPEG pictures on port 6000 (A1, P1): POP reads them itself, one
//!   connection per camera however many watch ([`Hub`]), and serves them as
//!   MJPEG. go2rtc is not involved.
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

use std::{
    collections::BTreeMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

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
use pop_integrations::printers::{Camera as PrinterCamera, Connection, PrinterConfig};
use pop_printers::{
    bambu::PinStore,
    bambu_camera::{BambuCamera, CameraError, PORT as JPEG_PORT, RTSPS_PATH, RTSPS_PORT, Relay},
};
use tokio::{sync::watch, task::JoinHandle};

use crate::{
    AppState,
    api::{AdminCamera, ApiError, CameraHealth},
    health::{CheckFuture, HealthCheck},
};

/// The start of every stream name POP owns.
pub const PREFIX: &str = "pop_";

/// A Bambu printer, for its own camera.
#[derive(Clone, PartialEq, Eq)]
pub struct Bambu {
    pub host: String,
    /// The camera's port: 322 for RTSPS, 6000 for JPEG pictures.
    pub port: u16,
    pub serial: String,
    pub access_code: String,
}

impl fmt::Debug for Bambu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bambu")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("serial", &self.serial)
            .field("access_code", &"<sealed>")
            .finish()
    }
}

/// Where a printer's camera comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// An address go2rtc plays: rtsp, rtsps, http or https.
    Address(String),
    /// A Bambu printer's RTSPS camera, played by go2rtc through POP's relay.
    BambuRtsps(Bambu),
    /// A Bambu printer's JPEG pictures on port 6000, read by POP.
    BambuJpeg(Bambu),
}

/// A printer's camera, as POP shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Camera {
    /// The printer's name.
    pub name: String,
    pub source: Source,
}

/// The cameras to show, by the printer's BAM! id.
pub type Wanted = BTreeMap<String, Camera>;

/// Where a running printer's camera comes from: none when it has none, or
/// when a Bambu printer's own camera has no access code to sign in with.
pub fn source_of(config: &PrinterConfig, access_code: Option<&str>) -> Option<Source> {
    let bambu = |port: u16| match (&config.connection, access_code) {
        (Connection::Bambu { host, serial }, Some(code)) => Some(Bambu {
            host: host.clone(),
            port,
            serial: serial.clone(),
            access_code: code.to_owned(),
        }),
        _ => None,
    };
    match config.camera.as_ref()? {
        PrinterCamera::Address { url } => Some(Source::Address(url.clone())),
        PrinterCamera::BambuRtsps => bambu(RTSPS_PORT).map(Source::BambuRtsps),
        PrinterCamera::BambuJpeg => bambu(JPEG_PORT).map(Source::BambuJpeg),
    }
}

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

/// `s` as it may stand in a URL's user part.
fn userinfo(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
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

/// The latest picture from a camera POP reads itself.
pub type Latest = watch::Receiver<Option<Arc<Vec<u8>>>>;

/// A Bambu JPEG camera, read once however many watch: connected while
/// anyone does, and not otherwise.
pub struct Hub {
    camera: BambuCamera,
    latest: watch::Sender<Option<Arc<Vec<u8>>>>,
    reading: AtomicBool,
    last_error: Mutex<Option<CameraError>>,
}

/// The cameras POP reads itself, by the printer's BAM! id.
pub type Jpegs = Arc<Mutex<BTreeMap<String, Arc<Hub>>>>;

/// How long a look waits for a picture from a camera POP reads itself.
const PICTURE_WITHIN: Duration = Duration::from_secs(10);

impl Hub {
    fn new(camera: BambuCamera) -> Arc<Self> {
        Arc::new(Self {
            camera,
            latest: watch::channel(None).0,
            reading: AtomicBool::new(false),
            last_error: Mutex::new(None),
        })
    }

    /// Its pictures, as they come; the camera is read while any are held.
    pub fn watch(self: &Arc<Self>) -> Latest {
        let rx = self.latest.subscribe();
        if !self.reading.swap(true, Ordering::SeqCst) {
            tokio::spawn(self.clone().read());
        }
        rx
    }

    async fn read(self: Arc<Self>) {
        loop {
            while self.latest.receiver_count() > 0 {
                match self.camera.connect().await {
                    Ok(mut pictures) => loop {
                        match pictures.next().await {
                            Ok(p) => {
                                *self.last_error.lock().unwrap() = None;
                                self.latest.send_replace(Some(Arc::new(p)));
                                if self.latest.receiver_count() == 0 {
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("camera {}: {e}", self.camera.serial);
                                *self.last_error.lock().unwrap() = Some(e);
                                break;
                            }
                        }
                    },
                    Err(e) => {
                        tracing::warn!("camera {}: {e}", self.camera.serial);
                        *self.last_error.lock().unwrap() = Some(e);
                    }
                }
                if self.latest.receiver_count() > 0 {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
            self.reading.store(false, Ordering::SeqCst);
            // One who came to watch as the reading stopped is not left waiting.
            if self.latest.receiver_count() == 0 || self.reading.swap(true, Ordering::SeqCst) {
                return;
            }
        }
    }

    /// Whether a picture comes, and if not, why.
    async fn look(self: &Arc<Self>) -> (CameraHealth, Option<String>) {
        let mut rx = self.watch();
        rx.mark_unchanged();
        if let Ok(Ok(())) = tokio::time::timeout(PICTURE_WITHIN, rx.changed()).await {
            return (CameraHealth::Live, None);
        }
        let why = self.last_error.lock().unwrap().clone();
        (CameraHealth::NoPicture, why.map(|e| said(&e)))
    }
}

/// What went wrong with a Bambu camera, in a sentence for admins.
fn said(e: &CameraError) -> String {
    match e {
        CameraError::Unreachable(_) => "the camera cannot be reached".to_owned(),
        other => other.to_string(),
    }
}

/// What the keeper has made: each camera's health, as admins see it, and the
/// cameras POP reads itself, for watching.
pub struct Kept {
    pub health: watch::Receiver<Vec<AdminCamera>>,
    pub jpegs: Jpegs,
}

/// Keep go2rtc serving `wanted`, and the cameras POP reads itself ready.
pub fn spawn(
    client: Go2rtc,
    wanted: watch::Receiver<Wanted>,
    opts: Options,
    pins: Arc<dyn PinStore>,
) -> (Kept, JoinHandle<()>) {
    let (tx, rx) = watch::channel(Vec::new());
    let jpegs: Jpegs = Arc::default();
    let keeper = Keeper {
        client,
        pins,
        relays: BTreeMap::new(),
        jpegs: jpegs.clone(),
    };
    (
        Kept { health: rx, jpegs },
        tokio::spawn(keeper.run(wanted, opts, tx)),
    )
}

struct Keeper {
    client: Go2rtc,
    pins: Arc<dyn PinStore>,
    /// The RTSPS cameras' relays, with the printer each was made for.
    relays: BTreeMap<String, (Bambu, Relay)>,
    jpegs: Jpegs,
}

impl Keeper {
    fn camera(&self, b: &Bambu) -> BambuCamera {
        BambuCamera {
            host: b.host.clone(),
            port: b.port,
            serial: b.serial.clone(),
            access_code: b.access_code.clone(),
            pins: self.pins.clone(),
        }
    }

    async fn run(
        mut self,
        mut wanted: watch::Receiver<Wanted>,
        opts: Options,
        tx: watch::Sender<Vec<AdminCamera>>,
    ) {
        loop {
            let want = wanted.borrow_and_update().clone();
            let (to_go2rtc, unmade) = self.arrange(&want).await;
            let synced = sync(&self.client, &to_go2rtc).await;
            if let Err(e) = &synced {
                tracing::warn!("cameras: {e}");
            }
            let report = self.look(&want, &synced, &unmade).await;
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

    /// Relays for the RTSPS cameras and hubs for the JPEG ones, as `want`
    /// has them; and what go2rtc should play, by printer: its name and the
    /// address go2rtc dials. Cameras that could not be made ready, with why.
    async fn arrange(
        &mut self,
        want: &Wanted,
    ) -> (BTreeMap<String, (String, String)>, BTreeMap<String, String>) {
        self.relays.retain(|id, (b, _)| {
            matches!(want.get(id), Some(Camera { source: Source::BambuRtsps(w), .. }) if w == b)
        });
        {
            let mut jpegs = self.jpegs.lock().unwrap();
            jpegs.retain(|id, hub| {
                matches!(
                    want.get(id),
                    Some(Camera { source: Source::BambuJpeg(w), .. })
                        if w.host == hub.camera.host
                            && w.port == hub.camera.port
                            && w.serial == hub.camera.serial
                            && w.access_code == hub.camera.access_code
                )
            });
            for (id, cam) in want {
                if let Source::BambuJpeg(b) = &cam.source {
                    jpegs
                        .entry(id.clone())
                        .or_insert_with(|| Hub::new(self.camera(b)));
                }
            }
        }
        let mut to_go2rtc = BTreeMap::new();
        let mut unmade = BTreeMap::new();
        for (id, cam) in want {
            match &cam.source {
                Source::Address(url) => {
                    to_go2rtc.insert(id.clone(), (cam.name.clone(), url.clone()));
                }
                Source::BambuRtsps(b) => {
                    if !self.relays.contains_key(id) {
                        match self.camera(b).relay().await {
                            Ok(r) => {
                                self.relays.insert(id.clone(), (b.clone(), r));
                            }
                            Err(e) => {
                                tracing::error!("cameras: a relay for {}: {e}", cam.name);
                                unmade.insert(id.clone(), "POP could not relay this camera".into());
                                continue;
                            }
                        }
                    }
                    let port = self.relays[id].1.port;
                    to_go2rtc.insert(
                        id.clone(),
                        (
                            cam.name.clone(),
                            format!(
                                "rtsp://bblp:{}@127.0.0.1:{port}{RTSPS_PATH}",
                                userinfo(&b.access_code)
                            ),
                        ),
                    );
                }
                Source::BambuJpeg(_) => {}
            }
        }
        (to_go2rtc, unmade)
    }

    /// Each camera's health: a picture from each, all at once.
    async fn look(
        &self,
        want: &Wanted,
        synced: &Result<BTreeMap<String, String>, Go2rtcError>,
        unmade: &BTreeMap<String, String>,
    ) -> Vec<AdminCamera> {
        enum Seen {
            Go2rtc(Result<Picture, Go2rtcError>),
            Pop(CameraHealth, Option<String>),
        }
        let mut asked = tokio::task::JoinSet::new();
        let jpegs = self.jpegs.lock().unwrap().clone();
        for (id, cam) in want {
            let id = id.clone();
            if matches!(cam.source, Source::BambuJpeg(_)) {
                if let Some(hub) = jpegs.get(&id).cloned() {
                    asked.spawn(async move {
                        let (h, why) = hub.look().await;
                        (id, Seen::Pop(h, why))
                    });
                }
            } else if !unmade.contains_key(&id) {
                let client = self.client.clone();
                asked.spawn(async move {
                    let picture = client.picture(&stream_name(&id)).await;
                    (id, Seen::Go2rtc(picture))
                });
            }
        }
        let mut seen = BTreeMap::new();
        while let Some(Ok((id, s))) = asked.join_next().await {
            seen.insert(id, s);
        }
        let mut out: Vec<AdminCamera> = want
            .iter()
            .map(|(id, cam)| {
                let pop_reads = matches!(cam.source, Source::BambuJpeg(_));
                let (health, mut detail) = match (seen.remove(id), synced) {
                    (Some(Seen::Pop(h, why)), _) => (h, why),
                    (_, _) if unmade.contains_key(id) => {
                        (CameraHealth::Unknown, unmade.get(id).cloned())
                    }
                    (_, Err(e)) => (CameraHealth::Unknown, Some(e.to_string())),
                    (_, Ok(refused)) if refused.contains_key(id) => {
                        (CameraHealth::Unknown, refused.get(id).cloned())
                    }
                    (Some(Seen::Go2rtc(Ok(Picture::Jpeg(_)))), _) => (CameraHealth::Live, None),
                    (Some(Seen::Go2rtc(Ok(Picture::Nothing))), _) => {
                        (CameraHealth::NoPicture, None)
                    }
                    (Some(Seen::Go2rtc(Ok(Picture::NoStream))), _) => {
                        (CameraHealth::NotInGo2rtc, None)
                    }
                    (Some(Seen::Go2rtc(Err(e))), _) => (CameraHealth::Unknown, Some(e.to_string())),
                    (None, _) => (CameraHealth::Unknown, None),
                };
                // Why the relay could not reach the camera says more than go2rtc can.
                if health != CameraHealth::Live
                    && let Some((_, relay)) = self.relays.get(id)
                    && let Some(e) = relay.last_error()
                {
                    detail = Some(said(&e));
                }
                AdminCamera {
                    bam_id: id.clone(),
                    name: cam.name.clone(),
                    stream: (!pop_reads).then(|| stream_name(id)),
                    health,
                    detail,
                }
            })
            .collect();
        out.sort_by_key(|c| c.name.to_lowercase());
        out
    }
}

/// Make go2rtc's `pop_` streams exactly `want`: by printer, its name and the
/// address go2rtc dials. A camera go2rtc refuses is said, by its printer's
/// id; the rest still go in.
async fn sync(
    client: &Go2rtc,
    want: &BTreeMap<String, (String, String)>,
) -> Result<BTreeMap<String, String>, Go2rtcError> {
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
    for (id, (printer, source)) in want {
        let name = stream_name(id);
        if have.get(&name).is_some_and(|s| *s == [source.as_str()]) {
            continue;
        }
        match client.put(&name, source).await {
            Ok(()) => tracing::info!("cameras: serving {printer}'s camera as {name}"),
            Err(Go2rtcError::Refused(why)) => {
                tracing::warn!("cameras: go2rtc will not serve {printer}'s camera: {why}");
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
            self.0.version().await.map_err(|e| {
                tracing::error!("health: {e}");
                match e {
                    Go2rtcError::Auth => "go2rtc refuses POP's user name and password".to_owned(),
                    _ => "go2rtc does not answer".to_owned(),
                }
            })?;
            // A password go2rtc does not ask for protects nothing (#149).
            if self.0.has_password() && self.0.answers_without_password().await.unwrap_or(false) {
                tracing::warn!(
                    "go2rtc answers requests with no password: turn on local_auth in its api section (docs/integrations/go2rtc.md)"
                );
                return Err(
                    "go2rtc answers without its password: turn on local_auth in its configuration"
                        .to_owned(),
                );
            }
            Ok(())
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
        .and_then(|c| c.stream.clone())
}

/// The hub of a camera POP reads itself, if `bam_id`'s is one.
fn jpeg_hub(state: &AppState, bam_id: &str) -> Option<Arc<Hub>> {
    state.jpegs.lock().unwrap().get(bam_id).cloned()
}

/// A camera POP reads itself, as MJPEG.
fn mjpeg_from(hub: &Arc<Hub>, place: tokio::sync::OwnedSemaphorePermit) -> Response {
    let rx = hub.watch();
    let parts =
        futures_util::stream::unfold((rx, place, true), |(mut rx, place, first)| async move {
            if !first || rx.borrow().is_none() {
                rx.changed().await.ok()?;
            }
            let picture = rx.borrow_and_update().clone()?;
            let mut part = format!(
                "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                picture.len()
            )
            .into_bytes();
            part.extend_from_slice(&picture);
            part.extend_from_slice(b"\r\n");
            Some((Ok::<_, std::convert::Infallible>(part), (rx, place, false)))
        });
    (
        [
            (
                header::CONTENT_TYPE,
                "multipart/x-mixed-replace; boundary=frame".to_owned(),
            ),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        Body::from_stream(parts),
    )
        .into_response()
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
    if let Some(hub) = jpeg_hub(&state, &bam_id) {
        let Some(place) = a_place(&state) else {
            return no_room();
        };
        return mjpeg_from(&hub, place);
    }
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
    if jpeg_hub(&state, &bam_id).is_some() {
        // JPEG pictures cannot be played as MSE: the viewer is told so.
        return ws.on_upgrade(|mut viewer| async move {
            let _ = tokio::time::timeout(Duration::from_secs(10), viewer.recv()).await;
            let said = serde_json::json!({"type": "error", "value": "unsupported"});
            let _ = viewer.send(Message::Text(said.to_string().into())).await;
        });
    }
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
