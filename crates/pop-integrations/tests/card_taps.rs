//! The card-tap listener (#19) against a broker that exists only here
//! (fakes::broker): what it subscribes to, what it hears, what it publishes
//! (nothing), and how it comes back after the broker drops it.

use std::time::Duration;

use pop_integrations::{
    card_taps::{self, Broker, LocalSignIn, Outcome, Tap},
    fakes::broker::FakeBroker,
};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_secs(10);

fn reply(resource: &str, code: &str) -> String {
    serde_json::json!({"resource": resource, "authorized": true, "reason": "authorized",
        "member": {"id": "u-1", "name": "Alex Doe"}, "sign_in": {"kind": "code", "code": code}})
    .to_string()
}

/// Wait until the listener says it hears the broker.
async fn heard(h: &mut card_taps::Hearing) {
    tokio::time::timeout(WAIT, h.wait_for(Option::is_none))
        .await
        .expect("the listener to connect in time")
        .expect("the listener to still be running");
}

/// Publish `payload` until a tap arrives, since a subscription made a moment
/// ago may not have reached the broker yet.
async fn tap_after(
    b: &FakeBroker,
    topic: &str,
    payload: &str,
    rx: &mut mpsc::Receiver<Tap>,
) -> Tap {
    tokio::time::timeout(WAIT, async {
        loop {
            b.publish(topic, payload.to_owned());
            if let Ok(Some(t)) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                return t;
            }
        }
    })
    .await
    .expect("a tap in time")
}

#[tokio::test]
async fn a_tap_is_heard_on_the_scan_replies_and_pop_publishes_nothing() {
    let b = FakeBroker::start(None).await;
    let (tx, mut rx) = mpsc::channel(8);
    let (mut h, task) = card_taps::spawn(Broker::parse(&b.url(), "", "").unwrap(), tx);
    heard(&mut h).await;
    let t = tap_after(
        &b,
        "resource/response/scan",
        &reply("tower-1", "c0de"),
        &mut rx,
    )
    .await;
    assert_eq!(t.resource, "tower-1");
    assert_eq!(
        t.outcome,
        Outcome::SignIn(LocalSignIn::Code {
            code: "c0de".into()
        })
    );
    // It asked for the scan replies and nothing else, and said nothing.
    assert_eq!(b.subscriptions(), ["resource/response/scan"]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        b.publishes().is_empty(),
        "POP published {:?}",
        b.publishes()
    );
    task.abort();
}

#[tokio::test]
async fn another_topic_is_not_a_tap() {
    let b = FakeBroker::start(None).await;
    let (tx, mut rx) = mpsc::channel(8);
    let (mut h, task) = card_taps::spawn(Broker::parse(&b.url(), "", "").unwrap(), tx);
    heard(&mut h).await;
    // The control first: the same reply on the right topic is heard...
    tap_after(
        &b,
        "resource/response/scan",
        &reply("tower-1", "a"),
        &mut rx,
    )
    .await;
    // ...and on any other, with time to arrive, it is not.
    b.publish("resource/response/stop", reply("tower-1", "b"));
    b.publish("resource/request/scan", reply("tower-1", "c"));
    assert!(
        tokio::time::timeout(Duration::from_millis(800), rx.recv())
            .await
            .is_err(),
        "a message on another topic was taken for a tap"
    );
    task.abort();
}

#[tokio::test]
async fn it_signs_in_to_the_broker_and_says_when_it_cannot() {
    let b = FakeBroker::start(Some(("pop", "right"))).await;
    let (tx, _rx) = mpsc::channel(8);
    let (mut h, task) = card_taps::spawn(Broker::parse(&b.url(), "pop", "wrong").unwrap(), tx);
    let why = tokio::time::timeout(
        WAIT,
        h.wait_for(|w| w.as_deref().is_some_and(|w| w.contains("broker"))),
    )
    .await
    .expect("a refusal reported in time")
    .unwrap()
    .clone();
    assert!(why.unwrap().contains("127.0.0.1"), "names the broker");
    assert_eq!(b.connections(), 0);
    task.abort();

    let (tx, mut rx) = mpsc::channel(8);
    let (mut h, task) = card_taps::spawn(Broker::parse(&b.url(), "pop", "right").unwrap(), tx);
    heard(&mut h).await;
    assert_eq!(b.connections(), 1);
    tap_after(
        &b,
        "resource/response/scan",
        &reply("tower-1", "c"),
        &mut rx,
    )
    .await;
    task.abort();
}

#[tokio::test]
async fn it_comes_back_after_the_broker_drops_it() {
    let b = FakeBroker::start(None).await;
    let (tx, mut rx) = mpsc::channel(8);
    let (mut h, task) = card_taps::spawn(Broker::parse(&b.url(), "", "").unwrap(), tx);
    heard(&mut h).await;
    b.drop_connections();
    tokio::time::timeout(WAIT, h.wait_for(Option::is_some))
        .await
        .expect("the drop noticed")
        .unwrap();
    heard(&mut h).await;
    assert_eq!(b.connections(), 2, "it connected again");
    assert_eq!(
        b.subscriptions(),
        ["resource/response/scan", "resource/response/scan"],
        "and subscribed again"
    );
    let t = tap_after(
        &b,
        "resource/response/scan",
        &reply("tower-1", "after"),
        &mut rx,
    )
    .await;
    assert_eq!(
        t.outcome,
        Outcome::SignIn(LocalSignIn::Code {
            code: "after".into()
        })
    );
    task.abort();
}
