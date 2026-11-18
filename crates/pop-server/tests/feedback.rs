//! Feedback from any screen, filed as GitHub issues (#96), end to end
//! against the fake GitHub: kept, sent, retried, scrubbed, and only for
//! those who hold feedback.file.
//!
//! What this does not prove: that GitHub itself takes these writes as its
//! documentation says (`pop_integrations::github` names which answers were
//! read from GitHub and which only from its documentation).

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use base64::Engine as _;
use http_body_util::BodyExt;
use pop_integrations::{
    fakes::github::FakeGitHub,
    github::{AppConfig, GitHub},
};
use pop_printers::{Job, PrinterState, PrinterStatus};
use pop_server::{
    AppState,
    api::{Role, TowerPrinter},
    assets::Embedded,
    audit::Actor,
    config, db,
    feedback::{self, Feedback, scrub_text, without_metadata},
    router,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::watch;
use tower::ServiceExt;

const BRANCH: &str = "feedback-attachments";

/// CRC-32, as PNG's chunks carry it.
fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for b in bytes {
        c ^= u32::from(*b);
        for _ in 0..8 {
            c = if c & 1 == 1 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    c.extend_from_slice(&body);
    c.extend_from_slice(&crc32(&body).to_be_bytes());
    c
}

/// A 1x1 PNG, with a text chunk naming its author, as an editor might add.
fn png_with_metadata() -> Vec<u8> {
    let mut p = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    p.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]));
    p.extend(chunk(b"tEXt", b"Author\0Ada Lovelace's laptop"));
    p.extend(chunk(
        b"IDAT",
        &[
            0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00,
        ],
    ));
    p.extend(chunk(b"IEND", &[]));
    p
}

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    github: FakeGitHub,
    ada: String,
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

async fn pop_with(configured: bool, per_hour: u32) -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    for (username, name, role) in [("ada", "Ada", Role::Admin), ("kim", "Kim", Role::Member)] {
        users::create(
            &db,
            NewAccount {
                username,
                name,
                role,
                password: &format!("{username}'s password"),
            },
            &Actor::command_line(),
        )
        .await
        .unwrap();
    }
    let github = FakeGitHub::start("acme", "pop", "12345", "678").await;
    let tower = TowerPrinter {
        id: "p1".into(),
        name: "Voron 2.4".into(),
        state: PrinterState {
            status: PrinterStatus::Printing,
            job: Some(Job {
                name: "benchy.gcode".into(),
                owner: Some("Sam Lee".into()),
            }),
            ..PrinterState::offline()
        },
        watched: true,
        materials: vec![],
        training: None,
        camera: false,
    };
    let (tx, rx) = watch::channel(vec![tower]);
    std::mem::forget(tx);
    let mut state = AppState::new(db.clone(), Arc::new(Embedded)).with_tower(rx);
    if configured {
        let f = Arc::new(Feedback {
            config: config::Feedback {
                app_id: "12345".into(),
                installation_id: "678".into(),
                private_key_file: "unused".into(),
                repo: "acme/pop".into(),
                api: github.url(),
                per_hour,
                ..config::Feedback::default()
            },
            github: GitHub::new(AppConfig {
                api: github.url(),
                app_id: "12345".into(),
                installation_id: "678".into(),
                private_key: github.private_key(),
                owner: "acme".into(),
                repo: "pop".into(),
            })
            .unwrap(),
            wake: Arc::new(tokio::sync::Notify::new()),
        });
        feedback::spawn(
            db.clone(),
            f.clone(),
            feedback::Options {
                first: Duration::from_millis(50),
                max: Duration::from_millis(200),
            },
        );
        state = state.with_feedback(f);
    }
    let app = router(state);
    let ada = signed_in(&app, "ada").await;
    let kim = signed_in(&app, "kim").await;
    Pop {
        _dir: dir,
        db,
        app,
        github,
        ada,
        kim,
    }
}

async fn pop() -> Pop {
    pop_with(true, 10).await
}

