//! Printers behind OctoPrint (#29): watching, for now. Upload and start come
//! with sending a print (#45).
//!
//! The driver asks `GET /api/printer` and `GET /api/job` every two seconds,
//! with the API key in `X-Api-Key`. When OctoPrint answers 409, it is up but
//! its printer is not connected to it: the printer reads offline, and the
//! driver keeps asking. When OctoPrint itself cannot be reached, or refuses
//! the key, `run` returns: the fleet reports the printer offline and tries
//! again, waiting longer each time while it keeps failing.
//!
//! Built against OctoPrint's REST documentation (docs.octoprint.org, "Printer
//! operations" and "Job operations"), not a recording. OctoPrint's flags,
//! mapped to POP's state, first that applies:
//!
//! | OctoPrint                                    | POP          |
//! | -------------------------------------------- | ------------ |
//! | `/api/printer` answers 409                   | offline      |
//! | `error`                                      | needs a hand |
//! | `paused` or `pausing`                        | paused       |
//! | `cancelling`                                 | idle         |
//! | `printing` or `finishing`                    | printing     |
//! | the last job at 100%                         | done         |
//! | otherwise                                    | idle         |

use std::time::Duration;

use reqwest::StatusCode;
use serde_json::Value;

use crate::{
    driver::{Capabilities, Driver, DriverError, DriverFuture, Reporter},
    errors::PrinterError,
    state::{Job, PrinterState, PrinterStatus},
};

const EVERY: Duration = Duration::from_secs(2);

/// One printer's OctoPrint.
pub struct OctoPrint {
    base: String,
    api_key: Option<String>,
    every: Duration,
    answer_within: Duration,
}

impl OctoPrint {
    /// OctoPrint at `base` (`http://octopi.local`), with its API key.
    pub fn new(base: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_owned(),
            api_key,
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

impl Driver for OctoPrint {
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
                .map_err(|e| DriverError(format!("OctoPrint: {e}")))?;
            loop {
                let printer = self.get(&http, "/api/printer?exclude=temperature").await?;
                let s = match printer {
                    None => PrinterState::offline(),
                    Some(p) => {
                        let job = self.get(&http, "/api/job").await?.unwrap_or(Value::Null);
                        state(&p, &job)
                    }
                };
                r.report(s);
                tokio::time::sleep(self.every).await;
            }
        })
    }
}

impl OctoPrint {
    /// GET `path` as JSON; `None` for 409, OctoPrint's "the printer is not
    /// connected".
    async fn get(&self, http: &reqwest::Client, path: &str) -> Result<Option<Value>, DriverError> {
        let mut req = http.get(format!("{}{path}", self.base));
        if let Some(k) = &self.api_key {
            req = req.header("X-Api-Key", k);
        }
        let res = req
            .send()
            .await
            .map_err(|e| DriverError(format!("OctoPrint at {}: {e}", self.base)))?;
        match res.status() {
            StatusCode::CONFLICT => Ok(None),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                Err(DriverError(match self.api_key {
                    None => "OctoPrint asks for an API key: enter it on the admin desk".into(),
                    Some(_) => "OctoPrint refused POP's API key: check it on the admin desk".into(),
                }))
            }
            s if s.is_success() => res.json().await.map(Some).map_err(|e| {
                DriverError(format!("OctoPrint's {path}: not the JSON POP expects: {e}"))
            }),
            s => Err(DriverError(format!("OctoPrint's {path} answered {s}"))),
        }
    }
}

