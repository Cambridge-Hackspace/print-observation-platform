//! Which printers a member may use, by their training (#17), from each
//! source: POP's desk, and CS:S asked at each use with POP's own token.
//!
//! The CS:S here exists only in this file. It serves discovery and keys,
//! gives POP an app token only by the client-credentials grant with
//! `training:members` and POP's own client id and secret, and answers
//! `GET /api/oauth/members/{sub}/tools` only to a token it gave out, as
//! CS:S 0.5.1 does. Each member's training is the test's to change while
//! they stay signed in.
//!
//! WHAT THIS DOES NOT PROVE: that a real CS:S answers as this one does. That
//! route's shape and refusals are CS:S's own e2e driver's claim
//! (cooperative-systems-spaces e2e/drivers/oidc.mjs, "any member's
//! training"), and `me/tools` shares its answer.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

use axum::{
    Form, Json, Router,
    body::Body,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, Method, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD, engine::general_purpose::URL_SAFE_NO_PAD};
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
    config::{Provider, Training, TrainingSource},
    db,
    printers::BamSource,
    router,
    sign_in::SignIns,
    users::{self, NewAccount},
};
use rsa::{RsaPrivateKey, pkcs1::DecodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

const RECORDING: &str =
    include_str!("../../pop-integrations/tests/recordings/v0.20.0/recordings.json");
const KEY: &str = include_str!("fixtures/oidc-test-key.pem");
const PUBLIC: &str = "https://pop.example.org";
// A client id and secret with characters RFC 6749 2.3.1 makes POP encode.
const CLIENT: &str = "pop:tower";
const SECRET: &str = "s3cret+/=";
const PW: &str = "a long enough password";
const LOCAL: &str = "127.0.0.1:4000";
/// The member's CS:S id, and the tool the printer needs, by external id.
const SUB: &str = "6f1c2d3e-0000-4000-8000-00000000beef";
const TOOL_ID: &str = "1a2b3c4d-0000-4000-8000-000000000001";
const TOOL_EXT: &str = "prusa-mk4";

// ---- the fake CS:S ----

/// A token request's form, and its Authorization header.
type TokenRequest = (HashMap<String, String>, Option<String>);

struct Css {
    issuer: String,
    key: RsaPrivateKey,
    /// Whether each member, by sub, is trained on the tool.
    trained: Mutex<HashMap<String, bool>>,
    /// Tokens it gave out and still believes.
    tokens: Mutex<Vec<String>>,
    /// Every token request's form and Authorization header.
    token_requests: Mutex<Vec<TokenRequest>>,
    /// How many times it was asked about a member's training.
    asked: AtomicU32,
    /// Answer every training question 503.
    down: AtomicBool,
}

struct Fake {
    issuer: String,
    css: Arc<Css>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn b64(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

impl Fake {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let css = Arc::new(Css {
            issuer: issuer.clone(),
            key: RsaPrivateKey::from_pkcs1_pem(KEY).unwrap(),
            trained: Mutex::new(HashMap::new()),
            tokens: Mutex::new(Vec::new()),
            token_requests: Mutex::new(Vec::new()),
            asked: AtomicU32::new(0),
            down: AtomicBool::new(false),
        });
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/token", post(token))
            .route("/api/oauth/members/{sub}/tools", get(tools))
            .with_state(css.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { issuer, css, task }
    }

    fn train(&self, sub: &str, trained: bool) {
        self.css.trained.lock().unwrap().insert(sub.into(), trained);
    }

    fn asked(&self) -> u32 {
        self.css.asked.load(Ordering::SeqCst)
    }

    fn token_requests(&self) -> Vec<TokenRequest> {
        self.css.token_requests.lock().unwrap().clone()
    }

    /// Stop believing every token it gave, as when the scope is taken away.
    fn forget_tokens(&self) {
        self.css.tokens.lock().unwrap().clear();
    }
}

async fn discovery(State(p): State<Arc<Css>>) -> Response {
    let i = &p.issuer;
    Json(json!({
        "issuer": i, "authorization_endpoint": format!("{i}/authorize"),
        "token_endpoint": format!("{i}/token"), "jwks_uri": format!("{i}/jwks"),
        "response_types_supported": ["code"], "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
    }))
    .into_response()
}

async fn jwks(State(p): State<Arc<Css>>) -> Response {
    let public = p.key.to_public_key();
    Json(json!({"keys": [{
        "kty": "RSA", "use": "sig", "alg": "RS256", "kid": "test-1",
        "n": b64(&public.n().to_bytes_be()), "e": b64(&public.e().to_bytes_be()),
    }]}))
    .into_response()
}

/// application/x-www-form-urlencoded, as RFC 6749 2.3.1 asks of each half.
fn url_enc(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                (b as char).to_string()
            }
            b' ' => "+".to_owned(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn token(
    State(p): State<Arc<Css>>,
    headers: HeaderMap,
    Form(f): Form<HashMap<String, String>>,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .map(|v| v.to_str().unwrap().to_owned());
    p.token_requests
        .lock()
        .unwrap()
        .push((f.clone(), auth.clone()));
    let want = format!(
        "Basic {}",
        STANDARD.encode(format!("{}:{}", url_enc(CLIENT), url_enc(SECRET)))
    );
    if auth.as_deref() != Some(want.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_client"})),
        )
            .into_response();
    }
    if f.get("grant_type").map(String::as_str) != Some("client_credentials")
        || f.get("scope").map(String::as_str) != Some("training:members")
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_scope"})),
        )
            .into_response();
    }
    let mut tokens = p.tokens.lock().unwrap();
    let t = format!("app-token-{}", tokens.len() + 100);
    tokens.push(t.clone());
    Json(json!({"access_token": t, "token_type": "Bearer", "expires_in": 3600, "scope": "training:members"}))
        .into_response()
}

async fn tools(State(p): State<Arc<Css>>, headers: HeaderMap, Path(sub): Path<String>) -> Response {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_owned();
    if !p.tokens.lock().unwrap().contains(&bearer) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    p.asked.fetch_add(1, Ordering::SeqCst);
    if p.down.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let Some(trained) = p.trained.lock().unwrap().get(&sub).copied() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "message": "No such member"})),
        )
            .into_response();
    };
    Json(json!({"success": true, "data": [
        {"id": "1a2b3c4d-0000-4000-8000-000000000009", "external_id": null, "name": "Laser cutter", "authorized": true},
        {"id": TOOL_ID, "external_id": TOOL_EXT, "name": "Prusa MK4", "authorized": trained},
    ]}))
    .into_response()
}

