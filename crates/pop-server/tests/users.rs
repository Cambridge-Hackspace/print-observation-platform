//! Local users and the two roles (#11), through the router.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use pop_server::{
    AppState,
    access::Who,
    api::Role,
    assets::Embedded,
    audit::Actor,
    db, router, routes,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    admin_id: i64,
    member_id: i64,
}

const ADMIN_PW: &str = "correct horse battery";
const MEMBER_PW: &str = "member's password";

async fn pop() -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let admin = users::create(
        &db,
        NewAccount {
            username: "ada",
            name: "Ada",
            role: Role::Admin,
            password: ADMIN_PW,
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    let member = users::create(
        &db,
        NewAccount {
            username: "grace",
            name: "Grace",
            role: Role::Member,
            password: MEMBER_PW,
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    let app = router(AppState::new(db.clone(), Arc::new(Embedded)));
    Pop {
        _dir: dir,
        db,
        app,
        admin_id: admin.id,
        member_id: member.id,
    }
}

struct Res {
    code: StatusCode,
    body: Value,
    cookie: Option<String>,
}

impl Pop {
    async fn send(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> Res {
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
        // From this machine: these tests are about who, not where (#7's
        // tests are about where).
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                4000,
            ))));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let cookie = res
            .headers()
            .get(header::SET_COOKIE)
            .map(|v| v.to_str().unwrap().to_owned());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Res { code, body, cookie }
    }

    /// Signs in, returning the Cookie header to send.
    async fn sign_in(&self, username: &str, password: &str) -> String {
        let r = self
            .send(
                Method::POST,
                "/api/session",
                None,
                Some(json!({"username": username, "password": password})),
            )
            .await;
        assert_eq!(r.code, StatusCode::OK, "{}", r.body);
        let set = r.cookie.expect("a session cookie");
        set.split(';').next().unwrap().to_owned()
    }
}

// ---- the issue's "done when": role checks on every route group --------------------

/// Every route that names who may use it refuses everyone else: no one
/// signed in gets 401 on any route for members or admins, and a member gets
/// 403 on every admin route, which an admin can reach. Walks the real route
/// table, so a route added later is covered as it lands.
#[tokio::test]
async fn every_route_refuses_whoever_it_does_not_name() {
    let pop = pop().await;
    let member = pop.sign_in("grace", MEMBER_PW).await;
    let admin = pop.sign_in("ada", ADMIN_PW).await;
    let table = routes();
    assert!(
        table.iter().any(|r| r.who == Who::Admin),
        "no admin routes to check"
    );
    for r in &table {
        let uri = r.path.replace("{id}", &pop.member_id.to_string());
        let body =
            matches!(r.method, Method::POST | Method::PUT | Method::PATCH).then(|| json!({}));
        let what = format!("{} {}", r.method, r.path);
        match r.who {
            Who::Anyone => {}
            Who::Member => {
                let anon = pop.send(r.method.clone(), &uri, None, body.clone()).await;
                assert_eq!(anon.code, StatusCode::UNAUTHORIZED, "{what}, signed out");
            }
            Who::Admin => {
                let anon = pop.send(r.method.clone(), &uri, None, body.clone()).await;
                assert_eq!(anon.code, StatusCode::UNAUTHORIZED, "{what}, signed out");
                let m = pop
                    .send(r.method.clone(), &uri, Some(&member), body.clone())
                    .await;
                assert_eq!(m.code, StatusCode::FORBIDDEN, "{what}, as a member");
                let a = pop
                    .send(r.method.clone(), &uri, Some(&admin), body.clone())
                    .await;
                assert!(
                    a.code != StatusCode::UNAUTHORIZED && a.code != StatusCode::FORBIDDEN,
                    "{what}, as an admin: {}",
                    a.code
                );
            }
        }
    }
}

#[tokio::test]
async fn every_admin_route_is_under_api_admin_and_nothing_else_is() {
    // A convention that keeps an admin route recognizable at a glance, and
    // a route under /api/admin from being opened to members by mistake.
    for r in routes() {
        assert_eq!(
            r.path.starts_with("/api/admin/"),
            r.who == Who::Admin,
            "{} {}",
            r.method,
            r.path
        );
    }
}

// ---- signing in and out ----------------------------------------------------------------