impl Pop {
    async fn send(
        &self,
        method: Method,
        uri: &str,
        cookie: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::COOKIE, cookie);
        let mut req = match body {
            Some(b) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        req.extensions_mut().insert(ConnectInfo(
            "203.0.113.9:4000".parse::<SocketAddr>().unwrap(),
        ));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn file(&self, cookie: &str, context: Value) -> (StatusCode, Value) {
        self.send(
            Method::POST,
            "/api/feedback",
            cookie,
            Some(json!({
                "kind": "bug",
                "title": "The X1 reads offline",
                "description": "It was printing a minute ago.",
                "device": "kiosk",
                "screenshot": base64::engine::general_purpose::STANDARD.encode(png_with_metadata()),
                "context": context,
            })),
        )
        .await
    }

    /// Wait until report `id` reads `state`; its status.
    async fn until(&self, id: &str, state: &str) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let (_, s) = self
                .send(Method::GET, &format!("/api/feedback/{id}"), &self.ada, None)
                .await;
            if s["state"] == state {
                return s;
            }
            assert!(tokio::time::Instant::now() < deadline, "never {state}: {s}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

#[tokio::test]
async fn a_report_becomes_an_issue_that_shows_its_screenshot() {
    let pop = pop().await;
    let (code, accepted) = pop.file(&pop.ada, json!({"route": "/kiosk"})).await;
    assert_eq!(code, StatusCode::ACCEPTED, "{accepted}");
    assert_eq!(accepted["state"], "pending");
    let id = accepted["id"].as_str().unwrap().to_owned();
    let sent = pop.until(&id, "sent").await;
    assert_eq!(sent["issue_url"], "https://github.com/acme/pop/issues/1");

    let issues = pop.github.issues();
    assert_eq!(issues.len(), 1);
    let issue = &issues[0];
    assert_eq!(issue.title, "The X1 reads offline");
    assert_eq!(issue.labels, ["feedback", "bug"]);
    assert!(
        issue.body.starts_with("It was printing a minute ago."),
        "{}",
        issue.body
    );
    assert!(
        issue.body.contains("Reported by Ada from the kiosk."),
        "{}",
        issue.body
    );
    let picture = format!("https://raw.githubusercontent.com/acme/pop/{BRANCH}/feedback/{id}.png");
    assert!(
        issue.body.contains(&format!("]({picture})")),
        "{}",
        issue.body
    );
    assert!(
        issue.body.contains(r#""route": "/kiosk""#),
        "{}",
        issue.body
    );
    // The picture went up, without its metadata.
    let uploaded = pop
        .github
        .file(BRANCH, &format!("feedback/{id}.png"))
        .unwrap();
    assert_eq!(uploaded, without_metadata(&png_with_metadata()).unwrap());
    assert!(!String::from_utf8_lossy(&uploaded).contains("Lovelace"));
    // And it is in the audit log, with the issue.
    let (_, log) = pop
        .send(Method::GET, "/api/admin/audit", &pop.ada, None)
        .await;
    let entry = log
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "feedback_filed")
        .unwrap_or_else(|| panic!("{log}"));
    assert!(
        entry["summary"].as_str().unwrap().contains("issues/1"),
        "{entry}"
    );
    assert_eq!(entry["actor_name"], "Ada");
}

/// A failed request's log, with a token planted in its header, its address
/// and its cookie, as the screen would gather it: none of it reaches GitHub.
#[tokio::test]
async fn no_token_cookie_or_authorization_reaches_the_issue() {
    let pop = pop().await;
    let context = json!({
        "route": "/admin/printers",
        "failed_requests": [{
            "url": "/api/admin/printers?access_token=planted-query-7c1d",
            "status": 502,
            "headers": {"Authorization": "Bearer planted-header-3b9e", "Cookie": "pop_session=planted-cookie-55aa"},
        }],
        "console_errors": [
            "fetch failed: Authorization: Bearer planted-console-81f0",
            "GET https://pop.example.org/api/events?last_event_id=planted-url-2e44 404",
            "token ghs_plantedghtoken0123456789abcdef",
        ],
        "session": "planted-session-9d0c",
    });
    let (_, accepted) = pop.file(&pop.ada, context).await;
    pop.until(accepted["id"].as_str().unwrap(), "sent").await;
    let body = &pop.github.issues()[0].body;
    for planted in [
        "planted-query-7c1d",
        "planted-header-3b9e",
        "planted-cookie-55aa",
        "planted-console-81f0",
        "planted-url-2e44",
        "plantedghtoken",
        "planted-session-9d0c",
    ] {
        assert!(
            !body.contains(planted),
            "{planted} reached the issue:\n{body}"
        );
    }
    // The precondition: the context did arrive, scrubbed rather than dropped.
    assert!(body.contains("/api/admin/printers?[redacted]"), "{body}");
    assert!(body.contains("fetch failed"), "{body}");
    // The printers' states, with nobody named.
    assert!(body.contains("benchy.gcode"), "{body}");
    assert!(!body.contains("Sam") && !body.contains("Lee"), "{body}");
}

#[tokio::test]
async fn a_report_made_while_github_is_down_is_sent_once_it_is_back_exactly_once() {
    let pop = pop().await;
    pop.github.down(true);
    let (_, accepted) = pop.file(&pop.ada, json!({})).await;
    let id = accepted["id"].as_str().unwrap().to_owned();
    // Tried, and tried again, while it is down.
    tokio::time::sleep(Duration::from_millis(400)).await;
    pop.until(&id, "pending").await;
    let attempts: i64 = sqlx::query_scalar("SELECT attempts FROM feedback_reports WHERE id = ?")
        .bind(&id)
        .fetch_one(&pop.db)
        .await
        .unwrap();
    assert!(attempts >= 2, "{attempts} attempts");
    assert!(pop.github.issues().is_empty());
    pop.github.down(false);
    pop.until(&id, "sent").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(pop.github.issues().len(), 1, "exactly once");
    assert_eq!(pop.github.files(BRANCH).len(), 1);
}

/// GitHub fails after the picture went up: the retry files the issue with
/// that picture, and does not put it up again.
#[tokio::test]
async fn a_report_retried_after_its_picture_went_up_uses_that_picture() {
    let pop = pop().await;
    pop.github.fail_next_issue();
    let (_, accepted) = pop.file(&pop.ada, json!({})).await;
    let id = accepted["id"].as_str().unwrap().to_owned();
    pop.until(&id, "sent").await;
    assert_eq!(pop.github.issues().len(), 1);
    assert_eq!(pop.github.files(BRANCH), [format!("feedback/{id}.png")]);
}

#[tokio::test]
async fn only_those_holding_feedback_file_can_file_and_see_the_button() {
    let pop = pop().await;
    let (_, ada) = pop.send(Method::GET, "/api/feedback", &pop.ada, None).await;
    assert_eq!(
        ada,
        json!({"enabled": true, "allowed": true, "per_hour": 10})
    );
    let (_, kim) = pop.send(Method::GET, "/api/feedback", &pop.kim, None).await;
    assert_eq!(kim["allowed"], false);
    let (code, _) = pop.file(&pop.kim, json!({})).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    // Nobody else's report can be read.
    let (_, accepted) = pop.file(&pop.ada, json!({})).await;
    let (code, _) = pop
        .send(
            Method::GET,
            &format!("/api/feedback/{}", accepted["id"].as_str().unwrap()),
            &pop.kim,
            None,
        )
        .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
    // Signed out: no.
    let (code, _) = pop
        .send(
            Method::POST,
            "/api/feedback",
            "pop_session=nobody",
            Some(json!({})),
        )
        .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn without_feedback_configured_there_is_no_button_and_nothing_to_file() {
    let pop = pop_with(false, 10).await;
    let (_, s) = pop.send(Method::GET, "/api/feedback", &pop.ada, None).await;
    assert_eq!(s["enabled"], false);
    assert_eq!(s["allowed"], false);
    let (code, _) = pop.file(&pop.ada, json!({})).await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn one_person_files_no_more_than_the_hour_allows() {
    let pop = pop_with(true, 2).await;
    for _ in 0..2 {
        assert_eq!(pop.file(&pop.ada, json!({})).await.0, StatusCode::ACCEPTED);
    }
    assert_eq!(
        pop.file(&pop.ada, json!({})).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn a_screenshot_must_be_a_png_and_a_title_must_say_something() {
    let pop = pop().await;
    let mut r = json!({
        "kind": "bug", "title": "t", "description": "", "device": "phone",
        "screenshot": base64::engine::general_purpose::STANDARD.encode(b"GIF89a not a png"),
        "context": {},
    });
    let (code, _) = pop
        .send(Method::POST, "/api/feedback", &pop.ada, Some(r.clone()))
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    r["screenshot"] = json!(base64::engine::general_purpose::STANDARD.encode(png_with_metadata()));
    r["title"] = json!("   ");
    let (code, _) = pop
        .send(Method::POST, "/api/feedback", &pop.ada, Some(r))
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
}

// ---- the parts, on their own -----------------------------------------------------

#[test]
fn metadata_goes_and_the_picture_stays() {
    let p = png_with_metadata();
    let stripped = without_metadata(&p).unwrap();
    assert!(stripped.len() < p.len());
    for kind in [&b"IHDR"[..], b"IDAT", b"IEND"] {
        assert!(stripped.windows(4).any(|w| w == kind), "{kind:?}");
    }
    assert!(!stripped.windows(4).any(|w| w == b"tEXt"));
    // Not a PNG, cut short, or not starting with its header: refused.
    assert!(without_metadata(b"not a png").is_none());
    assert!(without_metadata(&p[..p.len() - 6]).is_none());
    let mut no_header = p[..8].to_vec();
    no_header.extend(chunk(b"IDAT", &[0]));
    no_header.extend(chunk(b"IEND", &[]));
    assert!(without_metadata(&no_header).is_none());
}

#[test]
fn words_that_are_not_secrets_are_left_as_they_are() {
    assert_eq!(
        scrub_text("the X1 is offline since 10:42, layer 37 of 120"),
        "the X1 is offline since 10:42, layer 37 of 120"
    );
    assert_eq!(scrub_text("why? because"), "why? because");
}

/// Each way a token is caught, on its own: one alone must catch it.
#[test]
fn each_kind_of_token_is_caught_by_itself() {
    for (plain, scrubbed) in [
        // A GitHub token, short enough to pass for a word.
        ("ghs_abc", "[redacted]"),
        ("github_pat_x", "[redacted]"),
        // A session cookie.
        ("pop_session=1", "[redacted]"),
        // A JWT.
        ("eyJhbGciOi.eyJzdWIiOi.c2ln", "[redacted]"),
        // A long run of key-like characters with a digit in it.
        ("Abcdefghij0123456789abcdefghij0123", "[redacted]"),
        // The credential after a scheme.
        ("Bearer xyz", "Bearer [redacted]"),
        ("Basic dXNlcjpwYXNz", "Basic [redacted]"),
        // A query string.
        ("/api/x?a=1", "/api/x?[redacted]"),
    ] {
        assert_eq!(scrub_text(plain), scrubbed, "{plain}");
    }
}
