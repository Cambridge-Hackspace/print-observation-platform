//! The Bambu driver (#31), run by a real fleet against a fake Bambu printer
//! (#26) over real TLS.

use std::{sync::Arc, time::Duration};

use pop_printers::{
    Fleet, Printer, PrinterState, PrinterStatus,
    bambu::{Bambu, MemoryPins, PinStore},
    fakes::bambu::FakeBambu,
    fleet::Backoff,
};
use serde_json::json;

const SERIAL: &str = "01S00C123456789";
const CODE: &str = "12345678";

fn quick() -> Backoff {
    Backoff {
        first: Duration::from_millis(50),
        max: Duration::from_millis(200),
        healthy_after: Duration::from_secs(5),
    }
}

struct Rig {
    fake: FakeBambu,
    fleet: Fleet,
    pins: Arc<MemoryPins>,
}

fn add(fleet: &mut Fleet, fake: &FakeBambu, code: &str, pins: Arc<MemoryPins>) {
    let (host, port, code) = (fake.host(), fake.port(), code.to_owned());
    fleet.add(Printer {
        id: "x1".into(),
        factory: Arc::new(move || {
            Box::new(Bambu {
                port,
                answer_within: Duration::from_secs(2),
                hold_after_refusal: Duration::from_millis(1500),
                ..Bambu::new(host.clone(), SERIAL, code.clone(), pins.clone())
            })
        }),
    });
}

async fn rig_with(code: &str) -> Rig {
    let fake = FakeBambu::start(SERIAL, CODE).await;
    let pins = Arc::new(MemoryPins::default());
    let mut fleet = Fleet::new(quick());
    add(&mut fleet, &fake, code, pins.clone());
    Rig { fake, fleet, pins }
}

async fn rig() -> Rig {
    let r = rig_with(CODE).await;
    r.until(|s| s.status == PrinterStatus::Idle, "idle on connecting")
        .await;
    r
}

