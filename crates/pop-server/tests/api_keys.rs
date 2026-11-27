//! Personal API keys (#13): shown once, kept hashed, revocable by their owner,
//! and good for the slicer's routes and nothing else.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use pop_server::{
    AppState,
    access::Need,
    api::Role,
    assets::Embedded,
    audit::Actor,
    config::Access,
    db, router, routes,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

/// Inside the space: a configured network with a kiosk checked in from it.
const LOCAL: &str = "192.168.1.40:5000";
/// Outside it.
const REMOTE: &str = "203.0.113.9:5000";

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    grace: String,
    grace_id: i64,
    ada: String,
}

async fn pop() -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let account = |role, username: &'static str| {
        let db = db.clone();
        async move {
            let u = users::create(
                &db,
                NewAccount {
                    username,
                    name: username,
                    role,
                    password: "a long enough password",
                },
                &Actor::command_line(),
            )
            .await
            .unwrap();
            let t = users::start_session(&db, &u, false).await.unwrap();
            (format!("pop_session={t}"), u.id)
        }
    };
    let (grace, grace_id) = account(Role::Member, "grace").await;
    let (ada, _) = account(Role::Admin, "ada").await;
    let state = AppState::new(db.clone(), Arc::new(Embedded)).with_access(Access {
        local_networks: vec!["192.168.1.0/24".parse().unwrap()],
        trusted_proxies: vec![],
    });
    state.kiosks.saw_at(
        1,
        "192.168.1.10".parse().unwrap(),
        Instant::now() - Duration::from_secs(5),
    );
    let app = router(state);
    Pop {
        _dir: dir,
        db,
        app,
        grace,
        grace_id,
        ada,
    }
}

struct Res {
    code: StatusCode,
    body: Value,
}

