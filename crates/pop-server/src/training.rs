//! Which printers a member may use, by their training (#17).
//!
//! The source is `[training] source`:
//!
//! - **local:** admins tick each account's training on POP's desk, and a
//!   printer needs the level its BAM! training field names.
//! - **css:** CS:S says. A printer needs the CS:S tool its BAM! `css_tool`
//!   field names, by id or external id, and POP asks CS:S at every check,
//!   with POP's own token (`training:members`), about the member its account
//!   is linked to. Nothing is kept, so a member who loses training is refused
//!   at their next check. A member CS:S cannot be asked about, or a CS:S that
//!   does not answer, is refused, in words: training is never assumed.
//!
//! A printer that names no training needs none, from either source. An admin
//! needs training as a member does: being trusted with POP is not being
//! trained on a printer.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use pop_integrations::{bam::BamError, printers};
use sqlx::SqlitePool;

use crate::{
    AppState,
    api::{ApiError, PrinterTraining, User},
    config::{Training, TrainingSource},
    sign_in::{SignIns, ToolsFailed},
};

/// What a printer needs, from its BAM! fields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Needs {
    /// Its training level, for local training.
    pub training: Option<String>,
    /// Its CS:S tool, by id or external id, for training from CS:S.
    pub css_tool: Option<String>,
}

/// May they, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    May,
    MayNot(String),
}

/// The training levels an account holds, ticked on the desk.
async fn held(db: &SqlitePool, user: i64) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT training FROM user_training WHERE user_id = ?")
        .bind(user)
        .fetch_all(db)
        .await
}

/// The subject provider `provider` knows `user` by, if the account is linked.
async fn subject(
    db: &SqlitePool,
    provider: &str,
    user: i64,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT subject FROM user_identities WHERE provider = ? AND user_id = ?")
        .bind(provider)
        .bind(user)
        .fetch_optional(db)
        .await
}

/// Whether `user` may use a printer that needs `needs`, by `training`.
pub async fn may_use(
    db: &SqlitePool,
    training: &Training,
    sign_ins: Option<&SignIns>,
    user: &User,
    needs: &Needs,
) -> Verdict {
    match training.source {
        TrainingSource::Local => {
            let Some(level) = &needs.training else {
                return Verdict::May;
            };
            match held(db, user.id).await {
                // As the desk keeps them: the same level whatever its case.
                Ok(h) if h.iter().any(|t| t.eq_ignore_ascii_case(level)) => Verdict::May,
                Ok(_) => Verdict::MayNot(format!(
                    "This printer needs {level} training. Ask an admin once you have it."
                )),
                Err(e) => {
                    tracing::warn!("reading training: {e}");
                    Verdict::MayNot("POP couldn't check your training just now.".into())
                }
            }
        }
        TrainingSource::Css => {
            let Some(tool) = &needs.css_tool else {
                return Verdict::May;
            };
            let provider = training.provider.as_str();
            let Some(sign_ins) = sign_ins else {
                tracing::warn!("training from CS:S, with no sign-in provider configured");
                return Verdict::MayNot("POP couldn't check your training just now.".into());
            };
            let label = sign_ins
                .provider_of(provider)
                .map(|p| p.label.clone())
                .unwrap_or_else(|| "CS:S".into());
            let sub = match subject(db, provider, user.id).await {
                Ok(Some(s)) => s,
                Ok(None) => {
                    return Verdict::MayNot(format!(
                        "Your training is kept in {label}. Sign in with {label} once, so POP knows which member you are."
                    ));
                }
                Err(e) => {
                    tracing::warn!("reading a linked account: {e}");
                    return Verdict::MayNot("POP couldn't check your training just now.".into());
                }
            };
            match sign_ins.member_tools(provider, &sub).await {
                Ok(tools) => {
                    let found = tools
                        .iter()
                        .find(|t| t.id == *tool || t.external_id.as_deref() == Some(tool.as_str()));
                    match found {
                        Some(t) if t.authorized => Verdict::May,
                        Some(t) => Verdict::MayNot(format!(
                            "{label} says you aren't trained on {} yet.",
                            t.name
                        )),
                        None => {
                            tracing::warn!("{label} has no tool {tool:?}, named in BAM!");
                            Verdict::MayNot(format!(
                                "This printer's training isn't set up in {label} yet. Tell an admin."
                            ))
                        }
                    }
                }
                Err(ToolsFailed::NoMember) => {
                    Verdict::MayNot(format!("{label} has no active member for your account."))
                }
                Err(ToolsFailed::Broken(why)) => {
                    tracing::warn!("asking {label} about training: {why}");
                    Verdict::MayNot(format!(
                        "POP couldn't check your training with {label} just now."
                    ))
                }
            }
        }
    }
}

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ApiError { error: msg.into() })).into_response()
}

/// `GET /api/printers/{bam_id}/training`: whether the signed-in member may
/// use a printer, asked now.
pub async fn check(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(bam_id): Path<String>,
) -> Response {
    let Some(bam) = state.bam.as_ref() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "POP has no BAM! to read printers from",
        );
    };
    let asset = match bam.client.asset(&bam_id).await {
        Ok(a) => a,
        Err(BamError::NotFound) => {
            return error(StatusCode::NOT_FOUND, "There's no such printer.");
        }
        Err(e) => return error(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    // A printer is an unarchived asset in the printer category, as the
    // tower's list is.
    let in_category = asset.categories.iter().any(|c| c.id == bam.category);
    if !in_category || asset.asset.archived_at.is_some() {
        return error(StatusCode::NOT_FOUND, "There's no such printer.");
    }
    let (training, css_tool) = printers::needs(&asset, &bam.fields);
    let verdict = may_use(
        &state.db,
        &state.training,
        state.sign_ins.as_ref(),
        &user,
        &Needs { training, css_tool },
    )
    .await;
    Json(match verdict {
        Verdict::May => PrinterTraining {
            may_use: true,
            reason: None,
        },
        Verdict::MayNot(reason) => PrinterTraining {
            may_use: false,
            reason: Some(reason),
        },
    })
    .into_response()
}
