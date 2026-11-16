//! Running the tower's printers (#49): what is added runs, every screen sees
//! it, and it follows BAM! and the admins' decisions.

use std::{sync::Arc, time::Duration};

use ::tower::ServiceExt as _;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use pop_integrations::{
    bam::Bam,
    fakes::bam::{FakeBam, TOKEN},
    printers::FieldMap,
    sync,
};
use pop_printers::{bambu::PinStore, fakes::moonraker::FakeMoonraker, fleet::Backoff};
use pop_server::{
    AppState,
    api::{TowerPrinter, TowerState},
    assets::Embedded,
    audit::Actor,
    db,
    pins::DbPins,
    router, run,
    secrets::Secrets,
    tower,
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::watch;

const RECORDING: &str =
    include_str!("../../pop-integrations/tests/recordings/v0.20.0/recordings.json");

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    state: AppState,
    fake: FakeBam,
    printer: FakeMoonraker,
    tower: watch::Receiver<Vec<TowerPrinter>>,
}

async fn decide(db: &SqlitePool, id: &str, to: TowerState) {
    tower::decide(
        db,
        id,
        "a printer",
        to,
        &Actor::command_line(),
        "a test decided".into(),
    )
    .await
    .unwrap();
}

/// POP with the Voron pointed at a fake Moonraker, and `added` added.
async fn pop(added: &[&str]) -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let fake = FakeBam::start(RECORDING).await;
    let printer = FakeMoonraker::start().await;
    fake.edit_without_event(fake.id("voron"), "address", &printer.url());
    for name in added {
        decide(&db, fake.id(name), TowerState::Added).await;
    }
    let opts = sync::Options {
        first_retry: Duration::from_millis(50),
        ..sync::Options::default()
    };
    let (view, _) = sync::spawn(
        Bam::new(fake.url(), TOKEN).unwrap(),
        fake.id("printers").to_owned(),
        opts,
    );
    let state = AppState::new(db.clone(), Arc::new(Embedded));
    let (tower, _) = run::spawn(run::Runner {
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
    });
    let state = state.with_tower(tower.clone());
    Pop {
        _dir: dir,
        db,
        state,
        fake,
        printer,
        tower,
    }
}

async fn until(
    rx: &watch::Receiver<Vec<TowerPrinter>>,
    what: &str,
    ok: impl Fn(&[TowerPrinter]) -> bool,
) -> Vec<TowerPrinter> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let v = rx.borrow().clone();
        if ok(&v) {
            return v;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never {what}: {v:#?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn named<'a>(ps: &'a [TowerPrinter], name: &str) -> Option<&'a TowerPrinter> {
    ps.iter().find(|p| p.name == name)
}

fn status(ps: &[TowerPrinter], name: &str) -> Option<String> {
    named(ps, name).map(|p| {
        serde_json::to_value(p.state.status)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    })
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

#[tokio::test]
async fn an_added_printer_runs_and_every_screen_sees_it() {
    let pop = pop(&["voron"]).await;
    until(&pop.tower, "the Voron idle", |ps| {
        status(ps, "Voron 2.4").as_deref() == Some("idle")
    })
    .await;
    pop.printer.set(json!({
        "print_stats": {"state": "printing", "filename": "gear_housing.gcode"},
        "display_status": {"progress": 0.5},
    }));
    let ps = until(&pop.tower, "the Voron printing", |ps| {
        status(ps, "Voron 2.4").as_deref() == Some("printing")
    })
    .await;
    let voron = named(&ps, "Voron 2.4").unwrap();
    assert!(voron.watched);
    assert_eq!(voron.state.job.as_ref().unwrap().name, "gear_housing.gcode");
    // A screen that connects now is sent it in its snapshot.
    let snap = stream_until(router(pop.state.clone()), "a snapshot", |v| {
        v["type"] == "snapshot"
    })
    .await;
    assert_eq!(snap["data"]["printers"][0]["name"], "Voron 2.4");
    assert_eq!(snap["data"]["printers"][0]["state"]["status"], "printing");
    assert!(pop.printer.connections() >= 1);
}

#[tokio::test]
async fn a_change_reaches_a_connected_screen_as_an_event() {
    let pop = pop(&["voron"]).await;
    until(&pop.tower, "the Voron idle", |ps| {
        status(ps, "Voron 2.4").as_deref() == Some("idle")
    })
    .await;
    let app = router(pop.state.clone());
    let printer = &pop.printer;
    let watching = stream_until(app, "the Voron paused", |v| {
        v["type"] == "printers" && v["data"][0]["state"]["status"] == "paused"
    });
    let change = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        printer.set(json!({"print_stats": {"state": "paused", "filename": "a.gcode"}}));
    };
    let (seen, ()) = tokio::join!(watching, change);
    assert_eq!(seen["data"][0]["name"], "Voron 2.4");
}

#[tokio::test]
async fn only_added_printers_run() {
    // The MK4 is right in BAM! and could run: it is only offered.
    let pop = pop(&["voron"]).await;
    let ps = until(&pop.tower, "the Voron", |ps| {
        named(ps, "Voron 2.4").is_some()
    })
    .await;
    assert_eq!(
        ps.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["Voron 2.4"],
        "an offered printer runs"
    );
    // Declined, it still does not.
    decide(&pop.db, pop.fake.id("mk4"), TowerState::Declined).await;
    pop.state.decided.notify_one();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        named(&pop.tower.borrow(), "MK4").is_none(),
        "a declined printer runs"
    );
}

