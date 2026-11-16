//! Group notices, straight to a Discord webhook (#68), as BAM! does it: no
//! CS:S involved. Personal messages to one member go through CS:S (#70).
//!
//! The webhook URL is a secret: it is sealed (`Secrets::set_setting`) and
//! never shown in full once saved, only its host, its id and the end of its
//! token. Each notice is one plain sentence. Nothing in it can ping anyone:
//! every message says `allowed_mentions: {parse: []}`, so a printer or a
//! person named "@everyone" stays text.
//!
//! Notices are delivered one at a time, in order. A failure is retried with a
//! doubling wait, and Discord's rate-limit answers (429, with `retry_after`)
//! are waited out as Discord asks. Every failure is logged, and so is giving
//! up; a notice is never dropped silently. A refusal that waiting cannot fix
//! (the webhook was deleted: 401, 403, 404) is logged and not retried.

use std::time::Duration;

use sqlx::SqlitePool;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::secrets::{SecretError, Secrets};

/// The sealed setting the webhook URL is kept under.
pub const WEBHOOK: &str = "discord_webhook";

/// The hosts a Discord webhook lives on.
pub const DISCORD_HOSTS: &[&str] = &[
    "discord.com",
    "discordapp.com",
    "ptb.discord.com",
    "canary.discord.com",
];

/// The parts of a webhook URL: host, id, token.
fn parts(url: &str) -> Option<(&str, &str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    let path = path.strip_prefix("api/")?;
    // Discord also serves versioned paths: api/v10/webhooks/...
    let path = match path.split_once('/') {
        Some((v, p)) if v.starts_with('v') && v[1..].chars().all(|c| c.is_ascii_digit()) => p,
        _ => path,
    };
    let path = path.strip_prefix("webhooks/")?;
    let (id, token) = path.split_once('/')?;
    let token = token.split(['?', '#']).next()?.trim_end_matches('/');
    Some((host, id, token))
}

/// Whether `url` is a Discord webhook: https, on Discord's own host, with an
/// id and a token. Says what is wrong when it is not.
pub fn check_webhook(url: &str) -> Result<(), String> {
    let wrong = || {
        "expected a Discord webhook URL, as Discord's channel settings copy it: \
         https://discord.com/api/webhooks/<id>/<token>"
            .to_owned()
    };
    let (host, id, token) = parts(url.trim()).ok_or_else(wrong)?;
    if !DISCORD_HOSTS.contains(&host) {
        return Err(format!("{host} is not Discord: {}", wrong()));
    }
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err(wrong());
    }
    if token.len() < 8
        || !token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(wrong());
    }
    Ok(())
}

/// The webhook as it may be shown once saved: never its whole token.
pub fn masked(url: &str) -> String {
    match parts(url) {
        Some((host, id, token)) => {
            let tail: String = token
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("https://{host}/api/webhooks/{id}/…{tail}")
        }
        // Only ever saved after check_webhook, so this is not reached; if it
        // were, show nothing of it rather than all of it.
        None => "a webhook POP cannot read".to_owned(),
    }
}

/// How hard to try.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub first_retry: Duration,
    pub max_retry: Duration,
    /// Tries per notice, the first included, before giving up on it.
    pub attempts: u32,
    /// How long one try may take.
    pub timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            first_retry: Duration::from_secs(2),
            max_retry: Duration::from_secs(300),
            attempts: 10,
            timeout: Duration::from_secs(15),
        }
    }
}

/// How one try went.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Sent,
    /// Rate limited: try again after this long, as Discord says.
    RateLimited(Duration),
    /// Worth trying again: the network, or Discord's own trouble.
    Failed(String),
    /// Not worth trying again: the webhook is gone, or the message refused.
    Refused(String),
}

