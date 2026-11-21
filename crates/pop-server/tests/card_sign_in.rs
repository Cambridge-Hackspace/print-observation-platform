//! A card tap at a kiosk's reader signs the member in there (#19), against a
//! CS:S that exists only here. It issues card sign-in codes and TOTP tickets
//! as CS:S #241 does, redeems them only at its token endpoint with
//! `grant_type=urn:css:card-sign-in` and POP's own client id and secret,
//! counts a ticket's tries, and signs ID tokens with RSA itself, so what POP
//! verifies is not made by the code verifying it.
//!
//! Taps reach POP two ways here: handed to `CardSignIns::heard` (most tests,
//! which are about what happens at the kiosk), and once through a real
//! broker and the listener, end to end (`a_reply_on_the_broker_reaches_its_kiosk`).
//!
//! WHAT THIS DOES NOT PROVE: that a real CS:S answers as this one does. The
//! card grant's wire is CS:S's own e2e driver's claim
//! (cooperative-systems-spaces e2e/drivers/oidc.mjs, card-sign-in/*), and
//! the reply's shape is css_wire's, which both ends share.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Form, Json, Router,
    body::Body,
    extract::{ConnectInfo, State},
    http::{HeaderMap, Method, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD, engine::general_purpose::URL_SAFE_NO_PAD};
use http_body_util::BodyExt;
use pop_integrations::{
    card_taps::{self, Broker, LocalSignIn, Outcome, Tap},
    fakes::broker::FakeBroker,
};
use pop_server::{
    AppState,
    api::Role,
    assets::Embedded,
    audit::Actor,
    card_sign_in::CardSignIns,
    config::Provider,
    db, router,
    sign_in::SignIns,
    users::{self, NewAccount},
};
use rsa::{
    RsaPrivateKey,
    pkcs1::DecodeRsaPrivateKey,
    pkcs1v15::SigningKey,
    signature::{SignatureEncoding, Signer},
    traits::PublicKeyParts,
};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::SqlitePool;
use tower::ServiceExt;

const KEY: &str = include_str!("fixtures/oidc-test-key.pem");
const OTHER_KEY: &str = include_str!("fixtures/oidc-other-key.pem");
const PUBLIC: &str = "https://pop.example.org";
// A client id and secret with characters RFC 6749 2.3.1 makes POP encode.
const CLIENT: &str = "pop:tower";
const SECRET: &str = "s3cret+/=";
const PW: &str = "a long enough password";
const AT_KIOSK: &str = "192.168.1.5:4000";

// ---- the fake CS:S ----

/// A token request's form, and its Authorization header.
type TokenRequest = (HashMap<String, String>, Option<String>);

struct Css {
    issuer: String,
    key: RsaPrivateKey,
    /// A key it does not publish, and whether to sign with it.
    other: RsaPrivateKey,
    wrong_key: std::sync::atomic::AtomicBool,
    /// Codes it issued, by code: whose they are.
    codes: Mutex<HashMap<String, String>>,
    /// Tickets it issued, by ticket: whose, the TOTP code that is right, and
    /// the tries left.
    tickets: Mutex<HashMap<String, (String, String, u8)>>,
    /// Every token request's form, and its Authorization header.
    requests: Mutex<Vec<TokenRequest>>,
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
            other: RsaPrivateKey::from_pkcs1_pem(OTHER_KEY).unwrap(),
            wrong_key: std::sync::atomic::AtomicBool::new(false),
            codes: Mutex::new(HashMap::new()),
            tickets: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/token", post(token))
            .with_state(css.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { issuer, css, task }
    }

    /// A code CS:S issued for `sub`'s tap.
    fn code(&self, sub: &str) -> String {
        let mut codes = self.css.codes.lock().unwrap();
        let c = format!("code-{}", codes.len());
        codes.insert(c.clone(), sub.into());
        c
    }

    /// A ticket CS:S issued for `sub`'s tap, wanting `totp`.
    fn ticket(&self, sub: &str, totp: &str) -> String {
        let mut t = self.css.tickets.lock().unwrap();
        let k = format!("ticket-{}", t.len());
        t.insert(k.clone(), (sub.into(), totp.into(), 3));
        k
    }