// ---- POP, with BAM! and CS:S ----

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    bam: FakeBam,
    css: Fake,
    admin: String,
    /// The member's own session, kept across every check.
    member: String,
    member_id: i64,
}

struct Res {
    code: StatusCode,
    body: Value,
}

fn provider(issuer: &str) -> Provider {
    let mut p: Provider = toml::from_str(&format!(
        "id = \"css\"\nlabel = \"CS:S\"\nissuer = \"{issuer}\"\nclient_id = \"{CLIENT}\"\n"
    ))
    .unwrap();
    p.client_secret = SECRET.into();
    p
}

async fn pop(source: TrainingSource) -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    for (username, name, role) in [("ada", "Ada", Role::Admin), ("bo", "Bo", Role::Member)] {
        users::create(
            &db,
            NewAccount {
                username,
                name,
                role,
                password: PW,
            },
            &Actor::command_line(),
        )
        .await
        .unwrap();
    }
    let bam = FakeBam::start(RECORDING).await;
    let css = Fake::start().await;
    let state = AppState::new(db.clone(), Arc::new(Embedded))
        .with_bam(BamSource {
            client: Bam::new(bam.url(), TOKEN).unwrap(),
            category: bam.id("printers").to_owned(),
            fields: FieldMap::default(),
        })
        .with_sign_ins(SignIns::new(vec![provider(&css.issuer)], PUBLIC))
        .with_training(Training {
            source,
            provider: "css".into(),
        });
    let mut pop = Pop {
        _dir: dir,
        db,
        app: router(state),
        bam,
        css,
        admin: String::new(),
        member: String::new(),
        member_id: 0,
    };
    pop.admin = pop.sign_in("ada").await;
    pop.member = pop.sign_in("bo").await;
    pop.member_id = sqlx::query_scalar("SELECT id FROM users WHERE username = 'bo'")
        .fetch_one(&pop.db)
        .await
        .unwrap();
    pop
}

impl Pop {
    async fn send(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> (Res, HeaderMap) {
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
            .insert(ConnectInfo(LOCAL.parse::<SocketAddr>().unwrap()));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            Res {
                code,
                body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            },
            headers,
        )
    }

