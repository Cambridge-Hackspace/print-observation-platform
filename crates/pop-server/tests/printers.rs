//! Printers from BAM!, as admins see them (#21), against a fake BAM! serving
//! a real BAM!'s recorded answers.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use pop_integrations::{
    bam::Bam,
    fakes::bam::{FakeBam, TOKEN},
    printers::FieldMap,
};
use pop_server::{
    AppState,
    api::Role,
    assets::Embedded,
    audit::Actor,
    db,
    printers::BamSource,
    router,
    secrets::{SecretError, Secrets},
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

const RECORDING: &str =
    include_str!("../../pop-integrations/tests/recordings/v0.20.0/recordings.json");
const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    fake: FakeBam,
    admin: String,
}

async fn pop_with(token: &str, key: &str, bam: bool) -> Pop {
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
    let fake = FakeBam::start(RECORDING).await;
    let mut state = AppState::new(db.clone(), Arc::new(Embedded)).with_secrets(Secrets::new(key));
    if bam {
        state = state.with_bam(BamSource {
            client: Bam::new(fake.url(), token).unwrap(),
            category: fake.id("printers").to_owned(),
            fields: FieldMap::default(),
        });
    }
    let mut pop = Pop {
        _dir: dir,
        db,
        app: router(state),
        fake,
        admin: String::new(),
    };
    let (_, _, set) = pop
        .send(
            Method::POST,
            "/api/session",
            None,
            Some(json!({"username": "ada", "password": "ada's password"})),
            LOCAL,
        )
        .await;
    pop.admin = set.unwrap();
    pop
}

async fn pop() -> Pop {
    pop_with(TOKEN, KEY, true).await
}

const LOCAL: &str = "127.0.0.1:4000";
const REMOTE: &str = "203.0.113.9:4000";

impl Pop {
    async fn send(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
        from: &str,
    ) -> (StatusCode, Value, Option<String>) {
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
            .insert(ConnectInfo(from.parse::<SocketAddr>().unwrap()));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let set = res
            .headers()
            .get(header::SET_COOKIE)
            .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_owned());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            code,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            set,
        )
    }

    async fn printers(&self) -> Vec<Value> {
        let (code, body, _) = self
            .send(
                Method::GET,
                "/api/admin/printers",
                Some(&self.admin),
                None,
                LOCAL,
            )
            .await;
        assert_eq!(code, StatusCode::OK, "{body}");
        body.as_array().unwrap().clone()
    }

    async fn enter_code(&self, bam_id: &str, code: &str, from: &str) -> (StatusCode, Value) {
        let (c, b, _) = self
            .send(
                Method::PUT,
                &format!("/api/admin/printers/{bam_id}/access-code"),
                Some(&self.admin),
                Some(json!({ "access_code": code })),
                from,
            )
            .await;
        (c, b)
    }
}

fn named<'a>(all: &'a [Value], name: &str) -> &'a Value {
    all.iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

/// The issue's "done when": a printer with a missing or invalid field is
/// shown to admins with the field named, rather than half-added.
#[tokio::test]
async fn a_printer_with_a_missing_field_is_shown_with_the_field_named() {
    let pop = pop().await;
    let all = pop.printers().await;
    let ender = named(&all, "Ender 3");
    assert_eq!(ender["ready"], false);
    assert_eq!(ender["connection"], Value::Null, "nothing guessed for it");
    assert_eq!(ender["problems"][0]["field"], "Connection (connection)");
    let x1 = named(&all, "X1 Carbon");
    assert_eq!(x1["ready"], false);
    assert_eq!(x1["problems"][0]["field"], "LAN access code");
    for ready in ["Voron 2.4", "MK4"] {
        let p = named(&all, ready);
        assert_eq!(p["ready"], true, "{p}");
        assert_eq!(p["problems"], json!([]));
    }
    let names: Vec<_> = all.iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        ["Ender 3", "MK4", "Voron 2.4", "X1 Carbon"],
        "only the category's own, by name"
    );
}

