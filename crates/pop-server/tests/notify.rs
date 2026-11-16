//! Group notices to a Discord webhook (#68), against a fake Discord that
//! answers as Discord documents: 204 on success, 429 with `retry_after` when
//! rate limited, 404 for a deleted webhook.

use std::{
    collections::VecDeque,
    io::Write,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, State},
    http::{HeaderMap, Method, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use http_body_util::BodyExt;
use pop_server::{
    AppState,
    api::Role,
    assets::Embedded,
    audit::Actor,
    db,
    notify::{self, Options, Outcome, WEBHOOK},
    router,
    secrets::Secrets,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
const REAL: &str = "https://discord.com/api/webhooks/1234567890123/AbCdEf-gh_IjKlMnOpQrStUvWxYz";
const LOCAL: &str = "127.0.0.1:4000";

/// What the fake answers next, in order; then 204 for ever after.
#[derive(Clone)]
enum Answer {
    Ok,
    Status(u16),
    /// 429, with this many seconds in the body's retry_after.
    RateLimited(f64),
}

#[derive(Clone, Default)]
struct Fake {
    answers: Arc<Mutex<VecDeque<Answer>>>,
    got: Arc<Mutex<Vec<(Instant, Value)>>>,
}

impl Fake {
    async fn start(answers: Vec<Answer>) -> (Self, String) {
        let fake = Fake {
            answers: Arc::new(Mutex::new(answers.into())),
            ..Default::default()
        };
        let app = Router::new()
            .route("/api/webhooks/{id}/{token}", post(hook))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (
            fake,
            format!("http://{addr}/api/webhooks/1234567890123/fake-token-abcd"),
        )
    }

    fn got(&self) -> Vec<(Instant, Value)> {
        self.got.lock().unwrap().clone()
    }

    fn texts(&self) -> Vec<String> {
        self.got()
            .iter()
            .map(|(_, v)| v["content"].as_str().unwrap().to_owned())
            .collect()
    }
}

async fn hook(State(f): State<Fake>, headers: HeaderMap, body: String) -> Response {
    assert_eq!(headers[header::CONTENT_TYPE], "application/json");
    f.got
        .lock()
        .unwrap()
        .push((Instant::now(), serde_json::from_str(&body).unwrap()));
    match f.answers.lock().unwrap().pop_front().unwrap_or(Answer::Ok) {
        Answer::Ok => StatusCode::NO_CONTENT.into_response(),
        Answer::Status(s) => (
            StatusCode::from_u16(s).unwrap(),
            axum::Json(json!({"message": "Unknown Webhook", "code": 10015})),
        )
            .into_response(),
        Answer::RateLimited(secs) => (
            StatusCode::TOO_MANY_REQUESTS,
            axum::Json(json!({"message": "You are being rate limited.", "retry_after": secs, "global": false})),
        )
            .into_response(),
    }
}

/// A log destination the test can read back.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Write for Logs {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Logs {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
    fn install(&self) -> tracing::subscriber::DefaultGuard {
        let sink = self.clone();
        tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(move || sink.clone())
                .finish(),
        )
    }
}

async fn database() -> (tempfile::TempDir, SqlitePool) {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    (dir, db)
}

fn quick() -> Options {
    Options {
        first_retry: Duration::from_millis(50),
        max_retry: Duration::from_millis(200),
        attempts: 4,
        timeout: Duration::from_secs(2),
    }
}

