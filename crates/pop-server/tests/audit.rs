//! The audit log (#8). The issue's "done when": each write path has a test
//! asserting its entry. `every_write_route_is_in_this_file` holds that to the
//! route table, so a route that changes something cannot land without one.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use pop_server::{
    AppState,
    api::{AuditKind, Role},
    assets::Embedded,
    audit::{self, Actor, Event},
    db, router, routes,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

/// Every route that changes something, each with the test below that
/// asserts its entry.
const WRITE_ROUTES: &[(&str, &str, &str)] = &[
    (
        "POST",
        "/api/printers/{bam_id}/report",
        "tests/report.rs: a_report_tells_the_group_and_is_recorded",
    ),
    (
        "POST",
        "/api/files/local",
        "tests/slicer_uploads.rs: each_slicers_upload_is_stored_as_its_owners_print",
    ),
    (
        "DELETE",
        "/api/prints/{id}",
        "tests/prints.rs: deleting_a_print_deletes_its_files_and_is_recorded",
    ),
    (
        "POST",
        "/api/feedback",
        "tests/feedback.rs: a_report_becomes_an_issue_that_shows_its_screenshot",
    ),
    (
        "POST",
        "/api/session",
        "signing_in_is_recorded, a_failed_sign_in_is_recorded_without_the_password",
    ),
    ("DELETE", "/api/session", "signing_out_is_recorded"),
    (
        "POST",
        "/api/admin/users",
        "creating_an_account_is_recorded",
    ),
    (
        "PUT",
        "/api/admin/users/{id}/password",
        "a_password_reset_is_recorded",
    ),
    (
        "PUT",
        "/api/admin/users/{id}/role",
        "a_role_change_is_recorded_and_a_change_to_the_same_role_is_not",
    ),
    (
        "PUT",
        "/api/admin/users/{id}/name",
        "tests/users.rs: renaming_deactivating_and_training_are_recorded_and_no_change_is_not",
    ),
    (
        "PUT",
        "/api/admin/users/{id}/active",
        "tests/users.rs: renaming_deactivating_and_training_are_recorded_and_no_change_is_not",
    ),
    (
        "PUT",
        "/api/admin/users/{id}/training",
        "tests/users.rs: renaming_deactivating_and_training_are_recorded_and_no_change_is_not",
    ),
    (
        "PUT",
        "/api/admin/printers/{bam_id}/access-code",
        "tests/printers.rs: entering_an_access_code_is_recorded_without_the_code",
    ),
    (
        "POST",
        "/api/admin/printers/{bam_id}/add",
        "tests/tower.rs: adding_and_declining_are_recorded",
    ),
    (
        "POST",
        "/api/admin/printers/{bam_id}/decline",
        "tests/tower.rs: adding_and_declining_are_recorded",
    ),
    (
        "PUT",
        "/api/admin/notify/discord",
        "tests/notify.rs: setting_clearing_and_testing_are_recorded_without_the_url",
    ),
    (
        "DELETE",
        "/api/admin/notify/discord",
        "tests/notify.rs: setting_clearing_and_testing_are_recorded_without_the_url",
    ),
    (
        "POST",
        "/api/admin/notify/discord/test",
        "tests/notify.rs: setting_clearing_and_testing_are_recorded_without_the_url",
    ),
    (
        "POST",
        "/api/admin/guides",
        "tests/guides.rs: writing_editing_publishing_and_renaming_are_recorded",
    ),
    (
        "PUT",
        "/api/admin/guides/{slug}",
        "tests/guides.rs: writing_editing_publishing_and_renaming_are_recorded",
    ),
    (
        "POST",
        "/api/admin/guides/{slug}/images",
        "tests/guides.rs: writing_editing_publishing_and_renaming_are_recorded",
    ),
    (
        "PUT",
        "/api/admin/printers/{bam_id}/camera-login",
        "tests/printers.rs: a_camera_login_is_entered_kept_sealed_recorded_and_removed",
    ),
    (
        "DELETE",
        "/api/admin/printers/{bam_id}/camera-login",
        "tests/printers.rs: a_camera_login_is_entered_kept_sealed_recorded_and_removed",
    ),
    (
        "POST",
        "/api/keys",
        "tests/api_keys.rs: a_key_is_shown_once_and_kept_only_as_a_hash",
    ),
    (
        "DELETE",
        "/api/keys/{id}",
        "tests/api_keys.rs: a_key_works_at_the_slicers_route_and_once_revoked_is_refused",
    ),
    (
        "PUT",
        "/api/admin/printers/{bam_id}/printer-login",
        "tests/printers.rs: a_prusalink_login_is_entered_kept_sealed_bound_recorded_and_removed",
    ),
    (
        "DELETE",
        "/api/admin/printers/{bam_id}/printer-login",
        "tests/printers.rs: a_prusalink_login_is_entered_kept_sealed_bound_recorded_and_removed",
    ),
    (
        "POST",
        "/api/admin/kiosks",
        "tests/kiosks.rs: an_admin_enrolls_a_kiosk_whose_credential_pop_keeps_only_as_a_hash",
    ),
    (
        "DELETE",
        "/api/admin/kiosks/{id}",
        "tests/kiosks.rs: a_revoked_kiosk_vouches_for_nothing_at_once",
    ),
    // #19: a card tap's sign-in is recorded as any sign-in is; its card
    // reader's setting is recorded; a tap set aside is deliberately not.
    (
        "POST",
        "/api/kiosk/tap",
        "tests/card_sign_in.rs: a_tap_at_a_kiosks_reader_signs_the_member_in_there",
    ),
    (
        "DELETE",
        "/api/kiosk/tap",
        "tests/card_sign_in.rs: a_refusal_is_shown_and_set_aside",
    ),
    (
        "PUT",
        "/api/admin/kiosks/{id}/card-reader",
        "tests/card_sign_in.rs: a_card_reader_belongs_to_one_kiosk_and_is_recorded",
    ),
    // A check-in every half minute is deliberately NOT an audit entry.
    (
        "POST",
        "/api/kiosk/checkin",
        "tests/kiosks.rs: a_checkin_is_not_an_audit_entry",
    ),
    (
        "PUT",
        "/api/admin/error-guides/{key}",
        "tests/error_guides.rs: linking_and_unlinking_are_recorded",
    ),
];

