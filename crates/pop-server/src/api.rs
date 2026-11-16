//! The types the API and the live-update stream carry.
//!
//! The web app's TypeScript types are generated from these (ADR 0001, "Shared
//! types: generated from Rust"), so a type is defined once, here. After changing
//! one, run `tools/gen-types.sh`; `tests/types.rs` fails until the generated
//! files match.

use std::collections::BTreeMap;

use serde::Serialize;
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

/// One message on the live-update stream (`GET /api/events`).
///
/// Serialized as `{"type": "...", "data": ...}`, so the web app can switch on
/// `type` without guessing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerEvent {
    /// The server's status. Sent first on every connection, and again when a
    /// screen has fallen behind and missed events, so it catches up from a
    /// snapshot instead of from a gap.
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

/// The body of every API error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct ApiError {
    pub error: String,
}
