//! Signing in and out, and managing accounts (#11).

use axum::{
    Extension, Json,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::{
    AppState,
    access::{SESSION_COOKIE, session_token},
    api::{ApiError, NewPassword, NewRole, NewUser, SignIn, User},
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

/// `POST /api/session`: sign in.
pub async fn sign_in(
    State(state): State<AppState>,
    b: Result<Json<SignIn>, JsonRejection>,
) -> Response {
    let b = match body(b) {
        Ok(b) => b,
        Err(r) => return r.into_response(),
    };
    let user = match users::authenticate(&state.db, &b.username, &b.password).await {
        Ok(Some(u)) => u,
        // The same answer whichever was wrong.
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "wrong username or password"),
        Err(e) => return e.into_response(),
    };
    match users::start_session(&state.db, user.id).await {
        Ok(token) => {
            let mut res = Json(user).into_response();
            res.headers_mut()
                .insert(header::SET_COOKIE, session_cookie(&token, SESSION_SECS));
            res
        }
        Err(e) => e.into_response(),
    }
}

/// `DELETE /api/session`: sign out.
pub async fn sign_out(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(t) = session_token(&headers)
        && let Err(e) = users::end_session(&state.db, &t).await
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
    match users::list(&state.db).await {
        Ok(u) => Json(u).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `POST /api/admin/users`.
pub async fn create(
    State(state): State<AppState>,
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
    match users::create(&state.db, account).await {
        Ok(u) => (StatusCode::CREATED, Json(u)).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `PUT /api/admin/users/{id}/password`: an admin resets it. Every session
/// the account had ends.
pub async fn reset_password(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    b: Result<Json<NewPassword>, JsonRejection>,
) -> Response {
    let b = match body(b) {
        Ok(b) => b,
        Err(r) => return r.into_response(),
    };
    match users::reset_password(&state.db, id, &b.password).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

/// `PUT /api/admin/users/{id}/role`.
pub async fn set_role(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    b: Result<Json<NewRole>, JsonRejection>,
) -> Response {
    let b = match body(b) {
        Ok(b) => b,
        Err(r) => return r.into_response(),
    };
    match users::set_role(&state.db, id, b.role).await {
        Ok(u) => Json(u).into_response(),
        Err(e) => e.into_response(),
    }
}
