//! Signing in through an OpenID Connect provider (#14), CS:S first (#16).
//!
//! The authorization code flow with PKCE (S256), a `state` bound to the
//! browser by a short-lived cookie, and a nonce:
//!
//! 1. `GET /api/sign-in/{id}?then=/kiosk` sends the browser to the provider,
//!    and keeps what the return must match (the state, the PKCE verifier, the
//!    nonce, where to go after) for ten minutes, once.
//! 2. `GET /api/sign-in/{id}/callback?code&state` takes the code back: the
//!    state must be the one this browser was given, the code is exchanged
//!    with the verifier (and the client's secret, if it has one), and the ID
//!    token must be signed by a key in the provider's published set, for this
//!    client, from this issuer, unexpired, with the nonce. Then:
//!    - the account linked to the provider's `sub` is signed in; the first
//!      sign-in with a new `sub` makes a member account (cal, 2026-11-19).
//!      An account is linked by `sub` only, never by email or username, which
//!      would let someone take over an account by naming themselves after it;
//!    - a provider with `admin_roles` makes its members admins, and takes back
//!      what it gave (users::apply_provider_roles);
//!    - a session starts as a password sign-in's would, a kiosk session at an
//!      enrolled kiosk, and the browser goes where it came from.
//!
//! The provider's discovery document and keys are fetched for each sign-in,
//! so a key it rotates is seen at once. Its HTTP is POP's own client, which
//! follows no redirects.

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, HttpRequest,
    HttpResponse, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    TokenResponse as _,
    core::{CoreClient, CoreProviderMetadata, CoreResponseType},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::{AppState, config::Provider, users};

/// The cookie that binds a sign-in's state to the browser that began it.
pub const STATE_COOKIE: &str = "pop_sign_in";
/// How long a sign-in may be left half-finished.
const PENDING: Duration = Duration::from_secs(600);
/// The most sign-ins waiting at once; past it, the oldest are forgotten.
const MAX_PENDING: usize = 1000;
/// Where a sign-in may send the browser back to: POP's own screens only.
const BACK_TO: [&str; 3] = ["/", "/kiosk", "/admin"];

/// A sign-in provider, as screens offer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct SignInProvider {
    pub id: String,
    /// "Sign in with {label}".
    pub label: String,
}

/// What a sign-in's return must match.
struct Pending {
    provider: String,
    verifier: String,
    nonce: String,
    then: &'static str,
    at: Instant,
}

/// The configured providers, and the sign-ins under way.
#[derive(Clone)]
pub struct SignIns {
    providers: Arc<Vec<Provider>>,
    public_url: String,
    http: reqwest::Client,
    pending: Arc<Mutex<HashMap<String, Pending>>>,
}

