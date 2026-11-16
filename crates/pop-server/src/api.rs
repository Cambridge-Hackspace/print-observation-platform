//! The types the API and the live-update stream carry.
//!
//! The web app's TypeScript types are generated from these (ADR 0001, "Shared
//! types: generated from Rust"), so a type is defined once, here. After changing
//! one, run `tools/gen-types.sh`; `tests/types.rs` fails until the generated
//! files match.

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

/// The body of every API error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct ApiError {
    pub error: String,
}