/// The printer's state, as POP sees it, from `GET /api/printer` and
/// `GET /api/job`.
pub fn state(printer: &Value, job: &Value) -> PrinterState {
    let flag = |f: &str| printer["state"]["flags"][f].as_bool().unwrap_or(false);
    let completion = job["progress"]["completion"].as_f64();
    let name = job["job"]["file"]["display"]
        .as_str()
        .or_else(|| job["job"]["file"]["name"].as_str())
        .filter(|n| !n.is_empty());
    let status = if flag("error") {
        PrinterStatus::NeedsHand
    } else if flag("paused") || flag("pausing") {
        PrinterStatus::Paused
    } else if flag("cancelling") {
        PrinterStatus::Idle
    } else if flag("printing") || flag("finishing") {
        PrinterStatus::Printing
    } else if name.is_some() && completion.is_some_and(|c| c >= 100.0) {
        PrinterStatus::Done
    } else {
        PrinterStatus::Idle
    };
    let has_print = name.is_some()
        && matches!(
            status,
            PrinterStatus::Printing
                | PrinterStatus::Paused
                | PrinterStatus::Done
                | PrinterStatus::NeedsHand
        );
    let progress = completion
        .filter(|_| has_print)
        .map(|c| (c / 100.0).clamp(0.0, 1.0) as f32);
    let seconds_left = job["progress"]["printTimeLeft"]
        .as_f64()
        .filter(|_| has_print && matches!(status, PrinterStatus::Printing | PrinterStatus::Paused))
        .map(|s| s.max(0.0).round() as u64);
    let error = (status == PrinterStatus::NeedsHand).then(|| {
        let said = job["error"]
            .as_str()
            .or_else(|| printer["state"]["text"].as_str())
            .filter(|m| !m.is_empty())
            .unwrap_or("OctoPrint reports an error");
        PrinterError::raw(said)
    });
    PrinterState {
        status,
        progress,
        layer: None,
        seconds_left,
        job: name.filter(|_| has_print).map(|n| Job {
            name: n.to_owned(),
            owner: None,
        }),
        error,
        held: false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn printer(text: &str, flags: &[&str]) -> Value {
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
            "finishing",
        ] {
            f.insert(k.into(), json!(flags.contains(&k)));
        }
        json!({"state": {"text": text, "flags": f}})
    }

    fn job(completion: Option<f64>, left: Option<f64>) -> Value {
        json!({
            "job": {"file": {"name": "whistle_v2.gcode", "display": "Whistle v2.gcode", "origin": "local"}},
            "progress": {"completion": completion, "printTimeLeft": left, "printTime": 276},
            "state": "Printing",
        })
    }

    #[test]
    fn a_print_in_progress_reads_as_printing_with_its_progress_and_time() {
        let s = state(
            &printer("Printing", &["operational", "printing"]),
            &job(Some(22.98), Some(912.0)),
        );
        assert_eq!(s.status, PrinterStatus::Printing);
        assert!((s.progress.unwrap() - 0.2298).abs() < 1e-4);
        assert_eq!(s.seconds_left, Some(912));
        assert_eq!(s.job.unwrap().name, "Whistle v2.gcode");
    }

    #[test]
    fn each_flag_maps_as_the_table_says() {
        for (flags, want) in [
            (&["operational", "ready"][..], PrinterStatus::Idle),
            (&["operational", "printing"][..], PrinterStatus::Printing),
            (&["operational", "finishing"][..], PrinterStatus::Printing),
            (&["operational", "paused"][..], PrinterStatus::Paused),
            (
                &["operational", "printing", "pausing"][..],
                PrinterStatus::Paused,
            ),
            (
                &["operational", "printing", "cancelling"][..],
                PrinterStatus::Idle,
            ),
            (&["error", "closedOrError"][..], PrinterStatus::NeedsHand),
        ] {
            assert_eq!(
                state(&printer("x", flags), &job(Some(50.0), Some(10.0))).status,
                want,
                "{flags:?}"
            );
        }
    }

    #[test]
    fn a_finished_job_is_done_until_the_next_one_and_a_cancelled_one_is_not() {
        let ready = printer("Operational", &["operational", "ready"]);
        let s = state(&ready, &job(Some(100.0), Some(0.0)));
        assert_eq!(s.status, PrinterStatus::Done);
        assert_eq!(s.progress, Some(1.0));
        assert_eq!(s.seconds_left, None);
        assert_eq!(
            state(&ready, &job(Some(37.0), None)).status,
            PrinterStatus::Idle
        );
        assert!(state(&ready, &job(Some(37.0), None)).job.is_none());
    }

    #[test]
    fn an_error_says_what_octoprint_said() {
        let mut j = job(Some(40.0), None);
        j["error"] = json!("Thermal runaway on heater bed");
        let s = state(
            &printer("Offline after error", &["error", "closedOrError"]),
            &j,
        );
        assert_eq!(s.error.unwrap().raw, "Thermal runaway on heater bed");
        let s = state(
            &printer("Offline after error", &["error", "closedOrError"]),
            &Value::Null,
        );
        assert_eq!(s.error.unwrap().raw, "Offline after error");
    }

    #[test]
    fn an_idle_printer_with_no_job_shows_none() {
        let s = state(
            &printer("Operational", &["operational", "ready"]),
            &json!({"job": {"file": {"name": null}}, "progress": {"completion": null}}),
        );
        assert_eq!(s.status, PrinterStatus::Idle);
        assert!(s.job.is_none() && s.progress.is_none() && s.seconds_left.is_none());
    }
}
