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
    /// The same notice with no one named in it: what the public stream says
    /// when owners' names are not to go out on it (#134). Never sent itself.
    #[serde(skip)]
    #[ts(skip)]
    pub without_owner: String,
}

/// `GET /api/printers/{bam_id}/training`: whether the signed-in member may
/// use a printer, by its training (#17), and if not, why, in words for them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct PrinterTraining {
    pub may_use: bool,
    pub reason: Option<String>,
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
    /// not (USB, so far), the printer is listed but
    /// always reads offline.
    pub watched: bool,
    /// What it prints with, from BAM!.
    pub materials: Vec<String>,
    /// The training it needs, from BAM!, if any.
    pub training: Option<String>,
    /// Whether it has a camera a member can watch (#61): one in BAM!, and a
    /// go2rtc to serve it.
    pub camera: bool,
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

/// `POST /api/printers/{bam_id}/report`: something looks wrong (#63).
#[derive(Debug, Clone, Deserialize, TS)]
pub struct NewReport {
    /// What looks wrong, in a few words; may be empty.
    #[serde(default)]
    pub note: String,
}

/// What became of a report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct ReportTaken {
    /// The group chat is told now; false when someone flagged the same
    /// printer a few minutes ago, and it was told then.
    pub told: bool,
}

/// What a piece of feedback is (#96).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackKind {
    Bug,
    Suggestion,
    Question,
}

/// Where feedback was filed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Device {
    Kiosk,
    Phone,
    Desk,
}

/// `POST /api/feedback`: a report, its annotated screenshot, and what the
/// screen knew. Everything in it goes into a public issue.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct NewFeedback {
    pub kind: FeedbackKind,
    pub title: String,
    pub description: String,
    pub device: Device,
    /// The screenshot, redacted and annotated, as a PNG in base64.
    pub screenshot: String,
    /// What the screen knew: its route, viewport, recent console errors and
    /// failed requests. Scrubbed again on the server.
    #[ts(type = "Record<string, unknown>")]
    pub context: serde_json::Value,
}

/// Where a report stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackState {
    /// Kept, and being sent; tried again while GitHub cannot be reached.
    Pending,
    /// Filed: the issue exists.
    Sent,
    /// GitHub refused it, and trying again will not help.
    Failed,
}

/// `GET /api/feedback/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct FeedbackStatus {
    pub id: String,
    pub state: FeedbackState,
    /// The issue, once filed.
    pub issue_url: Option<String>,
}

/// `GET /api/feedback`: whether this person can file feedback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct FeedbackSettings {
    /// Feedback is configured.
    pub enabled: bool,
    /// And this person may file it.
    pub allowed: bool,
    /// How many reports a person may file in an hour.
    pub per_hour: u32,
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
    /// A camera's login was entered in POP (never the password) (#150).
    CameraLoginSet,
    /// A camera's login was removed from POP.
    CameraLoginCleared,
    /// A printer's own login was entered in POP (never the secret) (#29, #30).
    PrinterLoginSet,
    /// A printer's own login was removed from POP.
    PrinterLoginCleared,
    /// Someone made a personal API key (never the key) (#13).
    ApiKeyCreated,
    /// Someone revoked one of their API keys.
    ApiKeyRevoked,
    /// An admin enrolled a kiosk (#12).
    KioskEnrolled,
    /// An admin revoked a kiosk.
    KioskRevoked,
    /// An admin set or cleared a kiosk's card reader (#19).
    KioskReaderSet,
    /// The updater looked for a release (#148).
    UpdateChecked,
    /// The updater found one, and waited: a print, or outside its hours.
    UpdateWaited,
    /// The updater downloaded a release and its signatures checked.
    UpdateDownloaded,
    /// The updater refused a release: its signatures or sums did not check.
    UpdateRefused,
    /// The updater put a new release in, and it passed its health check.
    UpdateApplied,
    /// A new release failed its health check, and the updater put the one
    /// before it back.
    UpdateRolledBack,
    /// The updater could not do what it set out to.
    UpdateFailed,
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
    /// Feedback was filed on GitHub as an issue (#96); the summary has its URL.
    FeedbackFiled,
    /// Someone watching flagged that something looks wrong with a printer
    /// (#63), and the group chat was told.
    ProblemReported,
    /// A member's print was stored, to wait until they send it (#40).
    PrintStored,
    /// A waiting print was deleted, by its owner or an admin.
    PrintDeleted,
    /// A print waited unsent longer than prints.keep_days: its file was
    /// deleted and its owner told.
    PrintExpired,
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
    /// Whether POP holds its LAN access code (a Bambu printer's).
    pub access_code: bool,
    /// Whether its camera is an address a login can go with (#150): set
    /// in BAM!, and not a Bambu printer's own.
    pub camera_login_fits: bool,
    /// Whether POP holds a login for its camera (#150).
    pub camera_login: bool,
    /// Whether it is reached through a host that may ask for a login of its
    /// own: OctoPrint (an API key) or PrusaLink (a user and password, or a
    /// key) (#29, #30).
    pub printer_login_fits: bool,
    /// Whether POP holds that login, for the address BAM! gives now.
    pub printer_login: bool,
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
    /// The stream's name in go2rtc; none for a camera POP reads itself (#33).
    pub stream: Option<String>,
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
    /// Fixed in POP, not BAM!: a secret POP keeps, entered on the desk.
    pub in_pop: bool,
}

