//! Kiosks (#12): enrolled once by an admin, they confirm from their own side
//! that they are at the space, and a network is the space's only while one
//! has; and an admin signing in from outside is told to the group.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, State},
    http::{HeaderMap, Method, Request, StatusCode, header},
    routing::post,
};
use http_body_util::BodyExt;
use pop_server::{
    AppState,
    access::is_local,
    api::Role,
    assets::Embedded,
    audit::Actor,
    config::Access,
    db,
    notify::{self, Options, WEBHOOK},
    router,
    secrets::Secrets,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
const KIOSK_AT: &str = "192.168.1.5:4000";
const IN_SHOP: &str = "192.168.1.40:4000";
const OUTSIDE: &str = "203.0.113.9:4000";

/// The group chat: every message the notifier sends.
#[derive(Clone, Default)]
struct Chat(Arc<Mutex<Vec<String>>>);

async fn hook(State(chat): State<Chat>, body: String) -> StatusCode {
    let v: Value = serde_json::from_str(&body).unwrap();
    chat.0
        .lock()
        .unwrap()
        .push(v["content"].as_str().unwrap().to_owned());
    StatusCode::NO_CONTENT
}

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    state: AppState,
    app: Router,
    chat: Chat,
    admin: String,
    member: String,
}

async fn pop() -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    for (username, name, role) in [
        ("ada", "Ada", Role::Admin),
        ("grace", "Grace", Role::Member),
    ] {
        users::create(
            &db,
            NewAccount {
                username,
                name,
                role,
                password: "a long enough password",
            },
            &Actor::command_line(),
        )
        .await
        .unwrap();
    }
    let chat = Chat::default();
    let app = Router::new()
        .route("/api/webhooks/{id}/{token}", post(hook))
        .with_state(chat.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let secrets = Secrets::new(KEY);
    secrets
        .set_setting(&db, WEBHOOK, &format!("http://{addr}/api/webhooks/1/t"))
        .await
        .unwrap();
    let (notifier, _) = notify::spawn(
        db.clone(),
        secrets.clone(),
        reqwest::Client::new(),
        Options {
            first_retry: Duration::from_millis(50),
            max_retry: Duration::from_millis(200),
            attempts: 2,
            timeout: Duration::from_secs(2),
        },
    );
    let state = AppState::new(db.clone(), Arc::new(Embedded))
        .with_access(Access {
            local_networks: vec!["192.168.1.0/24".parse().unwrap()],
            trusted_proxies: vec![],
        })
        .with_secrets(secrets)
        .with_notifier(notifier);
    let app = router(state.clone());
    let mut pop = Pop {
        _dir: dir,
        db,
        state,
        app,
        chat,
        admin: String::new(),
        member: String::new(),
    };
    pop.admin = pop.sign_in("ada", OUTSIDE).await;
    pop.member = pop.sign_in("grace", OUTSIDE).await;
    pop.chat.0.lock().unwrap().clear();
    pop
}

