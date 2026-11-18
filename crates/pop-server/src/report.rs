//! "Something looks wrong" (#63): a member watching a print can flag a
//! problem, and POP tells the group chat, so someone at the space can check.
//! It never touches the printer: the route only reads which printers the
//! tower has, and posts to Discord (#68).
//!
//! For members signed in, not anyone: the group chat is the space's, and an
//! open form posting to it would be a way to spam it. One member may flag
//! [`PER_MEMBER_PER_HOUR`] times an hour; a printer flagged in the last
//! [`QUIET`] is not posted about again, and whoever flags it is told the
//! group already knows. Each report is in the audit log, which is also what
//! these limits count.
//!
//! What a member writes is posted as plain text: Discord's markdown in it is
//! escaped, so it cannot make a link or a heading, and the notifier lets it
//! ping nobody.

use std::time::Duration;

use axum::{
    Extension, Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::{
    AppState,
    api::{ApiError, AuditKind, NewReport, ReportTaken, User},
    audit::{self, Actor, Event},
};

/// How often one member may flag a problem.
pub const PER_MEMBER_PER_HOUR: i64 = 5;
/// How long a printer is not posted about again once flagged.
pub const QUIET: Duration = Duration::from_secs(10 * 60);
/// The longest note.
pub const MAX_NOTE: usize = 200;

fn refuse(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ApiError { error: msg.into() })).into_response()
}

/// `text` as Discord shows plain text: its markdown escaped, on one line.
pub fn plain(text: &str) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(one_line.len());
    for c in one_line.chars() {
        if "\\*_~`|>[]()#-:<".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The group's message.
pub fn message(who: &str, printer: &str, note: &str) -> String {
    let who = crate::notices::first_name(who).unwrap_or("Someone");
    let note = plain(note.trim());
    if note.is_empty() {
        format!(
            "{who}, watching, thinks something looks wrong with {printer}. Could someone at the space check it?"
        )
    } else {
        format!(
            "{who}, watching, thinks something looks wrong with {printer}: “{note}”. Could someone at the space check it?"
        )
    }
}

/// `POST /api/printers/{bam_id}/report`.
pub async fn report(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(bam_id): Path<String>,
    body: Result<Json<NewReport>, JsonRejection>,
) -> Response {
    let Json(r) = match body {
        Ok(b) => b,
        Err(e) => return refuse(StatusCode::BAD_REQUEST, e.body_text()),
    };
    if r.note.chars().count() > MAX_NOTE {
        return refuse(
            StatusCode::BAD_REQUEST,
            format!("note: at most {MAX_NOTE} characters"),
        );
    }
    let Some(printer) = state
        .tower
        .borrow()
        .iter()
        .find(|p| p.id == bam_id)
        .map(|p| p.name.clone())
    else {
        return refuse(StatusCode::NOT_FOUND, "that printer isn’t on the tower");
    };
    let Some(notifier) = state.notifier.clone() else {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "POP can’t tell the group chat: it has no notifier",
        );
    };
    match state
        .secrets
        .setting(&state.db, crate::notify::WEBHOOK)
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return refuse(
                StatusCode::CONFLICT,
                "the group chat isn’t set up yet: an admin adds its Discord webhook",
            );
        }
        Err(e) => return refuse(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
    let now = crate::users::now();
    let kind = audit::kind_str(AuditKind::ProblemReported);
    let mine: i64 = match sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit WHERE kind = ? AND actor_id = ? AND at > ?",
    )
    .bind(kind)
    .bind(user.id)
    .bind(now - 3600)
    .fetch_one(&state.db)
    .await
    {
        Ok(n) => n,
        Err(e) => return refuse(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    if mine >= PER_MEMBER_PER_HOUR {
        return refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "you’ve flagged a lot this hour; if it’s urgent, ask someone at the space",
        );
    }
    let quiet = i64::try_from(QUIET.as_secs()).unwrap_or(i64::MAX);
    let recent: i64 = match sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit WHERE kind = ? AND printer = ? AND at > ?",
    )
    .bind(kind)
    .bind(&bam_id)
    .bind(now - quiet)
    .fetch_one(&state.db)
    .await
    {
        Ok(n) => n,
        Err(e) => return refuse(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    let told = recent == 0;
    let mut conn = match state.db.acquire().await {
        Ok(c) => c,
        Err(e) => return refuse(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    let summary = if told {
        format!("flagged {printer}: {}", r.note.trim())
    } else {
        format!(
            "flagged {printer} again, within {} min: {}",
            QUIET.as_secs() / 60,
            r.note.trim()
        )
    };
    if let Err(e) = audit::record(
        &mut conn,
        &Actor::user(user.id, &user.name),
        Event {
            kind: AuditKind::ProblemReported,
            printer: Some(&bam_id),
            subject: None,
            summary,
        },
    )
    .await
    {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, e.to_string());
    }
    if told {
        notifier.post(message(&user.name, &printer, &r.note));
    }
    (StatusCode::ACCEPTED, Json(ReportTaken { told })).into_response()
}
