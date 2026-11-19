//! Prusa printers, through PrusaLink (#30): watching, for now. Upload and
//! start come with sending a print (#45).
//!
//! The driver asks `GET /api/v1/status` every two seconds, and
//! `GET /api/v1/job` when a new job appears, for its file's name. PrusaLink
//! on the printer itself (MK4, XL, MINI+) asks for HTTP Digest, with the user
//! and password the printer shows in its network settings; PrusaLink on a
//! Raspberry Pi takes an API key (`X-Api-Key`). When a request fails, `run`
//! returns: the fleet reports the printer offline and tries again, waiting
//! longer each time while it keeps failing.
//!
//! Built against PrusaLink's own OpenAPI description (Prusa-Link-Web,
//! spec/openapi.yaml), not a recording: no printer was at hand. Its words,
//! mapped to POP's state:
//!
//! | PrusaLink                    | POP                                   |
//! | ---------------------------- | ------------------------------------- |
//! | `IDLE`, `READY`, `STOPPED`   | idle                                  |
//! | `BUSY`                       | printing with a job, idle without one |
//! | `PRINTING`                   | printing                              |
//! | `PAUSED`                     | paused                                |
//! | `FINISHED`                   | done                                  |
//! | `ATTENTION`                  | needs a hand, the print held          |
//! | `ERROR`                      | needs a hand                          |
//! | anything else                | needs a hand, saying what it said     |

use std::time::Duration;

use reqwest::{StatusCode, header};
use serde_json::Value;

use crate::{
    driver::{Capabilities, Driver, DriverError, DriverFuture, Reporter},
    errors::PrinterError,
    login::Login,
    state::{Job, PrinterState, PrinterStatus},
};

/// How often PrusaLink is asked.
const EVERY: Duration = Duration::from_secs(2);

/// One Prusa printer's PrusaLink.
pub struct PrusaLink {
    base: String,
    login: Option<Login>,
    every: Duration,
    answer_within: Duration,
}

impl PrusaLink {
    /// PrusaLink at `base` (`http://192.168.1.40`), signing in with `login`
    /// when it asks.
    pub fn new(base: impl Into<String>, login: Option<Login>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_owned(),
            login,
            every: EVERY,
            answer_within: Duration::from_secs(10),
        }
    }

    /// Ask this often instead (tests).
    pub fn every(mut self, every: Duration) -> Self {
        self.every = every;
        self
    }
}

impl Driver for PrusaLink {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            status: true,
            errors: true,
            ..Capabilities::default()
        }
    }

    fn run(self: Box<Self>, r: Reporter) -> DriverFuture {
        Box::pin(async move {
            let http = reqwest::Client::builder()
                .timeout(self.answer_within)
                .build()
                .map_err(|e| DriverError(format!("PrusaLink: {e}")))?;
            let mut client = Client {
                http,
                base: self.base.clone(),
                login: self.login.clone(),
                challenge: None,
            };
            // The job whose file name is known, and that name.
            let mut named: Option<(i64, Option<String>)> = None;
            loop {
                let status = client.json("/api/v1/status").await?.unwrap_or(Value::Null);
                let id = status["job"]["id"].as_i64();
                let name = match (id, &named) {
                    (None, _) => None,
                    (Some(id), Some((known, name))) if *known == id => name.clone(),
                    (Some(id), _) => {
                        let job = client.json("/api/v1/job").await?;
                        let name = job.as_ref().and_then(file_name);
                        named = Some((id, name.clone()));
                        name
                    }
                };
                r.report(state(&status, name.as_deref()));
                tokio::time::sleep(self.every).await;
            }
        })
    }
}

/// A job's file, by the name the slicer gave it.
fn file_name(job: &Value) -> Option<String> {
    let file = &job["file"];
    file["display_name"]
        .as_str()
        .or_else(|| file["name"].as_str())
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
}