    fn requests(&self) -> Vec<TokenRequest> {
        self.css.requests.lock().unwrap().clone()
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

fn no(error: &str, why: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": error, "error_description": why})),
    )
        .into_response()
}

async fn token(
    State(p): State<Arc<Css>>,
    headers: HeaderMap,
    Form(f): Form<HashMap<String, String>>,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .map(|v| v.to_str().unwrap().to_owned());
    p.requests.lock().unwrap().push((f.clone(), auth.clone()));
    // POP's own credentials, each half form-encoded (RFC 6749 2.3.1).
    let enc = |v: &str| url_enc(v);
    let want = format!(
        "Basic {}",
        STANDARD.encode(format!("{}:{}", enc(CLIENT), enc(SECRET)))
    );
    if auth.as_deref() != Some(want.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_client"})),
        )
            .into_response();
    }
    if f.get("grant_type").map(String::as_str) != Some("urn:css:card-sign-in") {
        return no("unsupported_grant_type", "not a card sign-in");
    }
    let sub = match (f.get("code"), f.get("ticket"), f.get("totp")) {
        (Some(c), None, None) => match p.codes.lock().unwrap().remove(c) {
            Some(sub) => sub,
            None => return no("invalid_grant", "unknown card sign-in"),
        },
        (None, Some(t), Some(totp)) => {
            let mut all = p.tickets.lock().unwrap();
            let Some((sub, right, left)) = all.get_mut(t) else {
                return no("invalid_grant", "this sign-in has ended: tap again");
            };
            if totp != right {
                *left -= 1;
                if *left == 0 {
                    all.remove(t);
                }
                return no("invalid_grant", "that code is not right");
            }
            let sub = sub.clone();
            all.remove(t);
            sub
        }
        _ => {
            return no(
                "invalid_request",
                "a card sign-in takes code, or ticket and totp",
            );
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = json!({"iss": p.issuer, "aud": CLIENT, "sub": sub, "iat": now, "exp": now + 600,
        "name": format!("Member {sub}"), "preferred_username": sub, "amr": ["hwk"]});
    let header = json!({"alg": "RS256", "kid": "test-1", "typ": "JWT"});
    let signing = format!(
        "{}.{}",
        b64(header.to_string().as_bytes()),
        b64(claims.to_string().as_bytes())
    );
    let key = if p.wrong_key.load(std::sync::atomic::Ordering::SeqCst) {
        &p.other
    } else {
        &p.key
    };
    let sig = SigningKey::<Sha256>::new(key.clone()).sign(signing.as_bytes());
    Json(
        json!({"access_token": "at", "token_type": "Bearer", "expires_in": 600,
        "id_token": format!("{signing}.{}", b64(&sig.to_bytes()))}),
    )
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

// ---- POP, with CS:S as a provider and card sign-in on ----

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    cards: CardSignIns,
    css: Fake,
    admin: String,
}

struct Res {
    code: StatusCode,
    headers: HeaderMap,
    body: Value,
}

fn provider(issuer: &str, secret: &str) -> Provider {
    let mut p: Provider = toml::from_str(&format!(
        "id = \"css\"\nlabel = \"CS:S\"\nissuer = \"{issuer}\"\nclient_id = \"{CLIENT}\"\n"
    ))
    .unwrap();
    p.client_secret = secret.into();
    p
}

async fn pop_with_secret(secret: &str) -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    users::create(
        &db,
        NewAccount {
            username: "ada",
            name: "Ada",
            role: Role::Admin,
            password: PW,
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    let css = Fake::start().await;
    let cards = CardSignIns::new("css").with_long_poll(Duration::from_millis(300));
    let (_tx, hearing) = tokio::sync::watch::channel(None);
    let state = AppState::new(db.clone(), Arc::new(Embedded))
        .with_sign_ins(SignIns::new(vec![provider(&css.issuer, secret)], PUBLIC))
        .with_card_sign_ins(cards.clone(), hearing);
    let mut pop = Pop {
        _dir: dir,
        db,
        app: router(state),
        cards,
        css,
        admin: String::new(),
    };
    let r = pop
        .send(
            Method::POST,
            "/api/session",
            None,
            Some(json!({"username": "ada", "password": PW})),
        )
        .await;
    assert_eq!(r.code, StatusCode::OK, "{}", r.body);
    pop.admin = set_cookie(&r.headers, "pop_session");
    pop
}

async fn pop() -> Pop {
    pop_with_secret(SECRET).await
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
        req.extensions_mut()
            .insert(ConnectInfo(AT_KIOSK.parse::<SocketAddr>().unwrap()));
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

    /// A kiosk, enrolled, with its card reader at `resource`: its cookie.
    async fn kiosk(&self, name: &str, resource: &str) -> String {
        let r = self
            .send(
                Method::POST,
                "/api/admin/kiosks",
                Some(&self.admin),
                Some(json!({"name": name})),
            )
            .await;
        assert_eq!(r.code, StatusCode::CREATED, "{}", r.body);
        let cookie = set_cookie(&r.headers, "pop_kiosk");
        let id = r.body["id"].as_i64().unwrap();
        let r = self.reader(id, resource).await;
        assert_eq!(r.code, StatusCode::NO_CONTENT, "{}", r.body);
        cookie
    }

    async fn reader(&self, id: i64, resource: &str) -> Res {
        self.send(
            Method::PUT,
            &format!("/api/admin/kiosks/{id}/card-reader"),
            Some(&self.admin),
            Some(json!({"resource": resource})),
        )
        .await
    }

    async fn tap(&self, resource: &str, outcome: Outcome) {
        self.cards
            .heard(
                &self.db,
                Tap {
                    resource: resource.into(),
                    outcome,
                },
            )
            .await;
    }

    async fn waiting(&self, kiosk: &str) -> Res {
        self.send(Method::GET, "/api/kiosk/tap", Some(kiosk), None)
            .await
    }

    async fn take(&self, kiosk: &str, totp: Option<&str>) -> Res {
        let body = match totp {
            Some(t) => json!({"totp": t}),
            None => json!({}),
        };
        self.send(Method::POST, "/api/kiosk/tap", Some(kiosk), Some(body))
            .await
    }

    async fn audit(&self, kind: &str) -> Vec<String> {
        sqlx::query_scalar("SELECT summary FROM audit WHERE kind = ? ORDER BY id")
            .bind(kind)
            .fetch_all(&self.db)
            .await
            .unwrap()
    }
}

fn code(c: &str) -> Outcome {
    Outcome::SignIn(LocalSignIn::Code { code: c.into() })
}

// ---- the tests ----

#[tokio::test]
async fn a_tap_at_a_kiosks_reader_signs_the_member_in_there() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    let c = pop.css.code("u-alex");
    pop.tap("tower-1", code(&c)).await;

    // The kiosk is told there is a sign-in, and never sees the code.
    let w = pop.waiting(&kiosk).await;
    assert_eq!(w.code, StatusCode::OK);
    assert_eq!(w.body, json!({"tap": {"kind": "sign_in"}}));
    assert!(!w.body.to_string().contains(&c));

    let r = pop.take(&kiosk, None).await;
    assert_eq!(r.code, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["name"], "Member u-alex");
    let session = set_cookie(&r.headers, "pop_session");

    // CS:S was asked as #241 says: the card grant, the code, POP's own
    // credentials, and nothing a browser sign-in would send.
    let reqs = pop.css.requests();
    assert_eq!(reqs.len(), 1);
    let (form, _) = &reqs[0];
    assert_eq!(
        form.get("grant_type").map(String::as_str),
        Some("urn:css:card-sign-in")
    );
    assert_eq!(form.get("code"), Some(&c));
    assert!(!form.contains_key("code_verifier") && !form.contains_key("redirect_uri"));

    // A kiosk session, as any sign-in at an enrolled kiosk is (#12).
    let s = pop
        .send(Method::GET, "/api/session", Some(&session), None)
        .await;
    assert_eq!(s.code, StatusCode::OK);
    assert!(
        s.body["idle_secs"].is_number(),
        "a kiosk session: {}",
        s.body
    );
    let rec = pop.audit("signed_in").await;
    assert!(
        rec.iter()
            .any(|r| r == "Member u-alex signed in with a CS:S card at the kiosk"),
        "{rec:?}"
    );
    // And the tap is spent.
    assert_eq!(pop.waiting(&kiosk).await.body, json!({"tap": null}));
    assert_eq!(pop.take(&kiosk, None).await.code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_same_member_is_the_same_account_by_card_and_by_browser() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    pop.tap("tower-1", code(&pop.css.code("u-alex"))).await;
    let first = pop.take(&kiosk, None).await.body["id"].clone();
    pop.tap("tower-1", code(&pop.css.code("u-alex"))).await;
    let again = pop.take(&kiosk, None).await.body["id"].clone();
    assert!(first.is_number());
    assert_eq!(first, again, "a second tap is the same account");
    let linked: i64 = sqlx::query_scalar(
        "SELECT user_id FROM user_identities WHERE provider = 'css' AND subject = 'u-alex'",
    )
    .fetch_one(&pop.db)
    .await
    .unwrap();
    assert_eq!(
        json!(linked),
        first,
        "linked by the provider's sub, as a browser sign-in is"
    );
}

#[tokio::test]
async fn a_tap_is_for_its_own_kiosk_and_for_no_other() {
    let pop = pop().await;
    let one = pop.kiosk("Tower one", "tower-1").await;
    let two = pop.kiosk("Tower two", "tower-2").await;
    // A reader no kiosk has: no sign-in anywhere.
    pop.tap("tower-9", code(&pop.css.code("u-x"))).await;
    pop.tap("tower-1", code(&pop.css.code("u-alex"))).await;
    // The control first: the right kiosk has it...
    assert_eq!(
        pop.waiting(&one).await.body,
        json!({"tap": {"kind": "sign_in"}})
    );
    // ...and the other, waiting its while, does not, and cannot take it.
    assert_eq!(pop.waiting(&two).await.body, json!({"tap": null}));
    assert_eq!(pop.take(&two, None).await.code, StatusCode::NOT_FOUND);
    assert_eq!(pop.take(&one, None).await.code, StatusCode::OK);
    // CS:S was asked once, for the right tap.
    assert_eq!(pop.css.requests().len(), 1);
}

#[tokio::test]
async fn a_kiosk_waiting_hears_a_tap_as_it_happens() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    let cards = pop.cards.clone();
    let db = pop.db.clone();
    let c = pop.css.code("u-alex");
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cards
            .heard(
                &db,
                Tap {
                    resource: "tower-1".into(),
                    outcome: code(&c),
                },
            )
            .await;
    });
    // Asked before the tap; answered when it comes, inside the wait.
    let started = std::time::Instant::now();
    let w = pop.waiting(&kiosk).await;
    assert_eq!(w.body, json!({"tap": {"kind": "sign_in"}}));
    assert!(
        started.elapsed() < Duration::from_millis(290),
        "answered at the tap, not at the end of the wait"
    );
}