    async fn sign_in(&self, username: &str) -> String {
        let (r, h) = self
            .send(
                Method::POST,
                "/api/session",
                None,
                Some(json!({"username": username, "password": PW})),
            )
            .await;
        assert_eq!(r.code, StatusCode::OK, "{}", r.body);
        h.get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_owned())
            .find(|v| v.starts_with("pop_session="))
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }

    /// The member's answer for `printer`, in their own session.
    async fn check(&self, printer: &str) -> Res {
        self.send(
            Method::GET,
            &format!("/api/printers/{printer}/training"),
            Some(&self.member),
            None,
        )
        .await
        .0
    }

    /// The desk ticks the member's training.
    async fn tick(&self, training: &[&str]) {
        let (r, _) = self
            .send(
                Method::PUT,
                &format!("/api/admin/users/{}/training", self.member_id),
                Some(&self.admin),
                Some(json!({"training": training})),
            )
            .await;
        assert_eq!(r.code, StatusCode::OK, "{}", r.body);
    }

    /// The member's account linked to CS:S's `sub`, as a sign-in with it does.
    async fn link(&self, sub: &str) {
        sqlx::query(
            "INSERT INTO user_identities (provider, subject, user_id, created_at, last_sign_in_at)
             VALUES ('css', ?, ?, 0, 0)",
        )
        .bind(sub)
        .bind(self.member_id)
        .execute(&self.db)
        .await
        .unwrap();
    }

    /// The MK4 printer, needing `training` and `css_tool` where given.
    fn mk4(&self, training: Option<&str>, css_tool: Option<&str>) -> String {
        let id = self.bam.id("mk4").to_owned();
        self.bam.edit(&id, "training", training.unwrap_or(""));
        self.bam.edit(&id, "css_tool", css_tool.unwrap_or(""));
        id
    }
}

fn may(r: &Res) -> bool {
    assert_eq!(r.code, StatusCode::OK, "{}", r.body);
    r.body["may_use"].as_bool().expect("may_use")
}

fn reason(r: &Res) -> String {
    r.body["reason"].as_str().unwrap_or_default().to_owned()
}

// ---- training ticked on POP's desk ----

#[tokio::test]
async fn on_the_desk_a_member_who_loses_training_is_refused_at_the_next_check() {
    let pop = pop(TrainingSource::Local).await;
    let mk4 = pop.mk4(Some("FDM basics"), None);
    let r = pop.check(&mk4).await;
    assert!(!may(&r), "untrained, and allowed");
    assert!(
        reason(&r).contains("FDM basics"),
        "the refusal names what is needed: {}",
        reason(&r)
    );
    // The desk keeps a level whatever its case, so a check does too.
    pop.tick(&["fdm BASICS"]).await;
    let r = pop.check(&mk4).await;
    assert!(may(&r), "trained, and refused: {}", r.body);
    assert_eq!(r.body["reason"], Value::Null);
    // Taken away while the member stays signed in, in the same session.
    pop.tick(&[]).await;
    assert!(!may(&pop.check(&mk4).await), "untrained again, and allowed");
    // The desk's source never asks CS:S.
    assert_eq!(pop.css.asked(), 0);
    assert!(pop.css.token_requests().is_empty());
}

#[tokio::test]
async fn a_printer_that_names_no_training_needs_none() {
    let pop = pop(TrainingSource::Local).await;
    let mk4 = pop.mk4(None, Some(TOOL_EXT));
    assert!(may(&pop.check(&mk4).await));
    let pop = pop_css().await;
    let mk4 = pop.mk4(Some("FDM basics"), None);
    assert!(may(&pop.check(&mk4).await));
    // Nothing to ask CS:S about, so it is not asked.
    assert_eq!(pop.css.asked(), 0);
    assert!(pop.css.token_requests().is_empty());
}

// ---- training from CS:S ----

async fn pop_css() -> Pop {
    pop(TrainingSource::Css).await
}

#[tokio::test]
async fn from_cs_s_a_member_who_loses_training_is_refused_at_the_next_check() {
    let pop = pop_css().await;
    pop.link(SUB).await;
    pop.css.train(SUB, false);
    let mk4 = pop.mk4(Some("FDM basics"), Some(TOOL_EXT));
    // The desk's training is not CS:S's: ticking it changes nothing here.
    pop.tick(&["FDM basics"]).await;
    let r = pop.check(&mk4).await;
    assert!(!may(&r), "CS:S says untrained, and allowed");
    assert!(reason(&r).contains("Prusa MK4"), "{}", reason(&r));
    pop.css.train(SUB, true);
    let r = pop.check(&mk4).await;
    assert!(may(&r), "CS:S says trained, and refused: {}", r.body);
    pop.css.train(SUB, false);
    assert!(
        !may(&pop.check(&mk4).await),
        "trained no longer, still signed in, and allowed"
    );
    // Asked at every check: nothing about the member was kept.
    assert_eq!(pop.css.asked(), 3);
}

