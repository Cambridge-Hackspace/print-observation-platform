//! A fake Moonraker (#26): JSON-RPC over a real WebSocket on a real port.
//!
//! It answers `printer.objects.subscribe` with the fields asked for, as
//! Moonraker does, and sends `notify_status_update` with what changed when a
//! test changes its printer. Faults, each as a real Moonraker can show them:
//! the connection dropped, the answer slow, a message that is not JSON, the
//! subscription refused, and an API key required.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use tokio::{net::TcpListener, sync::broadcast, task::JoinHandle};
use tokio_tungstenite::tungstenite::{
    Message,
    handshake::server::{Callback, ErrorResponse, Request, Response},
    http::StatusCode,
};

#[derive(Default)]
struct Faults {
    slow_answer: Option<Duration>,
    refuse: bool,
    api_key: Option<String>,
}

#[derive(Clone)]
enum Push {
    Update(Value),
    Raw(String),
    Method(String),
    Drop,
}

struct Shared {
    objects: Mutex<Map<String, Value>>,
    faults: Mutex<Faults>,
    connections: Mutex<u32>,
    pushes: broadcast::Sender<Push>,
}

/// A fake Moonraker, listening until dropped.
pub struct FakeMoonraker {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Drop for FakeMoonraker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeMoonraker {
    /// Start one on a free port, with Klipper ready and the printer standing
    /// by.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let addr = listener.local_addr().expect("its address");
        let shared = Arc::new(Shared {
            objects: Mutex::new(
                json!({
                    "webhooks": {"state": "ready", "state_message": "Printer is ready"},
                    "print_stats": {"state": "standby", "filename": "", "print_duration": 0.0, "message": "",
                                    "info": {"current_layer": null, "total_layer": null}},
                    "display_status": {"progress": 0.0},
                    "virtual_sdcard": {"progress": 0.0},
                })
                .as_object()
                .cloned()
                .expect("an object"),
            ),
            faults: Mutex::new(Faults::default()),
            connections: Mutex::new(0),
            pushes: broadcast::channel(256).0,
        });
        let s = shared.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let s = s.clone();
                tokio::spawn(async move { serve(stream, s).await });
            }
        });
        Self { addr, shared, task }
    }

    /// The address a driver connects to.
    pub fn url(&self) -> String {
        format!("ws://{}/websocket", self.addr)
    }

    /// How many connections it has accepted.
    pub fn connections(&self) -> u32 {
        *self.shared.connections.lock().unwrap()
    }

    /// Change the printer: each object's fields as given replace what it had,
    /// and every subscriber is sent the change.
    pub fn set(&self, update: Value) {
        merge(&mut self.shared.objects.lock().unwrap(), &update);
        let _ = self.shared.pushes.send(Push::Update(update));
    }

    /// Send every subscriber a message that is not JSON.
    pub fn send_garbage(&self) {
        let _ = self.shared.pushes.send(Push::Raw("{not json".into()));
    }

    /// Send every subscriber a bare notification, `notify_klippy_disconnected`
    /// say.
    pub fn notify(&self, method: &str) {
        let _ = self.shared.pushes.send(Push::Method(method.into()));
    }

    /// Drop every connection, as a printer switched off does.
    pub fn drop_connections(&self) {
        let _ = self.shared.pushes.send(Push::Drop);
    }

    /// Answer the subscription this late.
    pub fn answer_slowly(&self, by: Duration) {
        self.shared.faults.lock().unwrap().slow_answer = Some(by);
    }

    /// Refuse the subscription with a JSON-RPC error.
    pub fn refuse_subscriptions(&self, refuse: bool) {
        self.shared.faults.lock().unwrap().refuse = refuse;
    }

    /// Accept only connections carrying this X-Api-Key.
    pub fn require_api_key(&self, key: &str) {
        self.shared.faults.lock().unwrap().api_key = Some(key.into());
    }
}

fn merge(objects: &mut Map<String, Value>, update: &Value) {
    let Some(update) = update.as_object() else {
        return;
    };
    for (name, fields) in update {
        let entry = objects.entry(name.clone()).or_insert_with(|| json!({}));
        if let (Some(have), Some(new)) = (entry.as_object_mut(), fields.as_object()) {
            for (k, v) in new {
                have.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Refuses an upgrade without the API key, when one is required: a trait
/// implementation, whose signature is tungstenite's to set.
struct KeyCheck(Option<String>);

impl Callback for KeyCheck {
    fn on_request(self, req: &Request, res: Response) -> Result<Response, ErrorResponse> {
        match &self.0 {
            Some(k)
                if req.headers().get("x-api-key").and_then(|v| v.to_str().ok())
                    != Some(k.as_str()) =>
            {
                let mut e = ErrorResponse::new(Some("Unauthorized".into()));
                *e.status_mut() = StatusCode::UNAUTHORIZED;
                Err(e)
            }
            _ => Ok(res),
        }
    }
}

async fn serve(stream: tokio::net::TcpStream, s: Arc<Shared>) {
    let check = KeyCheck(s.faults.lock().unwrap().api_key.clone());
    let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, check).await else {
        return;
    };
    *s.connections.lock().unwrap() += 1;
    let mut pushes = s.pushes.subscribe();
    let mut subscribed = false;
    loop {
        tokio::select! {
            m = ws.next() => {
                let Some(Ok(Message::Text(t))) = m else { return };
                let Ok(req) = serde_json::from_str::<Value>(&t) else { continue };
                if req.get("method").and_then(Value::as_str) != Some("printer.objects.subscribe") {
                    continue;
                }
                let (slow, refuse) = {
                    let f = s.faults.lock().unwrap();
                    (f.slow_answer, f.refuse)
                };
                if let Some(d) = slow {
                    tokio::time::sleep(d).await;
                }
                let id = req.get("id").cloned().unwrap_or(Value::Null);
                let answer = if refuse {
                    json!({"jsonrpc": "2.0", "error": {"code": 400, "message": "Klippy Disconnected"}, "id": id})
                } else {
                    // Only the fields asked for, as Moonraker answers.
                    let wanted = req.pointer("/params/objects").and_then(Value::as_object).cloned().unwrap_or_default();
                    let objects = s.objects.lock().unwrap().clone();
                    let mut status = Map::new();
                    for (name, fields) in wanted {
                        let have = objects.get(&name).and_then(Value::as_object).cloned().unwrap_or_default();
                        let picked: Map<String, Value> = match fields.as_array() {
                            Some(list) => list
                                .iter()
                                .filter_map(Value::as_str)
                                .filter_map(|f| have.get(f).map(|v| (f.to_owned(), v.clone())))
                                .collect(),
                            None => have,
                        };
                        status.insert(name, Value::Object(picked));
                    }
                    subscribed = true;
                    json!({"jsonrpc": "2.0", "result": {"eventtime": 1.0, "status": status}, "id": id})
                };
                if ws.send(Message::Text(answer.to_string().into())).await.is_err() {
                    return;
                }
            }
            p = pushes.recv() => {
                let out = match p {
                    Ok(Push::Update(u)) if subscribed => json!({"jsonrpc": "2.0", "method": "notify_status_update", "params": [u, 2.0]}).to_string(),
                    Ok(Push::Update(_)) => continue,
                    Ok(Push::Raw(r)) => r,
                    Ok(Push::Method(m)) => json!({"jsonrpc": "2.0", "method": m}).to_string(),
                    Ok(Push::Drop) | Err(_) => return,
                };
                if ws.send(Message::Text(out.into())).await.is_err() {
                    return;
                }
            }
        }
    }
}