impl Pop {
    /// A request, read in full.
    async fn send(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        key: Option<&str>,
        body: Option<Value>,
    ) -> Res {
        let res = self.raw(method, uri, LOCAL, cookie, key, body).await;
        let code = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        Res {
            code,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    /// A request, its body left unread: some routes stream for ever.
    async fn raw(
        &self,
        method: Method,
        uri: &str,
        peer: &str,
        cookie: Option<&str>,
        key: Option<&str>,
        body: Option<Value>,
    ) -> axum::response::Response {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        if let Some(k) = key {
            req = req.header("X-Api-Key", k);
        }
        let mut req = match body {
            Some(b) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        self.app.clone().oneshot(req).await.unwrap()
    }

    /// A new key for `cookie`'s account: (its id, the key).
    async fn make(&self, cookie: &str, name: &str) -> (i64, String) {
        let r = self
            .send(
                Method::POST,
                "/api/keys",
                Some(cookie),
                None,
                Some(json!({"name": name})),
            )
            .await;
        assert_eq!(r.code, StatusCode::CREATED, "{}", r.body);
        (
            r.body["key"]["id"].as_i64().unwrap(),
            r.body["secret"].as_str().unwrap().to_owned(),
        )
    }

    async fn version(&self, key: &str) -> Res {
        self.send(Method::GET, "/api/version", None, Some(key), None)
            .await
    }
}

#[tokio::test]
async fn a_key_is_shown_once_and_kept_only_as_a_hash() {
    let pop = pop().await;
    let r = pop
        .send(
            Method::POST,
            "/api/keys",
            Some(&pop.grace),
            None,
            Some(json!({"name": "PrusaSlicer on my laptop"})),
        )
        .await;
    assert_eq!(r.code, StatusCode::CREATED, "{}", r.body);
    let secret = r.body["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("pop_") && secret.len() > 40, "{secret}");
    let prefix = r.body["key"]["prefix"].as_str().unwrap();
    assert!(
        secret.starts_with(prefix) && prefix.len() < secret.len() / 3,
        "{prefix}"
    );

    // Listed by name and prefix, never the key.
    let list = pop
        .send(Method::GET, "/api/keys", Some(&pop.grace), None, None)
        .await;
    assert_eq!(list.code, StatusCode::OK);
    assert_eq!(list.body[0]["name"], "PrusaSlicer on my laptop");
    assert!(!list.body.to_string().contains(&secret));
    // Nothing in the database holds it.
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT name, prefix, token_hash FROM api_keys")
            .fetch_all(&pop.db)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        rows.iter()
            .all(|(a, b, c)| ![a, b, c].iter().any(|v| v.contains(&secret)))
    );
    // Nor the audit log.
    let log: Vec<String> = sqlx::query_scalar("SELECT summary FROM audit")
        .fetch_all(&pop.db)
        .await
        .unwrap();
    assert!(
        log.iter()
            .any(|s| s.contains("made the API key \"PrusaSlicer on my laptop\"")),
        "{log:?}"
    );
    assert!(!log.iter().any(|s| s.contains(&secret)));
    // Another account sees none of it.
    let theirs = pop
        .send(Method::GET, "/api/keys", Some(&pop.ada), None, None)
        .await;
    assert_eq!(theirs.body, json!([]));
}

#[tokio::test]
async fn a_key_works_at_the_slicers_route_and_once_revoked_is_refused() {
    let pop = pop().await;
    let (id, key) = pop.make(&pop.grace, "Orca").await;
    let ok = pop.version(&key).await;
    assert_eq!(ok.code, StatusCode::OK, "{}", ok.body);
    // The slicers refuse a host whose text does not start "OctoPrint"
    // (tests/slicer_uploads.rs replays their requests); POP names itself
    // after it.
    let text = ok.body["text"].as_str().unwrap();
    assert!(
        text.starts_with("OctoPrint ") && text.contains("(POP "),
        "{text}"
    );
    let list = pop
        .send(Method::GET, "/api/keys", Some(&pop.grace), None, None)
        .await;
    assert!(
        list.body[0]["last_used_at"].is_number(),
        "its use is shown: {}",
        list.body
    );

    let uri = format!("/api/keys/{id}");
    assert_eq!(
        pop.send(Method::DELETE, &uri, Some(&pop.grace), None, None)
            .await
            .code,
        StatusCode::NO_CONTENT
    );
    let refused = pop.version(&key).await;
    assert_eq!(refused.code, StatusCode::UNAUTHORIZED);
    assert!(
        refused.body["error"].as_str().unwrap().contains("revoked"),
        "{}",
        refused.body
    );
    assert_eq!(
        pop.send(Method::DELETE, &uri, Some(&pop.grace), None, None)
            .await
            .code,
        StatusCode::NOT_FOUND
    );
    let list = pop
        .send(Method::GET, "/api/keys", Some(&pop.grace), None, None)
        .await;
    assert!(
        list.body[0]["revoked_at"].is_number(),
        "kept, marked revoked: {}",
        list.body
    );
    let log: Vec<String> = sqlx::query_scalar("SELECT summary FROM audit")
        .fetch_all(&pop.db)
        .await
        .unwrap();
    assert!(
        log.iter()
            .any(|s| s.contains("revoked the API key \"Orca\"")),
        "{log:?}"
    );
}

#[tokio::test]
async fn only_its_owner_can_revoke_a_key() {
    let pop = pop().await;
    let (id, key) = pop.make(&pop.grace, "Orca").await;
    // Not even an admin: someone else's key is not found.
    let r = pop
        .send(
            Method::DELETE,
            &format!("/api/keys/{id}"),
            Some(&pop.ada),
            None,
            None,
        )
        .await;
    assert_eq!(r.code, StatusCode::NOT_FOUND);
    assert_eq!(
        pop.version(&key).await.code,
        StatusCode::OK,
        "it still works"
    );
}

#[tokio::test]
async fn a_deactivated_accounts_keys_are_refused_while_it_is() {
    let pop = pop().await;
    let (_, key) = pop.make(&pop.grace, "Orca").await;
    users::set_active(&pop.db, pop.grace_id, false, &Actor::command_line())
        .await
        .unwrap();
    assert_eq!(pop.version(&key).await.code, StatusCode::UNAUTHORIZED);
    users::set_active(&pop.db, pop.grace_id, true, &Actor::command_line())
        .await
        .unwrap();
    assert_eq!(pop.version(&key).await.code, StatusCode::OK);
}

#[tokio::test]
async fn what_cannot_be_a_key_is_refused_and_signing_in_comes_first() {
    let pop = pop().await;
    for name in [
        json!(""),
        json!("   "),
        json!("x".repeat(65)),
        json!("line\nbreak"),
    ] {
        let r = pop
            .send(
                Method::POST,
                "/api/keys",
                Some(&pop.grace),
                None,
                Some(json!({"name": name})),
            )
            .await;
        assert_eq!(r.code, StatusCode::BAD_REQUEST, "{name}");
    }
    let r = pop
        .send(
            Method::POST,
            "/api/keys",
            None,
            None,
            Some(json!({"name": "x"})),
        )
        .await;
    assert_eq!(r.code, StatusCode::UNAUTHORIZED);
    // Not a key POP made, nor one shaped like one.
    for k in [
        "",
        "pop_not-a-real-key",
        "a-key-from-octoprint-0123456789abcdef",
    ] {
        assert_eq!(pop.version(k).await.code, StatusCode::UNAUTHORIZED, "{k:?}");
    }
    // And a key cannot make a key: it is not an account's session.
    let (_, key) = pop.make(&pop.grace, "Orca").await;
    let r = pop
        .send(
            Method::POST,
            "/api/keys",
            None,
            Some(&key),
            Some(json!({"name": "another"})),
        )
        .await;
    assert_eq!(r.code, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn one_person_has_at_most_twenty_live_keys() {
    let pop = pop().await;
    for i in 0..20 {
        pop.make(&pop.grace, &format!("key {i}")).await;
    }
    let r = pop
        .send(
            Method::POST,
            "/api/keys",
            Some(&pop.grace),
            None,
            Some(json!({"name": "one more"})),
        )
        .await;
    assert_eq!(r.code, StatusCode::CONFLICT, "{}", r.body);
    // Revoking one makes room.
    let list = pop
        .send(Method::GET, "/api/keys", Some(&pop.grace), None, None)
        .await;
    let id = list.body[0]["id"].as_i64().unwrap();
    pop.send(
        Method::DELETE,
        &format!("/api/keys/{id}"),
        Some(&pop.grace),
        None,
        None,
    )
    .await;
    pop.make(&pop.grace, "one more").await;
}

/// The issue's "done when": a key reaches no route but the slicer's. On every
/// other route, a request carrying a live admin's key is answered exactly as
/// the same request without it, inside the space and outside: the key adds
/// nothing, so it reaches nothing that is local-only, controls a printer or
/// changes a setting. Walks the real route table, so a route added later is
/// covered as it lands.
#[tokio::test]
async fn a_key_adds_nothing_to_any_route_but_the_slicers() {
    let pop = pop().await;
    // An admin's key: the most a key could carry, if it carried anything.
    let (_, key) = pop.make(&pop.ada, "Orca").await;
    // The precondition: the key is live where keys are meant to work.
    assert_eq!(pop.version(&key).await.code, StatusCode::OK);
    let table = routes();
    let mut checked = 0;
    for r in &table {
        if r.need == Need::Key {
            continue;
        }
        let uri = r
            .path
            .replace("{id}", "1")
            .replace("{bam_id}", "x1")
            .replace("{slug}", "a-guide")
            .replace("{key}", "k");
        let body =
            matches!(r.method, Method::POST | Method::PUT | Method::PATCH).then(|| json!({}));
        for peer in [LOCAL, REMOTE] {
            let without = pop
                .raw(r.method.clone(), &uri, peer, None, None, body.clone())
                .await
                .status();
            let with = pop
                .raw(r.method.clone(), &uri, peer, None, Some(&key), body.clone())
                .await
                .status();
            assert_eq!(
                with, without,
                "{} {} from {peer}: a key changed the answer",
                r.method, r.path
            );
        }
        checked += 1;
    }
    assert!(checked > 40, "only {checked} routes walked");
    // And the slicer's route itself is not reached by a session in its place.
    let r = pop
        .raw(
            Method::GET,
            "/api/version",
            LOCAL,
            Some(&pop.ada),
            None,
            None,
        )
        .await
        .status();
    assert_eq!(r, StatusCode::UNAUTHORIZED);
}