#[tokio::test]
async fn pop_asks_with_its_own_token_and_keeps_only_that() {
    let pop = pop_css().await;
    pop.link(SUB).await;
    pop.css.train(SUB, true);
    let mk4 = pop.mk4(None, Some(TOOL_EXT));
    for _ in 0..3 {
        assert!(may(&pop.check(&mk4).await));
    }
    let asked = pop.css.token_requests();
    assert_eq!(asked.len(), 1, "one token for three checks: {asked:?}");
    let (form, auth) = &asked[0];
    assert_eq!(
        form.get("grant_type").map(String::as_str),
        Some("client_credentials")
    );
    assert_eq!(
        form.get("scope").map(String::as_str),
        Some("training:members")
    );
    assert_eq!(
        auth.as_deref(),
        Some(
            format!(
                "Basic {}",
                STANDARD.encode(format!("{}:{}", url_enc(CLIENT), url_enc(SECRET)))
            )
            .as_str()
        )
    );
    // A token CS:S stops believing is replaced once, and the answer is right.
    pop.css.forget_tokens();
    pop.css.train(SUB, false);
    assert!(!may(&pop.check(&mk4).await), "allowed on a refused token");
    assert_eq!(pop.css.token_requests().len(), 2);
    pop.css.train(SUB, true);
    assert!(may(&pop.check(&mk4).await));
}

#[tokio::test]
async fn a_cs_s_tool_is_named_by_its_external_id_or_its_id() {
    let pop = pop_css().await;
    pop.link(SUB).await;
    for named in [TOOL_EXT, TOOL_ID] {
        let mk4 = pop.mk4(None, Some(named));
        // Allowed only once the tool is found by that name: a tool not found
        // is refused, so refusals alone would not tell the two apart.
        pop.css.train(SUB, true);
        assert!(may(&pop.check(&mk4).await), "{named}");
        pop.css.train(SUB, false);
        let r = pop.check(&mk4).await;
        assert!(!may(&r), "{named}");
        assert!(reason(&r).contains("Prusa MK4"), "{named}: {}", reason(&r));
    }
    // Another tool the member may use is not this printer's.
    let mk4 = pop.mk4(None, Some("a-tool-cs-s-does-not-have"));
    let r = pop.check(&mk4).await;
    assert!(!may(&r), "a tool CS:S has not, and allowed");
    assert!(reason(&r).contains("isn't set up"), "{}", reason(&r));
}

#[tokio::test]
async fn a_member_cs_s_cannot_be_asked_about_is_refused_in_words() {
    let pop = pop_css().await;
    let mk4 = pop.mk4(None, Some(TOOL_EXT));
    // Never signed in with CS:S: POP doesn't know which member they are.
    let r = pop.check(&mk4).await;
    assert!(!may(&r));
    assert!(reason(&r).contains("Sign in with CS:S"), "{}", reason(&r));
    assert_eq!(pop.css.asked(), 0, "asked about nobody");
    // Linked, but CS:S has no active member by that id.
    pop.link(SUB).await;
    let r = pop.check(&mk4).await;
    assert!(!may(&r));
    assert!(reason(&r).contains("no active member"), "{}", reason(&r));
    // CS:S not answering is not training.
    pop.css.train(SUB, true);
    pop.css.css.down.store(true, Ordering::SeqCst);
    let r = pop.check(&mk4).await;
    assert!(!may(&r), "CS:S down, and allowed");
    assert!(reason(&r).contains("couldn't check"), "{}", reason(&r));
    pop.css.css.down.store(false, Ordering::SeqCst);
    assert!(may(&pop.check(&mk4).await), "CS:S back, and still refused");
}

#[tokio::test]
async fn only_a_signed_in_member_asks_and_only_about_a_printer() {
    let pop = pop_css().await;
    let mk4 = pop.mk4(None, None);
    let (r, _) = pop
        .send(
            Method::GET,
            &format!("/api/printers/{mk4}/training"),
            None,
            None,
        )
        .await;
    assert_eq!(r.code, StatusCode::UNAUTHORIZED);
    let r = pop.check("00000000-0000-4000-8000-00000000dead").await;
    assert_eq!(r.code, StatusCode::NOT_FOUND, "{}", r.body);
    // A printer that is, or a check below would prove nothing.
    assert_eq!(pop.check(&mk4).await.code, StatusCode::OK);
    // An asset BAM! has outside the printer category is not a printer.
    let x1 = pop.bam.id("x1").to_owned();
    pop.bam
        .recategorize(&x1, &["00000000-0000-4000-8000-00000000cafe"]);
    let r = pop.check(&x1).await;
    assert_eq!(r.code, StatusCode::NOT_FOUND, "{}", r.body);
    // Nor is one archived.
    pop.bam.archive(&mk4);
    let r = pop.check(&mk4).await;
    assert_eq!(r.code, StatusCode::NOT_FOUND, "{}", r.body);
    assert_eq!(pop.css.asked(), 0);
}
