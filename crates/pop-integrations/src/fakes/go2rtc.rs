//! A fake go2rtc (#66) built from a real one's recorded answers
//! (tests/recordings/go2rtc-1.9.15, made by tools/go2rtc/record.py), over
//! real HTTP on a real port.
//!
//! It keeps streams as go2rtc does: added or replaced by name, removed by
//! name, kept until removed. It is set up as docs/integrations/go2rtc.md says
//! to set up the real one, with `local_auth` on, so every request needs the
//! user name and password. A camera works only when a test says so
//! ([`FakeGo2rtc::working`]); a picture from any other is go2rtc's empty
//! answer. tests/go2rtc.rs plays the whole recording against the fake and
//! checks each answer is go2rtc's own.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    extract::{Query, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
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
}

struct Shared {
    said: Said,
    streams: Mutex<BTreeMap<String, Vec<String>>>,
    working: Mutex<BTreeSet<String>>,
    requests: Mutex<Vec<String>>,
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
        let said = Said {
            info: named("info")["body"].clone(),
            unauthorized: body_text("no-auth"),
            insecure: body_text("add-insecure"),
            no_stream: body_text("stream-missing"),
            no_picture: body_text("frame-missing"),
        };
        let shared = Arc::new(Shared {
            said,
            streams: Mutex::new(BTreeMap::new()),
            working: Mutex::new(BTreeSet::new()),
            requests: Mutex::new(Vec::new()),
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let addr = listener.local_addr().expect("its address");
        let auth = (user.to_owned(), password.to_owned());
        let app = Router::new()
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