#[tokio::test]
async fn adding_one_runs_it_without_waiting_for_bam() {
    let pop = pop(&[]).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(pop.tower.borrow().is_empty());
    decide(&pop.db, pop.fake.id("voron"), TowerState::Added).await;
    pop.state.decided.notify_one();
    until(&pop.tower, "the Voron, once added", |ps| {
        named(ps, "Voron 2.4").is_some()
    })
    .await;
}

#[tokio::test]
async fn a_printer_archived_in_bam_leaves_the_tower() {
    let pop = pop(&["voron"]).await;
    until(&pop.tower, "the Voron", |ps| {
        named(ps, "Voron 2.4").is_some()
    })
    .await;
    pop.fake.archive(pop.fake.id("voron"));
    until(&pop.tower, "the Voron gone", |ps| ps.is_empty()).await;
    // Its driver stopped: the fake sees no new connection after a while.
    let n = pop.printer.connections();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(pop.printer.connections(), n);
}

#[tokio::test]
async fn a_new_address_in_bam_gets_a_fresh_driver() {
    let pop = pop(&["voron"]).await;
    until(&pop.tower, "the Voron idle", |ps| {
        status(ps, "Voron 2.4").as_deref() == Some("idle")
    })
    .await;
    let moved = FakeMoonraker::start().await;
    moved.set(json!({"print_stats": {"state": "printing", "filename": "moved.gcode"}}));
    pop.fake.edit(pop.fake.id("voron"), "address", &moved.url());
    until(&pop.tower, "the Voron at its new address", |ps| {
        named(ps, "Voron 2.4")
            .and_then(|p| p.state.job.as_ref())
            .is_some_and(|j| j.name == "moved.gcode")
    })
    .await;
    assert!(moved.connections() >= 1);
}

#[tokio::test]
async fn a_printer_pop_cannot_watch_yet_is_listed_offline() {
    let pop = pop(&["mk4"]).await;
    let ps = until(&pop.tower, "the MK4", |ps| named(ps, "MK4").is_some()).await;
    let mk4 = named(&ps, "MK4").unwrap();
    assert!(!mk4.watched, "PrusaLink has no driver yet");
    assert_eq!(status(&ps, "MK4").as_deref(), Some("offline"));
}

#[tokio::test]
async fn a_bambu_pin_outlives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let pins = DbPins::load(db.clone()).await.unwrap();
    assert_eq!(pins.pinned("01S00C123456789"), None);
    pins.pin("01S00C123456789", "ab:cd");
    assert_eq!(pins.pinned("01S00C123456789").as_deref(), Some("ab:cd"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let again = DbPins::load(db.clone()).await.unwrap();
        if again.pinned("01S00C123456789").as_deref() == Some("ab:cd") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the pin never reached the database"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
