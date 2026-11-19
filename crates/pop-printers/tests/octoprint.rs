//! The OctoPrint driver (#29), run by a real fleet against a fake OctoPrint
//! over real HTTP.

use std::{sync::Arc, time::Duration};

use pop_printers::{
    Fleet, Printer, PrinterState, PrinterStatus,
    fakes::octoprint::{FakeOctoPrint, printer},
    fleet::{Backoff, Factory},
    octoprint::OctoPrint,
};
use serde_json::json;

fn quick() -> Backoff {
    Backoff {
        first: Duration::from_millis(50),
        max: Duration::from_millis(200),
        healthy_after: Duration::from_secs(5),
    }
}

fn fleet_for(fake: &FakeOctoPrint, key: Option<&str>) -> Fleet {
    let (url, key) = (fake.url(), key.map(str::to_owned));
    let factory: Factory = Arc::new(move || {
        Box::new(OctoPrint::new(url.clone(), key.clone()).every(Duration::from_millis(50)))
    });
    let mut fleet = Fleet::new(quick());
    fleet.add(Printer {
        id: "ender".into(),
        factory,
    });
    fleet
}

async fn until(fleet: &Fleet, ok: impl Fn(&PrinterState) -> bool, what: &str) -> PrinterState {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let s = fleet.state("ender").unwrap();
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

fn printing(fake: &FakeOctoPrint) {
    fake.set_printer(Some(printer("Printing", &["operational", "printing"])));
    fake.set_job(json!({
        "job": {"file": {"name": "whistle_v2.gcode", "display": "Whistle v2.gcode", "origin": "local"}, "estimatedPrintTime": 8811},
        "progress": {"completion": 22.98, "filepos": 337942, "printTime": 276, "printTimeLeft": 912, "printTimeLeftOrigin": "linear"},
        "state": "Printing",
    }));
}

#[tokio::test]
async fn a_print_reads_live() {
    let fake = FakeOctoPrint::start().await;
    fake.require_api_key("k3y");
    let fleet = fleet_for(&fake, Some("k3y"));
    until(&fleet, |s| s.status == PrinterStatus::Idle, "idle").await;
    printing(&fake);
    let s = until(&fleet, |s| s.status == PrinterStatus::Printing, "printing").await;
    assert!((s.progress.unwrap() - 0.2298).abs() < 1e-4);
    assert_eq!(s.seconds_left, Some(912));
    assert_eq!(s.job.unwrap().name, "Whistle v2.gcode");
    assert!(fake.asked().iter().all(|(_, ok)| *ok));
}

#[tokio::test]
async fn a_printer_not_connected_to_octoprint_reads_offline_and_is_still_asked() {
    let fake = FakeOctoPrint::start().await;
    let fleet = fleet_for(&fake, None);
    until(&fleet, |s| s.status == PrinterStatus::Idle, "idle").await;
    fake.set_printer(None);
    until(
        &fleet,
        |s| s.status == PrinterStatus::Offline,
        "offline on a 409",
    )
    .await;
    // Not a failure of POP's: the same driver keeps asking, and reads the
    // printer back as soon as it is connected again.
    let before = fake.asked().len();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(fake.asked().len() > before + 2, "{:?}", fake.asked());
    // While the printer is away, the job is not asked for.
    let away: Vec<_> = fake.asked().into_iter().skip(before).collect();
    assert!(
        away.iter().all(|(p, _)| p.starts_with("/api/printer")),
        "{away:?}"
    );
    printing(&fake);
    until(
        &fleet,
        |s| s.status == PrinterStatus::Printing,
        "back, printing",
    )
    .await;
}

#[tokio::test]
async fn a_wrong_key_is_never_let_in_and_reads_offline() {
    let fake = FakeOctoPrint::start().await;
    fake.require_api_key("k3y");
    printing(&fake);
    let fleet = fleet_for(&fake, Some("wrong"));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let asked = fake.asked();
    assert!(asked.len() >= 3, "it tried: {asked:?}");
    assert!(asked.iter().all(|(_, ok)| !ok), "{asked:?}");
    assert_eq!(fleet.state("ender").unwrap().status, PrinterStatus::Offline);
}

#[tokio::test]
async fn temperatures_are_not_asked_for() {
    // POP shows none, and they are most of /api/printer's answer.
    let fake = FakeOctoPrint::start().await;
    let fleet = fleet_for(&fake, None);
    until(&fleet, |s| s.status == PrinterStatus::Idle, "idle").await;
    let asked = fake.asked();
    let printer: Vec<_> = asked
        .iter()
        .filter(|(p, _)| p.starts_with("/api/printer"))
        .collect();
    assert!(!printer.is_empty());
    assert!(
        printer
            .iter()
            .all(|(p, _)| p == "/api/printer?exclude=temperature"),
        "{printer:?}"
    );
}
