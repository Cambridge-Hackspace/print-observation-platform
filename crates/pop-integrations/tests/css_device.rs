//! POP's CS:S device (#18) against a broker that exists only here
//! (fakes::broker), with a test playing the edge through CS:S's own seal code
//! (css_wire_sign): what POP subscribes to, what it says, which leases sign a
//! member in, and how it comes back after the broker drops it.
//!
//! What this does not prove: that CS:S's edge leases POP at all. That is the
//! edge's half, and pinned by CS:S's own e2e (`e2e/drivers/signedwire.mjs`).

use std::time::Duration;

use css_wire::{LocalAliveRequest, LocalMember, ResourceLeasePayload, local};
use css_wire_sign::{Peer, Refused};
use pop_integrations::{
    css_device::{self, Broker, Tap, Wire},
    fakes::broker::FakeBroker,
};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_secs(10);
const KEY: &str = "pop-command-key";
const ME: &str = "dev-pop";

fn wire() -> Wire {
    let mut n = 0;
    Wire::new(ME, KEY, move || {
        n += 1;
        format!("pop-{n}")
    })
}

fn lease(member: Option<(&str, &str)>) -> ResourceLeasePayload {
    ResourceLeasePayload {
        resource: "tower-uuid".into(),
        device_id: ME.into(),
        grant: true,
        ttl_ms: 3000,
        reason: None,
        member: member.map(|(id, name)| LocalMember {
            id: id.into(),
            name: name.into(),
        }),
        seal: None,
    }
}

/// Wait until POP says it hears the broker.
async fn heard(h: &mut css_device::Hearing) {
    tokio::time::timeout(WAIT, h.wait_for(Option::is_none))
        .await
        .expect("POP to connect in time")
        .expect("POP to still be running");
}

/// The edge, introduced to POP by POP's own sealed alive off the broker.
async fn edge_that_heard(b: &FakeBroker) -> Peer {
    let mut edge = Peer::new("edge-1".into());
    let alive = tokio::time::timeout(WAIT, async {
        loop {
            if let Some(p) = b.published(css_device::ALIVE).first() {
                return p.clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("an alive pulse in time");
    let a: LocalAliveRequest = serde_json::from_slice(&alive).expect("an alive pulse");
    assert_eq!(a.device_id, ME);
    assert!(a.seal.is_some(), "POP said it was alive unsigned");
    // A first message is stale to an edge that has not heard POP, and
    // teaches the edge POP's nonce.
    assert_eq!(
        edge.open(KEY.as_bytes(), &a, || "edge-2".into()),
        Err(Refused::Stale)
    );
    edge
}

/// Publish `payload` until a tap arrives, since a subscription made a moment
/// ago may not have reached the broker yet.
async fn tap_after(b: &FakeBroker, payload: Vec<u8>, rx: &mut mpsc::Receiver<Tap>) -> Tap {
    tokio::time::timeout(WAIT, async {
        loop {
            b.publish(local::RESOURCE_LEASE, payload.clone());
            if let Ok(Some(t)) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                return t;
            }
        }
    })
    .await
    .expect("a tap in time")
}

#[tokio::test]
async fn a_sealed_lease_naming_a_member_is_a_tap_and_pop_says_alive_sealed() {
    let b = FakeBroker::start(None).await;
    let (tx, mut rx) = mpsc::channel(8);
    let (mut h, task) = css_device::spawn(Broker::parse(&b.url(), "", "").unwrap(), wire(), tx);
    heard(&mut h).await;
    let mut edge = edge_that_heard(&b).await;

    let mut l = lease(Some(("u-alex", "Alex Doe")));
    edge.seal(KEY.as_bytes(), &mut l);
    // Replayed until heard, which a renewal is anyway: one tap, not one per copy.
    let t = tap_after(&b, serde_json::to_vec(&l).unwrap(), &mut rx).await;
    assert_eq!(t.resource, "tower-uuid");
    assert_eq!(t.member.id, "u-alex");
    assert_eq!(t.member.name, "Alex Doe");

    assert_eq!(
        b.subscriptions(),
        ["resource/lease", "resource/hello"],
        "leases and the edge's hello, and nothing else"
    );
    // It says only that it is alive, and keeps saying so.
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    let said = b.publishes();
    assert!(
        said.iter().all(|t| t == css_device::ALIVE),
        "POP published {said:?}"
    );
    assert!(said.len() >= 2, "one alive pulse a second: {said:?}");
    task.abort();
}

#[tokio::test]
async fn an_unsigned_lease_on_the_broker_is_no_tap() {
    let b = FakeBroker::start(None).await;
    let (tx, mut rx) = mpsc::channel(8);
    let (mut h, task) = css_device::spawn(Broker::parse(&b.url(), "", "").unwrap(), wire(), tx);
    heard(&mut h).await;
    let mut edge = edge_that_heard(&b).await;
    // The forgery first, many times, with time to arrive: no tap ...
    let forged = serde_json::to_vec(&lease(Some(("u-alex", "Alex Doe")))).unwrap();
    for _ in 0..5 {
        b.publish(local::RESOURCE_LEASE, forged.clone());
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(800), rx.recv())
            .await
            .is_err(),
        "an unsigned lease signed a member in"
    );
    // ... and the control: the same lease sealed is heard.
    let mut l = lease(Some(("u-alex", "Alex Doe")));
    edge.seal(KEY.as_bytes(), &mut l);
    tap_after(&b, serde_json::to_vec(&l).unwrap(), &mut rx).await;
    task.abort();
}

#[tokio::test]
async fn it_signs_in_to_the_broker_and_says_when_it_cannot() {
    let b = FakeBroker::start(Some(("pop", "right"))).await;
    let (tx, _rx) = mpsc::channel(8);
    let (mut h, task) =
        css_device::spawn(Broker::parse(&b.url(), "pop", "wrong").unwrap(), wire(), tx);
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

    let (tx, _rx) = mpsc::channel(8);
    let (mut h, task) =
        css_device::spawn(Broker::parse(&b.url(), "pop", "right").unwrap(), wire(), tx);
    heard(&mut h).await;
    assert_eq!(b.connections(), 1);
    task.abort();
}

#[tokio::test]
async fn it_comes_back_after_the_broker_drops_it() {
    let b = FakeBroker::start(None).await;
    let (tx, mut rx) = mpsc::channel(8);
    let (mut h, task) = css_device::spawn(Broker::parse(&b.url(), "", "").unwrap(), wire(), tx);
    heard(&mut h).await;
    let mut edge = edge_that_heard(&b).await;
    b.drop_connections();
    tokio::time::timeout(WAIT, h.wait_for(Option::is_some))
        .await
        .expect("the drop noticed")
        .unwrap();
    heard(&mut h).await;
    assert_eq!(b.connections(), 2, "it connected again");
    assert_eq!(
        b.subscriptions(),
        [
            "resource/lease",
            "resource/hello",
            "resource/lease",
            "resource/hello"
        ],
        "and subscribed again"
    );
    let mut l = lease(Some(("u-sam", "Sam Roe")));
    edge.seal(KEY.as_bytes(), &mut l);
    let t = tap_after(&b, serde_json::to_vec(&l).unwrap(), &mut rx).await;
    assert_eq!(t.member.id, "u-sam");
    task.abort();
}