#[tokio::test]
async fn a_second_factor_is_asked_for_tried_again_and_signs_in() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    let t = pop.css.ticket("u-alex", "123456");
    pop.tap(
        "tower-1",
        Outcome::SignIn(LocalSignIn::Totp { ticket: t.clone() }),
    )
    .await;
    let w = pop.waiting(&kiosk).await;
    assert_eq!(w.body, json!({"tap": {"kind": "second_factor"}}));
    assert!(
        !w.body.to_string().contains(&t),
        "the ticket never reaches the kiosk"
    );

    // No code, or not six digits: asked again, and CS:S is not troubled.
    for bad in [None, Some("12345"), Some("12345a")] {
        let r = pop.take(&kiosk, bad).await;
        assert_eq!(r.code, StatusCode::BAD_REQUEST, "{bad:?}");
    }
    assert!(pop.css.requests().is_empty());

    // A wrong code: CS:S's words, and the tap still waits, saying so.
    let r = pop.take(&kiosk, Some("000000")).await;
    assert_eq!(r.code, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["error"], "that code is not right");
    assert_eq!(
        pop.waiting(&kiosk).await.body,
        json!({"tap": {"kind": "second_factor", "problem": "that code is not right"}})
    );
    let r = pop.take(&kiosk, Some("123456")).await;
    assert_eq!(r.code, StatusCode::OK, "{}", r.body);
    let (form, _) = pop.css.requests().last().unwrap().clone();
    assert_eq!(form.get("ticket"), Some(&t));
    assert_eq!(form.get("totp").map(String::as_str), Some("123456"));
    assert!(!form.contains_key("code"));
}