impl Rig {
    async fn until(&self, ok: impl Fn(&PrinterState) -> bool, what: &str) -> PrinterState {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        loop {
            let s = self.fleet.state("x1").unwrap();
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
}

#[tokio::test]
async fn it_signs_in_asks_for_everything_and_reads_idle() {
    let r = rig().await;
    let pushall = r
        .fake
        .requests()
        .iter()
        .any(|v| v.pointer("/pushing/command") == Some(&json!("pushall")));
    assert!(pushall, "{:?}", r.fake.requests());
    assert_eq!(r.fake.connections(), 1);
}

#[tokio::test]
async fn a_print_reads_with_its_progress_layer_time_left_and_name() {
    let r = rig().await;
    r.fake.set(
        json!({"gcode_state": "RUNNING", "mc_percent": 37, "layer_num": 41, "total_layer_num": 120,
                      "mc_remaining_time": 52, "subtask_name": "lamp_shade"}),
    );
    let s = r
        .until(|s| s.status == PrinterStatus::Printing, "printing")
        .await;
    assert_eq!(s.progress, Some(0.37));
    assert_eq!(s.layer.map(|l| (l.current, l.total)), Some((41, Some(120))));
    assert_eq!(s.seconds_left, Some(52 * 60));
    assert_eq!(s.job.unwrap().name, "lamp_shade");
}

#[tokio::test]
async fn a_partial_report_keeps_the_rest() {
    let r = rig().await;
    r.fake
        .set(json!({"gcode_state": "RUNNING", "mc_percent": 10, "subtask_name": "lamp_shade"}));
    r.until(|s| s.progress == Some(0.1), "printing at 10%")
        .await;
    r.fake.set(json!({"mc_percent": 11}));
    let s = r.until(|s| s.progress == Some(0.11), "11%").await;
    assert_eq!(s.status, PrinterStatus::Printing);
    assert_eq!(s.job.unwrap().name, "lamp_shade");
}

#[tokio::test]
async fn filament_running_out_needs_a_hand_and_names_the_code() {
    let r = rig().await;
    r.fake
        .set(json!({"gcode_state": "PAUSE", "print_error": 0x0700_8011u64}));
    let s = r
        .until(|s| s.status == PrinterStatus::NeedsHand, "needing a hand")
        .await;
    assert_eq!(s.error.as_deref(), Some("Bambu error 0700_8011"));
}

#[tokio::test]
async fn the_certificate_is_pinned_on_first_contact() {
    let r = rig().await;
    assert_eq!(r.pins.pinned(SERIAL), Some(r.fake.fingerprint()));
}

#[tokio::test]
async fn a_different_certificate_is_refused_and_shown_not_just_offline() {
    let r = rig().await;
    let pinned = r.fake.fingerprint();
    r.fake.rotate_certificate();
    r.fake.drop_connections();
    let s = r
        .until(
            |s| {
                s.error
                    .as_deref()
                    .is_some_and(|e| e.contains("certificate changed"))
            },
            "refusing the new certificate",
        )
        .await;
    assert_eq!(s.status, PrinterStatus::Offline);
    assert_eq!(
        r.pins.pinned(SERIAL),
        Some(pinned),
        "the pin stayed as it was"
    );
    assert_eq!(r.fake.connections(), 1, "no sign-in went to the impostor");
}

#[tokio::test]
async fn a_wrong_access_code_is_shown() {
    let r = rig_with("87654321").await;
    let s = r
        .until(
            |s| {
                s.error
                    .as_deref()
                    .is_some_and(|e| e.contains("access code"))
            },
            "showing the refused code",
        )
        .await;
    assert_eq!(s.status, PrinterStatus::Offline);
    assert_eq!(r.fake.connections(), 0);
}

#[tokio::test]
async fn a_dropped_connection_reads_offline_then_comes_back_whole() {
    let r = rig().await;
    r.fake
        .set(json!({"gcode_state": "RUNNING", "mc_percent": 50}));
    r.until(|s| s.status == PrinterStatus::Printing, "printing")
        .await;
    r.fake.drop_connections();
    r.until(
        |s| s.status == PrinterStatus::Offline,
        "offline after the drop",
    )
    .await;
    let s = r
        .until(|s| s.status == PrinterStatus::Printing, "printing again")
        .await;
    assert_eq!(
        s.progress,
        Some(0.5),
        "pushall brought the whole state back"
    );
    assert!(r.fake.connections() >= 2);
}

#[tokio::test]
async fn a_report_that_is_not_json_is_ignored_not_fatal() {
    let r = rig().await;
    r.fake.send_garbage();
    r.fake.set(json!({"gcode_state": "RUNNING"}));
    r.until(
        |s| s.status == PrinterStatus::Printing,
        "printing after the garbage",
    )
    .await;
    assert_eq!(r.fake.connections(), 1);
}

#[tokio::test]
async fn pins_are_kept_per_printer() {
    let a = FakeBambu::start("SERIAL_A", CODE).await;
    let b = FakeBambu::start("SERIAL_B", CODE).await;
    let pins = Arc::new(MemoryPins::default());
    let mut fleet = Fleet::new(quick());
    for (id, f) in [("a", &a), ("b", &b)] {
        let (host, port, pins, serial) = (
            f.host(),
            f.port(),
            pins.clone(),
            if id == "a" { "SERIAL_A" } else { "SERIAL_B" },
        );
        fleet.add(Printer {
            id: id.into(),
            factory: Arc::new(move || {
                Box::new(Bambu {
                    port,
                    ..Bambu::new(host.clone(), serial, CODE, pins.clone())
                })
            }),
        });
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while pins.pinned("SERIAL_A").is_none() || pins.pinned("SERIAL_B").is_none() {
        assert!(tokio::time::Instant::now() < deadline, "both never pinned");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(pins.pinned("SERIAL_A"), Some(a.fingerprint()));
    assert_eq!(pins.pinned("SERIAL_B"), Some(b.fingerprint()));
    assert_ne!(a.fingerprint(), b.fingerprint());
}
