//! A client for BAM!'s REST API (#20), as of BAM! v0.20.0.
//!
//! POP reads printers from a BAM! category, never writes them, and never
//! keeps an inventory of its own (ADR 0001). It holds an API token whose role
//! has `assets.read` and `events.read`, ideally narrowed with a BAM! scope to
//! the printer category (docs/integrations/bam.md).
//!
//! What BAM! does that the client works with, all checked against its source
//! and recorded from a real one (tests/recordings):
//!
//! - `GET /api/v1/assets?category_id=` is not paged, and does not include
//!   subcategories; archived assets are left out unless `archived=true`,
//!   which gives only them.
//! - Field values are strings, keyed by the field's key.
//! - An asset the token may not read is a 404, and is left out of lists.

use std::{collections::HashMap, time::Duration};

use reqwest::{StatusCode, header};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

/// A category, flat: the tree is its `parent_id`s.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
}

/// A field's definition.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Field {
    pub id: String,
    pub key: String,
    pub label: String,
    /// text, number, date, url, select or person.
    pub validation_type: String,
    #[serde(default)]
    pub options: Option<Value>,
}

/// A field as a category has it, inherited or its own.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EffectiveField {
    pub field: Field,
    pub is_required: bool,
}

impl Field {
    /// The values a select field allows.
    pub fn choices(&self) -> Vec<String> {
        self.options
            .as_ref()
            .and_then(|o| o.get("choices"))
            .and_then(Value::as_array)
            .map(|cs| {
                cs.iter()
                    .filter_map(|c| c.get("value").and_then(Value::as_str).map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AssetCore {
    pub id: String,
    pub name: String,
    pub updated_at: String,
    /// When it was archived; absent when it is not.
    #[serde(default)]
    pub archived_at: Option<String>,
}

/// An asset, as much of it as POP reads.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Asset {
    pub asset: AssetCore,
    #[serde(default)]
    pub categories: Vec<Category>,
    /// Every value a string, by the field's key.
    #[serde(default)]
    pub field_values: HashMap<String, String>,
    #[serde(default)]
    pub effective_fields: Vec<EffectiveField>,
}

/// Something that happened in BAM!.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Event {
    pub id: String,
    /// asset.created, asset.edited, asset.archived, asset.restored, ...
    pub kind: String,
    #[serde(default)]
    pub category_id: Option<String>,
    #[serde(default)]
    pub asset_id: Option<String>,
    pub created_at: String,
}

/// Why a request to BAM! failed, in words that say what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BamError {
    /// The token is wrong, expired or revoked: an admin gives POP a new one.
    Token(String),
    /// The token's role lacks a permission POP needs.
    Permission(String),
    /// No such thing, or one the token may not see.
    NotFound,
    /// BAM! could not be reached at all.
    Unreachable(String),
    /// BAM! answered, but not with what POP expected.
    Unexpected(String),
}

impl std::fmt::Display for BamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Token(m) => write!(
                f,
                "BAM! refused POP's token ({m}); an admin needs to give POP a new one"
            ),
            Self::Permission(m) => write!(
                f,
                "BAM! says POP's token may not do this ({m}); its role needs assets.read and events.read"
            ),
            Self::NotFound => write!(f, "BAM! has no such thing, or POP's token may not see it"),
            Self::Unreachable(m) => write!(f, "BAM! cannot be reached: {m}"),
            Self::Unexpected(m) => write!(f, "BAM! answered unexpectedly: {m}"),
        }
    }
}

impl std::error::Error for BamError {}

/// BAM!'s error envelope: `{"error": "..."}`.
#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

#[derive(Deserialize)]
struct Assets {
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct One {
    asset: Asset,
}

#[derive(Deserialize)]
struct Categories {
    categories: Vec<Category>,
}

#[derive(Deserialize)]
struct Fields {
    fields: Vec<EffectiveField>,
}

#[derive(Deserialize)]
struct Events {
    events: Vec<Event>,
    more: bool,
}

/// A client for one BAM!.
#[derive(Clone)]
pub struct Bam {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Bam {
    /// `base` is BAM!'s address, `https://bam.example.org`.
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Result<Self, BamError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("POP/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| BamError::Unexpected(format!("cannot make an HTTP client: {e}")))?;
        Ok(Self {
            base: base.into().trim_end_matches('/').to_owned(),
            token: token.into(),
            http,
        })
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, BamError> {
        let res = self
            .http
            .get(format!("{}{path}", self.base))
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| BamError::Unreachable(format!("{}: {e}", self.base)))?;
        let status = res.status();
        let body = res
            .bytes()
            .await
            .map_err(|e| BamError::Unreachable(format!("the answer was cut off: {e}")))?;
        let said = || {
            serde_json::from_slice::<ErrorBody>(&body)
                .map(|e| e.error)
                .unwrap_or_else(|_| format!("HTTP {status}"))
        };
        match status {
            s if s.is_success() => serde_json::from_slice(&body).map_err(|e| {
                BamError::Unexpected(format!("{path}: not the JSON POP expects: {e}"))
            }),
            StatusCode::UNAUTHORIZED => Err(BamError::Token(said())),
            StatusCode::FORBIDDEN => Err(BamError::Permission(said())),
            StatusCode::NOT_FOUND => Err(BamError::NotFound),
            _ => Err(BamError::Unexpected(format!("{path}: {}", said()))),
        }
    }

    /// Every category, flat.
    pub async fn categories(&self) -> Result<Vec<Category>, BamError> {
        Ok(self
            .get::<Categories>("/api/v1/categories")
            .await?
            .categories)
    }

    /// The fields a category's assets have, its own and inherited.
    pub async fn fields(&self, category: &str) -> Result<Vec<EffectiveField>, BamError> {
        Ok(self
            .get::<Fields>(&format!("/api/v1/categories/{}/fields", enc(category)))
            .await?
            .fields)
    }

    /// The assets directly in `category` (not its subcategories): those in
    /// use, or (`archived`) only those archived.
    pub async fn assets(&self, category: &str, archived: bool) -> Result<Vec<Asset>, BamError> {
        let q = if archived { "archived=true&" } else { "" };
        Ok(self
            .get::<Assets>(&format!("/api/v1/assets?{q}category_id={}", enc(category)))
            .await?
            .assets)
    }

    pub async fn asset(&self, id: &str) -> Result<Asset, BamError> {
        Ok(self
            .get::<One>(&format!("/api/v1/assets/{}", enc(id)))
            .await?
            .asset)
    }

    /// Events of `kinds` in `category` and its subcategories, oldest first,
    /// after `after` (or the last hour), and whether there are more.
    pub async fn events(
        &self,
        after: Option<&str>,
        kinds: &[&str],
        category: &str,
    ) -> Result<(Vec<Event>, bool), BamError> {
        let mut path = format!(
            "/api/v1/events?kinds={}&categories={}&limit=500",
            kinds.join(","),
            enc(category)
        );
        if let Some(a) = after {
            path.push_str(&format!("&after={}", enc(a)));
        }
        let e: Events = self.get(&path).await?;
        Ok((e.events, e.more))
    }

    /// Whether BAM! answers, takes the token, and has the printer category.
    pub async fn check(&self, category: &str) -> Result<(), BamError> {
        let all = self.categories().await?;
        if all.iter().any(|c| c.id == category) {
            Ok(())
        } else {
            Err(BamError::Unexpected(format!(
                "there is no category {category}, or POP's token may not see it"
            )))
        }
    }
}

/// An id or name, made safe for a URL path or query.
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