#[tokio::test]
async fn a_ticket_ends_after_its_tries() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    let t = pop.css.ticket("u-alex", "123456");
    pop.tap("tower-1", Outcome::SignIn(LocalSignIn::Totp { ticket: t }))
        .await;
    for _ in 0..2 {
        assert_eq!(
            pop.take(&kiosk, Some("000000")).await.code,
            StatusCode::BAD_REQUEST
        );
        assert!(
            matches!(&pop.waiting(&kiosk).await.body["tap"]["kind"], Value::String(k) if k == "second_factor")
        );
    }
    // The third wrong code is the last: the tap is gone, here as at CS:S.
    assert_eq!(
        pop.take(&kiosk, Some("000000")).await.code,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(pop.waiting(&kiosk).await.body, json!({"tap": null}));
    assert_eq!(
        pop.take(&kiosk, Some("123456")).await.code,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_refusal_is_shown_and_set_aside() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    pop.tap("tower-1", Outcome::NotSignedIn("Unknown card".into()))
        .await;
    assert_eq!(
        pop.waiting(&kiosk).await.body,
        json!({"tap": {"kind": "refused", "reason": "Unknown card"}})
    );
    // It signs nobody in, and CS:S is not asked.
    assert_eq!(pop.take(&kiosk, None).await.code, StatusCode::CONFLICT);
    assert!(pop.css.requests().is_empty());
    // Set aside, it is gone, and setting it aside is not a record.
    pop.tap(
        "tower-1",
        Outcome::SignIn(LocalSignIn::Refused {
            reason: "Set up an authenticator app".into(),
        }),
    )
    .await;
    assert_eq!(
        pop.waiting(&kiosk).await.body["tap"]["reason"],
        "Set up an authenticator app"
    );
    let entries = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit")
            .fetch_one(&pop.db)
            .await
            .unwrap()
    };
    let before = entries().await;
    let d = pop
        .send(Method::DELETE, "/api/kiosk/tap", Some(&kiosk), None)
        .await;
    assert_eq!(d.code, StatusCode::NO_CONTENT);
    assert_eq!(pop.waiting(&kiosk).await.body, json!({"tap": null}));
    assert_eq!(
        entries().await,
        before,
        "a tap set aside is not an audit entry"
    );
}

