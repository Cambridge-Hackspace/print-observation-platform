//! The public live stream (#134). `/api/events` is reachable by anyone, from
//! anywhere (#7), so what it carries is pinned here field by field: a new
//! field fails this until someone has decided it may go out. And owners'
//! names go out only as `[public] owner_names` says, while signed-in screens
//! and the space's own network see them.

use std::{collections::BTreeSet, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, header},
};
use http_body_util::BodyExt;
use pop_printers::{Job, Layer, PrinterError, PrinterState, PrinterStatus};
use pop_server::{
    AppState,
    api::{GroupNotice, Role, ServerEvent, Snapshot, Status, TowerPrinter},
    assets::Embedded,
    audit::Actor,
    config::{Access, Public},
    db, router,
    users::{self, NewAccount},
};
use serde_json::Value;
use tokio::sync::watch;
use tower::ServiceExt;

/// A printer with every field the stream can carry filled in.
fn printer(owner: &str) -> TowerPrinter {
    TowerPrinter {
        id: "p1".into(),
        name: "Voron 2.4".into(),
        state: PrinterState {
            status: PrinterStatus::NeedsHand,
            progress: Some(0.5),
            layer: Some(Layer {
                current: 3,
                total: Some(10),
            }),
            seconds_left: Some(600),
            job: Some(Job {
                name: "benchy.gcode".into(),
                owner: Some(owner.into()),
            }),
            error: Some(PrinterError {
                words: "the nozzle is clogged".into(),
                raw: "E123".into(),
                known: true,
                key: Some("klipper.clog".into()),
            }),
            held: true,
        },
        watched: true,
        materials: vec!["PLA".into()],
        training: Some("basic".into()),
    }
}

fn notice() -> GroupNotice {
    GroupNotice {
        printer: "p1".into(),
        text: "Voron 2.4 needs a hand: the nozzle is clogged. Sam's print is paused.".into(),
        at: 1,
        without_owner: "Voron 2.4 needs a hand: the nozzle is clogged.".into(),
    }
}

/// Every key path in `v`: `a.b`, with `[]` for each array's items.
fn paths(v: &Value, at: &str, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(o) => {
            for (k, v) in o {
                let p = if at.is_empty() {
                    k.clone()
                } else {
                    format!("{at}.{k}")
                };
                out.insert(p.clone());
                paths(v, &p, out);
            }
        }
        Value::Array(a) => {
            for v in a {
                paths(v, &format!("{at}[]"), out);
            }
        }
        _ => {}
    }
}

/// Every field the public stream carries, as reviewed. Written out here, not
/// derived from the types: a field added to them fails this until it is
/// added here, by someone who has decided it may go out to anyone.
const REVIEWED: &[&str] = &[
    "type",
    "data",
    // The server's status.
    "data.status",
    "data.status.version",
    "data.status.schema_version",
    "data.status.public_url",
    // Each printer.
    "data.printers",
    "data.printers[].id",
    "data.printers[].name",
    "data.printers[].watched",
    "data.printers[].materials",
    "data.printers[].training",
    "data.printers[].state",
    "data.printers[].state.status",
    "data.printers[].state.progress",
    "data.printers[].state.layer",
    "data.printers[].state.layer.current",
    "data.printers[].state.layer.total",
    "data.printers[].state.seconds_left",
    "data.printers[].state.held",
    "data.printers[].state.job",
    "data.printers[].state.job.name",
    // Whose print it is: named only as [public] owner_names says.
    "data.printers[].state.job.owner",
    "data.printers[].state.error",
    "data.printers[].state.error.words",
    "data.printers[].state.error.raw",
    "data.printers[].state.error.known",
    "data.printers[].state.error.key",
    // What the group chat was told.
    "data.told",
    "data.told[].printer",
    "data.told[].text",
    "data.told[].at",
];

