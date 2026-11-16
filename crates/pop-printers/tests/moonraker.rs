//! The Moonraker driver (#27), run by a real fleet against a fake Moonraker
//! (#26) over a real WebSocket.

use std::{sync::Arc, time::Duration};

use pop_printers::{
    Fleet, Printer, PrinterState, PrinterStatus,
    fakes::moonraker::FakeMoonraker,
    fleet::{Backoff, Factory},
    moonraker::Moonraker,
};
use serde_json::json;

fn quick() -> Backoff {
    Backoff {
        first: Duration::from_millis(50),
        max: Duration::from_millis(200),
        healthy_after: Duration::from_secs(5),
    }
}

fn driver(url: String, key: Option<&str>, answer_within: Duration) -> Factory {
    let key = key.map(str::to_owned);
    Arc::new(move || {
        Box::new(Moonraker {
            answer_within,
            ..Moonraker::new(url.clone(), key.clone())
        })
    })
}

struct Rig {
    fake: FakeMoonraker,
    fleet: Fleet,
}

async fn rig() -> Rig {
    let fake = FakeMoonraker::start().await;
    let mut fleet = Fleet::new(quick());
    fleet.add(Printer {
        id: "voron".into(),
        factory: driver(fake.url(), None, Duration::from_secs(2)),
    });
    let r = Rig { fake, fleet };
    r.until(|s| s.status == PrinterStatus::Idle, "idle on connecting")
        .await;
    r
}

impl Rig {
    /// Waits up to three seconds for the printer's state to pass `ok`.
    async fn until(&self, ok: impl Fn(&PrinterState) -> bool, what: &str) -> PrinterState {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let s = self.fleet.state("voron").unwrap();
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

fn printing() -> serde_json::Value {
    json!({
        "print_stats": {"state": "printing", "filename": "benchy.gcode", "print_duration": 600.0,
                        "info": {"current_layer": 12, "total_layer": 80}},
        "display_status": {"progress": 0.25},
    })
}

#[tokio::test]
async fn a_print_reads_as_printing_with_its_progress_layer_and_time_left() {
    let r = rig().await;
    r.fake.set(printing());
    let s = r
        .until(|s| s.status == PrinterStatus::Printing, "printing")
        .await;
    assert_eq!(s.progress, Some(0.25));
    let layer = s.layer.unwrap();
    assert_eq!((layer.current, layer.total), (12, Some(80)));
    // 600 s for a quarter: 1800 s to go.
    assert_eq!(s.seconds_left, Some(1800));
    assert_eq!(s.job.unwrap().name, "benchy.gcode");
}

#[tokio::test]
async fn only_what_changed_is_sent_and_the_rest_is_kept() {
    let r = rig().await;
    r.fake.set(printing());
    r.until(|s| s.status == PrinterStatus::Printing, "printing")
        .await;
    // Moonraker sends just the progress and the layer; the file, the state
    // and the total layers stay as they were.
    r.fake.set(json!({"display_status": {"progress": 0.5}, "print_stats": {"info": {"current_layer": 40}}}));
    let s = r.until(|s| s.progress == Some(0.5), "half way").await;
    assert_eq!(s.status, PrinterStatus::Printing);
    let layer = s.layer.unwrap();
    assert_eq!((layer.current, layer.total), (40, Some(80)));
    assert_eq!(s.job.unwrap().name, "benchy.gcode");
}

#[tokio::test]
async fn each_of_klippers_states_reads_as_pop_says() {
    let r = rig().await;
    r.fake.set(printing());
    for (klipper, pop) in [
        ("paused", PrinterStatus::Paused),
        ("complete", PrinterStatus::Done),
        ("cancelled", PrinterStatus::Idle),
        ("standby", PrinterStatus::Idle),
        ("printing", PrinterStatus::Printing),
    ] {
        r.fake.set(json!({"print_stats": {"state": klipper}}));
        r.until(|s| s.status == pop, &format!("{klipper} read as {pop:?}"))
            .await;
    }
}

#[tokio::test]
async fn a_print_error_needs_a_hand_and_says_what_klipper_said() {
    let r = rig().await;
    r.fake.set(json!({"print_stats": {"state": "error", "message": "Heater extruder not heating at expected rate"}}));
    let s = r
        .until(|s| s.status == PrinterStatus::NeedsHand, "needing a hand")
        .await;
    assert_eq!(
        s.error.as_deref(),
        Some("Heater extruder not heating at expected rate")
    );
}

#[tokio::test]
async fn klipper_shut_down_needs_a_hand_without_dropping_the_connection() {
    let r = rig().await;
    r.fake.set(json!({"webhooks": {"state": "shutdown", "state_message": "MCU 'mcu' shutdown: Timer too close"}}));
    let s = r
        .until(|s| s.status == PrinterStatus::NeedsHand, "needing a hand")
        .await;
    assert_eq!(
        s.error.as_deref(),
        Some("MCU 'mcu' shutdown: Timer too close")
    );
    r.fake.notify("notify_klippy_shutdown");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        r.fake.connections(),
        1,
        "a shutdown is a state, not a reason to reconnect"
    );
}

#[tokio::test]
async fn klipper_starting_up_reads_as_offline() {
    let r = rig().await;
    r.fake.set(json!({"webhooks": {"state": "startup"}}));
    r.until(|s| s.status == PrinterStatus::Offline, "offline")
        .await;
}

#[tokio::test]
async fn a_dropped_connection_reads_offline_then_comes_back_where_it_was() {
    let r = rig().await;
    r.fake.set(printing());
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
        Some(0.25),
        "the fresh subscription brought the whole state back"
    );
    assert!(r.fake.connections() >= 2);
}