#[tokio::test]
async fn signing_in_starts_a_session_that_says_who_you_are() {
    let pop = pop().await;
    let r = pop
        .send(
            Method::POST,
            "/api/session",
            None,
            Some(json!({"username": "Grace", "password": MEMBER_PW})),
        )
        .await;
    assert_eq!(r.code, StatusCode::OK);
    assert_eq!(
        r.body["username"], "grace",
        "usernames match without regard to case"
    );
    let set = r.cookie.unwrap();
    for attr in ["HttpOnly", "SameSite=Lax", "Path=/"] {
        assert!(set.contains(attr), "{attr} missing: {set}");
    }
    let cookie = set.split(';').next().unwrap();
    let me = pop.send(Method::GET, "/api/me", Some(cookie), None).await;
    assert_eq!(me.code, StatusCode::OK);
    assert_eq!(me.body["role"], "member");
    assert!(me.body.get("password_hash").is_none(), "{}", me.body);
}

#[tokio::test]
async fn a_wrong_password_and_an_unknown_user_get_the_same_answer() {
    let pop = pop().await;
    let wrong = pop
        .send(
            Method::POST,
            "/api/session",
            None,
            Some(json!({"username": "grace", "password": "not it at all"})),
        )
        .await;
    let nobody = pop
        .send(
            Method::POST,
            "/api/session",
            None,
            Some(json!({"username": "nobody", "password": "not it at all"})),
        )
        .await;
    assert_eq!(wrong.code, StatusCode::UNAUTHORIZED);
    assert_eq!((wrong.code, &wrong.body), (nobody.code, &nobody.body));
    assert!(wrong.cookie.is_none() && nobody.cookie.is_none());
}

#[tokio::test]
async fn signing_out_ends_the_session() {
    let pop = pop().await;
    let cookie = pop.sign_in("grace", MEMBER_PW).await;
    let out = pop
        .send(Method::DELETE, "/api/session", Some(&cookie), None)
        .await;
    assert_eq!(out.code, StatusCode::NO_CONTENT);
    assert!(out.cookie.unwrap().contains("Max-Age=0"));
    let me = pop.send(Method::GET, "/api/me", Some(&cookie), None).await;
    assert_eq!(
        me.code,
        StatusCode::UNAUTHORIZED,
        "the old cookie no longer works"
    );
}

#[tokio::test]
async fn the_database_holds_neither_password_nor_session_token() {
    let pop = pop().await;
    let cookie = pop.sign_in("grace", MEMBER_PW).await;
    let token = cookie.split_once('=').unwrap().1;
    let hashes: Vec<String> = sqlx::query_scalar("SELECT password_hash FROM users")
        .fetch_all(&pop.db)
        .await
        .unwrap();
    for h in &hashes {
        assert!(h.starts_with("$argon2id$"), "{h}");
        assert!(!h.contains(MEMBER_PW) && !h.contains(ADMIN_PW));
    }
    let stored: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM sessions")
        .fetch_all(&pop.db)
        .await
        .unwrap();
    assert!(!stored.is_empty());
    assert!(stored.iter().all(|s| s != token), "the raw token is stored");
}

#[tokio::test]
async fn a_request_that_changes_something_must_be_json() {
    let pop = pop().await;
    let req = Request::post("/api/session")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("username=grace&password=member%27s+password"))
        .unwrap();
    let res = pop.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert!(res.headers().get(header::SET_COOKIE).is_none());
}

// ---- managing accounts ------------------------------------------------------------------

#[tokio::test]
async fn an_admin_creates_an_account_that_can_sign_in() {
    let pop = pop().await;
    let admin = pop.sign_in("ada", ADMIN_PW).await;
    let r = pop
        .send(
            Method::POST,
            "/api/admin/users",
            Some(&admin),
            Some(json!({"username": "lin", "name": "Lin", "role": "member", "password": "a long password"})),
        )
        .await;
    assert_eq!(r.code, StatusCode::CREATED, "{}", r.body);
    pop.sign_in("lin", "a long password").await;
    let list = pop
        .send(Method::GET, "/api/admin/users", Some(&admin), None)
        .await;
    let names: Vec<_> = list
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["username"].clone())
        .collect();
    assert_eq!(names, vec![json!("ada"), json!("grace"), json!("lin")]);
}

