//! Klipper printers, through Moonraker (#27): status over its JSON-RPC
//! WebSocket.
//!
//! The driver connects to `ws://<host>:7125/websocket` (with the API key
//! header, when the printer has one), subscribes to the printer objects that
//! carry a print's state, and reports each change. When the connection
//! drops, `run` returns: the fleet reports the printer offline and connects
//! again, waiting longer each time while it keeps failing.
//!
//! Klipper's own words, mapped to POP's state:
//!
//! | Klipper                                   | POP          |
//! | ----------------------------------------- | ------------ |
//! | webhooks `startup`                        | offline      |
//! | webhooks `shutdown` or `error`            | needs a hand |
//! | print_stats `standby` or `cancelled`      | idle         |
//! | print_stats `printing`                    | printing     |
//! | print_stats `paused`                      | paused       |
//! | print_stats `complete`                    | done         |
//! | print_stats `error`                       | needs a hand |

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest, http::HeaderValue};

use crate::{
    driver::{Capabilities, Driver, DriverError, DriverFuture, Reporter},
    errors::PrinterError,
    state::{Job, Layer, PrinterState, PrinterStatus},
};

/// One Moonraker printer.
#[derive(Debug, Clone)]
pub struct Moonraker {
    /// The WebSocket address: `ws://printer.local:7125/websocket`.
    pub url: String,
    /// Moonraker's API key, if it requires one.
    pub api_key: Option<String>,
    /// How long the printer may take to answer the subscription.
    pub answer_within: Duration,
}

impl Moonraker {
    pub fn new(url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            url: url.into(),
            api_key,
            answer_within: Duration::from_secs(10),
        }
    }
}

/// What Klipper has told us so far. Moonraker sends only what changed, so
/// each update is merged into this before the state is worked out again.
#[derive(Debug, Default, Clone)]
pub struct Objects(pub serde_json::Map<String, Value>);

impl Objects {
    /// Merge an update: each object's fields replace the ones they name.
    pub fn merge(&mut self, update: &Value) {
        let Some(update) = update.as_object() else {
            return;
        };
        for (name, fields) in update {
            let entry = self.0.entry(name.clone()).or_insert_with(|| json!({}));
            if let (Some(have), Some(new)) = (entry.as_object_mut(), fields.as_object()) {
                for (k, v) in new {
                    // print_stats.info arrives piecewise too.
                    if let (Some(Value::Object(h)), Some(n)) = (have.get_mut(k), v.as_object()) {
                        for (ik, iv) in n {
                            h.insert(ik.clone(), iv.clone());
                        }
                    } else {
                        have.insert(k.clone(), v.clone());
                    }
                }
            }
        }
    }

    fn get(&self, object: &str, field: &str) -> Option<&Value> {
        self.0.get(object)?.get(field)
    }

    fn str(&self, object: &str, field: &str) -> Option<&str> {
        self.get(object, field)?.as_str()
    }

