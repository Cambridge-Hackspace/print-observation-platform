//! "Something looks wrong" (#63): a member watching flags a problem, the
//! group chat is told, and the printer is never touched. Against a fake
//! BAM!, a fake Moonraker printer that keeps every method it is asked, and a
//! fake Discord.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use ::tower::ServiceExt as _;
use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, State},
    http::{Method, Request, StatusCode, header},
    routing::post,
};
use http_body_util::BodyExt;
use pop_integrations::{
    bam::Bam,
    fakes::bam::{FakeBam, TOKEN},
    printers::FieldMap,
    sync,
};
use pop_printers::{fakes::moonraker::FakeMoonraker, fleet::Backoff};
use pop_server::{
    AppState,
    api::{Role, TowerPrinter, TowerState},
    assets::Embedded,
    audit::Actor,
    db,
    notify::{self, WEBHOOK},
    pins::DbPins,
    report::{PER_MEMBER_PER_HOUR, message, plain},
    router, run,
    secrets::Secrets,
    tower,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::watch;

const RECORDING: &str =
    include_str!("../../pop-integrations/tests/recordings/v0.20.0/recordings.json");
const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

#[derive(Clone, Default)]
struct Discord(Arc<Mutex<Vec<Value>>>);

impl Discord {
    async fn start() -> (Self, String) {
        let d = Discord::default();
        let app = Router::new()
            .route(
                "/api/webhooks/{id}/{token}",
                post(|State(d): State<Discord>, body: String| async move {
                    d.0.lock()
                        .unwrap()
                        .push(serde_json::from_str(&body).unwrap());
                    StatusCode::NO_CONTENT
                }),
            )
            .with_state(d.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (
            d,
            format!("http://{addr}/api/webhooks/1234567890123/fake-token-abcd"),
        )
    }

    fn texts(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|v| v["content"].as_str().unwrap().to_owned())
            .collect()
    }
}

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    _bam: FakeBam,
    printer: FakeMoonraker,
    discord: Discord,
    voron: String,
    kim: String,
}

