//! A fake BAM! (#20) that serves answers recorded from a real one
//! (tests/recordings, made by tools/bam/record.py), over real HTTP on a real
//! port.
//!
//! A request is matched by method, path and query (in any order); one it
//! does not know is a 404 naming it, so a client asking for something the
//! recording lacks fails plainly. The token is checked as BAM! checks it,
//! with BAM!'s own recorded refusals. Faults: a body that is not JSON.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{StatusCode, header},
    response::Response,
};
use serde_json::Value;
use tokio::{net::TcpListener, task::JoinHandle};

/// The token the fake accepts.
pub const TOKEN: &str = "bam_fake_token_for_tests";

/// One recorded answer.
#[derive(Clone)]
struct Answer {
    status: u16,
    body: Value,
}

struct Shared {
    answers: BTreeMap<String, Answer>,
    unauthorized: Answer,
    anonymous: Answer,
    garbage: Mutex<bool>,
    requests: Mutex<Vec<String>>,
}

/// A fake BAM!, listening until dropped.
pub struct FakeBam {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
    /// The recording's ids, by the name the recorder gave them.
    pub ids: BTreeMap<String, String>,
}

impl Drop for FakeBam {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// "GET /api/v1/assets?b=2&a=1" as "GET /api/v1/assets?a=1&b=2".
fn key(method: &str, path_and_query: &str) -> String {
    let (path, query) = path_and_query
        .split_once('?')
        .unwrap_or((path_and_query, ""));
    let mut q: Vec<&str> = query.split('&').filter(|p| !p.is_empty()).collect();
    q.sort_unstable();
    if q.is_empty() {
        format!("{method} {path}")
    } else {
        format!("{method} {path}?{}", q.join("&"))
    }
}

impl FakeBam {
    /// Serve a recording: the JSON tools/bam/record.py wrote.
    pub async fn start(recording: &str) -> Self {
        let rec: Value = serde_json::from_str(recording).expect("a recording");
        let mut answers = BTreeMap::new();
        let mut named = BTreeMap::new();
        for (name, r) in rec["recorded"].as_object().expect("recorded answers") {
            let a = Answer {
                status: r["status"].as_u64().expect("a status") as u16,
                body: r["body"].clone(),
            };
            named.insert(name.clone(), a.clone());
            answers.insert(
                key(r["method"].as_str().unwrap(), r["path"].as_str().unwrap()),
                a,
            );
        }
        let ids = rec["ids"]
            .as_object()
            .expect("ids")
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
            .collect();
        let shared = Arc::new(Shared {
            answers,
            unauthorized: named
                .get("bad_token")
                .cloned()
                .expect("a recorded bad token"),
            anonymous: named
                .get("no_token")
                .cloned()
                .expect("a recorded missing token"),
            garbage: Mutex::new(false),
            requests: Mutex::new(Vec::new()),
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let addr = listener.local_addr().expect("its address");
        let app = Router::new().fallback(answer).with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            addr,
            shared,
            task,
            ids,
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Answer every request from now on with a body that is not JSON.
    pub fn send_garbage(&self, on: bool) {
        *self.shared.garbage.lock().unwrap() = on;
    }

    /// Every request it has answered, as "METHOD path?query".
    pub fn requests(&self) -> Vec<String> {
        self.shared.requests.lock().unwrap().clone()
    }

    /// An id from the recording, by the recorder's name for it.
    pub fn id(&self, name: &str) -> &str {
        &self.ids[name]
    }
}

fn respond(a: &Answer) -> Response {
    Response::builder()
        .status(a.status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(a.body.to_string()))
        .expect("a response")
}

async fn answer(State(s): State<Arc<Shared>>, req: Request) -> Response {
    let pq = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_default();
    let k = key(req.method().as_str(), &pq);
    s.requests.lock().unwrap().push(k.clone());
    let auth = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    match auth {
        None => return respond(&s.anonymous),
        Some(a) if a != format!("Bearer {TOKEN}") => return respond(&s.unauthorized),
        Some(_) => {}
    }
    if *s.garbage.lock().unwrap() {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("<html>a proxy's error page</html>"))
            .expect("a response");
    }
    match s.answers.get(&k) {
        Some(a) => respond(a),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({ "error": format!("the recording has no answer for {k}") })
                    .to_string(),
            ))
            .expect("a response"),
    }
}