    /// The printer's state, as POP sees it.
    pub fn state(&self) -> PrinterState {
        let message =
            |o: &str, f: &str| self.str(o, f).filter(|m| !m.is_empty()).map(str::to_owned);
        match self.str("webhooks", "state") {
            Some("ready") | None => {}
            Some("startup") => return PrinterState::offline(),
            Some(_) => {
                // shutdown or error: Klipper itself has stopped.
                return PrinterState {
                    status: PrinterStatus::NeedsHand,
                    error: Some(PrinterError::raw(
                        message("webhooks", "state_message")
                            .unwrap_or_else(|| "Klipper has stopped".to_owned()),
                    )),
                    ..PrinterState::idle()
                };
            }
        }
        let status = match self.str("print_stats", "state") {
            Some("printing") => PrinterStatus::Printing,
            Some("paused") => PrinterStatus::Paused,
            Some("complete") => PrinterStatus::Done,
            Some("error") => PrinterStatus::NeedsHand,
            // standby, cancelled, or not said yet
            _ => PrinterStatus::Idle,
        };
        let has_print = matches!(
            status,
            PrinterStatus::Printing
                | PrinterStatus::Paused
                | PrinterStatus::Done
                | PrinterStatus::NeedsHand
        );
        // display_status.progress is what Klipper shows on its own screen;
        // virtual_sdcard.progress, the file position, is the fallback.
        let progress = self
            .get("display_status", "progress")
            .and_then(Value::as_f64)
            .or_else(|| {
                self.get("virtual_sdcard", "progress")
                    .and_then(Value::as_f64)
            })
            .map(|p| p.clamp(0.0, 1.0) as f32)
            .filter(|_| has_print);
        let info = self.get("print_stats", "info");
        let layer = info
            .and_then(|i| i.get("current_layer"))
            .and_then(Value::as_u64)
            .filter(|_| has_print)
            .map(|current| Layer {
                current: current as u32,
                total: info
                    .and_then(|i| i.get("total_layer"))
                    .and_then(Value::as_u64)
                    .map(|t| t as u32),
            });
        let job = self
            .str("print_stats", "filename")
            .filter(|f| !f.is_empty() && has_print)
            .map(|f| Job {
                name: f.to_owned(),
                owner: None,
            });
        // As Mainsail estimates it: the time printed so far, scaled by how
        // much of the file is left.
        let seconds_left = match (
            status,
            progress,
            self.get("print_stats", "print_duration")
                .and_then(Value::as_f64),
        ) {
            (PrinterStatus::Printing | PrinterStatus::Paused, Some(p), Some(d))
                if p > 0.0 && d > 0.0 =>
            {
                Some((d / f64::from(p) - d).max(0.0).round() as u64)
            }
            _ => None,
        };
        let error = (status == PrinterStatus::NeedsHand).then(|| {
            PrinterError::raw(
                message("print_stats", "message")
                    .unwrap_or_else(|| "the print stopped with an error".to_owned()),
            )
        });
        PrinterState {
            status,
            progress,
            layer,
            seconds_left,
            job,
            error,
            // Klipper's error state has ended the print; a pause it can
            // resume from reads as paused, not as needing a hand.
            held: false,
        }
    }
}

/// The objects subscribed to, and the fields of each that matter.
pub fn subscription() -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "printer.objects.subscribe",
        "params": { "objects": {
            "webhooks": ["state", "state_message"],
            "print_stats": ["state", "filename", "print_duration", "message", "info"],
            "display_status": ["progress"],
            "virtual_sdcard": ["progress"],
        }},
        "id": 1,
    })
}

fn fail(m: impl Into<String>) -> DriverError {
    DriverError(m.into())
}

