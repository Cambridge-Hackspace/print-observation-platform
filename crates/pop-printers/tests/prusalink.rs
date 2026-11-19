//! The PrusaLink driver (#30), run by a real fleet against a fake PrusaLink
//! over real HTTP, signing in as the printer asks.

use std::{sync::Arc, time::Duration};

use pop_printers::{
    Fleet, Login, Printer, PrinterState, PrinterStatus,
    fakes::prusalink::FakePrusaLink,
    fleet::{Backoff, Factory},
    prusalink::PrusaLink,
};
use serde_json::json;

fn quick() -> Backoff {
    Backoff {
        first: Duration::from_millis(50),
        max: Duration::from_millis(200),
        healthy_after: Duration::from_secs(5),
    }
}

fn driver(url: String, login: Option<Login>) -> Factory {
    Arc::new(move || {
        Box::new(PrusaLink::new(url.clone(), login.clone()).every(Duration::from_millis(50)))
    })
}

fn digest(password: &str) -> Option<Login> {
    Some(Login::Digest {
        username: "maker".into(),
        password: password.into(),
    })
}

async fn fleet_for(fake: &FakePrusaLink, login: Option<Login>) -> Fleet {
    let mut fleet = Fleet::new(quick());
    fleet.add(Printer {
        id: "mk4".into(),
        factory: driver(fake.url(), login),
    });
    fleet
}

/// Waits up to three seconds for the printer's state to pass `ok`.
async fn until(fleet: &Fleet, ok: impl Fn(&PrinterState) -> bool, what: &str) -> PrinterState {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let s = fleet.state("mk4").unwrap();
        if ok(&s) {
            return s;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never {what}: {s:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn printing(fake: &FakePrusaLink) {
    fake.set_status(json!({
        "printer": {"state": "PRINTING", "temp_nozzle": 215.0, "temp_bed": 60.0},
        "job": {"id": 41, "progress": 25.0, "time_remaining": 1800, "time_printing": 600},
    }));
    fake.set_job(Some(json!({
        "id": 41, "state": "PRINTING", "progress": 25.0, "time_printing": 600,
        "file": {"name": "BENCHY~1.BGC", "display_name": "benchy_0.2mm_PLA_MK4_1h.bgcode", "path": "/usb", "m_timestamp": 0},
    })));
}

#[tokio::test]
async fn a_print_reads_live_with_its_file_name() {
    let fake = FakePrusaLink::start().await;
    let fleet = fleet_for(&fake, None).await;
    until(&fleet, |s| s.status == PrinterStatus::Idle, "idle").await;
    printing(&fake);
    let s = until(&fleet, |s| s.status == PrinterStatus::Printing, "printing").await;
    assert_eq!(s.progress, Some(0.25));
    assert_eq!(s.seconds_left, Some(1800));
    assert_eq!(s.job.unwrap().name, "benchy_0.2mm_PLA_MK4_1h.bgcode");
}

#[tokio::test]
async fn the_job_is_asked_for_once_per_job_not_every_time() {
    let fake = FakePrusaLink::start().await;
    let fleet = fleet_for(&fake, None).await;
    printing(&fake);
    until(&fleet, |s| s.job.is_some(), "a job").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let asked = fake.asked();
    let statuses = asked.iter().filter(|(p, _)| p == "/api/v1/status").count();
    let jobs = asked.iter().filter(|(p, _)| p == "/api/v1/job").count();
    assert!(statuses >= 5, "status asked {statuses} times");
    assert_eq!(jobs, 1, "{asked:?}");
}

#[tokio::test]
async fn it_signs_in_with_digest_as_the_printer_asks() {
    let fake = FakePrusaLink::start().await;
    fake.require_digest("maker", "s3cret");
    let fleet = fleet_for(&fake, digest("s3cret")).await;
    printing(&fake);
    until(
        &fleet,
        |s| s.status == PrinterStatus::Printing,
        "printing behind digest",
    )
    .await;
    // Each challenge is answered on later requests too, not asked for again
    // every time.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let asked = fake.asked();
    let refused = asked.iter().filter(|(_, ok)| !ok).count();
    let let_in = asked.iter().filter(|(_, ok)| *ok).count();
    assert_eq!(
        refused, 1,
        "only the first request is challenged: {asked:?}"
    );
    assert!(let_in >= 4, "{asked:?}");
}

#[tokio::test]
async fn a_new_nonce_is_answered_without_a_break() {
    let fake = FakePrusaLink::start().await;
    fake.require_digest("maker", "s3cret");
    let fleet = fleet_for(&fake, digest("s3cret")).await;
    printing(&fake);
    until(&fleet, |s| s.status == PrinterStatus::Printing, "printing").await;
    fake.new_nonce("e5f6a7b8");
    let before = fake.asked().len();
    tokio::time::sleep(Duration::from_millis(400)).await;
    // Still printing, never offline in between, and let in again after the
    // one refusal the new nonce costs.
    assert_eq!(fleet.state("mk4").unwrap().status, PrinterStatus::Printing);
    let after: Vec<_> = fake.asked().into_iter().skip(before).collect();
    assert_eq!(after.iter().filter(|(_, ok)| !ok).count(), 1, "{after:?}");
    assert!(after.last().unwrap().1, "{after:?}");
}

#[tokio::test]
async fn a_wrong_password_is_never_let_in_and_reads_offline() {
    let fake = FakePrusaLink::start().await;
    fake.require_digest("maker", "s3cret");
    printing(&fake);
    let fleet = fleet_for(&fake, digest("wrong")).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    // The precondition: it did try, more than once (the fleet retries).
    let asked = fake.asked();
    assert!(asked.len() >= 4, "{asked:?}");
    assert!(asked.iter().all(|(_, ok)| !ok), "{asked:?}");
    assert_eq!(fleet.state("mk4").unwrap().status, PrinterStatus::Offline);
}

#[tokio::test]
async fn with_no_login_a_printer_that_asks_for_one_reads_offline() {
    let fake = FakePrusaLink::start().await;
    fake.require_digest("maker", "s3cret");
    let fleet = fleet_for(&fake, None).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!fake.asked().is_empty());
    assert!(fake.asked().iter().all(|(_, ok)| !ok));
    assert_eq!(fleet.state("mk4").unwrap().status, PrinterStatus::Offline);
}

#[tokio::test]
async fn an_api_key_is_sent_as_prusalink_on_a_pi_asks() {
    let fake = FakePrusaLink::start().await;
    fake.require_api_key("k3y");
    let fleet = fleet_for(&fake, Some(Login::ApiKey("k3y".into()))).await;
    printing(&fake);
    until(
        &fleet,
        |s| s.status == PrinterStatus::Printing,
        "printing with a key",
    )
    .await;
    assert!(fake.asked().iter().all(|(_, ok)| *ok), "{:?}", fake.asked());
}

#[tokio::test]
async fn a_printer_that_goes_away_reads_offline() {
    // Coming back is the fleet's, tested in tests/fleet.rs.
    let fake = FakePrusaLink::start().await;
    let fleet = fleet_for(&fake, None).await;
    until(&fleet, |s| s.status == PrinterStatus::Idle, "idle").await;
    drop(fake);
    until(
        &fleet,
        |s| s.status == PrinterStatus::Offline,
        "offline once gone",
    )
    .await;
}