/// `PUT /api/admin/printers/{bam_id}/access-code`.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct AccessCode {
    pub access_code: String,
}

/// `PUT /api/admin/printers/{bam_id}/camera-login` (#150). No Debug: it
/// carries a password.
#[derive(Clone, Deserialize, TS)]
pub struct CameraLoginBody {
    /// Empty for a camera that asks for a password only.
    pub username: String,
    pub password: String,
}

/// A print waiting to be sent (#40), or one that expired unsent, as its
/// owner sees it and as admins do.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
pub struct Print {
    pub id: String,
    pub owner: PrintOwner,
    /// As it was uploaded.
    pub filename: String,
    pub format: PrintFormat,
    #[ts(type = "number")]
    pub bytes: u64,
    #[ts(type = "number")]
    pub created_at: i64,
    /// When it is deleted if it has not been sent.
    #[ts(type = "number")]
    pub expires_at: i64,
    /// When it expired unsent and its file was deleted. It is listed, to
    /// tell its owner, until they dismiss it.
    #[ts(type = "number | null")]
    pub expired_at: Option<i64>,
    pub plates: Vec<PrintPlate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PrintOwner {
    #[ts(type = "number")]
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum PrintFormat {
    #[serde(rename = "gcode")]
    Gcode,
    #[serde(rename = "gcode.3mf")]
    Gcode3mf,
}

/// One plate of a print: plain G-code is one plate. What its slicer wrote,
/// each value missing where the file does not say (pop_gcode).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct PrintPlate {
    pub index: u32,
    /// The slicer and its version: "PrusaSlicer 2.9.4".
    pub slicer: Option<String>,
    pub printer_model: Option<String>,
    pub nozzles_mm: Vec<f64>,
    #[ts(type = "number | null")]
    pub estimated_seconds: Option<u64>,
    pub filaments: Vec<PrintFilament>,
    pub total_grams: Option<f64>,
    /// Whether the slicer drew a picture of the plate: GET
    /// /api/prints/{id}/plates/{index}/thumbnail.
    pub has_thumbnail: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct PrintFilament {
    /// The extruder or slot, from 0.
    pub slot: u32,
    pub material: Option<String>,
    /// `#RRGGBB` or `#RRGGBBAA`.
    pub color: Option<String>,
    pub grams: Option<f64>,
}

/// One of a person's API keys, as they see it (#13): never the key itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct ApiKey {
    #[ts(type = "number")]
    pub id: i64,
    /// What it is for, as its owner named it.
    pub name: String,
    /// Its first characters, to tell it apart.
    pub prefix: String,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number | null")]
    pub last_used_at: Option<i64>,
    /// When it was revoked; it is refused from then on.
    #[ts(type = "number | null")]
    pub revoked_at: Option<i64>,
}

/// `POST /api/keys`.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct NewApiKey {
    pub name: String,
}

/// `POST /api/keys`'s answer: the only time the key itself is shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct CreatedApiKey {
    pub key: ApiKey,
    pub secret: String,
}

/// `PUT /api/admin/printers/{bam_id}/printer-login` (#29, #30): an API key,
/// or a user and password for PrusaLink on the printer itself. One or the
/// other, never both.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
pub struct PrinterLoginBody {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

/// `GET /api/updater/quiet` (#148): for the updater, from this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct UpdaterQuiet {
    /// The running version, `0.1.0`.
    pub version: String,
    /// No printer is busy with a print: an upgrade may go in.
    pub quiet: bool,
    /// The printers busy with a print, by name.
    pub busy: Vec<String>,
}

/// `GET /api/admin/updater` (#148).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct UpdaterStatus {
    /// The running version, `0.1.0`.
    pub running: String,
    /// The last thing the updater did, if it has done anything.
    pub last: Option<UpdaterEvent>,
}

/// One thing the updater did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct UpdaterEvent {
    /// Unix seconds.
    #[ts(type = "number")]
    pub at: i64,
    /// `checked`, `waiting`, `downloaded`, `refused`, `applied`,
    /// `rolled_back` or `failed`.
    pub kind: String,
    /// The release it concerned, `v0.2.0`.
    pub version: Option<String>,
    pub summary: String,
}

/// `GET /api/session` (#12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct SessionInfo {
    pub user: User,
    /// At a kiosk: how long the session lasts with the screen left alone, in
    /// seconds. None elsewhere.
    pub idle_secs: Option<u32>,
}

/// `POST /api/admin/kiosks` (#12): make this browser a kiosk.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct NewKiosk {
    /// What to call it on the desk: "The tower".
    pub name: String,
}

/// An enrolled kiosk, as the desk lists it (#12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct Kiosk {
    #[ts(type = "number")]
    pub id: i64,
    pub name: String,
    /// Unix seconds.
    #[ts(type = "number")]
    pub enrolled_at: i64,
    pub revoked: bool,
    /// Its last check-in, in Unix seconds, and the address it came from.
    #[ts(type = "number | null")]
    pub last_seen_at: Option<i64>,
    pub last_address: Option<String>,
    /// The CS:S resource its card reader is (#19), if it has one.
    pub card_resource: Option<String>,
}

/// `PUT /api/admin/kiosks/{id}/card-reader` (#19): the CS:S resource a
/// kiosk's card reader is, as the reader names it; empty for none.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
pub struct KioskCardReader {
    pub resource: String,
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
