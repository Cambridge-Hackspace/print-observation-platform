//! Guides (#71): one format, validated before it is kept; stable slugs that
//! survive a rename; drafts and published guides; images beside the print
//! files.

use std::{collections::BTreeSet, net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use base64::Engine;
use http_body_util::BodyExt;
use pop_server::{
    AppState,
    api::{Guide, GuideStatus, GuideStep, Role, Then},
    assets::Embedded,
    audit::Actor,
    db, guides, router,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

const PNG: &[u8] =
    b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89";

fn step(id: &str, title: &str, then: Then) -> GuideStep {
    GuideStep {
        id: id.into(),
        title: title.into(),
        body: String::new(),
        image: None,
        then,
    }
}
fn next(s: &str) -> Then {
    Then::Next { step: s.into() }
}
fn ask(yes: &str, no: &str) -> Then {
    Then::Question {
        yes: yes.into(),
        no: no.into(),
    }
}

/// The round 6 canvas's stringing flowchart.
fn stringing() -> Guide {
    Guide {
        slug: "stringing-on-petg".into(),
        title: "Stringing on PETG".into(),
        status: GuideStatus::Draft,
        summary: "Fine hairs between the parts of a print.".into(),
        start: "hairs".into(),
        steps: vec![
            step(
                "hairs",
                "Are there fine hairs between parts?",
                ask("dry-box", "blobs"),
            ),
            step(
                "blobs",
                "Those are blobs, not strings: see the blobs guide.",
                Then::Done,
            ),
            step(
                "dry-box",
                "Has this spool been out of the dry box for more than a day?",
                ask("dry-it", "cooler"),
            ),
            step("dry-it", "Dry it for 4 hours at 65 °C.", next("test-tower")),
            step(
                "cooler",
                "Print with the nozzle 5 °C cooler.",
                next("test-tower"),
            ),
            step("test-tower", "Print the test tower.", Then::Done),
        ],
    }
}

fn problems(g: &Guide) -> Vec<(Option<String>, String)> {
    guides::check(g, &BTreeSet::new())
        .into_iter()
        .map(|p| (p.step, p.problem))
        .collect()
}

// ---- the format --------------------------------------------------------------

#[test]
fn the_stringing_flowchart_is_a_guide() {
    assert_eq!(problems(&stringing()), []);
}

#[test]
fn a_branch_to_a_step_that_does_not_exist_is_named() {
    let mut g = stringing();
    g.steps[2].then = ask("dry-it", "colder");
    assert_eq!(
        problems(&g),
        [(
            Some("dry-box".into()),
            "no leads to \"colder\", which is not a step".into()
        )]
    );
}

#[test]
fn a_step_no_answer_reaches_is_named() {
    let mut g = stringing();
    g.steps
        .push(step("orphan", "Clean the nozzle.", Then::Done));
    assert_eq!(
        problems(&g),
        [(
            Some("orphan".into()),
            "no answer leads here from the start".into()
        )]
    );
}

#[test]
fn a_loop_back_is_fine_but_a_loop_with_no_way_out_is_not() {
    let mut g = stringing();
    // "Still stringing? Start again": a loop with a way out.
    g.steps[5].then = ask("hairs", "done");
    g.steps
        .push(step("done", "That's it: it prints clean.", Then::Done));
    assert_eq!(problems(&g), []);
    // Two steps that only lead to each other.
    let mut g = stringing();
    g.steps[3].then = next("cooler");
    g.steps[4].then = next("dry-it");
    let p = problems(&g);
    assert!(p.contains(&(
        Some("dry-it".into()),
        "every answer from here loops without reaching a step that says what to do".into()
    )));
    assert!(p.iter().any(|(s, _)| s.as_deref() == Some("cooler")));
}

#[test]
fn ids_titles_slugs_and_the_start_are_checked() {
    let mut g = stringing();
    g.slug = "Stringing on PETG".into();
    g.title = " ".into();
    g.start = "nowhere".into();
    g.steps[1].id = "hairs".into();
    g.steps[3].title = "".into();
    g.steps[0].title = "".into();
    let p = problems(&g);
    let has = |step: Option<&str>, start: &str| {
        p.iter()
            .any(|(s, m)| s.as_deref() == step && m.starts_with(start))
    };
    assert!(
        has(None, "slug: \"Stringing on PETG\" is not a slug"),
        "{p:?}"
    );
    assert!(has(None, "title: give the guide a title"), "{p:?}");
    assert!(has(None, "start: \"nowhere\" is not a step"), "{p:?}");
    assert!(has(Some("hairs"), "another step has the same id"), "{p:?}");
    assert!(has(Some("dry-it"), "say what to do"), "{p:?}");
    assert!(has(Some("hairs"), "the question is empty"), "{p:?}");
}

#[test]
fn a_step_may_use_only_the_guides_own_images() {
    let mut g = stringing();
    g.steps[3].image = Some("abc123".into());
    assert_eq!(
        problems(&g),
        [(
            Some("dry-it".into()),
            "image abc123 is not one of this guide's".into()
        )]
    );
    assert_eq!(
        guides::check(&g, &BTreeSet::from(["abc123".to_owned()])),
        []
    );
}

#[test]
fn slugs_are_lowercase_words_joined_by_hyphens() {
    for ok in ["stringing-on-petg", "a", "warping-2"] {
        assert!(guides::is_slug(ok), "{ok}");
    }
    for bad in ["", "-a", "a-", "a--b", "A", "a b", "a_b", "é"] {
        assert!(!guides::is_slug(bad), "{bad}");
    }
}

// ---- the admin API -----------------------------------------------------------

const LOCAL: &str = "127.0.0.1:4000";

struct Pop {
    dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    admin: String,
    member: String,
}

async fn sign_in(app: &Router, who: &str) -> String {
    let res = send(
        app,
        Method::POST,
        "/api/session",
        None,
        Some(json!({"username": who, "password": format!("{who}'s password")})),
    )
    .await;
    res.0
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (axum::response::Response, ()) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    let mut req = req
        .body(body.map(|b| Body::from(b.to_string())).unwrap_or_default())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(LOCAL.parse::<SocketAddr>().unwrap()));
    (app.clone().oneshot(req).await.unwrap(), ())
}

async fn pop() -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    for (u, role) in [("ada", Role::Admin), ("sam", Role::Member)] {
        let password = format!("{u}'s password");
        users::create(
            &db,
            NewAccount {
                username: u,
                name: u,
                role,
                password: &password,
            },
            &Actor::command_line(),
        )
        .await
        .unwrap();
    }
    let app = router(
        AppState::new(db.clone(), Arc::new(Embedded)).with_files_dir(dir.path().join("files")),
    );
    let admin = sign_in(&app, "ada").await;
    let member = sign_in(&app, "sam").await;
    Pop {
        dir,
        db,
        app,
        admin,
        member,
    }
}