impl SignIns {
    pub fn new(providers: Vec<Provider>, public_url: &str) -> Self {
        Self {
            providers: Arc::new(providers),
            public_url: public_url.trim_end_matches('/').to_owned(),
            http: reqwest::Client::builder()
                // A provider's answer is never followed elsewhere.
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()
                .expect("an HTTP client"),
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    fn redirect_uri(&self, id: &str) -> String {
        format!("{}/api/sign-in/{id}/callback", self.public_url)
    }

    fn keep(&self, state: String, p: Pending) {
        let mut all = self.pending.lock().unwrap();
        all.retain(|_, v| v.at.elapsed() < PENDING);
        if all.len() >= MAX_PENDING
            && let Some(oldest) = all.iter().min_by_key(|(_, v)| v.at).map(|(k, _)| k.clone())
        {
            all.remove(&oldest);
        }
        all.insert(state, p);
    }

    /// The sign-in `state` began, taken: a state is good once.
    fn take(&self, state: &str) -> Option<Pending> {
        let mut all = self.pending.lock().unwrap();
        all.remove(state).filter(|p| p.at.elapsed() < PENDING)
    }
}

type HttpFuture = Pin<Box<dyn Future<Output = Result<HttpResponse, HttpError>> + Send>>;

/// A request POP made for a sign-in that failed on the way.
#[derive(Debug)]
pub struct HttpError(String);

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HttpError {}

/// openidconnect's HTTP, through POP's own client.
fn through(client: reqwest::Client) -> impl Fn(HttpRequest) -> HttpFuture {
    move |req: HttpRequest| {
        let client = client.clone();
        Box::pin(async move {
            let (parts, body) = req.into_parts();
            let res = client
                .request(parts.method, parts.uri.to_string())
                .headers(parts.headers)
                .body(body)
                .send()
                .await
                .map_err(|e| HttpError(e.to_string()))?;
            let mut out = openidconnect::http::Response::builder().status(res.status());
            for (k, v) in res.headers() {
                out = out.header(k, v);
            }
            let bytes = res.bytes().await.map_err(|e| HttpError(e.to_string()))?;
            out.body(bytes.to_vec())
                .map_err(|e| HttpError(e.to_string()))
        }) as HttpFuture
    }
}

fn plain(code: StatusCode, msg: &str) -> Response {
    (code, msg.to_owned()).into_response()
}

/// Back to `then`, saying the sign-in did not finish, and why in a word.
fn back(then: &str, why: &str) -> Response {
    let mut res = StatusCode::SEE_OTHER.into_response();
    let to = format!("{then}?sign_in={why}");
    res.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&to).expect("a path"),
    );
    res.headers_mut()
        .append(header::SET_COOKIE, state_cookie("", 0));
    res
}

fn state_cookie(value: &str, max_age: u64) -> HeaderValue {
    // Lax, not Strict: the browser comes back from the provider's site, and
    // a Strict cookie would not come with it.
    HeaderValue::from_str(&format!(
        "{STATE_COOKIE}={value}; Path=/api/sign-in; HttpOnly; SameSite=Lax; Max-Age={max_age}"
    ))
    .expect("a cookie")
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
}