/// One try at posting `text` to `url`.
pub async fn attempt(http: &reqwest::Client, url: &str, text: &str, timeout: Duration) -> Outcome {
    let body = serde_json::json!({
        "content": text,
        "allowed_mentions": { "parse": [] },
    });
    let res = match http.post(url).timeout(timeout).json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            return Outcome::Failed(format!("Discord could not be reached: {}", without_url(&e)));
        }
    };
    let status = res.status();
    if status.is_success() {
        return Outcome::Sent;
    }
    if status.as_u16() == 429 {
        let header = res
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<f64>().ok());
        let body: Option<serde_json::Value> = res.json().await.ok();
        let secs = body
            .as_ref()
            .and_then(|b| b["retry_after"].as_f64())
            .or(header)
            .unwrap_or(1.0)
            .clamp(0.0, 3600.0);
        return Outcome::RateLimited(Duration::from_secs_f64(secs));
    }
    let said = res.text().await.unwrap_or_default();
    let said: String = said.chars().take(200).collect();
    if status.is_server_error() {
        Outcome::Failed(format!("Discord answered {status}: {said}"))
    } else {
        Outcome::Refused(format!("Discord refused the message ({status}): {said}"))
    }
}

/// A reqwest error, without the URL it carries: the URL holds the token.
fn without_url(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    if let Some(u) = e.url() {
        s = s.replace(u.as_str(), "the webhook");
    }
    s
}

/// Posts group notices, in order, in the background.
#[derive(Clone)]
pub struct Notifier {
    tx: mpsc::UnboundedSender<String>,
}

impl Notifier {
    /// Post `text` to the group, when it can be.
    pub fn post(&self, text: impl Into<String>) {
        let text = text.into();
        if self.tx.send(text.clone()).is_err() {
            tracing::error!("group notice not posted: the notifier has stopped: {text}");
        }
    }
}

/// Start posting notices, with the webhook as it is saved at each one.
pub fn spawn(
    db: SqlitePool,
    secrets: Secrets,
    http: reqwest::Client,
    opts: Options,
) -> (Notifier, JoinHandle<()>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let task = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            deliver(&db, &secrets, &http, opts, &text).await;
        }
    });
    (Notifier { tx }, task)
}

/// Deliver one notice, trying again as it should.
async fn deliver(
    db: &SqlitePool,
    secrets: &Secrets,
    http: &reqwest::Client,
    opts: Options,
    text: &str,
) {
    let mut wait = opts.first_retry;
    for n in 1..=opts.attempts {
        // Read each time, so a webhook put right while a notice waits is used.
        let url = match secrets.setting(db, WEBHOOK).await {
            Ok(Some(u)) => u,
            Ok(None) => {
                tracing::warn!("group notice not posted: no Discord webhook is set: {text}");
                return;
            }
            Err(e @ (SecretError::NoKey | SecretError::WillNotOpen)) => {
                tracing::error!("group notice not posted: the Discord webhook: {e}: {text}");
                return;
            }
            Err(e) => {
                tracing::warn!("group notice: reading the Discord webhook (try {n}): {e}");
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(opts.max_retry);
                continue;
            }
        };
        match attempt(http, &url, text, opts.timeout).await {
            Outcome::Sent => return,
            Outcome::RateLimited(d) => {
                tracing::warn!(
                    "group notice rate limited by Discord; trying again in {:.1} s (try {n})",
                    d.as_secs_f64()
                );
                tokio::time::sleep(d).await;
            }
            Outcome::Failed(why) => {
                tracing::warn!(
                    "group notice not posted yet (try {n}): {why}; trying again in {} s",
                    wait.as_secs_f64()
                );
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(opts.max_retry);
            }
            Outcome::Refused(why) => {
                tracing::error!("group notice not posted: {why}: {text}");
                return;
            }
        }
    }
    tracing::error!(
        "group notice not posted: gave up after {} tries: {text}",
        opts.attempts
    );
}

/// Post a test message now, once, and say how it went (#68's button).
pub async fn send_test(
    db: &SqlitePool,
    secrets: &Secrets,
    http: &reqwest::Client,
    by: &str,
) -> Result<(), String> {
    let url = secrets
        .setting(db, WEBHOOK)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no Discord webhook is set".to_owned())?;
    let text =
        format!("This is a test message from POP, sent by {by}. Group notices will appear here.");
    match attempt(http, &url, &text, Options::default().timeout).await {
        Outcome::Sent => Ok(()),
        Outcome::RateLimited(d) => Err(format!(
            "Discord is rate limiting this webhook: try again in {} s",
            d.as_secs_f64().ceil()
        )),
        Outcome::Failed(why) | Outcome::Refused(why) => Err(why),
    }
}

// ---- the admin API ---------------------------------------------------------

use axum::{
    Extension, Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::{
    AppState,
    api::{ApiError, AuditKind, DiscordWebhook, NotifySettings, User},
    audit::{self, Actor, Event},
};

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ApiError { error: msg.into() })).into_response()
}

