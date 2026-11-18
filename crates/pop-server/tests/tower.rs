//! "New in BAM!": adding and retiring printers (#23), against a fake BAM!
//! built from a real one's recorded answers.

use std::{net::SocketAddr, sync::Arc, time::Duration};

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
    fakes::bam::{FakeBam, TOKEN},
    printers::FieldMap,
    sync,
};
use pop_server::{
    AppState,
    api::Role,
    assets::Embedded,
    audit::Actor,
    db,
    printers::BamSource,
    router,
    secrets::Secrets,
    tower,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;

const RECORDING: &str =
    include_str!("../../pop-integrations/tests/recordings/v0.20.0/recordings.json");
const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
const LOCAL: &str = "127.0.0.1:4000";
const REMOTE: &str = "203.0.113.9:4000";

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    fake: FakeBam,
    admin: String,
}

async fn pop() -> Pop {
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
    let state = AppState::new(db.clone(), Arc::new(Embedded))
        .with_secrets(Secrets::new(KEY))
        .with_bam(BamSource {
            client: Bam::new(fake.url(), TOKEN).unwrap(),
            category: fake.id("printers").to_owned(),
            fields: FieldMap::default(),
        });
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
            false,
            Some(json!({"username": "ada", "password": "ada's password"})),
            LOCAL,
        )
        .await;
    pop.admin = set.unwrap();
    pop
}