/// `GET /api/sign-in/providers`: the providers a screen offers buttons for.
pub async fn providers(State(state): State<AppState>) -> Response {
    let list: Vec<SignInProvider> = state
        .sign_ins
        .as_ref()
        .map(|s| {
            s.providers
                .iter()
                .map(|p| SignInProvider {
                    id: p.id.clone(),
                    label: p.label.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    Json(list).into_response()
}

#[derive(Deserialize)]
pub struct Start {
    #[serde(default)]
    then: Option<String>,
}

/// `GET /api/sign-in/{id}`: to the provider.
pub async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<Start>,
) -> Response {
    let Some(s) = state.sign_ins.as_ref() else {
        return plain(StatusCode::NOT_FOUND, "POP signs in with no provider");
    };
    let Some(p) = s.provider(&id) else {
        return plain(StatusCode::NOT_FOUND, "POP has no such sign-in provider");
    };
    let then = BACK_TO
        .iter()
        .copied()
        .find(|t| q.then.as_deref() == Some(*t))
        .unwrap_or("/");
    let client = match client(s, p).await {
        Ok(c) => c,
        Err(why) => {
            tracing::warn!("signing in with {}: {why}", p.label);
            return back(then, "unreachable");
        }
    };
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let mut req = client.authorize_url(
        AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
        CsrfToken::new_random,
        Nonce::new_random,
    );
    for scope in &p.scopes {
        req = req.add_scope(Scope::new(scope.clone()));
    }
    let (url, csrf, nonce) = req.set_pkce_challenge(challenge).url();
    s.keep(
        csrf.secret().clone(),
        Pending {
            provider: p.id.clone(),
            verifier: verifier.secret().clone(),
            nonce: nonce.secret().clone(),
            then,
            at: Instant::now(),
        },
    );
    let mut res = StatusCode::SEE_OTHER.into_response();
    let h = res.headers_mut();
    h.insert(
        header::LOCATION,
        HeaderValue::from_str(url.as_str()).expect("a URL"),
    );
    h.insert(
        header::SET_COOKIE,
        state_cookie(csrf.secret(), PENDING.as_secs()),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

type Client = openidconnect::Client<
    openidconnect::EmptyAdditionalClaims,
    openidconnect::core::CoreAuthDisplay,
    openidconnect::core::CoreGenderClaim,
    openidconnect::core::CoreJweContentEncryptionAlgorithm,
    openidconnect::core::CoreJsonWebKey,
    openidconnect::core::CoreAuthPrompt,
    openidconnect::StandardErrorResponse<openidconnect::core::CoreErrorResponseType>,
    openidconnect::core::CoreTokenResponse,
    openidconnect::core::CoreTokenIntrospectionResponse,
    openidconnect::core::CoreRevocableToken,
    openidconnect::core::CoreRevocationErrorResponse,
    openidconnect::EndpointSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointMaybeSet,
    openidconnect::EndpointMaybeSet,
>;

/// The provider's client, from its discovery document, fetched now.
async fn client(s: &SignIns, p: &Provider) -> Result<Client, String> {
    let issuer = IssuerUrl::new(p.issuer.clone()).map_err(|e| format!("its issuer: {e}"))?;
    let http = through(s.http.clone());
    let meta = CoreProviderMetadata::discover_async(issuer, &http)
        .await
        .map_err(|e| format!("its discovery document: {e}"))?;
    let secret = (!p.client_secret.is_empty()).then(|| ClientSecret::new(p.client_secret.clone()));
    let redirect =
        RedirectUrl::new(s.redirect_uri(&p.id)).map_err(|e| format!("POP's address: {e}"))?;
    Ok(
        CoreClient::from_provider_metadata(meta, ClientId::new(p.client_id.clone()), secret)
            .set_redirect_uri(redirect),
    )
}

#[derive(Deserialize)]
pub struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// `GET /api/sign-in/{id}/callback`: back from the provider.
pub async fn callback(
    State(state): State<AppState>,
    Path(id): Path<String>,
    connect: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    Query(q): Query<Callback>,
) -> Response {
    let Some(s) = state.sign_ins.as_ref() else {
        return plain(StatusCode::NOT_FOUND, "POP signs in with no provider");
    };
    let Some(p) = s.provider(&id) else {
        return plain(StatusCode::NOT_FOUND, "POP has no such sign-in provider");
    };
    // The state must be the one this browser was given, and is good once.
    let Some(st) = q.state.filter(|v| !v.is_empty()) else {
        return back("/", "failed");
    };
    if cookie(&headers, STATE_COOKIE).as_deref() != Some(st.as_str()) {
        tracing::warn!(
            "signing in with {}: a state this browser was not given",
            p.label
        );
        return back("/", "failed");
    }
    let Some(pending) = s.take(&st).filter(|w| w.provider == p.id) else {
        return back("/", "expired");
    };
    let then = pending.then;
    if q.error.is_some() {
        return back(then, "cancelled");
    }
    let Some(code) = q.code.filter(|c| !c.is_empty()) else {
        return back(then, "failed");
    };
    let signed = match verified(s, p, code, &pending).await {
        Ok(v) => v,
        Err(why) => {
            tracing::warn!("signing in with {}: {why}", p.label);
            return back(then, "failed");
        }
    };
    let user = match users::by_identity(&state.db, &p.id, &signed.subject).await {
        Ok(Some((_, false))) => return back(then, "deactivated"),
        Ok(Some((u, true))) => u,
        Ok(None) => match users::create_from_provider(
            &state.db,
            &p.id,
            &p.label,
            &signed.subject,
            signed.username.as_deref().unwrap_or(""),
            signed.name.as_deref().unwrap_or(""),
        )
        .await
        {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("signing in with {}: making the account: {e:?}", p.label);
                return back(then, "failed");
            }
        },
        Err(e) => {
            tracing::error!("signing in with {}: {e:?}", p.label);
            return back(then, "failed");
        }
    };
    let user = if p.admin_roles.is_empty() {
        user
    } else {
        let holds = signed.roles.iter().any(|r| p.admin_roles.contains(r));
        match users::apply_provider_roles(&state.db, user, &p.id, &p.label, holds).await {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("signing in with {}: its roles: {e:?}", p.label);
                return back(then, "failed");
            }
        }
    };
    let peer = connect.map(|axum::Extension(c)| c.0.ip());
    let token = match crate::auth::begin(&state, peer, &headers, &user, Some(&p.label)).await {
        Ok(t) => t,
        Err(r) => return *r,
    };
    let mut res = StatusCode::SEE_OTHER.into_response();
    let h = res.headers_mut();
    h.insert(header::LOCATION, HeaderValue::from_static(then));
    h.append(
        header::SET_COOKIE,
        crate::auth::session_cookie(&token, users::SESSION_SECS),
    );
    h.append(header::SET_COOKIE, state_cookie("", 0));
    res
}

/// Who the provider says signed in, from an ID token it signed.
struct Signed {
    subject: String,
    username: Option<String>,
    name: Option<String>,
    roles: Vec<String>,
}

async fn verified(s: &SignIns, p: &Provider, code: String, w: &Pending) -> Result<Signed, String> {
    let client = client(s, p).await?;
    let http = through(s.http.clone());
    let token = client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|e| format!("its token endpoint: {e}"))?
        .set_pkce_verifier(PkceCodeVerifier::new(w.verifier.clone()))
        .request_async(&http)
        .await
        .map_err(|e| format!("exchanging the code: {e}"))?;
    let id_token = token.id_token().ok_or("its answer had no ID token")?;
    let claims = id_token
        .claims(&client.id_token_verifier(), &Nonce::new(w.nonce.clone()))
        .map_err(|e| format!("its ID token: {e}"))?;
    let subject = claims.subject().as_str().to_owned();
    let username = claims.preferred_username().map(|u| u.as_str().to_owned());
    let name = claims
        .name()
        .and_then(|n| n.get(None))
        .map(|n| n.as_str().to_owned());
    // The roles claim is not one of OpenID Connect's own. It is read from the
    // same token just verified: its payload, which the signature covers.
    let roles = roles_in(&id_token.to_string(), &p.roles_claim);
    Ok(Signed {
        subject,
        username,
        name,
        roles,
    })
}

/// The roles a compact JWT's payload names in `claim`: a list of strings, or
/// one string.
fn roles_in(jwt: &str, claim: &str) -> Vec<String> {
    let payload = jwt.split('.').nth(1).unwrap_or("");
    let Ok(bytes) = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')) else {
        return Vec::new();
    };
    match serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v.get(claim).cloned())
    {
        Some(Value::Array(a)) => a
            .into_iter()
            .filter_map(|r| r.as_str().map(str::to_owned))
            .collect(),
        Some(Value::String(r)) => vec![r],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(payload: &str) -> String {
        format!("e30.{}.c2ln", URL_SAFE_NO_PAD.encode(payload))
    }

    #[test]
    fn roles_are_read_as_a_list_or_one() {
        assert_eq!(
            roles_in(&jwt(r#"{"roles":["member","print-admin"]}"#), "roles"),
            ["member", "print-admin"]
        );
        assert_eq!(
            roles_in(&jwt(r#"{"groups":"print-admin"}"#), "groups"),
            ["print-admin"]
        );
        assert!(roles_in(&jwt(r#"{"roles":[1,2]}"#), "roles").is_empty());
        assert!(roles_in(&jwt(r#"{}"#), "roles").is_empty());
        assert!(roles_in("not a jwt", "roles").is_empty());
    }
}
