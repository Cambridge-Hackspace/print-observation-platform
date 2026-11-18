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
    /// What the group chat was told about each printer that needs a hand
    /// (#55).
    pub told: Vec<GroupNotice>,
}

/// A notice Discord has from POP about a printer that needs a hand (#55), so
/// its screen can show what the group chat saw. Only one Discord took: a
/// notice that was never posted is not shown as if it were. It goes when the
/// printer no longer needs a hand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct GroupNotice {
    /// The printer's BAM! id.
    pub printer: String,
    pub text: String,
    /// When Discord took it, in seconds since 1970.
    #[ts(type = "number")]
    pub at: i64,
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
    /// What the group chat has been told changed: every notice still standing,
    /// by printer (#55).
    Told(Vec<GroupNotice>),
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

/// An account as an admin manages it (#90): who it is, whether it can sign
/// in, and the training it holds here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct Account {
    #[ts(type = "number")]
    pub id: i64,
    pub username: String,
    pub name: String,
    pub role: Role,
    /// False once deactivated: signed out, and cannot sign in.
    pub active: bool,
    /// Training held, as a printer's BAM! training field names it, sorted.
    pub training: Vec<String>,
}

/// `PUT /api/admin/users/{id}/name`.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct NewName {
    pub name: String,
}

/// `PUT /api/admin/users/{id}/active`.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct NewActive {
    pub active: bool,
}

/// `PUT /api/admin/users/{id}/training`: every training it holds, replacing
/// what it held.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct NewTraining {
    pub training: Vec<String>,
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
    /// An admin set the group's Discord webhook (never the URL itself).
    WebhookSet,
    /// An admin removed the group's Discord webhook.
    WebhookCleared,
    /// An admin sent a test message to the group.
    WebhookTested,
    /// An admin wrote a new guide.
    GuideCreated,
    /// An admin edited, renamed, published or unpublished a guide, or added
    /// an image to it.
    GuideEdited,
    /// An admin linked a guide to an error the catalog knows, or unlinked it.
    ErrorGuideLinked,
    /// An admin changed an account's name.
    NameChanged,
    /// An admin deactivated an account, signing it out.
    AccountDeactivated,
    /// An admin reactivated an account.
    AccountReactivated,
    /// An admin changed the training an account holds.
    TrainingChanged,
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
    /// The BAM! categories it is in, by name (#86).
    pub categories: Vec<String>,
    /// The training it needs, as BAM! says, if any.
    pub training: Option<String>,
    /// Whether BAM! gives it a camera.
    pub camera: bool,
}

/// Whether a printer's camera works, as go2rtc last showed it (#66).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CameraHealth {
    /// go2rtc gave a picture from it.
    Live,
    /// go2rtc has the stream, but could not get a picture from the camera.
    NoPicture,
    /// go2rtc does not have the stream yet; POP adds it at its next look.
    NotInGo2rtc,
    /// Not looked at yet, or go2rtc could not be asked.
    Unknown,
}

/// A printer's camera, as admins see it (#66). Never its address, which may
/// hold the camera's password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct AdminCamera {
    pub bam_id: String,
    pub name: String,
    /// The stream's name in go2rtc.
    pub stream: String,
    pub health: CameraHealth,
    /// What went wrong, when go2rtc could not be asked.
    pub detail: Option<String>,
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

/// `GET /api/admin/notify`: where group notices go (#68).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct NotifySettings {
    /// The Discord webhook, without its token; none when unset.
    pub discord_webhook: Option<String>,
}

/// `PUT /api/admin/notify/discord`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct DiscordWebhook {
    /// As Discord's channel settings copy it.
    pub webhook_url: String,
}

/// A guide (#71): steps and yes/no flowcharts, as every viewer reads them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Guide {
    /// Stable, for links: `stringing-on-petg`.
    pub slug: String,
    pub title: String,
    pub status: GuideStatus,
    /// One sentence on what it fixes.
    pub summary: String,
    /// The id of the step it starts at.
    pub start: String,
    pub steps: Vec<GuideStep>,
}

/// Whether members and the public see a guide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum GuideStatus {
    Draft,
    Published,
}

/// One step: something to do, or a yes/no question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GuideStep {
    /// Unique in the guide: `dry-it`.
    pub id: String,
    /// What to do, or the question.
    pub title: String,
    /// More words, if any.
    #[serde(default)]
    pub body: String,
    /// An image of this guide's, by id.
    #[serde(default)]
    pub image: Option<String>,
    pub then: Then,
}

/// Where a step leads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Then {
    /// The end: this step says what to do.
    Done,
    /// On to another step.
    Next { step: String },
    /// A yes/no question: each answer leads to a step.
    Question { yes: String, no: String },
}

/// A guide, in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct GuideSummary {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub status: GuideStatus,
    #[ts(type = "number")]
    pub updated_at: i64,
}

/// One thing wrong with a guide, naming the step when it is one step's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct GuideProblem {
    pub step: Option<String>,
    pub problem: String,
}

/// `422` for a guide that cannot be kept: everything wrong with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct GuideProblems {
    pub error: String,
    pub problems: Vec<GuideProblem>,
}

/// `POST /api/admin/guides/{slug}/images`: an image, in base64, since every
/// request that changes something is JSON (#7).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NewGuideImage {
    /// image/png, image/jpeg or image/webp.
    pub content_type: String,
    pub data_base64: String,
}

/// An image kept for a guide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct GuideImage {
    pub id: String,
}

/// `GET /api/public/guides/{slug}`: a published guide, for anyone (#94).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct PublicGuide {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub start: String,
    pub steps: Vec<GuideStep>,
    /// The license it is offered under, when one is set.
    pub license: Option<String>,
}

/// `GET /api/public/guides`: the published guides, by title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct PublicGuideList {
    pub guides: Vec<PublicGuideSummary>,
    pub license: Option<String>,
}

/// A published guide, in the public list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct PublicGuideSummary {
    pub slug: String,
    pub title: String,
    pub summary: String,
}

/// An error the catalog knows, and the guide that fixes it (#73).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct ErrorGuide {
    /// The catalog entry's stable key.
    pub key: String,
    pub words: String,
    pub source: String,
    /// The guide's slug, when one is linked.
    pub guide: Option<String>,
}

/// `PUT /api/admin/error-guides/{key}`: link a guide, or none.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LinkGuide {
    pub guide: Option<String>,
}
