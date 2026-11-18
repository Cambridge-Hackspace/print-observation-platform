//! Signing in and out, and managing accounts (#11).

use axum::{
    Extension, Json,
    extract::{Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::{
    AppState,
    access::{SESSION_COOKIE, session_token},
    api::{
        ApiError, AuditKind, NewActive, NewName, NewPassword, NewRole, NewTraining, NewUser,
        SignIn, User,
    },
    audit::{self, Actor, Filter},
    users::{self, NewAccount, SESSION_SECS, UserError},
};

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ApiError { error: msg.into() })).into_response()
}

impl IntoResponse for UserError {
    fn into_response(self) -> Response {
        match self {
            UserError::Invalid { field, reason } => {
                error(StatusCode::BAD_REQUEST, format!("{field}: {reason}"))
            }
            UserError::UsernameTaken => error(StatusCode::CONFLICT, "username: already taken"),
            UserError::NotFound => error(StatusCode::NOT_FOUND, "no such account"),
            UserError::LastAdmin => error(
                StatusCode::CONFLICT,
                "role: this is the only admin; make someone else an admin first",
            ),
            UserError::Database(e) => {
                tracing::error!("accounts: {e}");
                error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "accounts are not working right now",
                )
            }
        }
    }
}

/// A body that is not the JSON a route expects, with what is wrong.
pub struct BadBody(String);

impl IntoResponse for BadBody {
    fn into_response(self) -> Response {
        error(StatusCode::BAD_REQUEST, self.0)
    }
}

/// The body a route expects, or what is wrong with the one it was sent.
fn body<T>(b: Result<Json<T>, JsonRejection>) -> Result<T, BadBody> {
    b.map(|Json(v)| v).map_err(|e| BadBody(e.body_text()))
}

fn session_cookie(token: &str, max_age: i64) -> HeaderValue {
    // HttpOnly: no script reads it. SameSite=Lax: another site's requests do
    // not carry it, except plain navigation. Secure is for a deployment
    // behind HTTPS, and arrives with the remote access guide (#93).
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}"
    ))
    .expect("a cookie of base64url and fixed text is a valid header")
}

/// `POST /api/session`: sign in. One from outside the space says so in the
/// audit log, and an admin's is told to the group chat as well (#12): an admin
/// online holds every class, so the group should know when one arrives.
pub async fn sign_in(
    State(state): State<AppState>,
    connect: Option<Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    b: Result<Json<SignIn>, JsonRejection>,
) -> Response {
    let b = match body(b) {
        Ok(b) => b,
        Err(r) => return r.into_response(),
    };
    let user = match users::authenticate(&state.db, &b.username, &b.password).await {
        Ok(Some(u)) => u,
        // The same answer whichever was wrong.
        Ok(None) => {
            if let Err(e) = users::sign_in_failed(&state.db, &b.username).await {
                return e.into_response();
            }
            return error(StatusCode::UNAUTHORIZED, "wrong username or password");
        }
        Err(e) => return e.into_response(),
    };
    let peer = connect.map(|Extension(c)| c.0.ip());
    let outside = !crate::access::is_local(&state, peer, &headers);
    match users::start_session(&state.db, &user, outside).await {
        Ok(token) => {
            if outside
                && user.role == crate::api::Role::Admin
                && let Some(n) = &state.notifier
            {
                n.post(format!(
                    "{} signed in to POP as an admin from outside the space.",
                    user.name
                ));
            }
            let mut res = Json(user).into_response();
            res.headers_mut()
                .insert(header::SET_COOKIE, session_cookie(&token, SESSION_SECS));
            res
        }
        Err(e) => e.into_response(),
    }
}

/// `DELETE /api/session`: sign out.
pub async fn sign_out(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    headers: HeaderMap,
) -> Response {
    if let Some(t) = session_token(&headers)
        && let Err(e) = users::end_session(&state.db, &t, &user).await
    {
        return e.into_response();
    }
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut()
        .insert(header::SET_COOKIE, session_cookie("", 0));
    res
}

/// `GET /api/me`: who is signed in.
pub async fn me(Extension(user): Extension<User>) -> Json<User> {
    Json(user)
}

