//! The fakes the end-to-end battery runs POP against (#59): the fake BAM!
//! and fake Moonraker printers the Rust tests use, served as one process,
//! with a small control API so a Playwright test can change a printer.
//!
//! Never part of a deployment: tools/web/e2e.sh builds it on its own, so its
//! `fakes` features never reach the `pop` binary under test.
//!
//! Environment:
//! - `POP_FAKES_LISTEN`: where the control API listens (127.0.0.1:8099).
//! - `POP_FAKES_PRINTERS`: how many printers besides the recording's Voron
//!   (0). Each is another fake Moonraker, named "Bench 01", "Bench 02" and
//!   so on.
//!
//! Control API:
//! - `GET  /bam`              `{url, token, category}`: what POP is given.
//! - `GET  /printers`         `{name: BAM! id}` for every fake printer.
//! - `POST /printers/{name}`  Moonraker objects to merge in, as Klipper would
//!   report them, for example
//!   `{"print_stats": {"state": "printing", "filename": "a.gcode"}}`.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use pop_integrations::fakes::bam::{FakeBam, TOKEN};
use pop_printers::fakes::moonraker::FakeMoonraker;
use serde_json::{Value, json};

const RECORDING: &str =
    include_str!("../../pop-integrations/tests/recordings/v0.20.0/recordings.json");

struct Fakes {
    bam: FakeBam,
    /// By name: its BAM! id and the fake printer behind it.
    printers: BTreeMap<String, (String, FakeMoonraker)>,
}

#[tokio::main]
async fn main() {
    let listen = std::env::var("POP_FAKES_LISTEN").unwrap_or_else(|_| "127.0.0.1:8099".into());
    let extra: usize = match std::env::var("POP_FAKES_PRINTERS") {
        Ok(v) => v
            .parse()
            .unwrap_or_else(|_| panic!("POP_FAKES_PRINTERS={v:?} is not a count")),
        Err(_) => 0,
    };

    let bam = FakeBam::start(RECORDING).await;
    let voron = bam.id("voron").to_owned();
    // The Voron's fields as recorded, for the printers made like it.
    let rec: Value = serde_json::from_str(RECORDING).expect("the recording is JSON");
    let fields = rec["recorded"]["printers"]["body"]["assets"]
        .as_array()
        .expect("the recording's printers")
        .iter()
        .find(|a| a["asset"]["id"] == voron.as_str())
        .expect("the Voron in the recording")["field_values"]
        .clone();

    let mut printers = BTreeMap::new();
    let first = FakeMoonraker::start().await;
    bam.edit_without_event(&voron, "address", &first.url());
    printers.insert("Voron 2.4".to_owned(), (voron.clone(), first));
    for i in 1..=extra {
        let name = format!("Bench {i:02}");
        let printer = FakeMoonraker::start().await;
        let id = bam.add(&voron, &name, fields.clone());
        bam.edit_without_event(&id, "address", &printer.url());
        printers.insert(name, (id, printer));
    }

    let category = bam.id("printers").to_owned();
    let url = bam.url();
    let fakes = Arc::new(Fakes { bam, printers });
    let app = Router::new()
        .route("/bam", get(bam_info))
        .route("/printers", get(list))
        .route("/printers/{name}", axum::routing::post(set))
        .with_state(fakes);
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .unwrap_or_else(|e| panic!("cannot listen on {listen}: {e}"));
    // One line a script can wait for and read.
    println!(
        "{}",
        json!({"ready": true, "control": format!("http://{listen}"), "bam": url, "token": TOKEN, "category": category})
    );
    axum::serve(listener, app).await.expect("the control API");
}

async fn bam_info(State(f): State<Arc<Fakes>>) -> Json<Value> {
    Json(json!({"url": f.bam.url(), "token": TOKEN, "category": f.bam.id("printers")}))
}

async fn list(State(f): State<Arc<Fakes>>) -> Json<Value> {
    Json(json!(
        f.printers
            .iter()
            .map(|(name, (id, _))| (name.clone(), json!(id)))
            .collect::<serde_json::Map<_, _>>()
    ))
}

async fn set(
    State(f): State<Arc<Fakes>>,
    Path(name): Path<String>,
    Json(update): Json<Value>,
) -> StatusCode {
    match f.printers.get(&name) {
        Some((_, printer)) if update.is_object() => {
            printer.set(update);
            StatusCode::NO_CONTENT
        }
        Some(_) => StatusCode::BAD_REQUEST,
        None => StatusCode::NOT_FOUND,
    }
}
