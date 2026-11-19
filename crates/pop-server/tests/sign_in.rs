//! Signing in through an OpenID Connect provider (#14), against a provider
//! that exists only here: it publishes a discovery document and its key,
//! approves what a test tells it to, checks PKCE and the redirect at its
//! token endpoint, and signs ID tokens with RSA itself (the `rsa` crate, not
//! openidconnect), so what POP verifies is not made by the code verifying it.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    Form, Json, Router,
    body::Body,
    extract::{ConnectInfo, State},
    http::{Method, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http_body_util::BodyExt;
use pop_server::{
    AppState,
    api::Role,
    assets::Embedded,
    audit::Actor,
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
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tower::ServiceExt;

const KEY: &str = include_str!("fixtures/oidc-test-key.pem");
const OTHER_KEY: &str = include_str!("fixtures/oidc-other-key.pem");
const PUBLIC: &str = "https://pop.example.org";
const CLIENT: &str = "pop";

/// What the provider will put in the next ID token it signs, and how.
#[derive(Clone)]
struct Approve {
    sub: String,
    claims: Value,
    /// Sign with the key it does not publish.
    wrong_key: bool,
}

struct Code {
    challenge: String,
    redirect: String,
    nonce: String,
    approve: Approve,
}

struct Provider_ {
    issuer: String,
    key: RsaPrivateKey,
    other: RsaPrivateKey,
    codes: Mutex<HashMap<String, Code>>,
    /// Every token request, and whether it was granted.
    tokens: Mutex<Vec<bool>>,
}

/// A test sign-in provider, listening until dropped.
struct Fake {
    issuer: String,
    shared: Arc<Provider_>,
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
        let shared = Arc::new(Provider_ {
            issuer: issuer.clone(),
            key: RsaPrivateKey::from_pkcs1_pem(KEY).unwrap(),
            other: RsaPrivateKey::from_pkcs1_pem(OTHER_KEY).unwrap(),
            codes: Mutex::new(HashMap::new()),
            tokens: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/token", post(token))
            .with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            issuer,
            shared,
            task,
        }
    }

    /// Approve the sign-in at `authorize` (the provider's URL POP sent the
    /// browser to): the code the provider sends the browser back with.
    fn approve(&self, authorize: &str, a: Approve) -> String {
        let url = reqwest::Url::parse(authorize).unwrap();
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["client_id"], CLIENT);
        assert_eq!(q["code_challenge_method"], "S256", "PKCE, S256");
        let code = format!("code-{}", self.shared.codes.lock().unwrap().len());
        self.shared.codes.lock().unwrap().insert(
            code.clone(),
            Code {
                challenge: q["code_challenge"].clone(),
                redirect: q["redirect_uri"].clone(),
                nonce: q["nonce"].clone(),
                approve: a,
            },
        );
        code
    }

    fn tokens(&self) -> Vec<bool> {
        self.shared.tokens.lock().unwrap().clone()
    }
}

async fn discovery(State(p): State<Arc<Provider_>>) -> Response {
    let i = &p.issuer;
    Json(json!({
        "issuer": i,
        "authorization_endpoint": format!("{i}/authorize"),
        "token_endpoint": format!("{i}/token"),
        "jwks_uri": format!("{i}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "scopes_supported": ["openid", "profile", "roles"],
        "code_challenge_methods_supported": ["S256"],
    }))
    .into_response()
}

async fn jwks(State(p): State<Arc<Provider_>>) -> Response {
    let public = p.key.to_public_key();
    Json(json!({"keys": [{
        "kty": "RSA", "use": "sig", "alg": "RS256", "kid": "test-1",
        "n": b64(&public.n().to_bytes_be()), "e": b64(&public.e().to_bytes_be()),
    }]}))
    .into_response()
}

async fn token(
    State(p): State<Arc<Provider_>>,
    Form(f): Form<HashMap<String, String>>,
) -> Response {
    let refuse = |p: &Provider_| {
        p.tokens.lock().unwrap().push(false);
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        )
            .into_response()
    };
    let Some(code) = f
        .get("code")
        .and_then(|c| p.codes.lock().unwrap().remove(c))
    else {
        return refuse(&p);
    };
    // PKCE: the verifier's SHA-256 is the challenge it was asked with.
    let verifier = f.get("code_verifier").cloned().unwrap_or_default();
    if b64(&Sha256::digest(verifier.as_bytes())) != code.challenge
        || f.get("redirect_uri") != Some(&code.redirect)
        || f.get("grant_type").map(String::as_str) != Some("authorization_code")
    {
        return refuse(&p);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut claims = json!({
        "iss": p.issuer, "aud": CLIENT, "sub": code.approve.sub,
        "iat": now, "exp": now + 600, "nonce": code.nonce,
    });
    for (k, v) in code.approve.claims.as_object().unwrap() {
        claims[k] = v.clone();
    }
    let header = json!({"alg": "RS256", "kid": "test-1", "typ": "JWT"});
    let signing = format!(
        "{}.{}",
        b64(header.to_string().as_bytes()),
        b64(claims.to_string().as_bytes())
    );
    let key = if code.approve.wrong_key {
        &p.other
    } else {
        &p.key
    };
    let sig = SigningKey::<Sha256>::new(key.clone()).sign(signing.as_bytes());
    p.tokens.lock().unwrap().push(true);
    Json(json!({
        "access_token": "an-access-token", "token_type": "Bearer", "expires_in": 600,
        "id_token": format!("{signing}.{}", b64(&sig.to_bytes())),
    }))
    .into_response()
}

fn approve(sub: &str, claims: Value) -> Approve {
    Approve {
        sub: sub.into(),
        claims,
        wrong_key: false,
    }
}

// ---- POP, with the provider configured ----

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    fake: Fake,
}

fn provider(issuer: &str, admin_roles: &[&str]) -> Provider {
    let toml = format!(
        "id = \"css\"\nlabel = \"CS:S\"\nissuer = \"{issuer}\"\nclient_id = \"{CLIENT}\"\nscopes = [\"profile\", \"roles\"]\nadmin_roles = {:?}\n",
        admin_roles
    );
    toml::from_str(&toml).unwrap()
}

async fn pop_with(admin_roles: &[&str]) -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let fake = Fake::start().await;
    let state = AppState::new(db.clone(), Arc::new(Embedded)).with_sign_ins(SignIns::new(
        vec![provider(&fake.issuer, admin_roles)],
        PUBLIC,
    ));
    Pop {
        _dir: dir,
        db,
        app: router(state),
        fake,
    }
}