impl Pop {
    async fn send(
        &self,
        method: Method,
        uri: &str,
        signed_in: bool,
        body: Option<Value>,
        from: &str,
    ) -> (StatusCode, Value, Option<String>) {
        let mut req = Request::builder().method(method).uri(uri);
        if signed_in {
            req = req.header(header::COOKIE, &self.admin);
        }
        let mut req = match body {
            Some(b) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string())),
            None => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::empty()),
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

    async fn tower(&self) -> Vec<(String, String)> {
        let (_, body, _) = self
            .send(Method::GET, "/api/admin/printers", true, None, LOCAL)
            .await;
        body.as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["name"].as_str().unwrap().to_owned(),
                    p["tower"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    async fn decide(&self, id: &str, what: &str, from: &str) -> (StatusCode, Value) {
        let (c, b, _) = self
            .send(
                Method::POST,
                &format!("/api/admin/printers/{id}/{what}"),
                true,
                None,
                from,
            )
            .await;
        (c, b)
    }
}

fn state_of(t: &[(String, String)], name: &str) -> String {
    t.iter().find(|(n, _)| n == name).unwrap().1.clone()
}

#[tokio::test]
async fn every_printer_is_offered_and_none_added_silently() {
    let pop = pop().await;
    for (name, state) in pop.tower().await {
        assert_eq!(state, "offered", "{name}");
    }
}

#[tokio::test]
async fn an_admin_adds_a_printer_or_says_it_is_not_one() {
    let pop = pop().await;
    let (c, b) = pop.decide(pop.fake.id("voron"), "add", LOCAL).await;
    assert_eq!(c, StatusCode::NO_CONTENT, "{b}");
    let (c, _) = pop.decide(pop.fake.id("mk4"), "decline", LOCAL).await;
    assert_eq!(c, StatusCode::NO_CONTENT);
    let t = pop.tower().await;
    assert_eq!(state_of(&t, "Voron 2.4"), "added");
    assert_eq!(state_of(&t, "MK4"), "declined");
    assert_eq!(state_of(&t, "X1 Carbon"), "offered");
}

#[tokio::test]
async fn a_printer_with_a_problem_cannot_be_added_and_its_field_is_named() {
    let pop = pop().await;
    let (c, b) = pop.decide(pop.fake.id("incomplete"), "add", LOCAL).await;
    assert_eq!(c, StatusCode::CONFLICT);
    assert!(
        b["error"]
            .as_str()
            .unwrap()
            .contains("Connection (connection)"),
        "{b}"
    );
    assert_eq!(
        state_of(&pop.tower().await, "Ender 3"),
        "offered",
        "not half-added"
    );
}

#[tokio::test]
async fn adding_and_declining_are_recorded() {
    let pop = pop().await;
    pop.decide(pop.fake.id("voron"), "add", LOCAL).await;
    pop.decide(pop.fake.id("mk4"), "decline", LOCAL).await;
    let (_, added, _) = pop
        .send(
            Method::GET,
            "/api/admin/audit?kind=printer_added",
            true,
            None,
            LOCAL,
        )
        .await;
    assert_eq!(added[0]["summary"], "Ada added Voron 2.4 to the tower");
    assert_eq!(added[0]["printer"], pop.fake.id("voron"));
    let (_, declined, _) = pop
        .send(
            Method::GET,
            "/api/admin/audit?kind=printer_declined",
            true,
            None,
            LOCAL,
        )
        .await;
    assert_eq!(
        declined[0]["summary"],
        "Ada said MK4 is not one for the tower"
    );
}

/// Which printers the tower runs is admin-class, which an admin holds from
/// anywhere (cal, #12).
#[tokio::test]
async fn which_printers_the_tower_runs_is_decided_by_an_admin_from_anywhere() {
    let pop = pop().await;
    let (c, b) = pop.decide(pop.fake.id("voron"), "add", REMOTE).await;
    assert!(c.is_success(), "{c} {b}");
    assert_eq!(state_of(&pop.tower().await, "Voron 2.4"), "added");
}

#[tokio::test]
async fn a_printer_not_in_the_category_cannot_be_decided() {
    let pop = pop().await;
    let (c, _) = pop
        .decide("00000000-0000-4000-8000-000000000000", "add", LOCAL)
        .await;
    assert_eq!(c, StatusCode::NOT_FOUND);
}

/// A printer archived in BAM! is retired in POP, through the live sync, and
/// its history stays.
#[tokio::test]
async fn a_printer_archived_in_bam_is_retired_and_keeps_its_history() {
    let pop = pop().await;
    let voron = pop.fake.id("voron").to_owned();
    pop.decide(&voron, "add", LOCAL).await;
    let opts = sync::Options {
        first_retry: Duration::from_millis(50),
        ..sync::Options::default()
    };
    let (view, _sync) = sync::spawn(
        Bam::new(pop.fake.url(), TOKEN).unwrap(),
        pop.fake.id("printers").to_owned(),
        opts,
    );
    let _retirer = tower::spawn_retirer(pop.db.clone(), view.clone());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while !view.borrow().live {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    pop.fake.archive(&voron);
    loop {
        let s = tower::states(&pop.db).await.unwrap();
        if s.get(&voron) == Some(&pop_server::api::TowerState::Retired) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never retired: {s:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (_, log, _) = pop
        .send(
            Method::GET,
            &format!("/api/admin/audit?printer={voron}"),
            true,
            None,
            LOCAL,
        )
        .await;
    let kinds: Vec<_> = log
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["printer_retired", "printer_added"],
        "its history stays: {log}"
    );
    assert_eq!(log[0]["actor_name"], "BAM!");
    assert_eq!(
        log[0]["summary"],
        "Voron 2.4 was archived in BAM!, so it is retired from the tower"
    );
    // Gone from BAM!'s list, it is not offered either; its row stays.
    assert!(!pop.tower().await.iter().any(|(n, _)| n == "Voron 2.4"));
    let kept: String = sqlx::query_scalar("SELECT name FROM tower_printers WHERE bam_id = ?")
        .bind(&voron)
        .fetch_one(&pop.db)
        .await
        .unwrap();
    assert_eq!(kept, "Voron 2.4");

    // Restored in BAM!, it is offered again.
    pop.fake.restore(&voron);
    assert_eq!(state_of(&pop.tower().await, "Voron 2.4"), "offered");
}

#[tokio::test]
async fn only_an_added_printer_is_retired() {
    let pop = pop().await;
    pop.decide(pop.fake.id("mk4"), "decline", LOCAL).await;
    let bam = Bam::new(pop.fake.url(), TOKEN).unwrap();
    pop.fake.archive(pop.fake.id("mk4"));
    pop.fake.archive(pop.fake.id("x1"));
    let archived = bam.assets(pop.fake.id("printers"), true).await.unwrap();
    let retired = tower::retire_archived(&pop.db, &archived).await.unwrap();
    assert!(retired.is_empty(), "{retired:?}");
    let s = tower::states(&pop.db).await.unwrap();
    assert_eq!(
        s.get(pop.fake.id("mk4")),
        Some(&pop_server::api::TowerState::Declined)
    );
}