#[tokio::test]
async fn entering_the_access_code_makes_a_bambu_printer_ready() {
    let pop = pop().await;
    let (code, body) = pop.enter_code(pop.fake.id("x1"), "12345678", LOCAL).await;
    assert_eq!(code, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(named(&pop.printers().await, "X1 Carbon")["ready"], true);
}

#[tokio::test]
async fn entering_an_access_code_is_recorded_without_the_code() {
    let pop = pop().await;
    pop.enter_code(pop.fake.id("x1"), "87654321", LOCAL).await;
    let (_, log, _) = pop
        .send(
            Method::GET,
            "/api/admin/audit?kind=access_code_set",
            Some(&pop.admin),
            None,
            LOCAL,
        )
        .await;
    let e = &log[0];
    assert_eq!(e["summary"], "Ada entered X1 Carbon's LAN access code");
    assert_eq!(e["printer"], pop.fake.id("x1"));
    assert!(
        !log.to_string().contains("87654321"),
        "the code reached the audit log"
    );
}

#[tokio::test]
async fn the_code_is_kept_sealed_and_opens_with_its_key_only() {
    let pop = pop().await;
    let x1 = pop.fake.id("x1").to_owned();
    pop.enter_code(&x1, "24681357", LOCAL).await;
    let (nonce, sealed): (Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT nonce, sealed FROM printer_secrets")
            .fetch_one(&pop.db)
            .await
            .unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("24681357"));
    assert_eq!(nonce.len(), 12);
    assert_eq!(
        Secrets::new(KEY)
            .access_code(&pop.db, &x1)
            .await
            .unwrap()
            .as_deref(),
        Some("24681357")
    );
    let other = "ZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmY=";
    assert_eq!(
        Secrets::new(other).access_code(&pop.db, &x1).await,
        Err(SecretError::WillNotOpen)
    );
    // Copied to another printer's row, it does not open as that printer's.
    sqlx::query("UPDATE printer_secrets SET bam_id = ?")
        .bind(pop.fake.id("voron"))
        .execute(&pop.db)
        .await
        .unwrap();
    assert_eq!(
        Secrets::new(KEY)
            .access_code(&pop.db, pop.fake.id("voron"))
            .await,
        Err(SecretError::WillNotOpen)
    );
}

#[tokio::test]
async fn a_code_is_entered_only_from_inside_the_space() {
    let pop = pop().await;
    let (code, _) = pop.enter_code(pop.fake.id("x1"), "12345678", REMOTE).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert_eq!(named(&pop.printers().await, "X1 Carbon")["ready"], false);
}

#[tokio::test]
async fn without_a_secrets_key_pop_refuses_to_keep_the_code() {
    let pop = pop_with(TOKEN, "", true).await;
    let (code, body) = pop.enter_code(pop.fake.id("x1"), "12345678", LOCAL).await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(
        body["error"].as_str().unwrap().contains("POP_SECRETS_KEY"),
        "{body}"
    );
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM printer_secrets")
        .fetch_one(&pop.db)
        .await
        .unwrap();
    assert_eq!(n, 0, "kept in plain text instead");
}

#[tokio::test]
async fn a_code_that_cannot_be_one_is_refused() {
    let pop = pop().await;
    for bad in ["", "12", "1234 5678", &"9".repeat(40)] {
        let (code, body) = pop.enter_code(pop.fake.id("x1"), bad, LOCAL).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{bad:?}");
        assert!(
            body["error"].as_str().unwrap().starts_with("access_code"),
            "{body}"
        );
    }
}

#[tokio::test]
async fn a_code_for_a_printer_bam_does_not_have_is_refused() {
    let pop = pop().await;
    let (code, _) = pop
        .enter_code("00000000-0000-4000-8000-000000000000", "12345678", LOCAL)
        .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn without_bam_the_printer_list_says_how_to_set_it_up() {
    let pop = pop_with(TOKEN, KEY, false).await;
    let (code, body, _) = pop
        .send(
            Method::GET,
            "/api/admin/printers",
            Some(&pop.admin),
            None,
            LOCAL,
        )
        .await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        body["error"].as_str().unwrap().contains("[bam] url"),
        "{body}"
    );
}

#[tokio::test]
async fn a_bam_that_refuses_the_token_is_said_plainly() {
    let pop = pop_with("bam_wrong", KEY, true).await;
    let (code, body, _) = pop
        .send(
            Method::GET,
            "/api/admin/printers",
            Some(&pop.admin),
            None,
            LOCAL,
        )
        .await;
    assert_eq!(code, StatusCode::BAD_GATEWAY);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("refused POP's token"),
        "{body}"
    );
}

#[tokio::test]
async fn healthz_reports_bam_by_name_and_without_detail() {
    let good = pop().await;
    let (code, body, _) = good.send(Method::GET, "/healthz", None, None, REMOTE).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["checks"]["bam"], json!({"ok": true}));
    let bad = pop_with("bam_wrong", KEY, true).await;
    let (code, body, _) = bad.send(Method::GET, "/healthz", None, None, REMOTE).await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["checks"]["bam"]["error"], "BAM! refused POP's token");
    assert!(
        !body.to_string().contains("127.0.0.1"),
        "an address reached a public answer: {body}"
    );
}

#[tokio::test]
async fn healthz_says_when_bams_live_connection_is_down() {
    let fake = FakeBam::start(RECORDING).await;
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let opts = pop_integrations::sync::Options {
        first_retry: std::time::Duration::from_millis(50),
        max_retry: std::time::Duration::from_millis(100),
        ..pop_integrations::sync::Options::default()
    };
    let (view, _sync) = pop_integrations::sync::spawn(
        Bam::new(fake.url(), TOKEN).unwrap(),
        fake.id("printers").to_owned(),
        opts,
    );
    let app = router(AppState::new(db, Arc::new(Embedded)).with_bam_sync(view.clone()));
    let health = |app: Router| async move {
        let res = app
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice::<Value>(&bytes).unwrap()
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(4);
    while !view.borrow().live {
        assert!(tokio::time::Instant::now() < deadline, "never live");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        health(app.clone()).await["checks"]["bam_live"],
        json!({"ok": true})
    );
    fake.refuse_live(true);
    fake.drop_live();
    while view.borrow().live {
        assert!(tokio::time::Instant::now() < deadline, "never down");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let h = health(app).await;
    assert_eq!(h["checks"]["bam_live"]["ok"], false);
    assert!(
        h["checks"]["bam_live"]["error"]
            .as_str()
            .unwrap()
            .contains("last said"),
        "{h}"
    );
}