/// The printer's state, as POP sees it, from `GET /api/v1/status` and the
/// job's file name.
pub fn state(status: &Value, name: Option<&str>) -> PrinterState {
    let said = status["printer"]["state"].as_str().unwrap_or("");
    let has_job = status["job"]["id"].as_i64().is_some();
    let (status_now, held) = match said {
        "IDLE" | "READY" | "STOPPED" => (PrinterStatus::Idle, false),
        "BUSY" if has_job => (PrinterStatus::Printing, false),
        "BUSY" => (PrinterStatus::Idle, false),
        "PRINTING" => (PrinterStatus::Printing, false),
        "PAUSED" => (PrinterStatus::Paused, false),
        "FINISHED" => (PrinterStatus::Done, false),
        // Waiting for a person at the printer, the print kept: a filament
        // change, a crash detected.
        "ATTENTION" => (PrinterStatus::NeedsHand, true),
        _ => (PrinterStatus::NeedsHand, false),
    };
    let has_print = has_job
        && matches!(
            status_now,
            PrinterStatus::Printing
                | PrinterStatus::Paused
                | PrinterStatus::Done
                | PrinterStatus::NeedsHand
        );
    let progress = status["job"]["progress"]
        .as_f64()
        .filter(|_| has_print)
        .map(|p| (p / 100.0).clamp(0.0, 1.0) as f32);
    let seconds_left = status["job"]["time_remaining"].as_u64().filter(|_| {
        matches!(status_now, PrinterStatus::Printing | PrinterStatus::Paused) && has_job
    });
    let job = name.filter(|_| has_print).map(|n| Job {
        name: n.to_owned(),
        owner: None,
    });
    let error = (status_now == PrinterStatus::NeedsHand).then(|| {
        let message = status["printer"]["status_printer"]["message"]
            .as_str()
            .filter(|m| !m.is_empty());
        PrinterError::raw(match (said, message) {
            (_, Some(m)) => m.to_owned(),
            ("ATTENTION", None) => "the printer is waiting for someone".to_owned(),
            ("ERROR", None) => "the printer reports an error".to_owned(),
            (other, None) => format!("PrusaLink says {other:?}, which POP does not know"),
        })
    });
    PrinterState {
        status: status_now,
        progress,
        layer: None,
        seconds_left,
        job,
        error,
        held,
    }
}

/// PrusaLink's HTTP, signing in as it asks.
struct Client {
    http: reqwest::Client,
    base: String,
    login: Option<Login>,
    /// The Digest challenge last given, answered on each request with its
    /// count going up, until PrusaLink gives a new one.
    challenge: Option<digest_auth::WwwAuthenticateHeader>,
}