impl Driver for Moonraker {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            status: true,
            errors: true,
            ..Capabilities::default()
        }
    }

    fn run(self: Box<Self>, r: Reporter) -> DriverFuture {
        Box::pin(async move {
            let mut req = self
                .url
                .as_str()
                .into_client_request()
                .map_err(|e| fail(format!("{}: not a WebSocket address: {e}", self.url)))?;
            if let Some(key) = &self.api_key {
                req.headers_mut().insert(
                    "X-Api-Key",
                    HeaderValue::from_str(key)
                        .map_err(|_| fail("the API key is not a valid header"))?,
                );
            }
            let (mut ws, _) = tokio_tungstenite::connect_async(req)
                .await
                .map_err(|e| fail(format!("cannot reach Moonraker at {}: {e}", self.url)))?;
            ws.send(Message::Text(subscription().to_string().into()))
                .await
                .map_err(|e| fail(format!("the connection dropped: {e}")))?;

            let mut objects = Objects::default();
            let mut subscribed = false;
            let deadline = tokio::time::sleep(self.answer_within);
            tokio::pin!(deadline);
            loop {
                let msg = tokio::select! {
                    m = ws.next() => m,
                    () = &mut deadline, if !subscribed => {
                        return Err(fail("Moonraker did not answer the subscription"));
                    }
                };
                let msg = match msg {
                    None => return Err(fail("Moonraker closed the connection")),
                    Some(Err(e)) => return Err(fail(format!("the connection dropped: {e}"))),
                    Some(Ok(m)) => m,
                };
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => return Err(fail("Moonraker closed the connection")),
                    // Pings are answered by the library; nothing else is sent.
                    _ => continue,
                };
                let Ok(v) = serde_json::from_str::<Value>(&text) else {
                    // One unreadable message is the printer's problem, not a
                    // reason to drop the connection and the state with it.
                    tracing::warn!("moonraker {}: an unreadable message, ignored", self.url);
                    continue;
                };
                if v.get("id") == Some(&json!(1)) {
                    if let Some(e) = v.get("error") {
                        return Err(fail(format!(
                            "Moonraker refused the subscription: {}",
                            e.get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("no reason given")
                        )));
                    }
                    let Some(status) = v.pointer("/result/status") else {
                        return Err(fail("Moonraker's answer to the subscription had no status"));
                    };
                    objects.merge(status);
                    subscribed = true;
                    r.report(objects.state());
                    continue;
                }
                match v.get("method").and_then(Value::as_str) {
                    Some("notify_status_update") if subscribed => {
                        if let Some(update) = v.pointer("/params/0") {
                            objects.merge(update);
                            r.report(objects.state());
                        }
                    }
                    // Klipper went away under Moonraker: start over, which
                    // reports offline meanwhile and subscribes afresh. (A
                    // shutdown is not this: Moonraker reports it as a status
                    // update, which reads as needing a hand.)
                    Some("notify_klippy_disconnected") => {
                        return Err(fail("Klipper disconnected from Moonraker"));
                    }
                    _ => {}
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn objects(v: Value) -> Objects {
        let mut o = Objects::default();
        o.merge(&v);
        o
    }

    #[test]
    fn progress_is_klippers_own_and_the_file_position_only_without_it() {
        let both = objects(json!({"print_stats": {"state": "printing"},
            "display_status": {"progress": 0.4}, "virtual_sdcard": {"progress": 0.9}}));
        assert_eq!(both.state().progress, Some(0.4));
        let file_only = objects(
            json!({"print_stats": {"state": "printing"}, "virtual_sdcard": {"progress": 0.9}}),
        );
        assert_eq!(file_only.state().progress, Some(0.9));
    }

    #[test]
    fn an_idle_printer_has_no_progress_layer_job_or_time_left() {
        let o = objects(
            json!({"print_stats": {"state": "standby", "filename": "old.gcode", "print_duration": 9.0,
            "info": {"current_layer": 3, "total_layer": 9}}, "display_status": {"progress": 1.0}}),
        );
        let s = o.state();
        assert_eq!(s.status, PrinterStatus::Idle);
        assert_eq!(
            (s.progress, s.layer, s.job, s.seconds_left),
            (None, None, None, None)
        );
    }

    #[test]
    fn there_is_no_time_left_to_guess_before_any_progress() {
        let o = objects(
            json!({"print_stats": {"state": "printing", "print_duration": 30.0}, "display_status": {"progress": 0.0}}),
        );
        assert_eq!(o.state().seconds_left, None);
    }

    #[test]
    fn a_done_print_keeps_its_file_and_has_no_time_left() {
        let o = objects(
            json!({"print_stats": {"state": "complete", "filename": "benchy.gcode", "print_duration": 600.0},
            "display_status": {"progress": 1.0}}),
        );
        let s = o.state();
        assert_eq!(s.status, PrinterStatus::Done);
        assert_eq!(s.job.unwrap().name, "benchy.gcode");
        assert_eq!(s.seconds_left, None);
    }

    #[test]
    fn a_progress_outside_0_to_1_is_kept_inside_it() {
        let o = objects(
            json!({"print_stats": {"state": "printing"}, "display_status": {"progress": 1.3}}),
        );
        assert_eq!(o.state().progress, Some(1.0));
    }
}