/// Record an admin's change; on failure, the reason, to answer with.
async fn record(
    state: &AppState,
    admin: &User,
    kind: AuditKind,
    summary: String,
) -> Result<(), String> {
    let mut conn = state.db.acquire().await.map_err(|e| e.to_string())?;
    audit::record(
        &mut conn,
        &Actor::user(admin.id, &admin.name),
        Event {
            kind,
            printer: None,
            subject: None,
            summary,
        },
    )
    .await
    .map_err(|e| e.to_string())
}

/// `GET /api/admin/notify`.
pub async fn settings(State(state): State<AppState>) -> Response {
    match state.secrets.setting(&state.db, WEBHOOK).await {
        Ok(url) => Json(NotifySettings {
            discord_webhook: url.as_deref().map(masked),
        })
        .into_response(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `PUT /api/admin/notify/discord`: where group notices go.
pub async fn set_webhook(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    body: Result<Json<DiscordWebhook>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(b) => b,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.body_text()),
    };
    let url = body.webhook_url.trim();
    if let Err(why) = check_webhook(url) {
        return error(StatusCode::BAD_REQUEST, format!("webhook_url: {why}"));
    }
    if let Err(e) = state.secrets.set_setting(&state.db, WEBHOOK, url).await {
        let code = if e == SecretError::NoKey {
            StatusCode::CONFLICT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        return error(code, e.to_string());
    }
    // Never the URL: it is the secret.
    let summary = format!("{} set the group's Discord webhook", admin.name);
    match record(&state, &admin, AuditKind::WebhookSet, summary).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}

/// `DELETE /api/admin/notify/discord`: no group notices.
pub async fn clear_webhook(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
) -> Response {
    match Secrets::clear_setting(&state.db, WEBHOOK).await {
        Ok(false) => return StatusCode::NO_CONTENT.into_response(),
        Ok(true) => {}
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
    let summary = format!("{} removed the group's Discord webhook", admin.name);
    match record(&state, &admin, AuditKind::WebhookCleared, summary).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}

/// `POST /api/admin/notify/discord/test`: post a test message now.
pub async fn test_webhook(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
) -> Response {
    let sent = send_test(&state.db, &state.secrets, &state.http, &admin.name).await;
    let summary = match &sent {
        Ok(()) => format!("{} sent a test message to the group", admin.name),
        Err(why) => format!(
            "{} sent a test message to the group, and it failed: {why}",
            admin.name
        ),
    };
    if let Err(e) = record(&state, &admin, AuditKind::WebhookTested, summary).await {
        return error(StatusCode::SERVICE_UNAVAILABLE, e);
    }
    match sent {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(why) if why == "no Discord webhook is set" => error(StatusCode::CONFLICT, why),
        Err(why) => error(StatusCode::BAD_GATEWAY, why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str =
        "https://discord.com/api/webhooks/1234567890123/AbCdEf-gh_IjKlMnOpQrStUvWxYz";

    #[test]
    fn a_discord_webhook_is_accepted_and_anything_else_is_named() {
        assert_eq!(check_webhook(GOOD), Ok(()));
        assert_eq!(
            check_webhook("https://discordapp.com/api/v10/webhooks/42/abcdefghij"),
            Ok(())
        );
        for bad in [
            "http://discord.com/api/webhooks/1/abcdefghij",
            "https://discord.com.evil.example/api/webhooks/1/abcdefghij",
            "https://evil.example/api/webhooks/1/abcdefghij",
            "https://discord.com/api/webhooks/abc/abcdefghij",
            "https://discord.com/api/webhooks/1/",
            "https://discord.com/api/webhooks/1/ab cd efgh",
            "https://discord.com/channels/1/2",
            "",
        ] {
            assert!(check_webhook(bad).is_err(), "{bad}");
        }
        assert!(
            check_webhook("https://evil.example/api/webhooks/1/abcdefghij")
                .unwrap_err()
                .starts_with("evil.example is not Discord")
        );
    }

    #[test]
    fn a_saved_webhook_is_shown_without_its_token() {
        let m = masked(GOOD);
        assert_eq!(m, "https://discord.com/api/webhooks/1234567890123/…WxYz");
        assert!(!m.contains("AbCdEf"));
    }
}