#[tokio::test]
async fn a_code_cs_s_never_issued_signs_nobody_in() {
    // What a reply forged on the broker carries: CS:S refuses it in words,
    // and POP starts no session.
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    pop.tap("tower-1", code("made-up")).await;
    let r = pop.take(&kiosk, None).await;
    assert_eq!(r.code, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["error"], "unknown card sign-in");
    assert!(r.headers.get(header::SET_COOKIE).is_none(), "no session");
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&pop.db)
        .await
        .unwrap();
    assert_eq!(sessions, 1, "the admin's own, and no other");
}

#[tokio::test]
async fn an_id_token_cs_s_did_not_sign_is_not_believed() {
    // A token endpoint that answers with a token signed by a key CS:S does
    // not publish: POP believes a card sign-in's ID token as it believes a
    // browser's, or not at all.
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    pop.css
        .css
        .wrong_key
        .store(true, std::sync::atomic::Ordering::SeqCst);
    pop.tap("tower-1", code(&pop.css.code("u-alex"))).await;
    let r = pop.take(&kiosk, None).await;
    assert_eq!(r.code, StatusCode::BAD_GATEWAY, "{}", r.body);
    assert!(r.headers.get(header::SET_COOKIE).is_none(), "no session");
    let made: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_identities")
        .fetch_one(&pop.db)
        .await
        .unwrap();
    assert_eq!(made, 0, "and no account made from what it said");
}

#[tokio::test]
async fn pop_s_own_setup_failing_is_not_put_to_the_member() {
    let pop = pop_with_secret("not the secret").await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    pop.tap("tower-1", code(&pop.css.code("u-alex"))).await;
    let r = pop.take(&kiosk, None).await;
    assert_eq!(r.code, StatusCode::BAD_GATEWAY);
    assert_eq!(
        r.body["error"],
        "POP could not finish signing you in with your card: sign in another way"
    );
}

