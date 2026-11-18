//! The printers' cameras through go2rtc (#66): POP keeps go2rtc serving one
//! stream per tower printer with a camera, as BAM! has them, and says whether
//! each works. Against a fake BAM! and a fake go2rtc, each serving answers
//! recorded from the real one.
//!
//! What this does not prove: that a real camera's stream plays. The fake
//! go2rtc says a camera works when the test says so; the recording is what
//! ties its answers to go2rtc's.

use std::{collections::BTreeMap, net::SocketAddr, sync::Arc, time::Duration};

use ::tower::ServiceExt as _;
use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use pop_integrations::{
    bam::Bam,
    fakes::{
        bam::{FakeBam, TOKEN},
        go2rtc::{self as fake_go2rtc, FakeGo2rtc},
    },
    go2rtc::Go2rtc,
    printers::FieldMap,
    sync,
};
use pop_printers::fleet::Backoff;
use pop_server::{
    AppState,
    api::{AdminCamera, CameraHealth, Role, TowerState},
    assets::Embedded,
    audit::Actor,
    cameras::{self, stream_name},
    db,
    pins::DbPins,
    router, run,
    secrets::Secrets,
    tower,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::watch;

const BAM: &str = include_str!("../../pop-integrations/tests/recordings/v0.20.0/recordings.json");
const GO2RTC: &str =
    include_str!("../../pop-integrations/tests/recordings/go2rtc-1.9.15/recordings.json");

/// The Voron's camera, password and all, as BAM! would hold it.
const VORON_CAMERA: &str = "rtsp://viewer:cam-s3cret@10.0.0.40:554/stream1";
const LOCAL: &str = "127.0.0.1:4000";

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    bam: FakeBam,
    go2rtc: FakeGo2rtc,
    health: watch::Receiver<Vec<AdminCamera>>,
    decided: Arc<tokio::sync::Notify>,
    admin: String,
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

/// POP with the Voron added to the tower and given a camera in BAM!, and
/// go2rtc as `go2rtc` sets it up.
async fn pop_with(setup: impl FnOnce(&FakeGo2rtc)) -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    users::create(
        &db,
        NewAccount {
            username: "ada",
            name: "Ada",
            role: Role::Admin,
            password: "ada's password",
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    let bam = FakeBam::start(BAM).await;
    bam.edit_without_event(bam.id("voron"), "camera", VORON_CAMERA);
    decide(&db, bam.id("voron"), TowerState::Added).await;
    let go2rtc = FakeGo2rtc::start(GO2RTC).await;
    setup(&go2rtc);
    let (view, _) = sync::spawn(
        Bam::new(bam.url(), TOKEN).unwrap(),
        bam.id("printers").to_owned(),
        sync::Options {
            first_retry: Duration::from_millis(50),
            ..sync::Options::default()
        },
    );
    let client = Go2rtc::new(
        go2rtc.url(),
        Some((
            fake_go2rtc::USER.to_owned(),
            fake_go2rtc::PASSWORD.to_owned(),
        )),
    )
    .unwrap();
    let (wanted_tx, wanted) = watch::channel(cameras::Wanted::new());
    let (health, _) = cameras::spawn(
        client.clone(),
        wanted,
        cameras::Options {
            every: Duration::from_millis(100),
        },
    );
    let state = AppState::new(db.clone(), Arc::new(Embedded));
    let decided = state.decided.clone();
    let (tower, _) = run::spawn(run::Runner {
        db: db.clone(),
        secrets: Secrets::none(),
        fields: FieldMap::default(),
        view,
        events: state.events.clone(),
        decided: decided.clone(),
        pins: Arc::new(DbPins::load(db.clone()).await.unwrap()),
        backoff: Backoff {
            first: Duration::from_millis(50),
            max: Duration::from_millis(200),
            healthy_after: Duration::from_secs(60),
        },
        cameras: wanted_tx,
    });
    let state = state.with_tower(tower).with_cameras(health.clone(), client);
    let mut pop = Pop {
        _dir: dir,
        db,
        app: router(state),
        bam,
        go2rtc,
        health,
        decided,
        admin: String::new(),
    };
    let (_, _, set, _) = pop
        .send(
            Method::POST,
            "/api/session",
            None,
            Some(json!({"username": "ada", "password": "ada's password"})),
        )
        .await;
    pop.admin = set.unwrap();
    pop
}

async fn pop() -> Pop {
    pop_with(|_| {}).await
}

impl Pop {
    async fn send(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value, Option<String>, String) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        let mut req = match body {
            Some(b) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(LOCAL.parse::<SocketAddr>().unwrap()));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let set = res
            .headers()
            .get(header::SET_COOKIE)
            .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_owned());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let raw = String::from_utf8_lossy(&bytes).into_owned();
        (
            code,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            set,
            raw,
        )
    }

    fn voron(&self) -> String {
        stream_name(self.bam.id("voron"))
    }

    /// Wait until go2rtc's streams are `ok`.
    async fn streams_until(
        &self,
        what: &str,
        ok: impl Fn(&BTreeMap<String, Vec<String>>) -> bool,
    ) -> BTreeMap<String, Vec<String>> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let s = self.go2rtc.streams();
            if ok(&s) {
                return s;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "never {what}: {s:#?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Wait until the Voron's camera reads `health`.
    async fn health_until(&self, health: CameraHealth) -> AdminCamera {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let v = self.health.borrow().clone();
            if let Some(c) = v.iter().find(|c| c.name == "Voron 2.4")
                && c.health == health
            {
                return c.clone();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the Voron's camera never read {health:?}: {v:#?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[tokio::test]
async fn a_tower_printers_camera_is_served_by_go2rtc_as_bam_has_it() {
    let pop = pop().await;
    let voron = pop.voron();
    let s = pop
        .streams_until("served the Voron's camera", |s| s.contains_key(&voron))
        .await;
    assert_eq!(s[&voron], [VORON_CAMERA]);
    // Only the Voron is added; the X1, which has a camera in BAM!, is not.
    assert_eq!(s.len(), 1, "{s:#?}");

    // Its camera changes in BAM!: go2rtc follows.
    pop.bam
        .edit(pop.bam.id("voron"), "camera", "http://10.0.0.41/mjpeg");
    pop.streams_until("followed the new camera", |s| {
        s.get(&voron)
            .is_some_and(|v| v == &["http://10.0.0.41/mjpeg"])
    })
    .await;

    // Its camera is taken out of BAM!: so is the stream.
    pop.bam.edit(pop.bam.id("voron"), "camera", " ");
    pop.streams_until("removed the stream", |s| s.is_empty())
        .await;
}

#[tokio::test]
async fn a_printer_that_leaves_the_tower_leaves_go2rtc() {
    let pop = pop().await;
    let voron = pop.voron();
    pop.streams_until("served the Voron", |s| s.contains_key(&voron))
        .await;
    decide(&pop.db, pop.bam.id("voron"), TowerState::Declined).await;
    pop.decided.notify_one();
    pop.streams_until("removed the Voron", |s| s.is_empty())
        .await;
}

#[tokio::test]
async fn streams_that_are_not_pops_are_left_alone_and_stale_ones_of_pops_go() {
    let pop = pop_with(|g| {
        g.add("front_door", "rtsp://10.0.0.2/door");
        g.add("pop_a-printer-long-gone", "rtsp://10.0.0.3/old");
    })
    .await;
    let voron = pop.voron();
    let s = pop
        .streams_until("kept go2rtc's own and dropped the stale one", |s| {
            s.contains_key(&voron) && !s.contains_key("pop_a-printer-long-gone")
        })
        .await;
    assert_eq!(s["front_door"], ["rtsp://10.0.0.2/door"]);
    assert_eq!(s.len(), 2, "{s:#?}");
}

/// go2rtc started from a fresh configuration has lost POP's streams; the next
/// look puts them back, with nothing changed in BAM!.
#[tokio::test]
async fn a_go2rtc_that_lost_its_streams_gets_them_back() {
    let pop = pop().await;
    let voron = pop.voron();
    pop.streams_until("served the Voron", |s| s.contains_key(&voron))
        .await;
    pop.go2rtc.forget();
    pop.streams_until("put the Voron back", |s| s.contains_key(&voron))
        .await;
}

#[tokio::test]
async fn each_cameras_health_is_what_go2rtc_shows_and_admins_see_it() {
    let pop = pop().await;
    pop.health_until(CameraHealth::NoPicture).await;
    pop.go2rtc.working(VORON_CAMERA, true);
    let c = pop.health_until(CameraHealth::Live).await;
    assert_eq!(c.stream, pop.voron());
    assert_eq!(c.bam_id, pop.bam.id("voron"));

    let (code, body, _, raw) = pop
        .send(Method::GET, "/api/admin/cameras", Some(&pop.admin), None)
        .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body[0]["name"], "Voron 2.4");
    assert_eq!(body[0]["health"], "live");
    // Never the camera's address: it holds the camera's password.
    assert!(!raw.contains("cam-s3cret"), "{raw}");
    assert!(!raw.contains("10.0.0.40"), "{raw}");

    pop.go2rtc.working(VORON_CAMERA, false);
    pop.health_until(CameraHealth::NoPicture).await;

    // Not for anyone signed out.
    let (code, ..) = pop
        .send(Method::GET, "/api/admin/cameras", None, None)
        .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn health_says_whether_go2rtc_answers_and_lets_pop_in() {
    let pop = pop().await;
    let (code, body, ..) = pop.send(Method::GET, "/healthz", None, None).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["checks"]["go2rtc"]["ok"], true);
    pop.go2rtc.stop();
    let (code, body, ..) = pop.send(Method::GET, "/healthz", None, None).await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["checks"]["go2rtc"]["error"], "go2rtc does not answer");
}

#[tokio::test]
async fn a_wrong_go2rtc_password_is_said_and_cameras_read_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let go2rtc = FakeGo2rtc::start(GO2RTC).await;
    let client = Go2rtc::new(go2rtc.url(), Some(("pop".into(), "wrong".into()))).unwrap();
    let mut wanted = cameras::Wanted::new();
    wanted.insert(
        "p1".into(),
        cameras::Camera {
            name: "A printer".into(),
            source: VORON_CAMERA.into(),
        },
    );
    let (_tx, rx) = watch::channel(wanted);
    let (mut health, _) = cameras::spawn(client.clone(), rx, cameras::Options::default());
    health.changed().await.unwrap();
    let c = health.borrow().clone();
    assert_eq!(c[0].health, CameraHealth::Unknown);
    assert!(
        c[0].detail
            .as_deref()
            .unwrap_or_default()
            .contains("user name and password"),
        "{c:#?}"
    );
    assert!(go2rtc.streams().is_empty(), "nothing went in");
    let state = AppState::new(db, Arc::new(Embedded)).with_cameras(health, client);
    let app = router(state);
    let res = app
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        body["checks"]["go2rtc"]["error"],
        "go2rtc refuses POP's user name and password"
    );
}
