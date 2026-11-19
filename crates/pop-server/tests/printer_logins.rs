//! A printer's own login reaching its driver (#29, #30): the tower runs a
//! PrusaLink printer behind Digest and an OctoPrint one behind a key, with
//! the logins POP keeps, and withholds a login from an address it was not
//! entered for.

use std::{sync::Arc, time::Duration};

use pop_integrations::{
    bam::Bam,
    fakes::bam::{FakeBam, TOKEN},
    printers::FieldMap,
    sync,
};
use pop_printers::{
    Login,
    bambu::PinStore,
    fakes::{octoprint::FakeOctoPrint, prusalink::FakePrusaLink},
    fleet::Backoff,
};
use pop_server::{
    AppState,
    api::{TowerPrinter, TowerState},
    assets::Embedded,
    audit::Actor,
    cameras, db,
    pins::DbPins,
    run,
    secrets::{PrinterLogin, Secrets},
    tower,
};
use serde_json::json;
use sqlx::SqlitePool;
use tokio::sync::watch;

const RECORDING: &str =
    include_str!("../../pop-integrations/tests/recordings/v0.20.0/recordings.json");
const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    state: AppState,
    fake: FakeBam,
    prusa: FakePrusaLink,
    octo: FakeOctoPrint,
    tower: watch::Receiver<Vec<TowerPrinter>>,
}

async fn keep(db: &SqlitePool, bam_id: &str, login: Login, url: &str) {
    Secrets::new(KEY)
        .set_printer_login(
            db,
            bam_id,
            &PrinterLogin {
                login,
                origin: cameras::origin(url),
            },
        )
        .await
        .unwrap();
}

/// The MK4 at a PrusaLink asking for Digest, the Voron moved to an OctoPrint
/// asking for a key, both added, and their logins kept.
async fn pop() -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let fake = FakeBam::start(RECORDING).await;
    let prusa = FakePrusaLink::start().await;
    prusa.require_digest("maker", "s3cret");
    let octo = FakeOctoPrint::start().await;
    octo.require_api_key("k3y");
    let (mk4, voron) = (fake.id("mk4").to_owned(), fake.id("voron").to_owned());
    fake.edit_without_event(&mk4, "address", &prusa.url());
    fake.edit_without_event(&voron, "connection", "octoprint");
    fake.edit_without_event(&voron, "address", &octo.url());
    for id in [&mk4, &voron] {
        tower::decide(
            &db,
            id,
            "a printer",
            TowerState::Added,
            &Actor::command_line(),
            "a test".into(),
        )
        .await
        .unwrap();
    }
    keep(
        &db,
        &mk4,
        Login::Digest {
            username: "maker".into(),
            password: "s3cret".into(),
        },
        &prusa.url(),
    )
    .await;
    keep(&db, &voron, Login::ApiKey("k3y".into()), &octo.url()).await;
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
    let pins: Arc<dyn PinStore> = Arc::new(DbPins::load(db.clone()).await.unwrap());
    let (tower, _) = run::spawn(run::Runner {
        db: db.clone(),
        secrets: Secrets::new(KEY),
        fields: FieldMap::default(),
        view,
        events: state.events.clone(),
        decided: state.decided.clone(),
        pins,
        backoff: Backoff {
            first: Duration::from_millis(50),
            max: Duration::from_millis(200),
            healthy_after: Duration::from_secs(60),
        },
        cameras: None,
    });
    Pop {
        _dir: dir,
        db,
        state,
        fake,
        prusa,
        octo,
        tower,
    }
}

async fn until(
    rx: &watch::Receiver<Vec<TowerPrinter>>,
    what: &str,
    ok: impl Fn(&[TowerPrinter]) -> bool,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if ok(&rx.borrow()) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never {what}: {:#?}",
            rx.borrow()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn status(ps: &[TowerPrinter], name: &str) -> Option<String> {
    ps.iter().find(|p| p.name == name).map(|p| {
        serde_json::to_value(p.state.status)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    })
}

fn printing(pop: &Pop) {
    pop.prusa.set_status(json!({"printer": {"state": "PRINTING"}, "job": {"id": 9, "progress": 40.0, "time_remaining": 300}}));
    pop.prusa.set_job(Some(json!({"id": 9, "state": "PRINTING", "progress": 40.0, "time_printing": 200,
                                    "file": {"name": "CLIP~1.BGC", "display_name": "clip.bgcode", "path": "/usb", "m_timestamp": 0}})));
    pop.octo
        .set_printer(Some(pop_printers::fakes::octoprint::printer(
            "Printing",
            &["operational", "printing"],
        )));
    pop.octo.set_job(json!({"job": {"file": {"name": "hook.gcode", "display": "hook.gcode"}},
                            "progress": {"completion": 10.0, "printTimeLeft": 900}, "state": "Printing"}));
}

#[tokio::test]
async fn both_run_live_signed_in_with_the_logins_pop_keeps() {
    let pop = pop().await;
    printing(&pop);
    until(&pop.tower, "both printing", |ps| {
        status(ps, "MK4").as_deref() == Some("printing")
            && status(ps, "Voron 2.4").as_deref() == Some("printing")
    })
    .await;
    let ps = pop.tower.borrow().clone();
    for p in &ps {
        assert!(p.watched, "{} reads as having no driver", p.name);
    }
    assert!(pop.prusa.asked().iter().any(|(_, ok)| *ok));
    assert!(
        pop.octo.asked().iter().all(|(_, ok)| *ok),
        "{:?}",
        pop.octo.asked()
    );
    let _ = &pop.state;
}

#[tokio::test]
async fn a_login_is_not_given_to_an_address_it_was_not_entered_for() {
    let pop = pop().await;
    printing(&pop);
    until(&pop.tower, "the MK4 printing", |ps| {
        status(ps, "MK4").as_deref() == Some("printing")
    })
    .await;
    // BAM! now names another host for the MK4: a second PrusaLink, asking
    // for the same login. POP does not sign in to it with the old one.
    let other = FakePrusaLink::start().await;
    other.require_digest("maker", "s3cret");
    other.set_status(json!({"printer": {"state": "PRINTING"}, "job": {"id": 1, "progress": 5.0}}));
    pop.fake.edit(pop.fake.id("mk4"), "address", &other.url());
    until(&pop.tower, "the MK4 offline at its new address", |ps| {
        status(ps, "MK4").as_deref() == Some("offline")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let asked = other.asked();
    // The precondition: it was asked, so the driver did go there.
    assert!(!asked.is_empty(), "the new address was never tried");
    assert!(
        asked.iter().all(|(_, ok)| !ok),
        "signed in at an address the login was not for: {asked:?}"
    );

    // And an API key, which goes in the request itself: an OctoPrint at a
    // new address asking for the same key is never sent it.
    let other = FakeOctoPrint::start().await;
    other.require_api_key("k3y");
    pop.fake.edit(pop.fake.id("voron"), "address", &other.url());
    until(&pop.tower, "the Voron offline at its new address", |ps| {
        status(ps, "Voron 2.4").as_deref() == Some("offline")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let asked = other.asked();
    assert!(!asked.is_empty(), "the new address was never tried");
    assert!(
        asked.iter().all(|(_, ok)| !ok),
        "the key went to an address it was not for: {asked:?}"
    );
    let _ = &pop.db;
}