impl Pop {
    async fn json(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.as_(&self.admin.clone(), method, uri, body).await
    }
    async fn as_(
        &self,
        cookie: &str,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let (res, ()) = send(
            &self.app,
            method,
            uri,
            (!cookie.is_empty()).then_some(cookie),
            body,
        )
        .await;
        let code = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

fn as_json(g: &Guide) -> Value {
    serde_json::to_value(g).unwrap()
}

#[tokio::test]
async fn a_guide_is_kept_and_read_back_as_written() {
    let pop = pop().await;
    let (c, _) = pop
        .json(
            Method::POST,
            "/api/admin/guides",
            Some(as_json(&stringing())),
        )
        .await;
    assert_eq!(c, StatusCode::CREATED);
    let (c, back) = pop
        .json(Method::GET, "/api/admin/guides/stringing-on-petg", None)
        .await;
    assert_eq!(c, StatusCode::OK);
    assert_eq!(back, as_json(&stringing()));
    let (_, list) = pop.json(Method::GET, "/api/admin/guides", None).await;
    assert_eq!(list[0]["slug"], "stringing-on-petg");
    assert_eq!(list[0]["status"], "draft");
    assert_eq!(
        list[0]["summary"],
        "Fine hairs between the parts of a print."
    );
}

#[tokio::test]
async fn a_broken_guide_is_refused_with_everything_wrong_by_step() {
    let pop = pop().await;
    let mut g = stringing();
    g.steps[2].then = ask("dry-it", "colder");
    let (c, b) = pop
        .json(Method::POST, "/api/admin/guides", Some(as_json(&g)))
        .await;
    assert_eq!(c, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        b["problems"],
        json!([{"step": "dry-box", "problem": "no leads to \"colder\", which is not a step"}])
    );
    assert!(
        b["error"]
            .as_str()
            .unwrap()
            .contains("step dry-box: no leads to"),
        "{b}"
    );
    let (c, _) = pop
        .json(Method::GET, "/api/admin/guides/stringing-on-petg", None)
        .await;
    assert_eq!(c, StatusCode::NOT_FOUND, "nothing kept");
}

#[tokio::test]
async fn a_renamed_guide_keeps_its_old_slug_as_a_redirect() {
    let pop = pop().await;
    pop.json(
        Method::POST,
        "/api/admin/guides",
        Some(as_json(&stringing())),
    )
    .await;
    let mut renamed = stringing();
    renamed.slug = "stringing".into();
    let (c, b) = pop
        .json(
            Method::PUT,
            "/api/admin/guides/stringing-on-petg",
            Some(as_json(&renamed)),
        )
        .await;
    assert_eq!(c, StatusCode::OK, "{b}");
    // The old slug leads to the guide as it is now.
    assert_eq!(
        guides::find(&pop.db, "stringing-on-petg")
            .await
            .unwrap()
            .map(|(_, s)| s)
            .as_deref(),
        Some("stringing")
    );
    // No other guide can take the old slug.
    let mut other = stringing();
    other.title = "Another".into();
    let (c, b) = pop
        .json(Method::POST, "/api/admin/guides", Some(as_json(&other)))
        .await;
    assert_eq!(c, StatusCode::CONFLICT, "{b}");
    // Editing goes by the slug it has now.
    let (c, _) = pop
        .json(
            Method::PUT,
            "/api/admin/guides/stringing-on-petg",
            Some(as_json(&renamed)),
        )
        .await;
    assert_eq!(c, StatusCode::NOT_FOUND);
    // And it may take its old slug back.
    let (c, _) = pop
        .json(
            Method::PUT,
            "/api/admin/guides/stringing",
            Some(as_json(&stringing())),
        )
        .await;
    assert_eq!(c, StatusCode::OK);
    assert_eq!(
        guides::find(&pop.db, "stringing")
            .await
            .unwrap()
            .map(|(_, s)| s)
            .as_deref(),
        Some("stringing-on-petg")
    );
}

#[tokio::test]
async fn two_guides_cannot_share_a_slug() {
    let pop = pop().await;
    pop.json(
        Method::POST,
        "/api/admin/guides",
        Some(as_json(&stringing())),
    )
    .await;
    let (c, b) = pop
        .json(
            Method::POST,
            "/api/admin/guides",
            Some(as_json(&stringing())),
        )
        .await;
    assert_eq!(c, StatusCode::CONFLICT);
    assert!(
        b["error"]
            .as_str()
            .unwrap()
            .contains("belongs to another guide"),
        "{b}"
    );
}

#[tokio::test]
async fn an_image_is_kept_beside_the_print_files_and_a_step_may_use_it() {
    let pop = pop().await;
    pop.json(
        Method::POST,
        "/api/admin/guides",
        Some(as_json(&stringing())),
    )
    .await;
    let data = base64::engine::general_purpose::STANDARD.encode(PNG);
    let (c, b) = pop
        .json(
            Method::POST,
            "/api/admin/guides/stringing-on-petg/images",
            Some(json!({"content_type": "image/png", "data_base64": data})),
        )
        .await;
    assert_eq!(c, StatusCode::CREATED, "{b}");
    let id = b["id"].as_str().unwrap().to_owned();
    // On disk, under files/, where a backup takes it.
    let path = pop
        .dir
        .path()
        .join("files")
        .join(guides::IMAGES_DIR)
        .join(&id);
    assert_eq!(std::fs::read(&path).unwrap(), PNG);
    // Served as what it is.
    let (res, ()) = send(
        &pop.app,
        Method::GET,
        &format!("/api/admin/guides/stringing-on-petg/images/{id}"),
        Some(&pop.admin),
        None,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(res.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(
        res.into_body().collect().await.unwrap().to_bytes().as_ref(),
        PNG
    );
    // A step can use it now.
    let mut g = stringing();
    g.steps[3].image = Some(id);
    let (c, b) = pop
        .json(
            Method::PUT,
            "/api/admin/guides/stringing-on-petg",
            Some(as_json(&g)),
        )
        .await;
    assert_eq!(c, StatusCode::OK, "{b}");
}

#[tokio::test]
async fn an_image_that_is_not_one_is_refused() {
    let pop = pop().await;
    pop.json(
        Method::POST,
        "/api/admin/guides",
        Some(as_json(&stringing())),
    )
    .await;
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    for (ct, data, says) in [
        (
            "image/svg+xml",
            b64(b"<svg/>"),
            "content_type: \"image/svg+xml\" is not one of",
        ),
        (
            "image/png",
            b64(b"<html>not a png</html>"),
            "these bytes are not image/png",
        ),
        ("image/png", "not base64!".to_owned(), "not base64"),
        ("image/png", String::new(), "an image is 1 byte to 5 MB"),
    ] {
        let (c, b) = pop
            .json(
                Method::POST,
                "/api/admin/guides/stringing-on-petg/images",
                Some(json!({"content_type": ct, "data_base64": data})),
            )
            .await;
        assert_eq!(c, StatusCode::BAD_REQUEST, "{ct}");
        assert!(b["error"].as_str().unwrap().contains(says), "{b}");
    }
    assert!(
        !pop.dir
            .path()
            .join("files")
            .join(guides::IMAGES_DIR)
            .exists()
            || std::fs::read_dir(pop.dir.path().join("files").join(guides::IMAGES_DIR))
                .unwrap()
                .next()
                .is_none()
    );
}

#[tokio::test]
async fn writing_editing_publishing_and_renaming_are_recorded() {
    let pop = pop().await;
    pop.json(
        Method::POST,
        "/api/admin/guides",
        Some(as_json(&stringing())),
    )
    .await;
    let mut g = stringing();
    g.slug = "stringing".into();
    g.status = GuideStatus::Published;
    pop.json(
        Method::PUT,
        "/api/admin/guides/stringing-on-petg",
        Some(as_json(&g)),
    )
    .await;
    let data = base64::engine::general_purpose::STANDARD.encode(PNG);
    pop.json(
        Method::POST,
        "/api/admin/guides/stringing/images",
        Some(json!({"content_type": "image/png", "data_base64": data})),
    )
    .await;
    let (_, log) = pop.json(Method::GET, "/api/admin/audit", None).await;
    let summaries: Vec<&str> = log
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"].as_str().unwrap().starts_with("guide"))
        .map(|e| e["summary"].as_str().unwrap())
        .collect();
    assert_eq!(
        summaries,
        [
            "ada added an image to the guide stringing",
            "ada edited the guide Stringing on PETG (stringing) and renamed it from stringing-on-petg, which still leads to it and published it",
            "ada wrote the guide Stringing on PETG (stringing-on-petg)",
        ]
    );
}

#[tokio::test]
async fn only_an_admin_writes_or_reads_drafts() {
    let pop = pop().await;
    pop.json(
        Method::POST,
        "/api/admin/guides",
        Some(as_json(&stringing())),
    )
    .await;
    for (method, uri) in [
        (Method::GET, "/api/admin/guides"),
        (Method::POST, "/api/admin/guides"),
        (Method::GET, "/api/admin/guides/stringing-on-petg"),
        (Method::PUT, "/api/admin/guides/stringing-on-petg"),
        (Method::POST, "/api/admin/guides/stringing-on-petg/images"),
    ] {
        let body = (method != Method::GET).then(|| as_json(&stringing()));
        let (c, _) = pop
            .as_(&pop.member.clone(), method.clone(), uri, body.clone())
            .await;
        assert_eq!(c, StatusCode::FORBIDDEN, "{method} {uri} as a member");
        let (c, _) = pop.as_("", method.clone(), uri, body).await;
        assert_eq!(c, StatusCode::UNAUTHORIZED, "{method} {uri} signed out");
    }
}
