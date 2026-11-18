//! A fake BAM! (#20, #22) built from a real one's recorded answers
//! (tests/recordings, made by tools/bam/record.py), over real HTTP and a real
//! WebSocket on a real port.
//!
//! The printer category's assets are state, seeded from the recording: a
//! test can edit, archive, restore or add one, and the fake answers the
//! category's reads from that state and tells live subscribers, with frames
//! made from the recorded ones. Every other request is answered as recorded,
//! matched by method, path and query (in any order); one the recording lacks
//! is a 404 naming it. The token is checked as BAM! checks it, with BAM!'s
//! own refusals.
//!
//! Faults: a body that is not JSON, the live connections dropped, new ones
//! refused (BAM! unreachable, for the live side), and a change made without
//! its event (the backstop's case).

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
};

use axum::{
    Router,
    body::Body,
    extract::{
        Request, State,
        ws::{Message, WebSocket, WebSocketUpgrade, rejection::WebSocketUpgradeRejection},
    },
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, sync::broadcast, task::JoinHandle};

/// The token the fake accepts.
pub const TOKEN: &str = "bam_fake_token_for_tests";

#[derive(Clone)]
struct Answer {
    status: u16,
    body: Value,
}

#[derive(Clone)]
enum Push {
    Frame(Value),
    Drop,
}

