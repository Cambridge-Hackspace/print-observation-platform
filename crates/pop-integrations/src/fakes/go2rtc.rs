//! A fake go2rtc (#66) built from a real one's recorded answers
//! (tests/recordings/go2rtc-1.9.15, made by tools/go2rtc/record.py), over
//! real HTTP on a real port.
//!
//! It keeps streams as go2rtc does: added or replaced by name, removed by
//! name, kept until removed. It is set up as docs/integrations/go2rtc.md says
//! to set up the real one, with `local_auth` on, so every request needs the
//! user name and password. A camera works only when a test says so:
//! a JPEG camera ([`FakeGo2rtc::working`]) gives pictures and MJPEG, an
//! H.264 camera ([`FakeGo2rtc::h264`]) plays as MSE; any other gets go2rtc's
//! empty answer, or its error. Every message a client sends on an MSE
//! WebSocket is kept ([`FakeGo2rtc::sent`]), so a test can say what reached
//! go2rtc. tests/go2rtc.rs plays the whole recording against the fake and
//! checks each answer is go2rtc's own.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::{
        Query, Request, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

/// The user name and password the fake asks for.
pub const USER: &str = "pop";
pub const PASSWORD: &str = "fake-go2rtc-password";

/// A small, well-formed JPEG: what a working camera gives.
pub const JPEG: &[u8] = &[
    0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00, 0x01,
    0x00, 0x01, 0x00, 0x00, 0xff, 0xd9,
];

struct Said {
    info: Value,
    unauthorized: String,
    insecure: String,
    no_stream: String,
    no_picture: String,
    mse_mime: String,
    mse_unsupported: String,
    mse_dead: String,
    /// The address the recording's dead camera had, which go2rtc's error names.
    dead_addr: String,
    mse_missing: String,
    mjpeg_type: String,
}

struct Shared {
    said: Said,
    streams: Mutex<BTreeMap<String, Vec<String>>>,
    working: Mutex<BTreeSet<String>>,
    h264: Mutex<BTreeSet<String>>,
    requests: Mutex<Vec<String>>,
    sent: Mutex<Vec<String>>,
    /// Media streams open now: MSE and MJPEG.
    watching: AtomicU32,
}

/// Counts one open media stream while it lives.
struct Watching(Arc<Shared>);

impl Watching {
    fn new(s: &Arc<Shared>) -> Self {
        s.watching.fetch_add(1, Ordering::SeqCst);
        Self(s.clone())
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.0.watching.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A fake go2rtc, listening until dropped.
pub struct FakeGo2rtc {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Drop for FakeGo2rtc {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn text(status: StatusCode, body: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body.to_owned(),
    )
        .into_response()
}

fn json_body(v: &Value) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        v.to_string(),
    )
        .into_response()
}

fn stream_json(sources: &[String]) -> Value {
    json!({
        "producers": sources.iter().map(|s| json!({"url": s})).collect::<Vec<_>>(),
        "consumers": null,
    })
}

impl FakeGo2rtc {
    /// Serve as the recording says go2rtc does: the JSON
    /// tools/go2rtc/record.py wrote.
    pub async fn start(recording: &str) -> Self {
        Self::start_with(recording, USER, PASSWORD).await
    }

