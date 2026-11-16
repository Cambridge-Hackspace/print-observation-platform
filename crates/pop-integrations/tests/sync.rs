//! Keeping printers in sync with BAM! (#22), against a fake BAM! built from a
//! real one's recorded answers and live frames.

use std::time::Duration;

use pop_integrations::{
    bam::Bam,
    fakes::bam::{FakeBam, TOKEN},
    sync::{self, Options, View},
};
use serde_json::json;
use tokio::sync::watch;

const RECORDING: &str = include_str!("recordings/v0.20.0/recordings.json");

fn quick(backstop: Duration) -> Options {
    Options {
        backstop,
        first_retry: Duration::from_millis(50),
        max_retry: Duration::from_millis(200),
        hello_within: Duration::from_secs(2),
    }
}

async fn until(rx: &watch::Receiver<View>, what: &str, ok: impl Fn(&View) -> bool) -> View {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    loop {
        let v = rx.borrow().clone();
        if ok(&v) {
            return v;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never {what}: {v:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn field(v: &View, name: &str, key: &str) -> Option<String> {
    v.printers
        .iter()
        .find(|a| a.asset.name == name)?
        .field_values
        .get(key)
        .cloned()
}

async fn running(
    backstop: Duration,
) -> (FakeBam, watch::Receiver<View>, tokio::task::JoinHandle<()>) {
    let fake = FakeBam::start(RECORDING).await;
    let (rx, task) = sync::spawn(
        Bam::new(fake.url(), TOKEN).unwrap(),
        fake.id("printers").to_owned(),
        quick(backstop),
    );
    until(&rx, "read once, live", |v| v.live && v.reads >= 1).await;
    (fake, rx, task)
}

/// The issue's "done when": a printer edited while POP is disconnected is
/// picked up when POP reconnects.
#[tokio::test]
async fn an_edit_made_while_disconnected_is_picked_up_on_reconnect() {
    let (fake, rx, _t) = running(Duration::from_secs(3600)).await;
    let voron = fake.id("voron").to_owned();
    let before = field(&rx.borrow(), "Voron 2.4", "address").unwrap();

    fake.refuse_live(true);
    fake.drop_live();
    until(&rx, "disconnected", |v| !v.live).await;
    fake.edit(&voron, "address", "ws://voron-new.local:7125/websocket");
    tokio::time::sleep(Duration::from_millis(400)).await;
    // Disconnected, the edit's event went nowhere: POP still has the old one.
    assert_eq!(field(&rx.borrow(), "Voron 2.4", "address").unwrap(), before);
    let reads_while_away = fake.category_reads();
    let opened_before = fake.live_opened();

    fake.refuse_live(false);
    let v = until(&rx, "the edit, after reconnecting", |v| {
        field(v, "Voron 2.4", "address").as_deref() == Some("ws://voron-new.local:7125/websocket")
    })
    .await;
    assert!(v.live);
    // Picked up by reading again on reconnecting: a new connection, and a
    // read after it.
    assert!(fake.live_opened() > opened_before);
    assert!(fake.category_reads() > reads_while_away);
}

#[tokio::test]
async fn an_edit_made_while_connected_arrives_by_its_event() {
    let (fake, rx, _t) = running(Duration::from_secs(3600)).await;
    let mk4 = fake.id("mk4").to_owned();
    let reads = rx.borrow().reads;
    fake.edit(&mk4, "address", "http://192.168.1.99");
    let v = until(&rx, "the MK4's new address", |v| {
        field(v, "MK4", "address").as_deref() == Some("http://192.168.1.99")
    })
    .await;
    assert!(v.reads > reads);
}

#[tokio::test]
async fn an_archived_printer_moves_to_the_archived_and_back_when_restored() {
    let (fake, rx, _t) = running(Duration::from_secs(3600)).await;
    let x1 = fake.id("x1").to_owned();
    fake.archive(&x1);
    until(&rx, "the X1 archived", |v| {
        !v.printers.iter().any(|a| a.asset.id == x1) && v.archived.iter().any(|a| a.asset.id == x1)
    })
    .await;
    fake.restore(&x1);
    until(&rx, "the X1 back", |v| {
        v.printers.iter().any(|a| a.asset.id == x1)
    })
    .await;
}

#[tokio::test]
async fn a_new_printer_appears() {
    let (fake, rx, _t) = running(Duration::from_secs(3600)).await;
    let id = fake.add(
        fake.id("mk4"),
        "MK4 number two",
        json!({"connection": "prusalink", "address": "http://192.168.1.70"}),
    );
    until(&rx, "the new printer", |v| {
        v.printers.iter().any(|a| a.asset.id == id)
    })
    .await;
}

#[tokio::test]
async fn the_backstop_catches_a_change_whose_event_never_came() {
    let (fake, rx, _t) = running(Duration::from_millis(200)).await;
    fake.edit_without_event(fake.id("mk4"), "model", "MK4S");
    until(&rx, "the change, by the backstop", |v| {
        field(v, "MK4", "model").as_deref() == Some("MK4S")
    })
    .await;
}

#[tokio::test]
async fn without_a_backstop_a_change_with_no_event_is_not_seen() {
    // The control for the test above: it is the backstop that caught it.
    let (fake, rx, _t) = running(Duration::from_secs(3600)).await;
    fake.edit_without_event(fake.id("mk4"), "model", "MK4S");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        field(&rx.borrow(), "MK4", "model").as_deref(),
        Some("Original Prusa MK4")
    );
}

#[tokio::test]
async fn a_bad_token_is_said_and_nothing_is_read() {
    let fake = FakeBam::start(RECORDING).await;
    let (rx, _t) = sync::spawn(
        Bam::new(fake.url(), "bam_wrong").unwrap(),
        fake.id("printers").to_owned(),
        quick(Duration::from_secs(3600)),
    );
    let v = until(&rx, "the refusal", |v| v.error.is_some()).await;
    assert!(!v.live && v.reads == 0);
    assert!(v.error.unwrap().contains("token"));
}