struct Shared {
    answers: BTreeMap<String, Answer>,
    unauthorized: Answer,
    anonymous: Answer,
    category: String,
    /// The category's assets, in use and archived, by id.
    assets: Mutex<BTreeMap<String, Value>>,
    /// Recorded frames, by event kind, to make new ones from; and hello.
    templates: BTreeMap<String, Value>,
    hello: Value,
    garbage: Mutex<bool>,
    refuse_live: Mutex<bool>,
    requests: Mutex<Vec<String>>,
    category_reads: AtomicU32,
    live_opened: AtomicU32,
    seq: AtomicU64,
    pushes: broadcast::Sender<Push>,
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

fn now() -> String {
    // A fixed, well-formed time: what matters to POP is that it changes.
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("2026-11-16T12:00:00.{:09}Z", n % 1_000_000_000)
}

impl FakeBam {
    /// Serve a recording: the JSON tools/bam/record.py wrote.
    pub async fn start(recording: &str) -> Self {
        let rec: Value = serde_json::from_str(recording).expect("a recording");
        let ids: BTreeMap<String, String> = rec["ids"]
            .as_object()
            .expect("ids")
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
            .collect();
        let recorded = rec["recorded"].as_object().expect("recorded answers");
        let mut answers = BTreeMap::new();
        for (name, r) in recorded {
            // Not answers by request: the WebSocket's frames are templates
            // (the plain request is BAM!'s 426, recorded on its own), and the
            // two refusals are served by the token check, whatever is asked.
            if ["live", "bad_token", "no_token"].contains(&name.as_str()) {
                continue;
            }
            let k = key(r["method"].as_str().unwrap(), r["path"].as_str().unwrap());
            let a = Answer {
                status: r["status"].as_u64().expect("a status") as u16,
                body: r["body"].clone(),
            };
            assert!(
                answers.insert(k.clone(), a).is_none(),
                "the recording answers {k} twice"
            );
        }
        let named = |n: &str| {
            let r = &recorded[n];
            Answer {
                status: r["status"].as_u64().unwrap() as u16,
                body: r["body"].clone(),
            }
        };
        // The category's assets, in use and archived, as state.
        let mut assets = BTreeMap::new();
        for n in ["printers", "printers_archived"] {
            for a in recorded[n]["body"]["assets"].as_array().expect("assets") {
                assets.insert(a["asset"]["id"].as_str().unwrap().to_owned(), a.clone());
            }
        }
        // Frames to make new ones from: the live ones recorded, and the event
        // history's, wrapped as BAM! wraps them (internal/api/live.go).
        let frames = recorded["live"]["body"]["frames"]
            .as_array()
            .expect("live frames")
            .clone();
        let hello = frames[0].clone();
        let mut templates = BTreeMap::new();
        for e in recorded["events"]["body"]["events"]
            .as_array()
            .expect("events")
        {
            templates.insert(
                e["kind"].as_str().unwrap().to_owned(),
                json!({"type": "event", "event": e}),
            );
        }
        for f in &frames[1..] {
            templates.insert(f["event"]["kind"].as_str().unwrap().to_owned(), f.clone());
        }
        let shared = Arc::new(Shared {
            answers,
            unauthorized: named("bad_token"),
            anonymous: named("no_token"),
            category: ids["printers"].clone(),
            assets: Mutex::new(assets),
            templates,
            hello,
            garbage: Mutex::new(false),
            refuse_live: Mutex::new(false),
            requests: Mutex::new(Vec::new()),
            category_reads: AtomicU32::new(0),
            live_opened: AtomicU32::new(0),
            seq: AtomicU64::new(1),
            pushes: broadcast::channel(256).0,
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let addr = listener.local_addr().expect("its address");
        let app = Router::new()
            .route("/api/v1/events/live", get(live))
            .fallback(answer)
            .with_state(shared.clone());
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

    /// An id from the recording, by the recorder's name for it.
    pub fn id(&self, name: &str) -> &str {
        &self.ids[name]
    }

    /// Answer every request from now on with a body that is not JSON.
    pub fn send_garbage(&self, on: bool) {
        *self.shared.garbage.lock().unwrap() = on;
    }

    /// Every request it has answered, as "METHOD path?query".
    pub fn requests(&self) -> Vec<String> {
        self.shared.requests.lock().unwrap().clone()
    }

    /// How many times the printer category's assets have been read.
    pub fn category_reads(&self) -> u32 {
        self.shared.category_reads.load(Ordering::SeqCst)
    }

    /// How many live connections it has accepted.
    pub fn live_opened(&self) -> u32 {
        self.shared.live_opened.load(Ordering::SeqCst)
    }

    /// Drop every live connection.
    pub fn drop_live(&self) {
        let _ = self.shared.pushes.send(Push::Drop);
    }

    /// Refuse new live connections, as a BAM! that cannot be reached does.
    pub fn refuse_live(&self, refuse: bool) {
        *self.shared.refuse_live.lock().unwrap() = refuse;
    }

    fn change(&self, id: &str, kind: &str, notify: bool, f: impl FnOnce(&mut Value)) {
        let name = {
            let mut assets = self.shared.assets.lock().unwrap();
            let a = assets
                .get_mut(id)
                .unwrap_or_else(|| panic!("no asset {id} in the category"));
            f(a);
            a["asset"]["updated_at"] = json!(now());
            a["asset"]["name"].clone()
        };
        if notify {
            self.notify(kind, id, name);
        }
    }

    fn notify(&self, kind: &str, id: &str, name: Value) {
        let mut frame = self
            .shared
            .templates
            .get(kind)
            .unwrap_or_else(|| panic!("no recorded {kind} frame"))
            .clone();
        let n = self.shared.seq.fetch_add(1, Ordering::SeqCst);
        let e = &mut frame["event"];
        e["id"] = json!(format!("fake-event-{n}"));
        e["asset_id"] = json!(id);
        e["created_at"] = json!(now());
        e["payload"]["asset"] = json!({ "id": id, "name": name });
        let _ = self.shared.pushes.send(Push::Frame(frame));
    }

    /// Set one of a printer's fields, as an admin editing it in BAM! does.
    pub fn edit(&self, id: &str, field: &str, value: &str) {
        self.change(id, "asset.edited", true, |a| {
            a["field_values"][field] = json!(value);
        });
    }

    /// Set a field with no event sent: a change a live connection missed.
    pub fn edit_without_event(&self, id: &str, field: &str, value: &str) {
        self.change(id, "asset.edited", false, |a| {
            a["field_values"][field] = json!(value);
        });
    }

    pub fn archive(&self, id: &str) {
        let at = now();
        self.change(id, "asset.archived", true, |a| {
            a["asset"]["archived_at"] = json!(at)
        });
    }

    pub fn restore(&self, id: &str) {
        self.change(id, "asset.restored", true, |a| {
            if let Some(o) = a["asset"].as_object_mut() {
                o.remove("archived_at");
            }
        });
    }

    /// A new printer in the category: a copy of `like`, with its own id, name
    /// and fields. Returns its id.
    pub fn add(&self, like: &str, name: &str, fields: Value) -> String {
        let n = self.shared.seq.fetch_add(1, Ordering::SeqCst);
        let id = format!("00000000-0000-4000-8000-{n:012}");
        let mut a = self.shared.assets.lock().unwrap()[like].clone();
        a["asset"]["id"] = json!(id);
        a["asset"]["name"] = json!(name);
        a["asset"]["created_at"] = json!(now());
        if let Some(o) = a["asset"].as_object_mut() {
            o.remove("archived_at");
        }
        a["field_values"] = fields;
        self.shared.assets.lock().unwrap().insert(id.clone(), a);
        self.notify("asset.created", &id, json!(name));
        id
    }
}

fn respond(a: &Answer) -> Response {
    json_response(a.status, &a.body)
}

fn json_response(status: u16, body: &Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("a response")
}

/// BAM!'s refusal for a request's token, if it has one to give.
fn refused(s: &Shared, req_auth: Option<&str>) -> Option<Response> {
    match req_auth {
        None => Some(respond(&s.anonymous)),
        Some(a) if a != format!("Bearer {TOKEN}") => Some(respond(&s.unauthorized)),
        Some(_) => None,
    }
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
    if let Some(r) = refused(&s, auth) {
        return r;
    }
    if *s.garbage.lock().unwrap() {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("<html>a proxy's error page</html>"))
            .expect("a response");
    }
    // The category's assets, from state, as BAM! orders them: newest first.
    let category = format!("category_id={}", s.category);
    let listing = |archived: bool| {
        s.category_reads.fetch_add(1, Ordering::SeqCst);
        let mut list: Vec<Value> = s
            .assets
            .lock()
            .unwrap()
            .values()
            .filter(|a| a["asset"].get("archived_at").is_some() == archived)
            .cloned()
            .collect();
        list.sort_by(|a, b| {
            b["asset"]["created_at"]
                .as_str()
                .cmp(&a["asset"]["created_at"].as_str())
        });
        json_response(200, &json!({ "assets": list }))
    };
    if k == format!("GET /api/v1/assets?{category}") {
        return listing(false);
    }
    if k == format!("GET /api/v1/assets?archived=true&{category}") {
        return listing(true);
    }
    if let Some(id) = k.strip_prefix("GET /api/v1/assets/")
        && let Some(a) = s.assets.lock().unwrap().get(id)
    {
        return json_response(200, &json!({ "asset": a }));
    }
    match s.answers.get(&k) {
        Some(a) => respond(a),
        None => json_response(
            404,
            &json!({ "error": format!("the recording has no answer for {k}") }),
        ),
    }
}

async fn live(
    State(s): State<Arc<Shared>>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    req: Request,
) -> Response {
    let pq = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_default();
    s.requests.lock().unwrap().push(key("GET", &pq));
    let auth = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if let Some(r) = refused(&s, auth) {
        return r;
    }
    let Ok(ws) = ws else {
        // A plain request, as BAM! answers one.
        return s
            .answers
            .iter()
            .find(|(k, _)| k.starts_with("GET /api/v1/events/live"))
            .map(|(_, a)| a)
            .filter(|a| a.status == 426)
            .map(respond)
            .unwrap_or_else(|| json_response(426, &json!({"error": "Connect with a WebSocket"})));
    };
    if *s.refuse_live.lock().unwrap() {
        return json_response(503, &json!({"error": "BAM! cannot be reached"}));
    }
    s.live_opened.fetch_add(1, Ordering::SeqCst);
    ws.on_upgrade(move |socket| serve_live(socket, s))
        .into_response()
}

async fn serve_live(mut socket: WebSocket, s: Arc<Shared>) {
    let mut pushes = s.pushes.subscribe();
    if socket
        .send(Message::Text(s.hello.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    loop {
        tokio::select! {
            m = socket.recv() => {
                if !matches!(m, Some(Ok(_))) {
                    return;
                }
            }
            p = pushes.recv() => match p {
                Ok(Push::Frame(f)) => {
                    if socket.send(Message::Text(f.to_string().into())).await.is_err() {
                        return;
                    }
                }
                Ok(Push::Drop) | Err(_) => {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
            }
        }
    }
}