#[test]
fn every_write_route_is_in_this_file() {
    let mut writes: Vec<(String, String)> = routes()
        .iter()
        .filter(|r| r.method != Method::GET && r.method != Method::HEAD)
        .map(|r| (r.method.to_string(), r.path.to_owned()))
        .collect();
    writes.sort();
    let mut listed: Vec<(String, String)> = WRITE_ROUTES
        .iter()
        .map(|(m, p, _)| ((*m).to_owned(), (*p).to_owned()))
        .collect();
    listed.sort();
    assert_eq!(
        writes, listed,
        "the route table's write routes and this file's list differ: give each new one a test asserting its audit entry"
    );
}

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    admin: String,
    grace: i64,
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
    let grace = users::create(
        &db,
        NewAccount {
            username: "grace",
            name: "Grace",
            role: Role::Member,
            password: "grace's password",
        },
        &Actor::command_line(),
    )
    .await
    .unwrap()
    .id;
    let app = router(AppState::new(db.clone(), Arc::new(Embedded)));
    let mut pop = Pop {
        _dir: dir,
        db,
        app,
        admin: String::new(),
        grace,
    };
    pop.admin = pop.sign_in("ada", "ada's password").await;
    pop
}

impl Pop {
    async fn send(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value, Option<String>) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        let req = match body {
            Some(b) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
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

    async fn sign_in(&self, u: &str, p: &str) -> String {
        let (code, body, set) = self
            .send(
                Method::POST,
                "/api/session",
                None,
                Some(json!({"username": u, "password": p})),
            )
            .await;
        assert_eq!(code, StatusCode::OK, "{body}");
        set.unwrap()
    }

    /// The newest entry of `kind`, through the admin's API.
    async fn newest(&self, kind: &str) -> Value {
        let (code, body, _) = self
            .send(
                Method::GET,
                &format!("/api/admin/audit?kind={kind}&limit=1"),
                Some(&self.admin),
                None,
            )
            .await;
        assert_eq!(code, StatusCode::OK, "{body}");
        body.as_array()
            .unwrap()
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("no {kind} entry"))
    }
}