#[tokio::test]
async fn an_account_that_cannot_be_made_names_the_field() {
    let pop = pop().await;
    let admin = pop.sign_in("ada", ADMIN_PW).await;
    for (body, code, field) in [
        (
            json!({"username": "GRACE", "name": "x", "role": "member", "password": "a long password"}),
            StatusCode::CONFLICT,
            "username",
        ),
        (
            json!({"username": "a b", "name": "x", "role": "member", "password": "a long password"}),
            StatusCode::BAD_REQUEST,
            "username",
        ),
        (
            json!({"username": "ok", "name": "  ", "role": "member", "password": "a long password"}),
            StatusCode::BAD_REQUEST,
            "name",
        ),
        (
            json!({"username": "ok", "name": "x", "role": "member", "password": "short"}),
            StatusCode::BAD_REQUEST,
            "password",
        ),
        (
            json!({"username": "ok", "name": "x", "role": "owner", "password": "a long password"}),
            StatusCode::BAD_REQUEST,
            "role",
        ),
    ] {
        let r = pop
            .send(
                Method::POST,
                "/api/admin/users",
                Some(&admin),
                Some(body.clone()),
            )
            .await;
        assert_eq!(r.code, code, "{body}: {}", r.body);
        assert!(
            r.body["error"].as_str().unwrap().contains(field),
            "{body}: {}",
            r.body
        );
    }
}

#[tokio::test]
async fn a_password_reset_works_and_signs_out_whoever_knew_the_old_one() {
    let pop = pop().await;
    let admin = pop.sign_in("ada", ADMIN_PW).await;
    let member = pop.sign_in("grace", MEMBER_PW).await;
    let r = pop
        .send(
            Method::PUT,
            &format!("/api/admin/users/{}/password", pop.member_id),
            Some(&admin),
            Some(json!({"password": "a brand new password"})),
        )
        .await;
    assert_eq!(r.code, StatusCode::NO_CONTENT, "{}", r.body);
    let me = pop.send(Method::GET, "/api/me", Some(&member), None).await;
    assert_eq!(
        me.code,
        StatusCode::UNAUTHORIZED,
        "the old session survived the reset"
    );
    let old = pop
        .send(
            Method::POST,
            "/api/session",
            None,
            Some(json!({"username": "grace", "password": MEMBER_PW})),
        )
        .await;
    assert_eq!(old.code, StatusCode::UNAUTHORIZED);
    pop.sign_in("grace", "a brand new password").await;
}

#[tokio::test]
async fn an_admin_can_change_a_role_but_not_remove_the_last_admin() {
    let pop = pop().await;
    let admin = pop.sign_in("ada", ADMIN_PW).await;
    let last = pop
        .send(
            Method::PUT,
            &format!("/api/admin/users/{}/role", pop.admin_id),
            Some(&admin),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(last.code, StatusCode::CONFLICT, "{}", last.body);
    let promote = pop
        .send(
            Method::PUT,
            &format!("/api/admin/users/{}/role", pop.member_id),
            Some(&admin),
            Some(json!({"role": "admin"})),
        )
        .await;
    assert_eq!(promote.body["role"], "admin");
    // Grace is an admin now, so Ada can step down.
    let demote = pop
        .send(
            Method::PUT,
            &format!("/api/admin/users/{}/role", pop.admin_id),
            Some(&admin),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(demote.code, StatusCode::OK, "{}", demote.body);
    let gone = pop
        .send(Method::GET, "/api/admin/users", Some(&admin), None)
        .await;
    assert_eq!(
        gone.code,
        StatusCode::FORBIDDEN,
        "a demoted admin keeps no admin access"
    );
}

#[tokio::test]
async fn an_unknown_account_is_a_404() {
    let pop = pop().await;
    let admin = pop.sign_in("ada", ADMIN_PW).await;
    let r = pop
        .send(
            Method::PUT,
            "/api/admin/users/9999/role",
            Some(&admin),
            Some(json!({"role": "admin"})),
        )
        .await;
    assert_eq!(r.code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_expired_session_signs_nobody_in() {
    let pop = pop().await;
    let cookie = pop.sign_in("grace", MEMBER_PW).await;
    sqlx::query("UPDATE sessions SET expires_at = ?")
        .bind(users::now() - 1)
        .execute(&pop.db)
        .await
        .unwrap();
    let me = pop.send(Method::GET, "/api/me", Some(&cookie), None).await;
    assert_eq!(me.code, StatusCode::UNAUTHORIZED);
}
