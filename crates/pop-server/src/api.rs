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
    /// The address phones reach POP at, when it is set (#49).
    pub public_url: Option<String>,
}

/// Everything a screen shows, at one moment. A screen that receives one
/// replaces its state with it.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
pub struct Snapshot {
    pub status: Status,
    /// The tower's printers, by name (#49).
    pub printers: Vec<TowerPrinter>,
}

/// A printer the tower runs, as every screen shows it (#49).
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
pub struct TowerPrinter {
    /// Its BAM! id.
    pub id: String,
    pub name: String,
    /// As its driver last reported; offline when POP has no driver for it.
    pub state: pop_printers::PrinterState,
    /// Whether POP has a driver for how this printer connects. When it has
    /// not (OctoPrint, PrusaLink and USB, so far), the printer is listed but
    /// always reads offline.
    pub watched: bool,
    /// What it prints with, from BAM!.
    pub materials: Vec<String>,
    /// The training it needs, from BAM!, if any.
    pub training: Option<String>,
}

/// One message on the live-update stream (`GET /api/events`).
///
/// Serialized as `{"type": "...", "data": ...}`, so the web app can switch on
/// `type` without guessing. A `snapshot` replaces a screen's state; every
/// other event changes part of it.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Everything, at once: sent to a screen that connects, to one that
    /// reconnects too late to catch up, and to one that falls behind.
    Snapshot(Snapshot),
    /// The server's status changed.
    Status(Status),
    /// A printer's state changed, or the tower gained or lost one: every
    /// printer, by name (#49).
    Printers(Vec<TowerPrinter>),
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
    /// A printer from BAM! was added to the tower.
    PrinterAdded,
    /// An admin said a printer in BAM! is not one for the tower.
    PrinterDeclined,
    /// A printer archived in BAM! was retired from the tower.
    PrinterRetired,
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
    /// Whether the tower runs it, or it waits for an admin to say (#23).
    pub tower: TowerState,
}

/// Where a printer from BAM! stands with the tower (#23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TowerState {
    /// New in BAM!, or back from retirement: waiting for an admin to say.
    Offered,
    Added,
    /// An admin said it is not one for the tower.
    Declined,
    /// Archived in BAM!: out of the tower, its history kept.
    Retired,
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
