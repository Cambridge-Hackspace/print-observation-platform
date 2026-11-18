//! The audit log (#8): who did what, and when.
//!
//! Every change POP makes is recorded with [`record`], in the same
//! transaction as the change itself, so neither can happen without the
//! other. Admins read it through `GET /api/admin/audit`, filtered by person,
//! printer and kind.

use sqlx::{Sqlite, SqliteConnection, SqlitePool};

use crate::{
    api::{AuditEntry, AuditKind},
    users::now,
};

/// Who did something: an account, or (`None`) the command line or no one
/// signed in.
#[derive(Debug, Clone, Default)]
pub struct Actor {
    pub id: Option<i64>,
    pub name: Option<String>,
}

impl Actor {
    pub fn user(id: i64, name: &str) -> Self {
        Self {
            id: Some(id),
            name: Some(name.to_owned()),
        }
    }

    /// The command line, run by whoever has the machine.
    pub fn command_line() -> Self {
        Self {
            id: None,
            name: Some("the command line".to_owned()),
        }
    }

    /// The updater service (#148).
    pub fn updater() -> Self {
        Self {
            id: None,
            name: Some("the updater".to_owned()),
        }
    }
}

/// One thing that happened.
pub struct Event<'a> {
    pub kind: AuditKind,
    pub printer: Option<&'a str>,
    /// The account it concerned, by id and name.
    pub subject: Option<(i64, &'a str)>,
    pub summary: String,
}

pub fn kind_str(k: AuditKind) -> &'static str {
    match k {
        AuditKind::SignedIn => "signed_in",
        AuditKind::SignInFailed => "sign_in_failed",
        AuditKind::SignedOut => "signed_out",
        AuditKind::UserCreated => "user_created",
        AuditKind::PasswordReset => "password_reset",
        AuditKind::RoleChanged => "role_changed",
        AuditKind::AccessCodeSet => "access_code_set",
        AuditKind::CameraLoginSet => "camera_login_set",
        AuditKind::CameraLoginCleared => "camera_login_cleared",
        AuditKind::KioskEnrolled => "kiosk_enrolled",
        AuditKind::KioskRevoked => "kiosk_revoked",
        AuditKind::UpdateChecked => "update_checked",
        AuditKind::UpdateWaited => "update_waited",
        AuditKind::UpdateDownloaded => "update_downloaded",
        AuditKind::UpdateRefused => "update_refused",
        AuditKind::UpdateApplied => "update_applied",
        AuditKind::UpdateRolledBack => "update_rolled_back",
        AuditKind::UpdateFailed => "update_failed",
        AuditKind::PrinterAdded => "printer_added",
        AuditKind::PrinterDeclined => "printer_declined",
        AuditKind::PrinterRetired => "printer_retired",
        AuditKind::WebhookSet => "webhook_set",
        AuditKind::WebhookCleared => "webhook_cleared",
        AuditKind::WebhookTested => "webhook_tested",
        AuditKind::GuideCreated => "guide_created",
        AuditKind::GuideEdited => "guide_edited",
        AuditKind::ErrorGuideLinked => "error_guide_linked",
        AuditKind::NameChanged => "name_changed",
        AuditKind::AccountDeactivated => "account_deactivated",
        AuditKind::AccountReactivated => "account_reactivated",
        AuditKind::TrainingChanged => "training_changed",
        AuditKind::FeedbackFiled => "feedback_filed",
        AuditKind::ProblemReported => "problem_reported",
        AuditKind::Other => "other",
    }
}

/// A kind this build knows, or Other: an audit log never hides an entry it
/// cannot name, as one written by a newer POP would be.
fn kind_from(s: &str) -> AuditKind {
    [
        AuditKind::SignedIn,
        AuditKind::SignInFailed,
        AuditKind::SignedOut,
        AuditKind::UserCreated,
        AuditKind::PasswordReset,
        AuditKind::RoleChanged,
        AuditKind::AccessCodeSet,
        AuditKind::CameraLoginSet,
        AuditKind::CameraLoginCleared,
        AuditKind::KioskEnrolled,
        AuditKind::KioskRevoked,
        AuditKind::UpdateChecked,
        AuditKind::UpdateWaited,
        AuditKind::UpdateDownloaded,
        AuditKind::UpdateRefused,
        AuditKind::UpdateApplied,
        AuditKind::UpdateRolledBack,
        AuditKind::UpdateFailed,
        AuditKind::PrinterAdded,
        AuditKind::PrinterDeclined,
        AuditKind::PrinterRetired,
        AuditKind::WebhookSet,
        AuditKind::WebhookCleared,
        AuditKind::WebhookTested,
        AuditKind::GuideCreated,
        AuditKind::GuideEdited,
        AuditKind::ErrorGuideLinked,
        AuditKind::NameChanged,
        AuditKind::AccountDeactivated,
        AuditKind::AccountReactivated,
        AuditKind::TrainingChanged,
        AuditKind::FeedbackFiled,
        AuditKind::ProblemReported,
    ]
    .into_iter()
    .find(|k| kind_str(*k) == s)
    .unwrap_or(AuditKind::Other)
}

/// Record `event`, done by `by`, on `conn`: pass the transaction the change
/// itself is in.
pub async fn record(
    conn: &mut SqliteConnection,
    by: &Actor,
    event: Event<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit (at, actor_id, actor_name, kind, printer, subject_id, subject_name, summary)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(now())
    .bind(by.id)
    .bind(by.name.as_deref())
    .bind(kind_str(event.kind))
    .bind(event.printer)
    .bind(event.subject.map(|s| s.0))
    .bind(event.subject.map(|s| s.1))
    .bind(event.summary)
    .execute(conn)
    .await?;
    Ok(())
}

/// What to show of the log.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    /// Entries where this account acted or was acted on.
    pub person: Option<i64>,
    pub printer: Option<String>,
    pub kind: Option<AuditKind>,
    /// Entries older than this id, to page back.
    pub before: Option<i64>,
    pub limit: i64,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    at: i64,
    actor_id: Option<i64>,
    actor_name: Option<String>,
    kind: String,
    printer: Option<String>,
    subject_id: Option<i64>,
    subject_name: Option<String>,
    summary: String,
}

/// The newest entries that pass `f`, newest first.
pub async fn read(db: &SqlitePool, f: &Filter) -> Result<Vec<AuditEntry>, sqlx::Error> {
    let mut q = sqlx::QueryBuilder::<Sqlite>::new(
        "SELECT id, at, actor_id, actor_name, kind, printer, subject_id, subject_name, summary FROM audit WHERE 1",
    );
    if let Some(p) = f.person {
        q.push(" AND (actor_id = ").push_bind(p);
        q.push(" OR subject_id = ").push_bind(p).push(")");
    }
    if let Some(p) = &f.printer {
        q.push(" AND printer = ").push_bind(p.clone());
    }
    if let Some(k) = f.kind {
        q.push(" AND kind = ").push_bind(kind_str(k));
    }
    if let Some(b) = f.before {
        q.push(" AND id < ").push_bind(b);
    }
    q.push(" ORDER BY id DESC LIMIT ").push_bind(f.limit);
    let rows: Vec<Row> = q.build_query_as().fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| AuditEntry {
            id: r.id,
            at: r.at,
            actor_id: r.actor_id,
            actor_name: r.actor_name,
            kind: kind_from(&r.kind),
            printer: r.printer,
            subject_id: r.subject_id,
            subject_name: r.subject_name,
            summary: r.summary,
        })
        .collect())
}
