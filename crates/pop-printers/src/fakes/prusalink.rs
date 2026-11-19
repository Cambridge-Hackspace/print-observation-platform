//! A fake PrusaLink (#30), speaking its HTTP API as its OpenAPI description
//! (Prusa-Link-Web, spec/openapi.yaml) gives it: `/api/v1/status`,
//! `/api/v1/job` (204 when there is none), behind HTTP Digest as on the
//! printer itself, or an `X-Api-Key` as on a Raspberry Pi.
//!
//! Its Digest check is RFC 2617's, written out here with MD5 rather than
//! taken from the crate the driver uses: a check made by the same code as the
//! answer would agree with it whatever it got wrong.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use md5::{Digest as _, Md5};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

const REALM: &str = "Printer API";

/// What it asks of a client.
#[derive(Clone)]
enum Asks {
    Nothing,
    Digest { username: String, password: String },
    ApiKey(String),
}

struct Shared {
    status: Mutex<Value>,
    job: Mutex<Option<Value>>,
    asks: Mutex<Asks>,
    /// The nonce it challenges with now; a new one makes the old stale.
    nonce: Mutex<String>,
    /// Every path asked, in order, and whether it was let in.
    asked: Mutex<Vec<(String, bool)>>,
}

/// A fake PrusaLink, listening until dropped.
pub struct FakePrusaLink {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Drop for FakePrusaLink {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakePrusaLink {
    /// Start one on a free port: an idle printer, asking nothing.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let addr = listener.local_addr().expect("its address");
        let shared = Arc::new(Shared {
            status: Mutex::new(
                json!({"printer": {"state": "IDLE", "temp_nozzle": 24.0, "temp_bed": 23.5}}),
            ),
            job: Mutex::new(None),
            asks: Mutex::new(Asks::Nothing),
            nonce: Mutex::new("a1b2c3d4".to_owned()),
            asked: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/api/v1/status", get(status))
            .route("/api/v1/job", get(job))
            .with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { addr, shared, task }
    }

    /// The address a driver is given.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Ask for HTTP Digest with this user and password, as on the printer.
    pub fn require_digest(&self, username: &str, password: &str) {
        *self.shared.asks.lock().unwrap() = Asks::Digest {
            username: username.to_owned(),
            password: password.to_owned(),
        };
    }

    /// Ask for this API key, as PrusaLink on a Raspberry Pi does.
    pub fn require_api_key(&self, key: &str) {
        *self.shared.asks.lock().unwrap() = Asks::ApiKey(key.to_owned());
    }

    /// Challenge with a new nonce from now on, as a printer does when its
    /// nonce runs out: answers to the old one are stale.
    pub fn new_nonce(&self, nonce: &str) {
        *self.shared.nonce.lock().unwrap() = nonce.to_owned();
    }

    /// The printer's state, as `/api/v1/status` gives it.
    pub fn set_status(&self, status: Value) {
        *self.shared.status.lock().unwrap() = status;
    }

    /// The job, as `/api/v1/job` gives it; `None` answers 204.
    pub fn set_job(&self, job: Option<Value>) {
        *self.shared.job.lock().unwrap() = job;
    }

    /// Every path asked, in order, and whether each was let in.
    pub fn asked(&self) -> Vec<(String, bool)> {
        self.shared.asked.lock().unwrap().clone()
    }
}

async fn status(State(s): State<Arc<Shared>>, req: Request) -> Response {
    if let Some(no) = gate(&s, req.uri().path(), req.headers()) {
        return no;
    }
    axum::Json(s.status.lock().unwrap().clone()).into_response()
}

async fn job(State(s): State<Arc<Shared>>, req: Request) -> Response {
    if let Some(no) = gate(&s, req.uri().path(), req.headers()) {
        return no;
    }
    match s.job.lock().unwrap().clone() {
        Some(j) => axum::Json(j).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

/// `None` to let the request in; otherwise PrusaLink's refusal.
fn gate(s: &Shared, path: &str, h: &HeaderMap) -> Option<Response> {
    let asks = s.asks.lock().unwrap().clone();
    let nonce = s.nonce.lock().unwrap().clone();
    let (ok, stale) = match &asks {
        Asks::Nothing => (true, false),
        Asks::ApiKey(key) => (
            h.get("X-Api-Key").and_then(|v| v.to_str().ok()) == Some(key.as_str()),
            false,
        ),
        Asks::Digest { username, password } => {
            match h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
                None => (false, false),
                Some(a) => {
                    let p = params(a);
                    let right = digest_response(username, password, &p, path);
                    let sent_nonce = p.get("nonce").map(String::as_str);
                    let good = right.is_some()
                        && p.get("response") == right.as_ref()
                        && p.get("uri").map(String::as_str) == Some(path);
                    // A correct answer to a nonce no longer given is stale.
                    (
                        good && sent_nonce == Some(nonce.as_str()),
                        good && sent_nonce != Some(nonce.as_str()),
                    )
                }
            }
        }
    };
    s.asked.lock().unwrap().push((path.to_owned(), ok));
    if ok {
        return None;
    }
    let mut res = (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    if matches!(asks, Asks::Digest { .. }) {
        let challenge = format!(
            "Digest realm=\"{REALM}\", nonce=\"{nonce}\", qop=\"auth\", algorithm=MD5{}",
            if stale { ", stale=true" } else { "" }
        );
        res.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            challenge.parse().expect("a header"),
        );
    }
    Some(res)
}

/// An Authorization: Digest header's parameters.
fn params(h: &str) -> std::collections::HashMap<String, String> {
    let rest = h.trim().strip_prefix("Digest").unwrap_or("").trim();
    let mut out = std::collections::HashMap::new();
    for part in rest.split(',') {
        if let Some((k, v)) = part.trim().split_once('=') {
            out.insert(k.trim().to_owned(), v.trim().trim_matches('"').to_owned());
        }
    }
    out
}

/// RFC 2617's `response`, for qop=auth with MD5, from what the client sent.
fn digest_response(
    username: &str,
    password: &str,
    p: &std::collections::HashMap<String, String>,
    path: &str,
) -> Option<String> {
    let md5 = |s: String| hex::encode(Md5::digest(s.as_bytes()));
    if p.get("username").map(String::as_str) != Some(username)
        || p.get("realm").map(String::as_str) != Some(REALM)
    {
        return None;
    }
    let ha1 = md5(format!("{username}:{REALM}:{password}"));
    let ha2 = md5(format!("GET:{path}"));
    let (nonce, nc, cnonce, qop) = (
        p.get("nonce")?,
        p.get("nc")?,
        p.get("cnonce")?,
        p.get("qop")?,
    );
    Some(md5(format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}")))
}
