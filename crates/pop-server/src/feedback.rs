//! Feedback from any screen, filed as GitHub issues (#96).
//!
//! A person holding `feedback.file` (a role named in `[feedback] roles`)
//! sends a report: what it is, a title, what happened, the redacted and
//! annotated screenshot, and what the screen knew. POP keeps it, and a
//! sender files it through the GitHub App: the screenshot onto a branch of
//! the repository, then an issue that shows it. A report made while GitHub
//! or the network is down stays kept and is tried again, with a growing
//! pause, until GitHub has it; the person sees it sent only once the issue
//! exists. GitHub refusing the App is not tried again: the report is failed,
//! and the log says why.
//!
//! Everything in a report goes into a public issue. So the context is
//! scrubbed again here, whatever the screen did: secrets by their keys, and
//! tokens, cookies and query strings wherever they appear. The printers'
//! states are added by POP itself, with nobody's name in them. The
//! screenshot must be a PNG, and leaves without its metadata.
//!
//! Sending is safe to repeat (`pop_integrations::github`): a report retried
//! after its picture went up finds the picture there. One whose issue was
//! filed but whose answer was lost would be filed twice; POP cannot tell
//! that apart from one never filed.

use std::{sync::Arc, time::Duration};

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use base64::Engine as _;
use pop_integrations::github::GitHub;
use serde_json::{Map, Value, json};
use sqlx::{Row, SqlitePool};
use tokio::{sync::Notify, task::JoinHandle};

use crate::{
    AppState,
    api::{
        ApiError, AuditKind, Device, FeedbackKind, FeedbackSettings, FeedbackState, FeedbackStatus,
        NewFeedback, Role, User,
    },
    audit::{self, Actor, Event},
    config,
};

/// The largest screenshot taken, decoded.
pub const MAX_SCREENSHOT: usize = 4 << 20;
const MAX_TITLE: usize = 120;
const MAX_DESCRIPTION: usize = 5000;
const MAX_CONTEXT: usize = 64 << 10;

/// Feedback, as configured: who may file it, and where it goes.
pub struct Feedback {
    pub config: config::Feedback,
    pub github: GitHub,
    /// Told when a report is kept, so the sender looks at once.
    pub wake: Arc<Notify>,
}

impl Feedback {
    fn allowed(&self, user: &User) -> bool {
        let role = match user.role {
            Role::Admin => "admin",
            Role::Member => "member",
        };
        self.config.roles.iter().any(|r| r == role)
    }
}

fn refuse(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ApiError { error: msg.into() })).into_response()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Now, in milliseconds: when a report is next tried is kept to the
/// millisecond, so a retry's pause is the one asked for.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// `GET /api/feedback`: whether the signed-in person can file feedback, so
/// a screen shows the button only when they can.
pub async fn settings(State(state): State<AppState>, Extension(user): Extension<User>) -> Response {
    let (enabled, allowed, per_hour) = match &state.feedback {
        Some(f) => (true, f.allowed(&user), f.config.per_hour),
        None => (false, false, 0),
    };
    Json(FeedbackSettings {
        enabled,
        allowed,
        per_hour,
    })
    .into_response()
}