#[tokio::test]
async fn klipper_disconnecting_from_moonraker_starts_over() {
    let r = rig().await;
    r.fake.notify("notify_klippy_disconnected");
    r.until(|_| r.fake.connections() >= 2, "a fresh connection")
        .await;
}

#[tokio::test]
async fn a_message_that_is_not_json_is_ignored_not_fatal() {
    let r = rig().await;
    r.fake.send_garbage();
    r.fake.set(printing());
    r.until(
        |s| s.status == PrinterStatus::Printing,
        "printing after the garbage",
    )
    .await;
    assert_eq!(r.fake.connections(), 1);
}

#[tokio::test]
async fn a_refused_subscription_reads_offline_and_is_tried_again() {
    let fake = FakeMoonraker::start().await;
    fake.refuse_subscriptions(true);
    let mut fleet = Fleet::new(quick());
    fleet.add(Printer {
        id: "voron".into(),
        factory: driver(fake.url(), None, Duration::from_secs(2)),
    });
    let r = Rig { fake, fleet };
    r.until(|_| r.fake.connections() >= 3, "three tries").await;
    assert_eq!(
        r.fleet.state("voron").unwrap().status,
        PrinterStatus::Offline
    );
    r.fake.refuse_subscriptions(false);
    r.until(|s| s.status == PrinterStatus::Idle, "idle once it accepts")
        .await;
}

#[tokio::test]
async fn a_subscription_never_answered_is_given_up_on_and_tried_again() {
    let fake = FakeMoonraker::start().await;
    fake.answer_slowly(Duration::from_secs(30));
    let mut fleet = Fleet::new(quick());
    fleet.add(Printer {
        id: "voron".into(),
        factory: driver(fake.url(), None, Duration::from_millis(200)),
    });
    let r = Rig { fake, fleet };
    r.until(
        |_| r.fake.connections() >= 2,
        "a second try after the first went unanswered",
    )
    .await;
    assert_eq!(
        r.fleet.state("voron").unwrap().status,
        PrinterStatus::Offline
    );
}

#[tokio::test]
async fn an_api_key_is_sent_when_the_printer_needs_one() {
    let fake = FakeMoonraker::start().await;
    fake.require_api_key("s3cret");
    let mut fleet = Fleet::new(quick());
    fleet.add(Printer {
        id: "voron".into(),
        factory: driver(fake.url(), Some("s3cret"), Duration::from_secs(2)),
    });
    fleet.add(Printer {
        id: "keyless".into(),
        factory: driver(fake.url(), None, Duration::from_secs(2)),
    });
    let r = Rig { fake, fleet };
    r.until(|s| s.status == PrinterStatus::Idle, "idle with the key")
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        r.fleet.state("keyless").unwrap().status,
        PrinterStatus::Offline
    );
}
