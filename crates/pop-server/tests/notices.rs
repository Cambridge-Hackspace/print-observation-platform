//! Which events notify the group (#69), end to end: a printer the tower runs
//! stops for a person, and the group's Discord channel hears it; it prints
//! again, and the channel hears that.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{Router, extract::State, http::StatusCode, routing::post};
use pop_integrations::{
    bam::Bam,
    fakes::bam::{FakeBam, TOKEN},
    printers::FieldMap,
    sync,
};
use pop_printers::{fakes::moonraker::FakeMoonraker, fleet::Backoff};
use pop_server::{
    api::TowerState, audit::Actor, db, notices, notify, pins::DbPins, run, secrets::Secrets, tower,
};
use serde_json::{Value, json};

const RECORDING: &str =
    include_str!("../../pop-integrations/tests/recordings/v0.20.0/recordings.json");
const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

type Got = Arc<Mutex<Vec<String>>>;

async fn discord() -> (Got, String) {
    let got: Got = Arc::default();
    let app = Router::new()
        .route(
            "/api/webhooks/{id}/{token}",
            post(|State(g): State<Got>, body: String| async move {
                let v: Value = serde_json::from_str(&body).unwrap();
                g.lock()
                    .unwrap()
                    .push(v["content"].as_str().unwrap().to_owned());
                StatusCode::NO_CONTENT
            }),
        )
        .with_state(got.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (got, format!("http://{addr}/api/webhooks/1/test-token-abcd"))
}

async fn until(what: &str, ok: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !ok() {
        assert!(tokio::time::Instant::now() < deadline, "never {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn the_group_hears_when_a_printer_needs_a_hand_and_when_it_prints_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let bam = FakeBam::start(RECORDING).await;
    let printer = FakeMoonraker::start().await;
    bam.edit_without_event(bam.id("voron"), "address", &printer.url());
    tower::decide(
        &db,
        bam.id("voron"),
        "Voron 2.4",
        TowerState::Added,
        &Actor::command_line(),
        "test".into(),
    )
    .await
    .unwrap();
    let (view, _) = sync::spawn(
        Bam::new(bam.url(), TOKEN).unwrap(),
        bam.id("printers").to_owned(),
        sync::Options {
            first_retry: Duration::from_millis(50),
            ..sync::Options::default()
        },
    );
    let events = pop_server::events::Events::new();
    let (tower_rx, _) = run::spawn(run::Runner {
        db: db.clone(),
        secrets: Secrets::none(),
        fields: FieldMap::default(),
        view,
        events,
        decided: Arc::new(tokio::sync::Notify::new()),
        pins: Arc::new(DbPins::load(db.clone()).await.unwrap()),
        backoff: Backoff {
            first: Duration::from_millis(50),
            max: Duration::from_millis(200),
            healthy_after: Duration::from_secs(60),
        },
    });
    let (got, url) = discord().await;
    let secrets = Secrets::new(KEY);
    secrets
        .set_setting(&db, notify::WEBHOOK, &url)
        .await
        .unwrap();
    let (notifier, _) = notify::spawn(
        db.clone(),
        secrets,
        reqwest::Client::new(),
        notify::Options::default(),
    );
    let _watch = notices::spawn(tower_rx.clone(), notifier);

    printer.set(json!({"print_stats": {"state": "printing", "filename": "gear.gcode"}}));
    let printing = || {
        tower_rx
            .borrow()
            .iter()
            .any(|p| serde_json::to_value(p.state.status).unwrap() == "printing")
    };
    until("the Voron printing", printing).await;
    printer.set(json!({"print_stats": {"state": "error", "message": "Must home axis first: 10.0 10.0 5.0"}}));
    until("the notice", || !got.lock().unwrap().is_empty()).await;
    assert_eq!(
        got.lock().unwrap()[0],
        "Voron 2.4 needs a hand: the printer was asked to move before finding its home position."
    );
    printer.set(json!({"print_stats": {"state": "printing", "message": ""}}));
    until("the second notice", || got.lock().unwrap().len() == 2).await;
    assert_eq!(got.lock().unwrap()[1], "Voron 2.4 is printing again.");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(got.lock().unwrap().len(), 2, "each said once");
}