async fn until(what: &str, ok: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok() {
        assert!(Instant::now() < deadline, "never {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ---- delivery ----------------------------------------------------------------

#[tokio::test]
async fn a_notice_is_one_plain_sentence_that_can_ping_nobody() {
    let (_d, db) = database().await;
    let (fake, url) = Fake::start(vec![]).await;
    let secrets = Secrets::new(KEY);
    secrets.set_setting(&db, WEBHOOK, &url).await.unwrap();
    let (n, _) = notify::spawn(db, secrets, reqwest::Client::new(), quick());
    n.post("Ender 3 needs a hand: the filament ran out at layer 112. @everyone's print is paused.");
    until("the notice", || fake.got().len() == 1).await;
    let (_, body) = &fake.got()[0];
    assert_eq!(
        body,
        &json!({
            "content": "Ender 3 needs a hand: the filament ran out at layer 112. @everyone's print is paused.",
            "allowed_mentions": {"parse": []},
        })
    );
}

#[tokio::test]
async fn notices_arrive_in_order() {
    let (_d, db) = database().await;
    let (fake, url) = Fake::start(vec![Answer::Status(502)]).await;
    let secrets = Secrets::new(KEY);
    secrets.set_setting(&db, WEBHOOK, &url).await.unwrap();
    let (n, _) = notify::spawn(db, secrets, reqwest::Client::new(), quick());
    for i in 1..=3 {
        n.post(format!("notice {i}"));
    }
    until("three notices", || fake.texts().len() == 4).await;
    // The first failed once and was sent again before the others.
    assert_eq!(
        fake.texts(),
        ["notice 1", "notice 1", "notice 2", "notice 3"]
    );
}

#[tokio::test]
async fn a_failed_delivery_is_retried_with_a_growing_wait_and_logged() {
    let logs = Logs::default();
    let _g = logs.install();
    let (_d, db) = database().await;
    let (fake, url) = Fake::start(vec![Answer::Status(500), Answer::Status(503), Answer::Ok]).await;
    let secrets = Secrets::new(KEY);
    secrets.set_setting(&db, WEBHOOK, &url).await.unwrap();
    let (n, _) = notify::spawn(db, secrets, reqwest::Client::new(), quick());
    n.post("Mk4 is printing again.");
    until("the third try", || fake.got().len() == 3).await;
    let t: Vec<Instant> = fake.got().iter().map(|(t, _)| *t).collect();
    let (first, second) = (t[1] - t[0], t[2] - t[1]);
    assert!(first >= Duration::from_millis(50), "{first:?}");
    assert!(
        second >= Duration::from_millis(100),
        "the wait doubles: {second:?}"
    );
    let text = logs.text();
    assert!(
        text.contains("not posted yet (try 1): Discord answered 500"),
        "{text}"
    );
    assert!(
        text.contains("not posted yet (try 2): Discord answered 503"),
        "{text}"
    );
}

#[tokio::test]
async fn a_rate_limit_is_waited_out_as_discord_asks() {
    let logs = Logs::default();
    let _g = logs.install();
    let (_d, db) = database().await;
    let (fake, url) = Fake::start(vec![Answer::RateLimited(0.6)]).await;
    let secrets = Secrets::new(KEY);
    secrets.set_setting(&db, WEBHOOK, &url).await.unwrap();
    let (n, _) = notify::spawn(db, secrets, reqwest::Client::new(), quick());
    n.post("X1 needs a hand.");
    until("the retry", || fake.got().len() == 2).await;
    let t: Vec<Instant> = fake.got().iter().map(|(t, _)| *t).collect();
    // Discord's 0.6 s, not the notifier's own 50 ms.
    assert!(
        t[1] - t[0] >= Duration::from_millis(600),
        "{:?}",
        t[1] - t[0]
    );
    assert!(
        logs.text()
            .contains("rate limited by Discord; trying again in 0.6 s"),
        "{}",
        logs.text()
    );
}

#[tokio::test]
async fn giving_up_is_logged_with_the_notice_never_dropped_silently() {
    let logs = Logs::default();
    let _g = logs.install();
    let (_d, db) = database().await;
    let (fake, url) = Fake::start(vec![Answer::Status(500); 10]).await;
    let secrets = Secrets::new(KEY);
    secrets.set_setting(&db, WEBHOOK, &url).await.unwrap();
    let (n, _) = notify::spawn(db, secrets, reqwest::Client::new(), quick());
    n.post("Ender 3 is printing again.");
    until("four tries", || fake.got().len() == 4).await;
    until("giving up, in the log", || {
        logs.text().contains("gave up after 4 tries")
    })
    .await;
    assert!(
        logs.text()
            .contains("gave up after 4 tries: Ender 3 is printing again.")
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(fake.got().len(), 4, "no fifth try");
}

#[tokio::test]
async fn a_deleted_webhook_is_logged_and_not_retried() {
    let logs = Logs::default();
    let _g = logs.install();
    let (_d, db) = database().await;
    let (fake, url) = Fake::start(vec![Answer::Status(404)]).await;
    let secrets = Secrets::new(KEY);
    secrets.set_setting(&db, WEBHOOK, &url).await.unwrap();
    let (n, _) = notify::spawn(db, secrets, reqwest::Client::new(), quick());
    n.post("Mini+ needs a hand.");
    until("the refusal, in the log", || {
        logs.text().contains("refused the message (404")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(fake.got().len(), 1);
    assert!(logs.text().contains("Mini+ needs a hand."));
}

#[tokio::test]
async fn with_no_webhook_a_notice_is_logged_not_posted() {
    let logs = Logs::default();
    let _g = logs.install();
    let (_d, db) = database().await;
    let (n, _) = notify::spawn(db, Secrets::new(KEY), reqwest::Client::new(), quick());
    n.post("Voron needs a hand.");
    until("the log line", || {
        logs.text()
            .contains("no Discord webhook is set: Voron needs a hand.")
    })
    .await;
}

#[tokio::test]
async fn the_token_never_reaches_the_log() {
    let logs = Logs::default();
    let _g = logs.install();
    let (_d, db) = database().await;
    // Nothing listens here, so every try fails at the network.
    let url = "http://127.0.0.1:9/api/webhooks/1234567890123/secret-token-abcd";
    let secrets = Secrets::new(KEY);
    secrets.set_setting(&db, WEBHOOK, url).await.unwrap();
    let (n, _) = notify::spawn(db, secrets, reqwest::Client::new(), quick());
    n.post("Ender 3 needs a hand.");
    until("giving up", || logs.text().contains("gave up")).await;
    let text = logs.text();
    assert!(text.contains("Discord could not be reached"), "{text}");
    assert!(!text.contains("secret-token"), "{text}");
}

#[tokio::test]
async fn one_try_says_how_it_went() {
    let (fake, url) = Fake::start(vec![
        Answer::RateLimited(2.5),
        Answer::Status(500),
        Answer::Status(401),
        Answer::Status(404),
        Answer::Status(403),
        Answer::Status(503),
    ])
    .await;
    let http = reqwest::Client::new();
    let t = Duration::from_secs(2);
    assert_eq!(
        notify::attempt(&http, &url, "a", t).await,
        Outcome::RateLimited(Duration::from_secs_f64(2.5))
    );
    assert!(matches!(
        notify::attempt(&http, &url, "b", t).await,
        Outcome::Failed(_)
    ));
    assert!(matches!(
        notify::attempt(&http, &url, "c", t).await,
        Outcome::Refused(_)
    ));
    // A deleted or forbidden webhook is refused for good; Discord's own
    // trouble is worth another try.
    assert!(matches!(
        notify::attempt(&http, &url, "d", t).await,
        Outcome::Refused(_)
    ));
    assert!(matches!(
        notify::attempt(&http, &url, "e", t).await,
        Outcome::Refused(_)
    ));
    assert!(matches!(
        notify::attempt(&http, &url, "f", t).await,
        Outcome::Failed(_)
    ));
    assert_eq!(notify::attempt(&http, &url, "g", t).await, Outcome::Sent);
    assert_eq!(fake.got().len(), 7);
}

// ---- the admin API -----------------------------------------------------------

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    cookie: String,
}

async fn pop() -> Pop {
    let (dir, db) = database().await;
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
    let state = AppState::new(db.clone(), Arc::new(Embedded)).with_secrets(Secrets::new(KEY));
    let mut pop = Pop {
        _dir: dir,
        db,
        app: router(state),
        cookie: String::new(),
    };
    let res = pop
        .send(
            Method::POST,
            "/api/session",
            Some(json!({"username": "ada", "password": "ada's password"})),
            LOCAL,
        )
        .await;
    pop.cookie = res.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    pop
}

impl Pop {
    async fn send(&self, method: Method, uri: &str, body: Option<Value>, from: &str) -> Response {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json");
        if !self.cookie.is_empty() {
            req = req.header(header::COOKIE, &self.cookie);
        }
        let mut req = req
            .body(body.map(|b| Body::from(b.to_string())).unwrap_or_default())
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(from.parse::<SocketAddr>().unwrap()));
        self.app.clone().oneshot(req).await.unwrap()
    }

    async fn json(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let res = self.send(method, uri, body, LOCAL).await;
        let code = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn a_saved_webhook_is_sealed_and_never_shown_in_full() {
    let pop = pop().await;
    let (c, _) = pop
        .json(
            Method::PUT,
            "/api/admin/notify/discord",
            Some(json!({"webhook_url": REAL})),
        )
        .await;
    assert_eq!(c, StatusCode::NO_CONTENT);
    let (_, shown) = pop.json(Method::GET, "/api/admin/notify", None).await;
    assert_eq!(
        shown,
        json!({"discord_webhook": "https://discord.com/api/webhooks/1234567890123/…WxYz"})
    );
    // Sealed at rest: the token is nowhere in the database's row.
    let (sealed,): (Vec<u8>,) = sqlx::query_as("SELECT sealed FROM sealed_settings WHERE key = ?")
        .bind(WEBHOOK)
        .fetch_one(&pop.db)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("AbCdEf"));
    // And opens to what was saved.
    assert_eq!(
        Secrets::new(KEY)
            .setting(&pop.db, WEBHOOK)
            .await
            .unwrap()
            .as_deref(),
        Some(REAL)
    );
}

#[tokio::test]
async fn a_webhook_that_is_not_discords_is_refused_by_name() {
    let pop = pop().await;
    let (c, b) = pop
        .json(
            Method::PUT,
            "/api/admin/notify/discord",
            Some(json!({"webhook_url": "https://evil.example/api/webhooks/1/abcdefghij"})),
        )
        .await;
    assert_eq!(c, StatusCode::BAD_REQUEST);
    assert!(
        b["error"]
            .as_str()
            .unwrap()
            .starts_with("webhook_url: evil.example is not Discord"),
        "{b}"
    );
    let (_, shown) = pop.json(Method::GET, "/api/admin/notify", None).await;
    assert_eq!(shown, json!({"discord_webhook": null}));
}

#[tokio::test]
async fn setting_clearing_and_testing_are_recorded_without_the_url() {
    let pop = pop().await;
    pop.json(
        Method::PUT,
        "/api/admin/notify/discord",
        Some(json!({"webhook_url": REAL})),
    )
    .await;
    pop.json(Method::DELETE, "/api/admin/notify/discord", None)
        .await;
    let (c, b) = pop
        .json(Method::POST, "/api/admin/notify/discord/test", None)
        .await;
    assert_eq!(c, StatusCode::CONFLICT, "{b}");
    assert_eq!(b["error"], "no Discord webhook is set");
    let (_, log) = pop.json(Method::GET, "/api/admin/audit", None).await;
    let entries: Vec<(&str, &str)> = log
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["kind"].as_str().unwrap(), e["summary"].as_str().unwrap()))
        .filter(|(k, _)| k.starts_with("webhook"))
        .collect();
    assert_eq!(
        entries,
        [
            (
                "webhook_tested",
                "Ada sent a test message to the group, and it failed: no Discord webhook is set"
            ),
            ("webhook_cleared", "Ada removed the group's Discord webhook"),
            ("webhook_set", "Ada set the group's Discord webhook"),
        ]
    );
    assert!(!log.to_string().contains("AbCdEf"));
}

#[tokio::test]
async fn the_test_button_posts_now_and_says_how_it_went() {
    let pop = pop().await;
    let (fake, url) = Fake::start(vec![Answer::Ok, Answer::Status(404)]).await;
    // Saved directly: the API takes only Discord's own hosts.
    Secrets::new(KEY)
        .set_setting(&pop.db, WEBHOOK, &url)
        .await
        .unwrap();
    let (c, _) = pop
        .json(Method::POST, "/api/admin/notify/discord/test", None)
        .await;
    assert_eq!(c, StatusCode::NO_CONTENT);
    assert_eq!(
        fake.texts(),
        ["This is a test message from POP, sent by Ada. Group notices will appear here."]
    );
    let (c, b) = pop
        .json(Method::POST, "/api/admin/notify/discord/test", None)
        .await;
    assert_eq!(c, StatusCode::BAD_GATEWAY);
    assert!(
        b["error"]
            .as_str()
            .unwrap()
            .contains("refused the message (404"),
        "{b}"
    );
}

#[tokio::test]
async fn where_notices_go_is_set_only_at_the_space_and_only_by_an_admin() {
    let pop = pop().await;
    let res = pop
        .send(
            Method::PUT,
            "/api/admin/notify/discord",
            Some(json!({"webhook_url": REAL})),
            "203.0.113.9:4000",
        )
        .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let anon = Pop {
        cookie: String::new(),
        ..pop
    };
    let (c, _) = anon
        .json(
            Method::PUT,
            "/api/admin/notify/discord",
            Some(json!({"webhook_url": REAL})),
        )
        .await;
    assert_eq!(c, StatusCode::UNAUTHORIZED);
    let (c, _) = anon.json(Method::GET, "/api/admin/notify", None).await;
    assert_eq!(c, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn without_a_secrets_key_no_webhook_is_kept() {
    let (_d, db) = database().await;
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
    let mut pop = Pop {
        _dir: tempfile::tempdir().unwrap(),
        db: db.clone(),
        app: router(AppState::new(db.clone(), Arc::new(Embedded))),
        cookie: String::new(),
    };
    let res = pop
        .send(
            Method::POST,
            "/api/session",
            Some(json!({"username": "ada", "password": "ada's password"})),
            LOCAL,
        )
        .await;
    pop.cookie = res.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let (c, b) = pop
        .json(
            Method::PUT,
            "/api/admin/notify/discord",
            Some(json!({"webhook_url": REAL})),
        )
        .await;
    assert_eq!(c, StatusCode::CONFLICT);
    assert!(
        b["error"].as_str().unwrap().contains("POP_SECRETS_KEY"),
        "{b}"
    );
}