struct Res {
    code: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Pop {
    async fn send(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
        from: &str,
        extra: &[(&str, &str)],
    ) -> Res {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        for (k, v) in extra {
            req = req.header(*k, *v);
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
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        Res {
            code,
            headers,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    async fn sign_in(&self, username: &str, from: &str) -> String {
        let r = self
            .send(
                Method::POST,
                "/api/session",
                None,
                Some(json!({"username": username, "password": "a long enough password"})),
                from,
                &[],
            )
            .await;
        assert_eq!(r.code, StatusCode::OK, "{}", r.body);
        set_cookie(&r.headers, "pop_session")
    }

    /// Enroll a kiosk as the admin, from `from`: its credential's cookie.
    async fn enroll(&self, name: &str, from: &str) -> String {
        let r = self
            .send(
                Method::POST,
                "/api/admin/kiosks",
                Some(&self.admin),
                Some(json!({"name": name})),
                from,
                &[],
            )
            .await;
        assert_eq!(r.code, StatusCode::CREATED, "{}", r.body);
        set_cookie(&r.headers, "pop_kiosk")
    }

    async fn checkin(&self, kiosk: Option<&str>, from: &str, extra: &[(&str, &str)]) -> StatusCode {
        self.send(
            Method::POST,
            "/api/kiosk/checkin",
            kiosk,
            Some(json!({})),
            from,
            extra,
        )
        .await
        .code
    }

    fn local(&self, from: &str) -> bool {
        is_local(
            &self.state,
            Some(from.parse::<SocketAddr>().unwrap().ip()),
            &HeaderMap::new(),
        )
    }

    async fn audit(&self, kind: &str) -> Vec<String> {
        sqlx::query_scalar("SELECT summary FROM audit WHERE kind = ? ORDER BY id")
            .bind(kind)
            .fetch_all(&self.db)
            .await
            .unwrap()
    }
}

/// The `name=value` of the cookie `name` a response sets.
fn set_cookie(h: &HeaderMap, name: &str) -> String {
    h.get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .find(|v| v.starts_with(&format!("{name}=")))
        .unwrap_or_else(|| panic!("no {name} cookie in {h:?}"))
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn an_admin_enrolls_a_kiosk_whose_credential_pop_keeps_only_as_a_hash() {
    let pop = pop().await;
    let r = pop
        .send(
            Method::POST,
            "/api/admin/kiosks",
            Some(&pop.admin),
            Some(json!({"name": "The tower"})),
            OUTSIDE,
            &[],
        )
        .await;
    assert_eq!(r.code, StatusCode::CREATED, "{}", r.body);
    let raw = r.headers[header::SET_COOKIE].to_str().unwrap().to_owned();
    // Path=/api: the check-in reads it, and so does signing in (#12).
    for part in ["HttpOnly", "SameSite=Strict", "Path=/api;"] {
        assert!(raw.contains(part), "{raw}");
    }
    let token = set_cookie(&r.headers, "pop_kiosk")["pop_kiosk=".len()..].to_owned();
    assert!(token.len() >= 40, "{token}");
    let stored: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM kiosks")
        .fetch_all(&pop.db)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_ne!(stored[0], token, "the credential itself was kept");
    assert_eq!(stored[0].len(), 64);
    assert_eq!(
        pop.audit("kiosk_enrolled").await,
        ["Ada enrolled the kiosk The tower"]
    );
    // Nobody else enrolls one, and a name is needed.
    let as_member = pop
        .send(
            Method::POST,
            "/api/admin/kiosks",
            Some(&pop.member),
            Some(json!({"name": "x"})),
            IN_SHOP,
            &[],
        )
        .await;
    assert_eq!(as_member.code, StatusCode::FORBIDDEN);
    let signed_out = pop
        .send(
            Method::POST,
            "/api/admin/kiosks",
            None,
            Some(json!({"name": "x"})),
            IN_SHOP,
            &[],
        )
        .await;
    assert_eq!(signed_out.code, StatusCode::UNAUTHORIZED);
    for bad in ["", "   ", &"k".repeat(65), "line\nbreak"] {
        let r = pop
            .send(
                Method::POST,
                "/api/admin/kiosks",
                Some(&pop.admin),
                Some(json!({"name": bad})),
                IN_SHOP,
                &[],
            )
            .await;
        assert_eq!(r.code, StatusCode::BAD_REQUEST, "{bad:?}");
    }
}

/// A kiosk's check-in makes its network the space's, and only its network:
/// told by is_local, and by what a sign-in from that network records.
#[tokio::test]
async fn a_kiosks_checkin_makes_its_own_network_local() {
    let pop = pop().await;
    let kiosk = pop.enroll("The tower", OUTSIDE).await;
    assert!(!pop.local(IN_SHOP), "local before any check-in");
    pop.sign_in("grace", IN_SHOP).await;
    assert_eq!(
        pop.checkin(Some(&kiosk), KIOSK_AT, &[]).await,
        StatusCode::NO_CONTENT
    );
    assert!(pop.local(IN_SHOP));
    assert!(!pop.local(OUTSIDE));
    pop.sign_in("grace", IN_SHOP).await;
    let signed_in = pop.audit("signed_in").await;
    let last_two = &signed_in[signed_in.len() - 2..];
    assert_eq!(
        last_two,
        ["Grace signed in from outside the space", "Grace signed in"],
        "the same address, before and after the kiosk's word"
    );
    // The desk sees where and when it last checked in.
    let list = pop
        .send(
            Method::GET,
            "/api/admin/kiosks",
            Some(&pop.admin),
            None,
            OUTSIDE,
            &[],
        )
        .await;
    assert_eq!(list.body[0]["last_address"], "192.168.1.5");
    assert!(list.body[0]["last_seen_at"].is_number(), "{}", list.body);
    assert_eq!(list.body[0]["revoked"], false);
}

#[tokio::test]
async fn a_checkin_without_a_kiosks_credential_counts_for_nothing() {
    let pop = pop().await;
    pop.enroll("The tower", OUTSIDE).await;
    assert_eq!(
        pop.checkin(None, KIOSK_AT, &[]).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        pop.checkin(Some("pop_kiosk=made-up"), KIOSK_AT, &[]).await,
        StatusCode::UNAUTHORIZED
    );
    // A session is not a kiosk credential, an admin's included.
    assert_eq!(
        pop.checkin(Some(&pop.admin), KIOSK_AT, &[]).await,
        StatusCode::UNAUTHORIZED
    );
    assert!(!pop.local(IN_SHOP));
}

/// POP believes where a check-in came from, never what it says: a forwarding
/// header from an unnamed peer is no address at all, and a kiosk outside the
/// space's networks vouches for none of them.
#[tokio::test]
async fn a_kiosk_cannot_say_where_it_is() {
    let pop = pop().await;
    let kiosk = pop.enroll("The tower", OUTSIDE).await;
    let claimed = pop
        .checkin(
            Some(&kiosk),
            "127.0.0.1:4000",
            &[("x-forwarded-for", "192.168.1.5")],
        )
        .await;
    assert_eq!(claimed, StatusCode::BAD_REQUEST);
    assert!(!pop.local(IN_SHOP));
    assert_eq!(
        pop.checkin(Some(&kiosk), OUTSIDE, &[]).await,
        StatusCode::NO_CONTENT
    );
    assert!(
        !pop.local(IN_SHOP),
        "a kiosk outside made the space's network local"
    );
}

#[tokio::test]
async fn a_revoked_kiosk_vouches_for_nothing_at_once() {
    let pop = pop().await;
    let kiosk = pop.enroll("The tower", OUTSIDE).await;
    pop.checkin(Some(&kiosk), KIOSK_AT, &[]).await;
    assert!(pop.local(IN_SHOP));
    let id = pop
        .send(
            Method::GET,
            "/api/admin/kiosks",
            Some(&pop.admin),
            None,
            OUTSIDE,
            &[],
        )
        .await
        .body[0]["id"]
        .clone();
    let uri = format!("/api/admin/kiosks/{id}");
    assert_eq!(
        pop.send(Method::DELETE, &uri, Some(&pop.admin), None, OUTSIDE, &[])
            .await
            .code,
        StatusCode::NO_CONTENT
    );
    assert!(!pop.local(IN_SHOP), "its last check-in still counted");
    assert_eq!(
        pop.checkin(Some(&kiosk), KIOSK_AT, &[]).await,
        StatusCode::UNAUTHORIZED
    );
    assert!(!pop.local(IN_SHOP));
    assert_eq!(
        pop.send(Method::DELETE, &uri, Some(&pop.admin), None, OUTSIDE, &[])
            .await
            .code,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        pop.audit("kiosk_revoked").await,
        ["Ada revoked the kiosk The tower"]
    );
    let list = pop
        .send(
            Method::GET,
            "/api/admin/kiosks",
            Some(&pop.admin),
            None,
            OUTSIDE,
            &[],
        )
        .await;
    assert_eq!(list.body[0]["revoked"], true);
}

/// A check-in every half minute is not an audit entry: it would bury the log.
#[tokio::test]
async fn a_checkin_is_not_an_audit_entry() {
    let pop = pop().await;
    let kiosk = pop.enroll("The tower", OUTSIDE).await;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit")
        .fetch_one(&pop.db)
        .await
        .unwrap();
    for _ in 0..3 {
        assert_eq!(
            pop.checkin(Some(&kiosk), KIOSK_AT, &[]).await,
            StatusCode::NO_CONTENT
        );
    }
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit")
        .fetch_one(&pop.db)
        .await
        .unwrap();
    assert_eq!(after, before);
}

/// An admin online holds every class, so the group is told when one arrives
/// (#12); a member online, or an admin at the space, is not news.
#[tokio::test]
async fn an_admin_signing_in_from_outside_is_told_to_the_group() {
    let pop = pop().await;
    pop.sign_in("ada", OUTSIDE).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while pop.chat.0.lock().unwrap().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the group was never told"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        *pop.chat.0.lock().unwrap(),
        ["Ada signed in to POP as an admin from outside the space."]
    );
    assert_eq!(
        pop.audit("signed_in").await.last().unwrap(),
        "Ada signed in from outside the space"
    );
    // Not news: a member online, an admin at the space.
    let kiosk = pop.enroll("The tower", OUTSIDE).await;
    pop.chat.0.lock().unwrap().clear();
    pop.checkin(Some(&kiosk), KIOSK_AT, &[]).await;
    pop.sign_in("grace", OUTSIDE).await;
    pop.sign_in("ada", IN_SHOP).await;
    assert_eq!(
        pop.audit("signed_in").await.last().unwrap(),
        "Ada signed in",
        "the precondition: it was local"
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        pop.chat.0.lock().unwrap().is_empty(),
        "{:?}",
        pop.chat.0.lock().unwrap()
    );
}

// ---- kiosk sessions: signed in at a kiosk, ended by being left alone (#12) ---------

impl Pop {
    /// Sign in as `username` from the browser holding `kiosk`'s credential.
    async fn sign_in_at(&self, kiosk: &str, username: &str) -> String {
        let r = self
            .send(
                Method::POST,
                "/api/session",
                Some(kiosk),
                Some(json!({"username": username, "password": "a long enough password"})),
                KIOSK_AT,
                &[],
            )
            .await;
        assert_eq!(r.code, StatusCode::OK, "{}", r.body);
        set_cookie(&r.headers, "pop_session")
    }

    /// Make every kiosk session look last used `secs` ago.
    async fn left_alone(&self, secs: i64) {
        sqlx::query(
            "UPDATE sessions SET last_used_at = last_used_at - ? WHERE idle_secs IS NOT NULL",
        )
        .bind(secs)
        .execute(&self.db)
        .await
        .unwrap();
    }

    async fn me(&self, session: &str) -> StatusCode {
        self.send(Method::GET, "/api/me", Some(session), None, IN_SHOP, &[])
            .await
            .code
    }
}

/// Signing in at a kiosk starts a kiosk session: it says so, and it ends once
/// the screen has been left alone for the configured time, told by the API
/// and by the session lookup itself.
#[tokio::test]
async fn a_sign_in_at_a_kiosk_ends_when_the_screen_is_left_alone() {
    let pop = pop().await;
    let kiosk = pop.enroll("The tower", OUTSIDE).await;
    let session = pop.sign_in_at(&kiosk, "grace").await;
    let info = pop
        .send(
            Method::GET,
            "/api/session",
            Some(&session),
            None,
            KIOSK_AT,
            &[],
        )
        .await;
    assert_eq!(info.body["idle_secs"], 90, "{}", info.body);
    assert_eq!(info.body["user"]["name"], "Grace");
    assert_eq!(
        pop.audit("signed_in").await.last().unwrap(),
        "Grace signed in at the kiosk"
    );
    pop.left_alone(91).await;
    assert_eq!(pop.me(&session).await, StatusCode::UNAUTHORIZED);
    let token = &session["pop_session=".len()..];
    assert_eq!(users::session_user(&pop.db, token).await.unwrap(), None);
}

/// Being used keeps it going: each request starts its idle time again.
#[tokio::test]
async fn using_a_kiosk_session_keeps_it_going() {
    let pop = pop().await;
    let kiosk = pop.enroll("The tower", OUTSIDE).await;
    let session = pop.sign_in_at(&kiosk, "grace").await;
    for _ in 0..3 {
        pop.left_alone(80).await;
        assert_eq!(
            pop.me(&session).await,
            StatusCode::OK,
            "used within its time"
        );
    }
    // Three times 80 is past 90 in all: only being left alone ends it.
}

/// An ordinary sign-in is not one: not limited by being left alone.
#[tokio::test]
async fn a_sign_in_elsewhere_is_not_a_kiosk_session() {
    let pop = pop().await;
    pop.enroll("The tower", OUTSIDE).await;
    let session = pop.sign_in("grace", IN_SHOP).await;
    let info = pop
        .send(
            Method::GET,
            "/api/session",
            Some(&session),
            None,
            IN_SHOP,
            &[],
        )
        .await;
    assert_eq!(info.body["idle_secs"], Value::Null);
    sqlx::query("UPDATE sessions SET last_used_at = last_used_at - 100000")
        .execute(&pop.db)
        .await
        .unwrap();
    assert_eq!(pop.me(&session).await, StatusCode::OK);
}

/// The time is the configured one.
#[tokio::test]
async fn a_kiosk_session_lasts_as_configured() {
    let mut pop = pop().await;
    pop.state = pop
        .state
        .clone()
        .with_kiosk(pop_server::config::Kiosk { idle_secs: 30 });
    pop.app = router(pop.state.clone());
    let kiosk = pop.enroll("The tower", OUTSIDE).await;
    let session = pop.sign_in_at(&kiosk, "grace").await;
    let info = pop
        .send(
            Method::GET,
            "/api/session",
            Some(&session),
            None,
            KIOSK_AT,
            &[],
        )
        .await;
    assert_eq!(info.body["idle_secs"], 30);
    pop.left_alone(31).await;
    assert_eq!(pop.me(&session).await, StatusCode::UNAUTHORIZED);
}