impl Client {
    /// GET `path` as JSON; `None` for 204, PrusaLink's "there is none".
    async fn json(&mut self, path: &str) -> Result<Option<Value>, DriverError> {
        let mut fresh = false;
        loop {
            let mut req = self.http.get(format!("{}{path}", self.base));
            match &self.login {
                Some(Login::ApiKey(key)) => req = req.header("X-Api-Key", key),
                Some(Login::Digest { username, password }) => {
                    if let Some(c) = &mut self.challenge {
                        let answer = c
                            .respond(&digest_auth::AuthContext::new(
                                username.as_str(),
                                password.as_str(),
                                path,
                            ))
                            .map_err(|e| {
                                DriverError(format!("PrusaLink's login challenge: {e}"))
                            })?;
                        req = req.header(header::AUTHORIZATION, answer.to_header_string());
                    }
                }
                None => {}
            }
            let res = req
                .send()
                .await
                .map_err(|e| DriverError(format!("PrusaLink at {}: {e}", self.base)))?;
            match res.status() {
                StatusCode::NO_CONTENT => return Ok(None),
                s if s.is_success() => {
                    return res.json().await.map(Some).map_err(|e| {
                        DriverError(format!("PrusaLink's {path}: not the JSON POP expects: {e}"))
                    });
                }
                StatusCode::UNAUTHORIZED => {
                    let digest = matches!(self.login, Some(Login::Digest { .. }));
                    let offered = res
                        .headers()
                        .get(header::WWW_AUTHENTICATE)
                        .and_then(|v| v.to_str().ok())
                        .filter(|v| v.trim_start().to_ascii_lowercase().starts_with("digest"));
                    match (&self.login, offered) {
                        (None, _) => {
                            return Err(DriverError(
                                "PrusaLink asks for a login: enter the printer's user and password on the admin desk".into(),
                            ));
                        }
                        // A first challenge, or a new one after the last ran
                        // out: answer it once.
                        (_, Some(c)) if digest && !fresh => {
                            self.challenge = Some(digest_auth::parse(c).map_err(|e| {
                                DriverError(format!("PrusaLink's login challenge: {e}"))
                            })?);
                            fresh = true;
                        }
                        _ => {
                            return Err(DriverError(
                                "PrusaLink refused POP's login: check the printer's user and password on the admin desk"
                                    .into(),
                            ));
                        }
                    }
                }
                s => return Err(DriverError(format!("PrusaLink's {path} answered {s}"))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn st(printer: &str, job: Value) -> Value {
        json!({"printer": {"state": printer}, "job": job})
    }

    #[test]
    fn a_print_in_progress_reads_as_printing_with_its_progress_and_time() {
        let s = state(
            &st(
                "PRINTING",
                json!({"id": 7, "progress": 42.0, "time_remaining": 600}),
            ),
            Some("benchy.gcode"),
        );
        assert_eq!(s.status, PrinterStatus::Printing);
        assert_eq!(s.progress, Some(0.42));
        assert_eq!(s.seconds_left, Some(600));
        assert_eq!(s.job.unwrap().name, "benchy.gcode");
        assert!(s.error.is_none());
    }

    #[test]
    fn each_state_maps_as_the_table_says() {
        let job = json!({"id": 1, "progress": 10.0});
        for (said, want, held) in [
            ("IDLE", PrinterStatus::Idle, false),
            ("READY", PrinterStatus::Idle, false),
            ("STOPPED", PrinterStatus::Idle, false),
            ("BUSY", PrinterStatus::Printing, false),
            ("PRINTING", PrinterStatus::Printing, false),
            ("PAUSED", PrinterStatus::Paused, false),
            ("FINISHED", PrinterStatus::Done, false),
            ("ATTENTION", PrinterStatus::NeedsHand, true),
            ("ERROR", PrinterStatus::NeedsHand, false),
            ("SOMETHING_NEW", PrinterStatus::NeedsHand, false),
        ] {
            let s = state(&st(said, job.clone()), Some("a.gcode"));
            assert_eq!((s.status, s.held), (want, held), "{said}");
        }
        // Busy with no job is not a print.
        assert_eq!(
            state(&st("BUSY", Value::Null), None).status,
            PrinterStatus::Idle
        );
    }

    #[test]
    fn needing_a_hand_says_why_in_the_printers_words_or_plainly() {
        let mut v = st("ERROR", json!({"id": 1}));
        v["printer"]["status_printer"] = json!({"ok": false, "message": "Heater thermal runaway"});
        assert_eq!(state(&v, None).error.unwrap().raw, "Heater thermal runaway");
        assert_eq!(
            state(&st("ATTENTION", json!({"id": 1})), None)
                .error
                .unwrap()
                .raw,
            "the printer is waiting for someone"
        );
        let odd = state(&st("SOMETHING_NEW", Value::Null), None)
            .error
            .unwrap()
            .raw;
        assert!(odd.contains("SOMETHING_NEW"), "{odd}");
    }

    #[test]
    fn an_idle_printer_shows_no_leftover_job() {
        // PrusaLink may keep the last job's figures after it ends.
        let s = state(
            &st(
                "IDLE",
                json!({"id": 3, "progress": 100.0, "time_remaining": 0}),
            ),
            Some("old.gcode"),
        );
        assert_eq!(s.progress, None);
        assert_eq!(s.seconds_left, None);
        assert!(s.job.is_none());
    }

    #[test]
    fn a_finished_print_keeps_its_name_and_reads_full() {
        let s = state(
            &st(
                "FINISHED",
                json!({"id": 3, "progress": 100.0, "time_remaining": 0}),
            ),
            Some("done.gcode"),
        );
        assert_eq!(s.status, PrinterStatus::Done);
        assert_eq!(s.progress, Some(1.0));
        assert_eq!(s.seconds_left, None);
        assert_eq!(s.job.unwrap().name, "done.gcode");
    }

    #[test]
    fn a_jobs_name_is_the_long_one_when_given() {
        let j = json!({"file": {"name": "SPICE~1.GCO", "display_name": "Spice_Harvester.gcode"}});
        assert_eq!(file_name(&j).as_deref(), Some("Spice_Harvester.gcode"));
        assert_eq!(
            file_name(&json!({"file": {"name": "SPICE~1.GCO"}})).as_deref(),
            Some("SPICE~1.GCO")
        );
        assert_eq!(file_name(&json!({})), None);
    }

    #[test]
    fn a_login_never_shows_its_secret() {
        let d = Login::Digest {
            username: "maker".into(),
            password: "hunter2".into(),
        };
        assert!(!format!("{d:?}").contains("hunter2"));
        assert!(!format!("{:?}", Login::ApiKey("k3y".into())).contains("k3y"));
    }
}