// ---- one test per write path --------------------------------------------------------

#[tokio::test]
async fn signing_in_is_recorded() {
    let pop = pop().await;
    pop.sign_in("grace", "grace's password").await;
    let e = pop.newest("signed_in").await;
    assert_eq!(e["actor_id"], pop.grace);
    assert_eq!(e["actor_name"], "Grace");
    // No kiosk here, so this harness is outside the space (#12); a sign-in
    // at the space is tests/kiosks.rs: a_kiosks_checkin_makes_its_own_network_local.
    assert_eq!(e["summary"], "Grace signed in from outside the space");
}

#[tokio::test]
async fn a_failed_sign_in_is_recorded_without_the_password() {
    let pop = pop().await;
    let (code, _, _) = pop
        .send(
            Method::POST,
            "/api/session",
            None,
            Some(json!({"username": "grace", "password": "hunter2-guess"})),
        )
        .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    let e = pop.newest("sign_in_failed").await;
    assert_eq!(e["actor_id"], Value::Null);
    assert!(e["summary"].as_str().unwrap().contains("\"grace\""), "{e}");
    assert!(
        !e.to_string().contains("hunter2"),
        "the password tried reached the log: {e}"
    );
}

#[tokio::test]
async fn signing_out_is_recorded() {
    let pop = pop().await;
    let c = pop.sign_in("grace", "grace's password").await;
    let (code, _, _) = pop
        .send(Method::DELETE, "/api/session", Some(&c), None)
        .await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    let e = pop.newest("signed_out").await;
    assert_eq!(e["actor_id"], pop.grace);
    assert_eq!(e["summary"], "Grace signed out");
}

#[tokio::test]
async fn creating_an_account_is_recorded() {
    let pop = pop().await;
    let (code, body, _) = pop
        .send(
            Method::POST,
            "/api/admin/users",
            Some(&pop.admin),
            Some(json!({"username": "lin", "name": "Lin", "role": "member", "password": "lin's password"})),
        )
        .await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
    let e = pop.newest("user_created").await;
    assert_eq!(e["actor_name"], "Ada");
    assert_eq!(e["subject_id"], body["id"]);
    assert_eq!(e["summary"], "Ada created Lin's account (lin, member)");
}

#[tokio::test]
async fn creating_an_account_from_the_command_line_is_recorded() {
    let pop = pop().await;
    let e = pop
        .send(
            Method::GET,
            &format!("/api/admin/audit?kind=user_created&person={}", pop.grace),
            Some(&pop.admin),
            None,
        )
        .await
        .1;
    assert_eq!(e[0]["actor_id"], Value::Null);
    assert_eq!(
        e[0]["summary"],
        "the command line created Grace's account (grace, member)"
    );
}

#[tokio::test]
async fn a_password_reset_is_recorded() {
    let pop = pop().await;
    pop.sign_in("grace", "grace's password").await;
    let (code, _, _) = pop
        .send(
            Method::PUT,
            &format!("/api/admin/users/{}/password", pop.grace),
            Some(&pop.admin),
            Some(json!({"password": "a new password"})),
        )
        .await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    let e = pop.newest("password_reset").await;
    assert_eq!(e["subject_id"], pop.grace);
    assert_eq!(
        e["summary"],
        "Ada reset Grace's password, ending 1 sign-in(s)"
    );
    assert!(!e.to_string().contains("a new password"));
}

#[tokio::test]
async fn a_role_change_is_recorded_and_a_change_to_the_same_role_is_not() {
    let pop = pop().await;
    let uri = format!("/api/admin/users/{}/role", pop.grace);
    pop.send(
        Method::PUT,
        &uri,
        Some(&pop.admin),
        Some(json!({"role": "admin"})),
    )
    .await;
    let e = pop.newest("role_changed").await;
    assert_eq!(e["summary"], "Ada made Grace an admin (was member)");
    pop.send(
        Method::PUT,
        &uri,
        Some(&pop.admin),
        Some(json!({"role": "admin"})),
    )
    .await;
    let all = pop
        .send(
            Method::GET,
            "/api/admin/audit?kind=role_changed",
            Some(&pop.admin),
            None,
        )
        .await
        .1;
    assert_eq!(
        all.as_array().unwrap().len(),
        1,
        "nothing changed, so nothing to record: {all}"
    );
}