async fn signed_in(app: &Router, username: &str) -> String {
    let mut req = Request::post("/api/session")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"username": username, "password": format!("{username}'s password")}).to_string(),
        ))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:4000".parse::<SocketAddr>().unwrap()));
    let res = app.clone().oneshot(req).await.unwrap();
    res.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn pop_with(webhook: bool) -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    users::create(
        &db,
        NewAccount {
            username: "kim",
            name: "Kim Okafor",
            role: Role::Member,
            password: "kim's password",
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    let bam = FakeBam::start(RECORDING).await;
    let printer = FakeMoonraker::start().await;
    let voron = bam.id("voron").to_owned();
    bam.edit_without_event(&voron, "address", &printer.url());
    tower::decide(
        &db,
        &voron,
        "Voron 2.4",
        TowerState::Added,
        &Actor::command_line(),
        "test".into(),
    )
    .await
    .unwrap();
    let (view, _) = sync::spawn(
        Bam::new(bam.url(), TOKEN).unwrap(),
        bam.id("printers").to_owned(),
        sync::Options {
            first_retry: Duration::from_millis(50),
            ..sync::Options::default()
        },
    );
    let secrets = Secrets::new(KEY);
    let (discord, url) = Discord::start().await;
    if webhook {
        secrets.set_setting(&db, WEBHOOK, &url).await.unwrap();
    }
    let state = AppState::new(db.clone(), Arc::new(Embedded)).with_secrets(secrets.clone());
    let (tower_rx, _) = run::spawn(run::Runner {
        db: db.clone(),
        secrets: secrets.clone(),
        fields: FieldMap::default(),
        view,
        events: state.events.clone(),
        decided: state.decided.clone(),
        pins: Arc::new(DbPins::load(db.clone()).await.unwrap()),
        backoff: Backoff {
            first: Duration::from_millis(50),
            max: Duration::from_millis(200),
            healthy_after: Duration::from_secs(60),
        },
        cameras: None,
    });
    until_on_tower(&tower_rx).await;
    let (notifier, _) = notify::spawn(
        db.clone(),
        secrets,
        reqwest::Client::new(),
        notify::Options::default(),
    );
    let state = state.with_tower(tower_rx).with_notifier(notifier);
    let app = router(state);
    let kim = signed_in(&app, "kim").await;
    Pop {
        _dir: dir,
        db,
        app,
        _bam: bam,
        printer,
        discord,
        voron,
        kim,
    }
}

async fn until_on_tower(rx: &watch::Receiver<Vec<TowerPrinter>>) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !rx.borrow().iter().any(|p| p.name == "Voron 2.4") {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the Voron never joined the tower"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn pop() -> Pop {
    pop_with(true).await
}

impl Pop {
    async fn report(&self, printer: &str, cookie: Option<&str>, note: &str) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/printers/{printer}/report"))
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        let mut req = req
            .body(Body::from(json!({"note": note}).to_string()))
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(
            "203.0.113.9:4000".parse::<SocketAddr>().unwrap(),
        ));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn discord_has(&self, n: usize) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let t = self.discord.texts();
            if t.len() >= n {
                return t;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "Discord never got {n}: {t:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[tokio::test]
async fn a_report_tells_the_group_and_is_recorded() {
    let pop = pop().await;
    let (code, body) = pop
        .report(&pop.voron, Some(&pop.kim), "smoke from the hotend")
        .await;
    assert_eq!(code, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body, json!({"told": true}));
    let said = pop.discord_has(1).await;
    assert_eq!(
        said,
        [
            "Kim, watching, thinks something looks wrong with Voron 2.4: “smoke from the hotend”. Could someone at the space check it?"
        ]
    );
    let row: (String, Option<String>, String) = sqlx::query_as(
        "SELECT actor_name, printer, summary FROM audit WHERE kind = 'problem_reported'",
    )
    .fetch_one(&pop.db)
    .await
    .unwrap();
    assert_eq!(row.0, "Kim Okafor");
    assert_eq!(row.1.as_deref(), Some(pop.voron.as_str()));
    assert!(row.2.contains("smoke from the hotend"), "{}", row.2);
}

/// Two oracles: the route sends the printer nothing, and the printer was
/// asked nothing new.
#[tokio::test]
async fn a_report_never_touches_the_printer() {
    let pop = pop().await;
    let before = pop.printer.asked();
    assert!(!before.is_empty(), "the driver is talking to the printer");
    pop.report(&pop.voron, Some(&pop.kim), "it looks stuck")
        .await;
    pop.discord_has(1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        pop.printer.asked(),
        before,
        "the printer was asked something new"
    );
}

#[tokio::test]
async fn a_printer_flagged_a_moment_ago_is_not_posted_about_again() {
    let pop = pop().await;
    assert_eq!(
        pop.report(&pop.voron, Some(&pop.kim), "").await.1,
        json!({"told": true})
    );
    assert_eq!(
        pop.report(&pop.voron, Some(&pop.kim), "still").await.1,
        json!({"told": false})
    );
    pop.discord_has(1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(pop.discord.texts().len(), 1);
}

#[tokio::test]
async fn one_member_flags_no_more_than_the_hour_allows() {
    let pop = pop().await;
    for _ in 0..PER_MEMBER_PER_HOUR {
        assert_eq!(
            pop.report(&pop.voron, Some(&pop.kim), "").await.0,
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(
        pop.report(&pop.voron, Some(&pop.kim), "").await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn only_members_flag_and_only_a_printer_on_the_tower() {
    let pop = pop().await;
    assert_eq!(
        pop.report(&pop.voron, None, "").await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        pop.report("no-such-printer", Some(&pop.kim), "").await.0,
        StatusCode::NOT_FOUND
    );
    let long = "x".repeat(201);
    assert_eq!(
        pop.report(&pop.voron, Some(&pop.kim), &long).await.0,
        StatusCode::BAD_REQUEST
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(pop.discord.texts().is_empty());
}

#[tokio::test]
async fn without_a_group_chat_a_report_says_so() {
    let pop = pop_with(false).await;
    let (code, body) = pop.report(&pop.voron, Some(&pop.kim), "").await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(
        body["error"].as_str().unwrap().contains("webhook"),
        "{body}"
    );
}

#[test]
fn what_a_member_writes_is_posted_as_plain_text() {
    assert_eq!(
        message(
            "Kim Okafor",
            "Voron 2.4",
            "**smoke** [here](http://x.example) # big\n> quote"
        ),
        "Kim, watching, thinks something looks wrong with Voron 2.4: “\\*\\*smoke\\*\\* \\[here\\]\\(http\\://x.example\\) \\# big \\> quote”. Could someone at the space check it?"
    );
    assert_eq!(plain("a_b ~c~ `d` |e|"), "a\\_b \\~c\\~ \\`d\\` \\|e\\|");
    assert_eq!(
        message("", "Voron 2.4", "  "),
        "Someone, watching, thinks something looks wrong with Voron 2.4. Could someone at the space check it?"
    );
}
