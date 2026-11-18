//! Which events notify the group (#69), end to end: a printer the tower runs
//! stops for a person, and the group's Discord channel hears it; it prints
//! again, and the channel hears that. And what the channel took stands for
//! the printer's screen while the trouble lasts (#55).

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use ::tower::ServiceExt as _;
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    routing::post,
};
use http_body_util::BodyExt;
use pop_integrations::{
    bam::Bam,
    fakes::bam::{FakeBam, TOKEN},
    printers::FieldMap,
    sync,
};
use pop_printers::{fakes::moonraker::FakeMoonraker, fleet::Backoff};
use pop_server::{
    AppState,
    api::{GroupNotice, TowerState},
    assets::Embedded,
    audit::Actor,
    db, notices, notify,
    pins::DbPins,
    router, run,
    secrets::Secrets,
    tower,
};
use serde_json::{Value, json};
use tokio::sync::watch;

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

struct Pop {
    _dir: tempfile::TempDir,
    _bam: FakeBam,
    printer: FakeMoonraker,
    state: AppState,
    told: watch::Receiver<Vec<GroupNotice>>,
}

impl Pop {
    fn status(&self) -> Option<String> {
        self.state
            .tower
            .borrow()
            .first()
            .map(|p| serde_json::to_value(p.state.status).unwrap().to_string())
    }
}

/// POP running the Voron, posting to `webhook` if there is one.
async fn pop(webhook: Option<&str>) -> Pop {
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
    let state = AppState::new(db.clone(), Arc::new(Embedded));
    let (tower_rx, _) = run::spawn(run::Runner {
        db: db.clone(),
        secrets: Secrets::none(),
        fields: FieldMap::default(),
        view,
        events: state.events.clone(),
        decided: state.decided.clone(),
        pins: Arc::new(DbPins::load(db.clone()).await.unwrap()),
        backoff: Backoff {
            first: Duration::from_millis(50),
            max: Duration::from_millis(200),
            healthy_after: Duration::from_secs(60),
        },
        cameras: None,
    });
    let secrets = Secrets::new(KEY);
    if let Some(url) = webhook {
        secrets
            .set_setting(&db, notify::WEBHOOK, url)
            .await
            .unwrap();
    }
    let (notifier, _) = notify::spawn(
        db.clone(),
        secrets,
        reqwest::Client::new(),
        notify::Options::default(),
    );
    let (told, _) = notices::spawn(tower_rx.clone(), notifier, state.events.clone());
    let state = state.with_tower(tower_rx).with_told(told.clone());
    Pop {
        _dir: dir,
        _bam: bam,
        printer,
        state,
        told,
    }
}

/// The live stream, read until a frame's data passes `ok`.
async fn stream_until(app: Router, what: &str, ok: impl Fn(&Value) -> bool) -> Value {
    let res = app
        .oneshot(Request::get("/api/events").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let mut body = res.into_body();
    let mut buf = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        while let Some(end) = buf.find("\n\n") {
            let block: String = buf.drain(..end + 2).collect();
            if let Some(d) = block.lines().find_map(|l| l.strip_prefix("data:")) {
                let v: Value = serde_json::from_str(d.trim()).unwrap();
                if ok(&v) {
                    return v;
                }
            }
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, body.frame()).await {
            Ok(Some(Ok(f))) => {
                if let Ok(d) = f.into_data() {
                    buf.push_str(std::str::from_utf8(&d).unwrap());
                }
            }
            _ => panic!("the stream never said {what}"),
        }
    }
}

const HOMING: &str =
    "Voron 2.4 needs a hand: the printer was asked to move before finding its home position.";

#[tokio::test]
async fn the_group_hears_when_a_printer_needs_a_hand_and_when_it_prints_again() {
    let (got, url) = discord().await;
    let pop = pop(Some(&url)).await;
    pop.printer
        .set(json!({"print_stats": {"state": "printing", "filename": "gear.gcode"}}));
    until("the Voron printing", || {
        pop.status().as_deref() == Some("\"printing\"")
    })
    .await;
    // A screen already connected hears what the group was told.
    let watching = stream_until(router(pop.state.clone()), "what the group was told", |v| {
        v["type"] == "told" && v["data"][0]["text"] == HOMING
    });
    let stop = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        pop.printer.set(
            json!({"print_stats": {"state": "error", "message": "Must home axis first: 10.0 10.0 5.0"}}),
        );
    };
    let (seen, ()) = tokio::join!(watching, stop);
    assert_eq!(
        got.lock().unwrap().as_slice(),
        [HOMING],
        "Discord had it first"
    );
    let id = pop.state.tower.borrow()[0].id.clone();
    assert_eq!(seen["data"][0]["printer"], id.as_str());
    assert!(seen["data"][0]["at"].as_i64().unwrap() > 1_700_000_000);
    // And one that connects now is sent it in its snapshot.
    let snap = stream_until(router(pop.state.clone()), "a snapshot", |v| {
        v["type"] == "snapshot"
    })
    .await;
    assert_eq!(snap["data"]["told"][0]["text"], HOMING);

    pop.printer
        .set(json!({"print_stats": {"state": "printing", "message": ""}}));
    until("the second notice", || got.lock().unwrap().len() == 2).await;
    assert_eq!(got.lock().unwrap()[1], "Voron 2.4 is printing again.");
    until("the notice to go", || pop.told.borrow().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(got.lock().unwrap().len(), 2, "each said once");
}

#[tokio::test]
async fn a_notice_never_posted_is_not_shown_as_told() {
    let pop = pop(None).await;
    pop.printer
        .set(json!({"print_stats": {"state": "printing", "filename": "gear.gcode"}}));
    until("the Voron printing", || {
        pop.status().as_deref() == Some("\"printing\"")
    })
    .await;
    pop.printer.set(
        json!({"print_stats": {"state": "error", "message": "Must home axis first: 10.0 10.0 5.0"}}),
    );
    until("the Voron needing a hand", || {
        pop.status().as_deref() == Some("\"needs_hand\"")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        pop.told.borrow().is_empty(),
        "with no webhook, nothing was told"
    );
}