async fn pop() -> Pop {
    pop_with(&[]).await
}

struct Res {
    code: StatusCode,
    location: Option<String>,
    cookies: Vec<String>,
    body: Value,
}

impl Pop {
    async fn get(&self, uri: &str, cookies: &[&str]) -> Res {
        let mut req = Request::builder().method(Method::GET).uri(uri);
        if !cookies.is_empty() {
            req = req.header(header::COOKIE, cookies.join("; "));
        }
        let mut req = req.body(Body::empty()).unwrap();
        req.extensions_mut().insert(ConnectInfo(
            "203.0.113.9:5000".parse::<SocketAddr>().unwrap(),
        ));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let location = res
            .headers()
            .get(header::LOCATION)
            .map(|v| v.to_str().unwrap().to_owned());
        let cookies = res
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_owned())
            .collect();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        Res {
            code,
            location,
            cookies,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    /// Begin a sign-in: the provider's URL, and the state cookie set.
    async fn begin(&self, then: &str) -> (String, String) {
        let r = self
            .get(&format!("/api/sign-in/css?then={then}"), &[])
            .await;
        assert_eq!(r.code, StatusCode::SEE_OTHER, "{:?}", r.body);
        let to = r.location.unwrap();
        assert!(
            to.starts_with(&format!("{}/authorize?", self.fake.issuer)),
            "{to}"
        );
        let cookie = r
            .cookies
            .iter()
            .find(|c| c.starts_with("pop_sign_in="))
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        (to, cookie)
    }

    /// Back from the provider with `code`, for the state in `authorize`.
    async fn back(&self, authorize: &str, code: &str, cookie: &str) -> Res {
        let state = reqwest::Url::parse(authorize)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        self.get(
            &format!("/api/sign-in/css/callback?code={code}&state={state}"),
            &[cookie],
        )
        .await
    }

    /// A whole sign-in, approved as `a`: where it ends up, and the session
    /// cookie if it made one.
    async fn sign_in(&self, a: Approve) -> (Res, Option<String>) {
        let (to, cookie) = self.begin("/kiosk").await;
        let code = self.fake.approve(&to, a);
        let r = self.back(&to, &code, &cookie).await;
        let session = r
            .cookies
            .iter()
            .find(|c| c.starts_with("pop_session=") && !c.starts_with("pop_session=;"))
            .map(|c| c.split(';').next().unwrap().to_owned());
        (r, session)
    }

    async fn me(&self, session: &str) -> Value {
        let r = self.get("/api/session", &[session]).await;
        assert_eq!(r.code, StatusCode::OK, "{:?}", r.body);
        r.body
    }

    async fn audit(&self) -> Vec<String> {
        sqlx::query_scalar("SELECT summary FROM audit ORDER BY id")
            .fetch_all(&self.db)
            .await
            .unwrap()
    }
}

fn grace() -> Approve {
    approve(
        "sub-grace",
        json!({"name": "Grace Hopper", "preferred_username": "grace"}),
    )
}

#[tokio::test]
async fn the_providers_are_listed_for_a_screens_buttons() {
    let pop = pop().await;
    let r = pop.get("/api/sign-in/providers", &[]).await;
    assert_eq!(r.body, json!([{"id": "css", "label": "CS:S"}]));
}

#[tokio::test]
async fn a_first_sign_in_makes_a_member_account_and_signs_it_in() {
    let pop = pop().await;
    let (r, session) = pop.sign_in(grace()).await;
    assert_eq!(r.code, StatusCode::SEE_OTHER);
    assert_eq!(r.location.as_deref(), Some("/kiosk"), "back where it began");
    let me = pop.me(&session.expect("a session")).await;
    assert_eq!(me["user"]["name"], "Grace Hopper");
    assert_eq!(me["user"]["username"], "grace");
    assert_eq!(me["user"]["role"], "member");
    let log = pop.audit().await;
    assert!(
        log.contains(
            &"Grace Hopper's account was made at their first sign-in with CS:S".to_owned()
        ),
        "{log:?}"
    );
    assert!(
        log.iter()
            .any(|l| l.starts_with("Grace Hopper signed in with CS:S")),
        "{log:?}"
    );
    // The state cookie is cleared, and the state is good once.
    assert!(
        r.cookies.iter().any(|c| c.starts_with("pop_sign_in=;")),
        "{:?}",
        r.cookies
    );
}

#[tokio::test]
async fn the_next_sign_in_is_the_same_account() {
    let pop = pop().await;
    let (_, a) = pop.sign_in(grace()).await;
    let (_, b) = pop
        .sign_in(approve(
            "sub-grace",
            json!({"name": "Grace H.", "preferred_username": "ghopper"}),
        ))
        .await;
    let (a, b) = (pop.me(&a.unwrap()).await, pop.me(&b.unwrap()).await);
    assert_eq!(
        a["user"]["id"], b["user"]["id"],
        "linked by sub, whatever it is called now"
    );
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pop.db)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn an_account_is_never_taken_over_by_naming_yourself_after_it() {
    let pop = pop().await;
    users::create(
        &pop.db,
        NewAccount {
            username: "ada",
            name: "Ada",
            role: Role::Admin,
            password: "a long enough password",
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    // Someone at the provider calls themselves ada.
    let (_, s) = pop
        .sign_in(approve(
            "sub-someone",
            json!({"name": "Ada", "preferred_username": "ada", "email": "ada@example.org"}),
        ))
        .await;
    let me = pop.me(&s.unwrap()).await;
    assert_ne!(
        me["user"]["username"], "ada",
        "signed in as the existing ada"
    );
    assert_eq!(me["user"]["role"], "member");
    assert_eq!(me["user"]["username"], "ada-2");
}

#[tokio::test]
async fn a_token_the_provider_did_not_sign_signs_no_one_in() {
    let pop = pop().await;
    let (r, s) = pop
        .sign_in(Approve {
            wrong_key: true,
            ..grace()
        })
        .await;
    // The precondition: the provider did grant a token; POP refused it.
    assert_eq!(pop.fake.tokens(), [true]);
    assert_eq!(r.location.as_deref(), Some("/kiosk?sign_in=failed"));
    assert!(s.is_none());
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pop.db)
        .await
        .unwrap();
    assert_eq!(n, 0, "an account was made");
}

#[tokio::test]
async fn a_token_for_another_client_from_another_issuer_or_expired_signs_no_one_in() {
    for (what, claims) in [
        ("audience", json!({"aud": "another-app"})),
        ("issuer", json!({"iss": "https://elsewhere.example"})),
        ("expiry", json!({"exp": 1_000_000_000})),
        ("nonce", json!({"nonce": "not-the-one-asked-for"})),
    ] {
        let pop = pop().await;
        let (r, s) = pop.sign_in(approve("sub-grace", claims)).await;
        assert_eq!(
            pop.fake.tokens(),
            [true],
            "{what}: the provider granted one"
        );
        assert_eq!(
            r.location.as_deref(),
            Some("/kiosk?sign_in=failed"),
            "{what}"
        );
        assert!(s.is_none(), "{what}");
    }
}

#[tokio::test]
async fn a_return_this_browser_did_not_begin_is_refused() {
    let pop = pop().await;
    let (to, cookie) = pop.begin("/").await;
    let code = pop.fake.approve(&to, grace());
    // Without the cookie: someone else's state, carried to this browser.
    let r = pop.back(&to, &code, "pop_sign_in=another").await;
    assert_eq!(r.location.as_deref(), Some("/?sign_in=failed"));
    assert!(
        pop.fake.tokens().is_empty(),
        "the code was exchanged anyway"
    );
    // With it, the same return still works: a refused attempt spends nothing.
    let r = pop.back(&to, &code, &cookie).await;
    assert_eq!(r.location.as_deref(), Some("/"));
    // And once used, the state is spent.
    let again = pop.back(&to, &code, &cookie).await;
    assert_eq!(again.location.as_deref(), Some("/?sign_in=expired"));
}

#[tokio::test]
async fn only_pops_own_screens_are_gone_back_to() {
    let pop = pop().await;
    let (to, cookie) = pop.begin("https://evil.example/").await;
    let code = pop.fake.approve(&to, grace());
    let r = pop.back(&to, &code, &cookie).await;
    assert_eq!(r.location.as_deref(), Some("/"));
}

#[tokio::test]
async fn a_deactivated_account_is_not_signed_in() {
    let pop = pop().await;
    let (_, s) = pop.sign_in(grace()).await;
    let id = pop.me(&s.unwrap()).await["user"]["id"].as_i64().unwrap();
    users::set_active(&pop.db, id, false, &Actor::command_line())
        .await
        .unwrap();
    let (r, s) = pop.sign_in(grace()).await;
    assert_eq!(r.location.as_deref(), Some("/kiosk?sign_in=deactivated"));
    assert!(s.is_none());
}

#[tokio::test]
async fn a_cancelled_sign_in_goes_back_saying_so() {
    let pop = pop().await;
    let (to, cookie) = pop.begin("/admin").await;
    let state = reqwest::Url::parse(&to)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    let r = pop
        .get(
            &format!("/api/sign-in/css/callback?error=access_denied&state={state}"),
            &[&cookie],
        )
        .await;
    assert_eq!(r.location.as_deref(), Some("/admin?sign_in=cancelled"));
}

// ---- roles (cal, 2026-11-19) ----

#[tokio::test]
async fn a_configured_role_makes_an_admin_and_losing_it_takes_back_only_that() {
    let pop = pop_with(&["print-admin"]).await;
    // Someone must stay an admin whatever happens to Grace.
    users::create(
        &pop.db,
        NewAccount {
            username: "ada",
            name: "Ada",
            role: Role::Admin,
            password: "a long enough password",
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    let with = |roles: Value| {
        approve(
            "sub-grace",
            json!({"name": "Grace Hopper", "preferred_username": "grace", "roles": roles}),
        )
    };
    let (_, s) = pop.sign_in(with(json!(["member", "print-admin"]))).await;
    assert_eq!(pop.me(&s.unwrap()).await["user"]["role"], "admin");
    let (_, s) = pop.sign_in(with(json!(["member"]))).await;
    assert_eq!(
        pop.me(&s.unwrap()).await["user"]["role"],
        "member",
        "what the role gave, it took back"
    );
    let log = pop.audit().await;
    assert!(
        log.iter()
            .any(|l| l
                == "the CS:S sign-in made Grace Hopper an admin: CS:S gives them an admin role"),
        "{log:?}"
    );
    assert!(log.iter().any(|l| l == "the CS:S sign-in made Grace Hopper a member: CS:S no longer gives them an admin role"), "{log:?}");
}

#[tokio::test]
async fn an_admin_made_on_the_desk_is_not_demoted_for_lacking_the_role() {
    let pop = pop_with(&["print-admin"]).await;
    let (_, s) = pop.sign_in(grace()).await;
    let id = pop.me(&s.unwrap()).await["user"]["id"].as_i64().unwrap();
    // Two admins, so the last-admin rule is not what keeps her one.
    users::create(
        &pop.db,
        NewAccount {
            username: "ada",
            name: "Ada",
            role: Role::Admin,
            password: "a long enough password",
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    users::set_role(&pop.db, id, Role::Admin, &Actor::command_line())
        .await
        .unwrap();
    let (_, s) = pop.sign_in(grace()).await;
    assert_eq!(pop.me(&s.unwrap()).await["user"]["role"], "admin");
}

#[tokio::test]
async fn the_desk_taking_over_a_role_the_provider_gave_keeps_it() {
    // An admin the role made, then confirmed on the desk: the desk's now.
    let pop = pop_with(&["print-admin"]).await;
    users::create(
        &pop.db,
        NewAccount {
            username: "ada",
            name: "Ada",
            role: Role::Admin,
            password: "a long enough password",
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    let with = |roles: Value| approve("sub-grace", json!({"name": "Grace", "roles": roles}));
    let (_, s) = pop.sign_in(with(json!(["print-admin"]))).await;
    let id = pop.me(&s.unwrap()).await["user"]["id"].as_i64().unwrap();
    users::set_role(&pop.db, id, Role::Member, &Actor::command_line())
        .await
        .unwrap();
    users::set_role(&pop.db, id, Role::Admin, &Actor::command_line())
        .await
        .unwrap();
    let (_, s) = pop.sign_in(with(json!([]))).await;
    assert_eq!(pop.me(&s.unwrap()).await["user"]["role"], "admin");
}

#[tokio::test]
async fn the_last_admin_is_never_taken_back() {
    let pop = pop_with(&["print-admin"]).await;
    let with = |roles: Value| approve("sub-grace", json!({"name": "Grace", "roles": roles}));
    let (_, s) = pop.sign_in(with(json!(["print-admin"]))).await;
    assert_eq!(pop.me(&s.unwrap()).await["user"]["role"], "admin");
    // She is the only admin: losing the role does not leave POP with none.
    let (_, s) = pop.sign_in(with(json!([]))).await;
    assert_eq!(pop.me(&s.unwrap()).await["user"]["role"], "admin");
}

#[tokio::test]
async fn with_no_admin_roles_configured_the_provider_decides_nothing() {
    let pop = pop().await;
    let (_, s) = pop
        .sign_in(approve(
            "sub-grace",
            json!({"name": "Grace", "roles": ["print-admin", "admin"]}),
        ))
        .await;
    assert_eq!(pop.me(&s.unwrap()).await["user"]["role"], "member");
}