/// `POST /api/feedback`: keep a report and send it.
pub async fn file(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    body: Result<Json<NewFeedback>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some(f) = state.feedback.clone() else {
        return refuse(StatusCode::NOT_FOUND, "feedback is not set up on this POP");
    };
    if !f.allowed(&user) {
        return refuse(StatusCode::FORBIDDEN, "filing feedback needs feedback.file");
    }
    let Json(r) = match body {
        Ok(b) => b,
        Err(e) => return refuse(StatusCode::BAD_REQUEST, e.body_text()),
    };
    let title = r.title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE {
        return refuse(
            StatusCode::BAD_REQUEST,
            format!("title: say what it is in 1 to {MAX_TITLE} characters"),
        );
    }
    if r.description.chars().count() > MAX_DESCRIPTION {
        return refuse(
            StatusCode::BAD_REQUEST,
            format!("description: at most {MAX_DESCRIPTION} characters"),
        );
    }
    let Ok(png) = base64::engine::general_purpose::STANDARD.decode(r.screenshot.trim()) else {
        return refuse(StatusCode::BAD_REQUEST, "screenshot: not base64");
    };
    if png.len() > MAX_SCREENSHOT {
        return refuse(StatusCode::PAYLOAD_TOO_LARGE, "screenshot: at most 4 MB");
    }
    let Some(png) = without_metadata(&png) else {
        return refuse(StatusCode::BAD_REQUEST, "screenshot: not a PNG");
    };
    let hour_ago = now() - 3600;
    let recent: i64 = match sqlx::query_scalar(
        "SELECT COUNT(*) FROM feedback_reports WHERE account_id = ? AND created_at > ?",
    )
    .bind(user.id)
    .bind(hour_ago)
    .fetch_one(&state.db)
    .await
    {
        Ok(n) => n,
        Err(e) => return refuse(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    if recent >= i64::from(f.config.per_hour) {
        return refuse(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "at most {} reports an hour; try again later",
                f.config.per_hour
            ),
        );
    }
    let mut context = scrub(&r.context);
    if serde_json::to_string(&context).map_or(usize::MAX, |s| s.len()) > MAX_CONTEXT {
        context = json!({"note": "the screen's context was too large to attach"});
    }
    // What POP itself knows, nobody named.
    let printers = crate::events::named(
        &crate::api::ServerEvent::Printers(state.tower.borrow().clone()),
        crate::events::Names::None,
    );
    let id = report_id();
    let body = compose(
        &r,
        &user,
        &context,
        &serde_json::to_value(printers).unwrap_or_default(),
        &id,
    );
    let kind = match r.kind {
        FeedbackKind::Bug => "bug",
        FeedbackKind::Suggestion => "suggestion",
        FeedbackKind::Question => "question",
    };
    if let Err(e) = sqlx::query(
        "INSERT INTO feedback_reports (id, created_at, account_id, kind, title, body, screenshot, next_try_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(now())
    .bind(user.id)
    .bind(kind)
    .bind(title)
    .bind(&body)
    .bind(&png)
    .bind(now_ms())
    .execute(&state.db)
    .await
    {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, e.to_string());
    }
    f.wake.notify_one();
    (
        StatusCode::ACCEPTED,
        Json(FeedbackStatus {
            id,
            state: FeedbackState::Pending,
            issue_url: None,
        }),
    )
        .into_response()
}

/// `GET /api/feedback/{id}`: where a report stands, for whoever filed it.
pub async fn status(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Response {
    let row = sqlx::query(
        "SELECT state, issue_url FROM feedback_reports WHERE id = ? AND account_id = ?",
    )
    .bind(&id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some(r)) => {
            let s: String = r.get("state");
            Json(FeedbackStatus {
                id,
                state: match s.as_str() {
                    "sent" => FeedbackState::Sent,
                    "failed" => FeedbackState::Failed,
                    _ => FeedbackState::Pending,
                },
                issue_url: r.get("issue_url"),
            })
            .into_response()
        }
        Ok(None) => refuse(StatusCode::NOT_FOUND, "no such report of yours"),
        Err(e) => refuse(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

fn report_id() -> String {
    use rand::Rng as _;
    let bytes: [u8; 12] = rand::thread_rng().r#gen();
    hex::encode(bytes)
}

/// The issue's body: what the person wrote, who and where, the context. The
/// screenshot's place is `{screenshot}`, filled in once it is uploaded.
fn compose(r: &NewFeedback, user: &User, context: &Value, printers: &Value, id: &str) -> String {
    // The screen it was sent from, by the name people know it by: "phone"
    // alone read as the device, for a report sent from the phone app open on
    // a desktop browser.
    let device = match r.device {
        Device::Kiosk => "kiosk",
        Device::Phone => "phone app",
        Device::Desk => "admin desk",
    };
    let details = json!({
        "pop_version": env!("CARGO_PKG_VERSION"),
        "screen": context,
        "printers": printers,
    });
    format!(
        "{}\n\n![The screen, as {} marked it]({{screenshot}})\n\nReported by {} from the {device}.\n\n<details><summary>Context</summary>\n\n```json\n{}\n```\n\n</details>\n\n<!-- pop-feedback {id} -->\n",
        r.description.trim(),
        user.name,
        user.name,
        serde_json::to_string_pretty(&details).unwrap_or_default(),
    )
}

// ---- scrubbing ------------------------------------------------------------------

/// Keys whose values are never sent, whatever they hold.
fn secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    [
        "authorization",
        "cookie",
        "token",
        "password",
        "secret",
        "session",
        "api_key",
        "apikey",
        "access_code",
        "credential",
    ]
    .iter()
    .any(|s| k.contains(s))
}

/// `v` with every secret-looking key's value, token, cookie and query string
/// taken out.
pub fn scrub(v: &Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| {
                    let v = if secret_key(k) {
                        Value::String("[redacted]".into())
                    } else {
                        scrub(v)
                    };
                    (k.clone(), v)
                })
                .collect::<Map<_, _>>(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(scrub).collect()),
        Value::String(s) => Value::String(scrub_text(s)),
        other => other.clone(),
    }
}

/// Whether a word is a token: a GitHub token, a JWT, a bearer credential, a
/// session cookie, or a long run of key-like characters.
fn tokenish(w: &str) -> bool {
    let w = w.trim_matches(|c: char| "\"'(),;[]{}<>".contains(c));
    let prefixes = [
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "pop_session=",
    ];
    if prefixes.iter().any(|p| w.starts_with(p)) {
        return true;
    }
    if w.starts_with("eyJ") && w.matches('.').count() == 2 {
        return true;
    }
    let keyish = w
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_+/=".contains(c));
    keyish && w.len() >= 32 && w.chars().any(|c| c.is_ascii_digit())
}

/// `s` with tokens, credentials after `Bearer`/`Basic`, and every query
/// string taken out.
pub fn scrub_text(s: &str) -> String {
    let mut out = Vec::new();
    let mut after_scheme = false;
    for word in s.split(' ') {
        let w = if after_scheme || tokenish(word) {
            "[redacted]".to_owned()
        } else {
            // A query string, wherever it starts: /x?token=..., https://...?a=b
            match word.find('?') {
                Some(i) if i > 0 && i + 1 < word.len() => format!("{}?[redacted]", &word[..i]),
                _ => word.to_owned(),
            }
        };
        after_scheme = ["bearer", "basic", "token"].contains(&word.to_ascii_lowercase().as_str());
        out.push(w);
    }
    out.join(" ")
}

// ---- the screenshot -------------------------------------------------------------

/// The PNG with only the chunks that draw it: no text, no time, no EXIF.
/// `None` for anything that is not a whole PNG.
pub fn without_metadata(png: &[u8]) -> Option<Vec<u8>> {
    const SIGNATURE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    const KEEP: [&[u8; 4]; 5] = [b"IHDR", b"PLTE", b"IDAT", b"IEND", b"tRNS"];
    let rest = png.strip_prefix(SIGNATURE)?;
    let mut out = SIGNATURE.to_vec();
    let mut i = 0;
    let mut first = true;
    loop {
        let len = u32::from_be_bytes(rest.get(i..i + 4)?.try_into().ok()?) as usize;
        let kind: &[u8; 4] = rest.get(i + 4..i + 8)?.try_into().ok()?;
        let end = i.checked_add(12)?.checked_add(len)?;
        let chunk = rest.get(i..end)?;
        if first && kind != b"IHDR" {
            return None;
        }
        first = false;
        if KEEP.contains(&kind) {
            out.extend_from_slice(chunk);
        }
        i = end;
        if kind == b"IEND" {
            return Some(out);
        }
    }
}

// ---- sending --------------------------------------------------------------------

/// How long to wait before trying a report again: `first`, doubling, at most
/// `max`.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub first: Duration,
    pub max: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(30),
            max: Duration::from_secs(30 * 60),
        }
    }
}