    /// The same, with this user name and password.
    pub async fn start_with(recording: &str, user: &str, password: &str) -> Self {
        let rec: Value = serde_json::from_str(recording).expect("a recording");
        let named = |n: &str| -> &Value {
            rec["exchanges"]
                .as_array()
                .expect("exchanges")
                .iter()
                .find(|e| e["name"] == n)
                .unwrap_or_else(|| panic!("the recording has no {n}"))
        };
        let body_text = |n: &str| named(n)["body"].as_str().expect("a text body").to_owned();
        let first = |n: &str| {
            named(n)["body"]["frames"][0]["text"]["value"]
                .as_str()
                .unwrap_or_else(|| panic!("{n}: a first text frame"))
                .to_owned()
        };
        let said = Said {
            info: named("info")["body"].clone(),
            unauthorized: body_text("no-auth"),
            insecure: body_text("add-insecure"),
            no_stream: body_text("stream-missing"),
            no_picture: body_text("frame-missing"),
            mse_mime: first("mse-h264"),
            mse_unsupported: first("mse-mjpeg-camera"),
            mse_dead: first("mse-dead"),
            dead_addr: host_port(rec["dead"].as_str().expect("the dead camera")),
            mse_missing: first("mse-missing"),
            mjpeg_type: named("mjpeg-mjpeg-camera")["content_type"]
                .as_str()
                .expect("an MJPEG type")
                .to_owned(),
        };
        let shared = Arc::new(Shared {
            said,
            streams: Mutex::new(BTreeMap::new()),
            working: Mutex::new(BTreeSet::new()),
            h264: Mutex::new(BTreeSet::new()),
            requests: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            watching: AtomicU32::new(0),
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let addr = listener.local_addr().expect("its address");
        let auth = (user.to_owned(), password.to_owned());
        let ws_auth = auth.clone();
        let app = Router::new()
            .route(
                "/api/ws",
                get(move |s, h, q, ws| mse(s, h, q, ws, ws_auth.clone())),
            )
            .fallback(move |s, m, h, q, r| answer(s, m, h, q, r, auth.clone()))
            .with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { addr, shared, task }
    }

    /// The API's address, as POP is configured with it.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The streams the fake has, with their sources.
    pub fn streams(&self) -> BTreeMap<String, Vec<String>> {
        self.shared.streams.lock().unwrap().clone()
    }

    /// Give the fake a stream as if someone other than POP had added it.
    pub fn add(&self, name: &str, source: &str) {
        self.shared
            .streams
            .lock()
            .unwrap()
            .insert(name.to_owned(), vec![source.to_owned()]);
    }

    /// Lose every stream, as a go2rtc started from a fresh configuration does.
    pub fn forget(&self) {
        self.shared.streams.lock().unwrap().clear();
    }

    /// Whether the camera at `source` gives pictures.
    pub fn working(&self, source: &str, works: bool) {
        let mut w = self.shared.working.lock().unwrap();
        if works {
            w.insert(source.to_owned());
        } else {
            w.remove(source);
        }
    }

    /// Stop answering, as a go2rtc that is down.
    pub fn stop(&self) {
        self.task.abort();
    }

    /// Whether the camera at `source` sends H.264, and so plays as MSE.
    pub fn h264(&self, source: &str, works: bool) {
        let mut w = self.shared.h264.lock().unwrap();
        if works {
            w.insert(source.to_owned());
        } else {
            w.remove(source);
        }
    }

    /// Every message clients sent on MSE WebSockets, in order.
    pub fn sent(&self) -> Vec<String> {
        self.shared.sent.lock().unwrap().clone()
    }

    /// How many media streams (MSE or MJPEG) are open now.
    pub fn watching(&self) -> u32 {
        self.shared.watching.load(Ordering::SeqCst)
    }

    /// Every request so far, as "METHOD /path".
    pub fn requests(&self) -> Vec<String> {
        self.shared.requests.lock().unwrap().clone()
    }
}

fn authorized(headers: &HeaderMap, user: &str, password: &str) -> bool {
    use base64::Engine;
    let Some(v) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(b64) = v.strip_prefix("Basic ") else {
        return false;
    };
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b64) else {
        return false;
    };
    raw == format!("{user}:{password}").as_bytes()
}

async fn answer(
    State(s): State<Arc<Shared>>,
    method: Method,
    headers: HeaderMap,
    Query(q): Query<BTreeMap<String, String>>,
    req: Request,
    (user, password): (String, String),
) -> Response {
    let path = req.uri().path().to_owned();
    s.requests.lock().unwrap().push(format!("{method} {path}"));
    if !authorized(&headers, &user, &password) {
        let mut r = text(StatusCode::UNAUTHORIZED, &s.said.unauthorized);
        r.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            header::HeaderValue::from_static(r#"Basic realm="go2rtc""#),
        );
        return r;
    }
    let src = q.get("src").cloned();
    match (method, path.as_str()) {
        (Method::GET, "/api") => json_body(&s.said.info),
        (Method::GET, "/api/streams") => {
            let streams = s.streams.lock().unwrap();
            match src {
                None => json_body(&Value::Object(
                    streams
                        .iter()
                        .map(|(n, srcs)| (n.clone(), stream_json(srcs)))
                        .collect(),
                )),
                Some(name) => match streams.get(&name) {
                    Some(srcs) => json_body(&stream_json(srcs)),
                    None => text(StatusCode::NOT_FOUND, &s.said.no_stream),
                },
            }
        }
        (Method::PUT, "/api/streams") => {
            let (Some(name), Some(src)) = (q.get("name"), src) else {
                return text(StatusCode::BAD_REQUEST, "the fake needs name and src\n");
            };
            // What the recording shows go2rtc refusing.
            if src.starts_with("exec:") {
                return text(StatusCode::BAD_REQUEST, &s.said.insecure);
            }
            s.streams.lock().unwrap().insert(name.clone(), vec![src]);
            StatusCode::OK.into_response()
        }
        (Method::DELETE, "/api/streams") => {
            if let Some(name) = src {
                s.streams.lock().unwrap().remove(&name);
            }
            StatusCode::OK.into_response()
        }
        (Method::GET, "/api/stream.mjpeg") => {
            let source = src.and_then(|n| s.streams.lock().unwrap().get(&n).cloned());
            match source {
                None => text(StatusCode::NOT_FOUND, &s.said.no_picture),
                Some(srcs) if srcs.iter().any(|x| s.working.lock().unwrap().contains(x)) => {
                    let watching = Watching::new(&s);
                    let parts = futures_util::stream::unfold(watching, |w| async move {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        let mut part = format!(
                            "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                            JPEG.len()
                        )
                        .into_bytes();
                        part.extend_from_slice(JPEG);
                        part.extend_from_slice(b"\r\n");
                        Some((Ok::<_, std::convert::Infallible>(part), w))
                    });
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, s.said.mjpeg_type.clone())],
                        Body::from_stream(parts),
                    )
                        .into_response()
                }
                Some(_) => StatusCode::OK.into_response(),
            }
        }
        (Method::GET, "/api/frame.jpeg") => {
            let source = src.and_then(|n| s.streams.lock().unwrap().get(&n).cloned());
            match source {
                None => text(StatusCode::NOT_FOUND, &s.said.no_picture),
                Some(srcs) if srcs.iter().any(|x| s.working.lock().unwrap().contains(x)) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "image/jpeg")],
                    JPEG.to_vec(),
                )
                    .into_response(),
                Some(_) => StatusCode::OK.into_response(),
            }
        }
        (m, p) => text(
            StatusCode::NOT_FOUND,
            &format!("the fake go2rtc does not answer {m} {p}\n"),
        ),
    }
}