#[tokio::test]
async fn a_deactivated_account_is_not_signed_in_by_card() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    pop.tap("tower-1", code(&pop.css.code("u-alex"))).await;
    let id = pop.take(&kiosk, None).await.body["id"].as_i64().unwrap();
    let r = pop
        .send(
            Method::PUT,
            &format!("/api/admin/users/{id}/active"),
            Some(&pop.admin),
            Some(json!({"active": false})),
        )
        .await;
    assert!(r.code.is_success(), "{}", r.body);
    pop.tap("tower-1", code(&pop.css.code("u-alex"))).await;
    let r = pop.take(&kiosk, None).await;
    assert_eq!(r.code, StatusCode::FORBIDDEN);
    assert!(r.headers.get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn a_card_reader_belongs_to_one_kiosk_and_is_recorded() {
    let pop = pop().await;
    let _one = pop.kiosk("Tower one", "tower-1").await;
    let r = pop
        .send(
            Method::POST,
            "/api/admin/kiosks",
            Some(&pop.admin),
            Some(json!({"name": "Tower two"})),
        )
        .await;
    let two = r.body["id"].as_i64().unwrap();
    let taken = pop.reader(two, "tower-1").await;
    assert_eq!(taken.code, StatusCode::CONFLICT, "{}", taken.body);
    assert_eq!(
        pop.reader(two, "x".repeat(129).as_str()).await.code,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(pop.reader(999, "tower-3").await.code, StatusCode::NOT_FOUND);
    assert_eq!(
        pop.reader(two, "tower-2").await.code,
        StatusCode::NO_CONTENT
    );
    // The desk's list shows each kiosk's reader.
    let list = pop
        .send(Method::GET, "/api/admin/kiosks", Some(&pop.admin), None)
        .await;
    let readers: Vec<Value> = list
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["card_resource"].clone())
        .collect();
    assert_eq!(readers, [json!("tower-1"), json!("tower-2")]);
    // Taken off, it is free for another; and each change is recorded.
    assert_eq!(pop.reader(two, "").await.code, StatusCode::NO_CONTENT);
    assert_eq!(
        pop.audit("kiosk_reader_set").await,
        [
            "Ada set the kiosk Tower one's card reader to tower-1",
            "Ada set the kiosk Tower two's card reader to tower-2",
            "Ada took the card reader off the kiosk Tower two",
        ]
    );
    // A revoked kiosk's reader is free too.
    let r = pop
        .send(
            Method::DELETE,
            "/api/admin/kiosks/1",
            Some(&pop.admin),
            None,
        )
        .await;
    assert_eq!(r.code, StatusCode::NO_CONTENT);
    assert_eq!(
        pop.reader(two, "tower-1").await.code,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn a_reply_on_the_broker_reaches_its_kiosk() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    let broker = FakeBroker::start(None).await;
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let (mut hearing, listener) =
        card_taps::spawn(Broker::parse(&broker.url(), "", "").unwrap(), tx);
    let keeper = pop.cards.clone().spawn(pop.db.clone(), rx);
    tokio::time::timeout(Duration::from_secs(10), hearing.wait_for(Option::is_none))
        .await
        .unwrap()
        .unwrap();
    let c = pop.css.code("u-alex");
    let reply = json!({"resource": "tower-1", "authorized": true, "reason": "authorized",
        "member": {"id": "u-alex", "name": "Alex"}, "sign_in": {"kind": "code", "code": c}})
    .to_string();
    let seen = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            broker.publish("resource/response/scan", reply.clone());
            let w = pop.waiting(&kiosk).await;
            if !w.body["tap"].is_null() {
                return w.body;
            }
        }
    })
    .await
    .expect("the tap to reach the kiosk");
    assert_eq!(seen, json!({"tap": {"kind": "sign_in"}}));
    assert_eq!(pop.take(&kiosk, None).await.code, StatusCode::OK);
    assert!(broker.publishes().is_empty(), "POP publishes nothing");
    listener.abort();
    keeper.abort();
}