// ---- a change and its entry are one --------------------------------------------------

/// A change whose entry cannot be written does not happen: they share one
/// transaction.
#[tokio::test]
async fn a_change_that_cannot_be_recorded_does_not_happen() {
    let pop = pop().await;
    sqlx::query("DROP TABLE audit")
        .execute(&pop.db)
        .await
        .unwrap();
    let (code, _, _) = pop
        .send(
            Method::POST,
            "/api/admin/users",
            Some(&pop.admin),
            Some(json!({"username": "lin", "name": "Lin", "role": "member", "password": "lin's password"})),
        )
        .await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE username = 'lin'")
        .fetch_one(&pop.db)
        .await
        .unwrap();
    assert_eq!(n, 0, "the account was created without its entry");
}

// ---- reading it ----------------------------------------------------------------------

async fn seed_printer_events(db: &SqlitePool) {
    let mut conn = db.acquire().await.unwrap();
    for (printer, summary) in [
        ("x1", "a print went to X1"),
        ("mk4", "a print went to MK4"),
        ("x1", "X1 was paused"),
    ] {
        audit::record(
            &mut conn,
            &Actor::user(1, "Ada"),
            Event {
                kind: AuditKind::SignedIn,
                printer: Some(printer),
                subject: None,
                summary: summary.to_owned(),
            },
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn the_log_filters_by_person_printer_and_kind_newest_first() {
    let pop = pop().await;
    pop.sign_in("grace", "grace's password").await;
    seed_printer_events(&pop.db).await;
    let get = |q: String| {
        let pop = &pop;
        async move {
            pop.send(
                Method::GET,
                &format!("/api/admin/audit?{q}"),
                Some(&pop.admin),
                None,
            )
            .await
            .1
        }
    };
    let x1 = get("printer=x1".into()).await;
    let s: Vec<_> = x1
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["summary"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        s,
        ["X1 was paused", "a print went to X1"],
        "newest first, X1 only"
    );

    let grace = get(format!("person={}", pop.grace)).await;
    assert!(
        grace
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["actor_id"] == pop.grace || e["subject_id"] == pop.grace)
    );
    let kinds: Vec<_> = grace
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["signed_in", "user_created"], "{grace}");

    let failed = get("kind=sign_in_failed".into()).await;
    assert_eq!(failed.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn the_log_pages_back_with_before_and_limit() {
    let pop = pop().await;
    seed_printer_events(&pop.db).await;
    let page1 = pop
        .send(
            Method::GET,
            "/api/admin/audit?limit=2",
            Some(&pop.admin),
            None,
        )
        .await
        .1;
    let p1 = page1.as_array().unwrap();
    assert_eq!(p1.len(), 2);
    let last = p1[1]["id"].as_i64().unwrap();
    let page2 = pop
        .send(
            Method::GET,
            &format!("/api/admin/audit?limit=2&before={last}"),
            Some(&pop.admin),
            None,
        )
        .await
        .1;
    let p2 = page2.as_array().unwrap();
    assert!(
        p2.iter().all(|e| e["id"].as_i64().unwrap() < last),
        "{page2}"
    );
    assert_eq!(p2.len(), 2);
}

#[tokio::test]
async fn an_entry_of_a_kind_this_build_does_not_know_is_shown_not_hidden() {
    let pop = pop().await;
    sqlx::query("INSERT INTO audit (at, kind, summary) VALUES (1, 'from_the_future', 'something new happened')")
        .execute(&pop.db)
        .await
        .unwrap();
    let all = pop
        .send(Method::GET, "/api/admin/audit", Some(&pop.admin), None)
        .await
        .1;
    let e = all
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["summary"] == "something new happened")
        .expect("hidden");
    assert_eq!(e["kind"], "other");
}

#[tokio::test]
async fn a_malformed_filter_is_refused() {
    let pop = pop().await;
    let (code, _, _) = pop
        .send(
            Method::GET,
            "/api/admin/audit?kind=everything",
            Some(&pop.admin),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
}