/// `GET /api/ws`: MSE, as the recording shows go2rtc serving it.
async fn mse(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    Query(q): Query<BTreeMap<String, String>>,
    ws: WebSocketUpgrade,
    (user, password): (String, String),
) -> Response {
    s.requests.lock().unwrap().push("GET /api/ws".to_owned());
    if !authorized(&headers, &user, &password) {
        return text(StatusCode::UNAUTHORIZED, &s.said.unauthorized);
    }
    let name = q.get("src").cloned().unwrap_or_default();
    ws.on_upgrade(move |socket| play(s, name, socket))
}

/// A box of `kind` with `n` bytes in all, as MP4 frames it.
fn mp4_box(kind: &[u8; 4], n: usize) -> Vec<u8> {
    let mut b = u32::try_from(n).unwrap_or(u32::MAX).to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    b.resize(n, 0);
    b
}

async fn play(s: Arc<Shared>, name: String, mut socket: WebSocket) {
    // go2rtc waits for the client to say what it can play.
    let Some(Ok(Message::Text(asked))) = socket.recv().await else {
        return;
    };
    s.sent.lock().unwrap().push(asked.to_string());
    let error = |v: &str| Message::Text(json!({"type": "error", "value": v}).to_string().into());
    let source = s.streams.lock().unwrap().get(&name).cloned();
    let Some(srcs) = source else {
        let _ = socket.send(error(&s.said.mse_missing)).await;
        return;
    };
    let h264 = srcs.iter().any(|x| s.h264.lock().unwrap().contains(x));
    let jpeg = srcs.iter().any(|x| s.working.lock().unwrap().contains(x));
    if !h264 {
        // go2rtc's error names the camera it could not reach, as recorded.
        let why = if jpeg {
            s.said.mse_unsupported.clone()
        } else {
            s.said.mse_dead.replace(
                &s.said.dead_addr,
                &host_port(srcs.first().map_or("", String::as_str)),
            )
        };
        let _ = socket.send(error(&why)).await;
        return;
    }
    let _watching = Watching::new(&s);
    let mime = json!({"type": "mse", "value": s.said.mse_mime}).to_string();
    if socket.send(Message::Text(mime.into())).await.is_err()
        || socket
            .send(Message::Binary(mp4_box(b"ftyp", 672).into()))
            .await
            .is_err()
    {
        return;
    }
    loop {
        tokio::select! {
            m = socket.recv() => match m {
                Some(Ok(Message::Text(t))) => s.sent.lock().unwrap().push(t.to_string()),
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
            () = tokio::time::sleep(Duration::from_millis(50)) => {
                if socket.send(Message::Binary(mp4_box(b"moof", 625).into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// A source's host and port, as go2rtc's dial errors name them.
fn host_port(source: &str) -> String {
    let rest = source.split_once("://").map_or(source, |(_, r)| r);
    let rest = rest.split('/').next().unwrap_or_default();
    rest.rsplit_once('@').map_or(rest, |(_, h)| h).to_owned()
}