#[test]
fn the_public_stream_carries_only_reviewed_fields() {
    let status = Status {
        version: "0.1.0".into(),
        schema_version: 10,
        public_url: Some("https://pop.example.org".into()),
    };
    let events = [
        ServerEvent::Snapshot(Snapshot {
            status: status.clone(),
            printers: vec![printer("Sam Lee")],
            told: vec![notice()],
        }),
        ServerEvent::Status(status),
        ServerEvent::Printers(vec![printer("Sam Lee")]),
        ServerEvent::Told(vec![notice()]),
    ];
    let mut seen = BTreeSet::new();
    for e in &events {
        let v = serde_json::to_value(e).unwrap();
        let mut p = BTreeSet::new();
        paths(&v, "", &mut p);
        // Each kind's fields sit under data as the snapshot's do.
        for path in p {
            let path = path
                .replace("data[]", "data.printers[]")
                .replace("data.version", "data.status.version")
                .replace("data.schema_version", "data.status.schema_version")
                .replace("data.public_url", "data.status.public_url");
            seen.insert(path);
        }
    }
    // The told list's own paths, from a told event, read as the snapshot's.
    let told = serde_json::to_value(ServerEvent::Told(vec![notice()])).unwrap();
    for k in told["data"][0].as_object().unwrap().keys() {
        seen.remove(&format!("data.printers[].{k}"));
        seen.insert(format!("data.told[].{k}"));
    }
    let reviewed: BTreeSet<String> = REVIEWED.iter().map(|s| (*s).to_owned()).collect();
    let unreviewed: Vec<_> = seen.difference(&reviewed).collect();
    let gone: Vec<_> = reviewed.difference(&seen).collect();
    assert!(
        unreviewed.is_empty(),
        "the public stream carries fields no one has reviewed: {unreviewed:?}"
    );
    assert!(
        gone.is_empty(),
        "reviewed fields it no longer carries: {gone:?}"
    );
    // What a notice says without anyone in it is never sent itself.
    assert!(!told.to_string().contains("without_owner"));
}

// ---- whose names go out --------------------------------------------------------

const LOCAL: &str = "192.168.1.20:5000";
const REMOTE: &str = "203.0.113.9:5000";

struct Pop {
    _dir: tempfile::TempDir,
    app: Router,
    member: String,
    printers: watch::Sender<Vec<TowerPrinter>>,
    state: AppState,
}

async fn pop(owner_names: bool) -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    users::create(
        &db,
        NewAccount {
            username: "kim",
            name: "Kim",
            role: Role::Member,
            password: "kim's password",
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    let (printers, tower) = watch::channel(vec![printer("Sam Lee")]);
    let (_told_tx, told) = watch::channel(vec![notice()]);
    std::mem::forget(_told_tx);
    let state = AppState::new(db, Arc::new(Embedded))
        .with_access(Access {
            local_networks: vec!["192.168.1.0/24".parse().unwrap()],
            trusted_proxies: vec![],
        })
        .with_public(Public { owner_names })
        .with_tower(tower)
        .with_told(told);
    let app = router(state.clone());
    let mut req = Request::post("/api/session")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"username":"kim","password":"kim's password"}"#,
        ))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(LOCAL.parse::<SocketAddr>().unwrap()));
    let res = app.clone().oneshot(req).await.unwrap();
    let member = res.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    Pop {
        _dir: dir,
        app,
        member,
        printers,
        state,
    }
}

impl Pop {
    /// Open the stream from `from`, as `cookie`: the snapshot, then one more
    /// event after a printer changes, each as its JSON text.
    async fn watch(&self, from: &str, cookie: Option<&str>) -> (String, String) {
        let mut req = Request::get("/api/events");
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        let mut req = req.body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(from.parse::<SocketAddr>().unwrap()));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let mut body = res.into_body();
        let mut next = async || loop {
            let f = tokio::time::timeout(Duration::from_secs(5), body.frame())
                .await
                .expect("an event in time")
                .unwrap()
                .unwrap();
            if let Some(d) = f.data_ref() {
                let t = String::from_utf8_lossy(d).into_owned();
                if t.contains("data:") {
                    return t;
                }
            }
        };
        let snapshot = next().await;
        // A later change, published as it happens.
        let mut p = printer("Sam Lee");
        p.state.progress = Some(0.6);
        self.printers.send_replace(vec![p.clone()]);
        self.state.events.publish(ServerEvent::Printers(vec![p]));
        let later = next().await;
        (snapshot, later)
    }
}