/// `GET /api/admin/users`.
pub async fn list(State(state): State<AppState>) -> Response {
    match users::accounts(&state.db).await {
        Ok(u) => Json(u).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `POST /api/admin/users`.
pub async fn create(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    b: Result<Json<NewUser>, JsonRejection>,
) -> Response {
    let b = match body(b) {
        Ok(b) => b,
        Err(r) => return r.into_response(),
    };
    let account = NewAccount {
        username: &b.username,
        name: &b.name,
        role: b.role,
        password: &b.password,
    };
    match users::create(&state.db, account, &Actor::user(admin.id, &admin.name)).await {
        Ok(u) => (StatusCode::CREATED, Json(u)).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `PUT /api/admin/users/{id}/password`: an admin resets it. Every session
/// the account had ends.
pub async fn reset_password(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(id): Path<i64>,
    b: Result<Json<NewPassword>, JsonRejection>,
) -> Response {
    let b = match body(b) {
        Ok(b) => b,
        Err(r) => return r.into_response(),
    };
    match users::reset_password(
        &state.db,
        id,
        &b.password,
        &Actor::user(admin.id, &admin.name),
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

/// `PUT /api/admin/users/{id}/role`.
pub async fn set_role(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(id): Path<i64>,
    b: Result<Json<NewRole>, JsonRejection>,
) -> Response {
    let b = match body(b) {
        Ok(b) => b,
        Err(r) => return r.into_response(),
    };
    match users::set_role(&state.db, id, b.role, &Actor::user(admin.id, &admin.name)).await {
        Ok(u) => Json(u).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `PUT /api/admin/users/{id}/name`.
pub async fn set_name(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(id): Path<i64>,
    b: Result<Json<NewName>, JsonRejection>,
) -> Response {
    let b = match body(b) {
        Ok(b) => b,
        Err(r) => return r.into_response(),
    };
    match users::set_name(&state.db, id, &b.name, &Actor::user(admin.id, &admin.name)).await {
        Ok(a) => Json(a).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `PUT /api/admin/users/{id}/active`.
pub async fn set_active(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(id): Path<i64>,
    b: Result<Json<NewActive>, JsonRejection>,
) -> Response {
    let b = match body(b) {
        Ok(b) => b,
        Err(r) => return r.into_response(),
    };
    match users::set_active(&state.db, id, b.active, &Actor::user(admin.id, &admin.name)).await {
        Ok(a) => Json(a).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `PUT /api/admin/users/{id}/training`.
pub async fn set_training(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(id): Path<i64>,
    b: Result<Json<NewTraining>, JsonRejection>,
) -> Response {
    let b = match body(b) {
        Ok(b) => b,
        Err(r) => return r.into_response(),
    };
    match users::set_training(
        &state.db,
        id,
        &b.training,
        &Actor::user(admin.id, &admin.name),
    )
    .await
    {
        Ok(a) => Json(a).into_response(),
        Err(e) => e.into_response(),
    }
}

/// The query `GET /api/admin/audit` takes.
#[derive(serde::Deserialize)]
pub struct AuditQuery {
    /// An account: entries where it acted or was acted on.
    pub person: Option<i64>,
    pub printer: Option<String>,
    pub kind: Option<AuditKind>,
    /// Entries older than this one, to page back.
    pub before: Option<i64>,
    /// How many, at most 500; 100 if not given.
    pub limit: Option<i64>,
}

/// `GET /api/admin/audit`: the newest entries first.
pub async fn audit_log(
    State(state): State<AppState>,
    q: Result<Query<AuditQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Query(q) = match q {
        Ok(q) => q,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.body_text()),
    };
    let f = Filter {
        person: q.person,
        printer: q.printer,
        kind: q.kind,
        before: q.before,
        limit: q.limit.unwrap_or(100).clamp(1, 500),
    };
    match audit::read(&state.db, &f).await {
        Ok(entries) => Json(entries).into_response(),
        Err(e) => {
            tracing::error!("audit: {e}");
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "the audit log is not working right now",
            )
        }
    }
}
