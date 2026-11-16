//! The types the API and the live-update stream carry.
//!
//! The web app's TypeScript types are generated from these (ADR 0001, "Shared
//! types: generated from Rust"), so a type is defined once, here. After changing
//! one, run `tools/gen-types.sh`; `tests/types.rs` fails until the generated
//! files match.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What the server says about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct Status {
    /// The running build's version, from Cargo.
    pub version: String,
    /// The newest database migration applied.
    #[ts(type = "number")]
    pub schema_version: i64,
}

/// Everything a screen shows, at one moment. A screen that receives one
/// replaces its state with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct Snapshot {
    pub status: Status,
}

/// One message on the live-update stream (`GET /api/events`).
///
/// Serialized as `{"type": "...", "data": ...}`, so the web app can switch on
/// `type` without guessing. A `snapshot` replaces a screen's state; every
/// other event changes part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Everything, at once: sent to a screen that connects, to one that
    /// reconnects too late to catch up, and to one that falls behind.
    Snapshot(Snapshot),
    /// The server's status changed.
    Status(Status),
}

/// `GET /healthz`: whether each part POP depends on is working.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct Health {
    /// Every check passed.
    pub ok: bool,
    /// Each check by name: `database`, and each configured integration.
    pub checks: BTreeMap<String, CheckResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct CheckResult {
    pub ok: bool,
    /// What is wrong, in a sentence, when it is not ok.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
}

/// What an account may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Member,
    Admin,
}

/// An account, as the API shows it. Never its password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct User {
    #[ts(type = "number")]
    pub id: i64,
    pub username: String,
    pub name: String,
    pub role: Role,
}

/// `POST /api/session`.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct SignIn {
    pub username: String,
    pub password: String,
}

/// `POST /api/admin/users`.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct NewUser {
    pub username: String,
    pub name: String,
    pub role: Role,
    pub password: String,
}

/// `PUT /api/admin/users/{id}/password`.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct NewPassword {
    pub password: String,
}

/// `PUT /api/admin/users/{id}/role`.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct NewRole {
    pub role: Role,
}

/// What an audit entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AuditKind {
    SignedIn,
    SignInFailed,
    SignedOut,
    UserCreated,
    PasswordReset,
    RoleChanged,
    /// A printer's LAN access code was entered in POP (never the code).
    AccessCodeSet,
    /// A kind this build does not know, written by a newer POP. Shown, never
    /// hidden.
    Other,
}

/// One entry of the audit log (#8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct AuditEntry {
    #[ts(type = "number")]
    pub id: i64,
    /// Seconds since the Unix epoch.
    #[ts(type = "number")]
    pub at: i64,
    /// Who acted; absent for the command line or no one signed in.
    #[ts(type = "number | null")]
    pub actor_id: Option<i64>,
    pub actor_name: Option<String>,
    pub kind: AuditKind,
    pub printer: Option<String>,
    /// The account it concerned.
    #[ts(type = "number | null")]
    pub subject_id: Option<i64>,
    pub subject_name: Option<String>,
    /// One line a person reads.
    pub summary: String,
}

/// A printer from BAM!, as admins see it (#21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct AdminPrinter {
    pub bam_id: String,
    pub name: String,
    /// Every field right, every secret held: POP can run it.
    pub ready: bool,
    /// How POP reaches it, as BAM! says, when it says.
    pub connection: Option<String>,
    /// What is wrong, each naming the field. Empty when ready.
    pub problems: Vec<PrinterProblem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct PrinterProblem {
    /// The BAM! field, as BAM! labels it, with its key.
    pub field: String,
    pub problem: String,
}

/// `PUT /api/admin/printers/{bam_id}/access-code`.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct AccessCode {
    pub access_code: String,
}

/// The body of every API error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct ApiError {
    pub error: String,
}