/// Send kept reports as they come and as they fall due.
pub fn spawn(db: SqlitePool, feedback: Arc<Feedback>, opts: Options) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let next = send_due(&db, &feedback, opts).await;
            let pause = next.unwrap_or(Duration::from_secs(60));
            tokio::select! {
                () = feedback.wake.notified() => {}
                () = tokio::time::sleep(pause) => {}
            }
        }
    })
}

/// Send every report that is due; how long until the next one is.
async fn send_due(db: &SqlitePool, f: &Feedback, opts: Options) -> Option<Duration> {
    let due = sqlx::query(
        "SELECT r.id, r.title, r.kind, r.body, r.screenshot, r.attempts, r.account_id, u.name
         FROM feedback_reports r JOIN users u ON u.id = r.account_id
         WHERE r.state = 'pending' AND r.next_try_at <= ? ORDER BY r.created_at",
    )
    .bind(now_ms())
    .fetch_all(db)
    .await;
    let due = match due {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("feedback: reading the reports kept: {e}");
            return Some(opts.first);
        }
    };
    for row in due {
        let id: String = row.get("id");
        let title: String = row.get("title");
        let kind: String = row.get("kind");
        let body: String = row.get("body");
        let png: Vec<u8> = row.get("screenshot");
        let attempts: i64 = row.get("attempts");
        let who = Actor::user(row.get("account_id"), &row.get::<String, _>("name"));
        match send(f, &id, &title, &kind, &body, &png).await {
            Ok(url) => {
                let mut tx = match db.begin().await {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::error!("feedback: {id} was filed as {url}, but: {e}");
                        continue;
                    }
                };
                let done = sqlx::query(
                    "UPDATE feedback_reports SET state = 'sent', issue_url = ?, last_error = NULL WHERE id = ?",
                )
                .bind(&url)
                .bind(&id)
                .execute(&mut *tx)
                .await;
                let recorded = audit::record(
                    &mut tx,
                    &who,
                    Event {
                        kind: AuditKind::FeedbackFiled,
                        printer: None,
                        subject: None,
                        summary: format!("filed {url}: {title}"),
                    },
                )
                .await;
                match (done, recorded) {
                    (Ok(_), Ok(())) => match tx.commit().await {
                        Ok(()) => tracing::info!("feedback: {id} filed as {url}"),
                        Err(e) => tracing::error!("feedback: {id} was filed as {url}, but: {e}"),
                    },
                    (Err(e), _) | (_, Err(e)) => {
                        tracing::error!("feedback: {id} was filed as {url}, but: {e}");
                    }
                }
            }
            Err(e) if e.is_transient() => {
                let wait = opts
                    .first
                    .saturating_mul(1 << attempts.clamp(0, 16))
                    .min(opts.max);
                tracing::warn!("feedback: {id} not sent yet, trying again in {wait:?}: {e}");
                let _ = sqlx::query(
                    "UPDATE feedback_reports SET attempts = attempts + 1, last_error = ?, next_try_at = ? WHERE id = ?",
                )
                .bind(e.to_string())
                .bind(now_ms().saturating_add(i64::try_from(wait.as_millis()).unwrap_or(i64::MAX)))
                .bind(&id)
                .execute(db)
                .await;
            }
            Err(e) => {
                tracing::error!("feedback: {id} cannot be sent: {e}");
                let _ = sqlx::query(
                    "UPDATE feedback_reports SET state = 'failed', last_error = ? WHERE id = ?",
                )
                .bind(e.to_string())
                .bind(&id)
                .execute(db)
                .await;
            }
        }
    }
    let next: Option<i64> =
        sqlx::query_scalar("SELECT MIN(next_try_at) FROM feedback_reports WHERE state = 'pending'")
            .fetch_one(db)
            .await
            .ok()
            .flatten();
    next.map(|t| Duration::from_millis(u64::try_from((t - now_ms()).max(1)).unwrap_or(1)))
}

async fn send(
    f: &Feedback,
    id: &str,
    title: &str,
    kind: &str,
    body: &str,
    png: &[u8],
) -> Result<String, pop_integrations::github::GitHubError> {
    let branch = &f.config.branch;
    f.github.ensure_branch(branch).await?;
    let picture = f
        .github
        .upload(
            branch,
            &format!("feedback/{id}.png"),
            png,
            &format!("Feedback {id}"),
        )
        .await?;
    let label = match kind {
        "bug" => "bug",
        "suggestion" => "enhancement",
        _ => "question",
    };
    let issue = f
        .github
        .create_issue(
            title,
            &body.replace("{screenshot}", &picture),
            &["feedback".to_owned(), label.to_owned()],
        )
        .await?;
    Ok(issue.html_url)
}