#[tokio::test]
async fn with_owner_names_off_the_public_sees_nobody_named() {
    let pop = pop(false).await;
    let (snapshot, later) = pop.watch(REMOTE, None).await;
    for (what, t) in [("snapshot", &snapshot), ("later", &later)] {
        assert!(!t.contains("Sam"), "{what}: {t}");
        assert!(!t.contains("Lee"), "{what}: {t}");
    }
    // The precondition: what it is sent does carry the job and the notice.
    assert!(snapshot.contains("benchy.gcode"), "{snapshot}");
    assert!(
        snapshot.contains("Voron 2.4 needs a hand: the nozzle is clogged."),
        "{snapshot}"
    );
    assert!(later.contains("benchy.gcode"), "{later}");
}

#[tokio::test]
async fn with_owner_names_on_the_public_sees_first_names_only() {
    let pop = pop(true).await;
    let (snapshot, later) = pop.watch(REMOTE, None).await;
    for (what, t) in [("snapshot", &snapshot), ("later", &later)] {
        assert!(t.contains(r#""owner":"Sam""#), "{what}: {t}");
        assert!(!t.contains("Lee"), "{what}: {t}");
    }
    assert!(snapshot.contains("Sam's print is paused."), "{snapshot}");
}

#[tokio::test]
async fn signed_in_or_at_the_space_names_are_seen_either_way() {
    for owner_names in [false, true] {
        let pop = pop(owner_names).await;
        let member = pop.member.clone();
        for (who, from, cookie) in [
            ("a member from anywhere", REMOTE, Some(member.as_str())),
            ("the space's own network", LOCAL, None),
        ] {
            let (snapshot, later) = pop.watch(from, cookie).await;
            for (what, t) in [("snapshot", &snapshot), ("later", &later)] {
                assert!(t.contains(r#""owner":"Sam Lee""#), "{who}, {what}: {t}");
            }
            assert!(
                snapshot.contains("Sam's print is paused."),
                "{who}: {snapshot}"
            );
        }
    }
}

#[tokio::test]
async fn a_session_that_is_not_one_is_the_public() {
    let pop = pop(false).await;
    let (snapshot, _) = pop.watch(REMOTE, Some("pop_session=not-a-session")).await;
    assert!(!snapshot.contains("Sam"), "{snapshot}");
}

/// A screen that reconnects is sent the events it missed from the replay
/// buffer, not a snapshot: those are named as the connection may see, too.
#[tokio::test]
async fn what_a_reconnecting_screen_missed_is_named_as_it_may_see() {
    let pop = pop(false).await;
    let (snapshot, _) = pop.watch(REMOTE, None).await;
    let id = snapshot
        .lines()
        .find_map(|l| l.strip_prefix("id:"))
        .expect("an event id")
        .trim()
        .to_owned();
    // Something it misses while away.
    pop.state
        .events
        .publish(ServerEvent::Printers(vec![printer("Sam Lee")]));
    let mut req = Request::get("/api/events")
        .header("last-event-id", &id)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(REMOTE.parse::<SocketAddr>().unwrap()));
    let res = pop.app.clone().oneshot(req).await.unwrap();
    let mut body = res.into_body();
    let mut first = String::new();
    while !first.contains("data:") {
        let f = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Some(d) = f.data_ref() {
            first.push_str(&String::from_utf8_lossy(d));
        }
    }
    // The precondition: it was replayed, not a snapshot.
    assert!(first.contains(r#""type":"printers""#), "{first}");
    assert!(!first.contains("Sam"), "{first}");
}
