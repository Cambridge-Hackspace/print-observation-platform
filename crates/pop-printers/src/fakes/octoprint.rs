//! A fake OctoPrint (#29), speaking its REST API as its documentation
//! (docs.octoprint.org, "Printer operations", "Job operations") gives it:
//! `/api/printer`, 409 while the printer is not connected to it, and
//! `/api/job`, behind an `X-Api-Key`.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

struct Shared {
    /// `/api/printer`'s answer; `None` while the printer is not connected.
    printer: Mutex<Option<Value>>,
    job: Mutex<Value>,
    key: Mutex<Option<String>>,
    /// Every path asked, query included, and whether it was let in.
    asked: Mutex<Vec<(String, bool)>>,
}

/// A fake OctoPrint, listening until dropped.
pub struct FakeOctoPrint {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Drop for FakeOctoPrint {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// OctoPrint's printer state, with the flags named set and the rest not.
pub fn printer(text: &str, flags: &[&str]) -> Value {
    let mut f = serde_json::Map::new();
    for k in [
        "operational",
        "paused",
        "printing",
        "pausing",
        "cancelling",
        "sdReady",
        "error",
        "ready",
        "closedOrError",
    ] {
        f.insert(k.into(), json!(flags.contains(&k)));
    }
    json!({"state": {"text": text, "flags": f}, "temperature": {"tool0": {"actual": 24.0, "target": 0.0}}})
}

impl FakeOctoPrint {
    /// Start one on a free port: connected, operational, no job, no key asked.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let addr = listener.local_addr().expect("its address");
        let shared = Arc::new(Shared {
            printer: Mutex::new(Some(printer("Operational", &["operational", "ready"]))),
            job: Mutex::new(json!({
                "job": {"file": {"name": null, "display": null, "origin": null}, "estimatedPrintTime": null},
                "progress": {"completion": null, "filepos": null, "printTime": null, "printTimeLeft": null},
                "state": "Operational",
            })),
            key: Mutex::new(None),
            asked: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/api/printer", get(printer_state))
            .route("/api/job", get(job))
            .with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { addr, shared, task }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Ask for this API key.
    pub fn require_api_key(&self, key: &str) {
        *self.shared.key.lock().unwrap() = Some(key.to_owned());
    }

    /// `/api/printer`'s answer; `None` answers 409, the printer disconnected.
    pub fn set_printer(&self, p: Option<Value>) {
        *self.shared.printer.lock().unwrap() = p;
    }

    pub fn set_job(&self, j: Value) {
        *self.shared.job.lock().unwrap() = j;
    }

    pub fn asked(&self) -> Vec<(String, bool)> {
        self.shared.asked.lock().unwrap().clone()
    }
}

fn gate(s: &Shared, req: &Request) -> Option<Response> {
    let want = s.key.lock().unwrap().clone();
    let sent = req.headers().get("X-Api-Key").and_then(|v| v.to_str().ok());
    let ok = want.as_deref().is_none_or(|w| sent == Some(w));
    let pq = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_default();
    s.asked.lock().unwrap().push((pq, ok));
    // OctoPrint answers a missing or wrong key with 403.
    (!ok).then(|| (StatusCode::FORBIDDEN, "Forbidden").into_response())
}

async fn printer_state(State(s): State<Arc<Shared>>, req: Request) -> Response {
    if let Some(no) = gate(&s, &req) {
        return no;
    }
    match s.printer.lock().unwrap().clone() {
        Some(p) => axum::Json(p).into_response(),
        None => (StatusCode::CONFLICT, "Printer is not operational").into_response(),
    }
}

async fn job(State(s): State<Arc<Shared>>, req: Request) -> Response {
    if let Some(no) = gate(&s, &req) {
        return no;
    }
    axum::Json(s.job.lock().unwrap().clone()).into_response()
}
