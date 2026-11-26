//! POP's identity as a CS:S device (#18): registered once, from the command
//! line, with an invite an administrator mints in CS:S
//! (`pop css-device register`), and kept sealed with POP's other secrets.
//!
//! The request is the one every CS:S device sends (CS:S's FIRMWARE.md,
//! "Registration"), built here rather than through spaces-firmware's
//! `spaces-device` crate, which is AGPL-licensed where POP is Blue Oak. It
//! registers POP as a `power` device that switches nothing: the edge leases
//! only `power` bindings, and a lease is what tells POP who tapped.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::secrets::{SecretError, Secrets};

/// The sealed setting the identity is kept in.
pub const SETTING: &str = "css_device";
/// Where an invite is claimed, on the CS:S server.
pub const REGISTER_PATH: &str = "/api/devices/register";

/// Who POP is to CS:S. `command_key` is what the local wire is sealed with;
/// a registration that came back with none cannot sign POP in by card.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub device_id: String,
    pub auth_token: String,
    pub command_key: Option<String>,
}

impl std::fmt::Debug for Identity {
    // Never the token or the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("device_id", &self.device_id)
            .field("command_key", &self.command_key.as_ref().map(|_| "****"))
            .finish_non_exhaustive()
    }
}

/// The registration body. Pure, so what goes on the wire is tested without a
/// server.
pub fn request_body(invite: &str, name: &str, mac: &str, version: &str) -> serde_json::Value {
    serde_json::json!({
        "device_code": invite,
        "name": name,
        "capabilities": {
            "roles": ["power"],
            // It switches nothing, so it can cut nothing and hold nothing:
            // each declared as the "cannot" it is rather than left to the
            // server's default.
            "local_inputs": [],
            "local_inhibit": false,
            "countdown": false,
            "holds_last_on_disconnect": false,
        },
        "mac_address": mac,
        "software_version": version,
        "platform": std::env::consts::OS
            .parse::<Platform>()
            .unwrap_or(Platform::Other)
            .as_str(),
    })
}

/// CS:S's `platform` vocabulary, exactly: anything else is refused.
enum Platform {
    Linux,
    Macos,
    Windows,
    Other,
}

impl std::str::FromStr for Platform {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(match s {
            "linux" => Self::Linux,
            "macos" => Self::Macos,
            "windows" => Self::Windows,
            _ => Self::Other,
        })
    }
}

impl Platform {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Other => "other",
        }
    }
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    success: bool,
    data: Option<Data>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Data {
    device_id: String,
    auth_token: String,
    #[serde(default)]
    mqtt_config: Option<MqttConfig>,
}

#[derive(Deserialize)]
struct MqttConfig {
    #[serde(default)]
    command_key: Option<String>,
}

/// The identity a registration reply gives, or why there is none, in words
/// for whoever ran the command.
pub fn identity_from(status: u16, body: &[u8]) -> Result<Identity, String> {
    let e: Envelope = serde_json::from_slice(body).map_err(|_| {
        format!(
            "CS:S answered HTTP {status}, and not with the JSON it sends: {}",
            String::from_utf8_lossy(&body[..body.len().min(200)])
        )
    })?;
    if !e.success || !(200..300).contains(&status) {
        let why = match e.error {
            Some(serde_json::Value::String(s)) => s,
            Some(other) => other.to_string(),
            None => "no reason given".into(),
        };
        return Err(format!(
            "CS:S refused the registration (HTTP {status}): {why}"
        ));
    }
    let d = e
        .data
        .ok_or("CS:S said yes but sent no identity: register again with a new invite")?;
    if d.device_id.is_empty() || d.auth_token.is_empty() {
        return Err("CS:S sent an empty identity: register again with a new invite".into());
    }
    Ok(Identity {
        device_id: d.device_id,
        auth_token: d.auth_token,
        command_key: d
            .mqtt_config
            .and_then(|m| m.command_key)
            .filter(|k| !k.is_empty()),
    })
}

/// The identity POP keeps, if it has registered.
pub async fn load(db: &SqlitePool, secrets: &Secrets) -> Result<Option<Identity>, SecretError> {
    let Some(text) = secrets.setting(db, SETTING).await? else {
        return Ok(None);
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|_| SecretError::WillNotOpen)
}

/// Keep `id`, sealed. Refuses to replace one already kept: CS:S's bindings
/// name the old device, so a second registration would quietly orphan them.
pub async fn save(db: &SqlitePool, secrets: &Secrets, id: &Identity) -> Result<(), String> {
    if load(db, secrets)
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err(
            "POP is already registered with CS:S; to register it again, run \
                    `pop css-device forget` first, then bind the new device in CS:S"
                .into(),
        );
    }
    let text = serde_json::to_string(id).expect("an identity serializes");
    secrets
        .set_setting(db, SETTING, &text)
        .await
        .map_err(|e| e.to_string())
}

/// Claim `invite` at the CS:S server `server`, as `name`, and keep what it
/// gives.
pub async fn register(
    db: &SqlitePool,
    secrets: &Secrets,
    http: &reqwest::Client,
    server: &str,
    invite: &str,
    name: &str,
    mac: &str,
) -> Result<Identity, String> {
    if !secrets.has_key() {
        return Err(SecretError::NoKey.to_string());
    }
    if load(db, secrets)
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err(
            "POP is already registered with CS:S; to register it again, run \
                    `pop css-device forget` first"
                .into(),
        );
    }
    let url = format!("{}{REGISTER_PATH}", server.trim_end_matches('/'));
    let body = request_body(invite, name, mac, env!("CARGO_PKG_VERSION"));
    let res = http
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("POST {url}: {e}"))?;
    let status = res.status().as_u16();
    let bytes = res
        .bytes()
        .await
        .map_err(|e| format!("reading CS:S's answer: {e}"))?;
    let id = identity_from(status, &bytes)?;
    save(db, secrets, &id).await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_is_a_power_device_that_switches_nothing() {
        let v = request_body("INVITE", "POP", "02:00:00:00:00:01", "0.1.2");
        assert_eq!(v["device_code"], "INVITE");
        assert_eq!(v["capabilities"]["roles"], serde_json::json!(["power"]));
        for cannot in ["local_inhibit", "countdown", "holds_last_on_disconnect"] {
            assert_eq!(v["capabilities"][cannot], false, "{cannot}");
        }
        assert_eq!(v["mac_address"], "02:00:00:00:00:01");
        assert!(
            ["linux", "macos", "windows", "other"].contains(&v["platform"].as_str().unwrap()),
            "{}",
            v["platform"]
        );
    }

    fn reply(mqtt: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "success": true,
            "data": {"device_id": "dev-pop", "auth_token": "tok", "mqtt_config": mqtt}
        }))
        .unwrap()
    }

    #[test]
    fn the_command_key_is_kept_and_an_absent_one_is_none() {
        let id = identity_from(200, &reply(serde_json::json!({"command_key": "ab12"}))).unwrap();
        assert_eq!(id.device_id, "dev-pop");
        assert_eq!(id.auth_token, "tok");
        assert_eq!(id.command_key.as_deref(), Some("ab12"));
        let id = identity_from(200, &reply(serde_json::json!({"mqtt_namespace": "x"}))).unwrap();
        assert_eq!(id.command_key, None);
        let id = identity_from(200, &reply(serde_json::Value::Null)).unwrap();
        assert_eq!(id.command_key, None);
        let id = identity_from(200, &reply(serde_json::json!({"command_key": ""}))).unwrap();
        assert_eq!(id.command_key, None, "an empty key is none");
    }

    #[test]
    fn a_refusal_says_why() {
        let body = br#"{"success": false, "data": null, "error": "Invalid or expired code"}"#;
        let e = identity_from(400, body).unwrap_err();
        assert!(
            e.contains("Invalid or expired code") && e.contains("400"),
            "{e}"
        );
        assert!(identity_from(502, b"<html>bad gateway</html>").is_err());
    }

    #[test]
    fn the_token_and_key_are_never_shown() {
        let id = Identity {
            device_id: "dev-pop".into(),
            auth_token: "hunter2".into(),
            command_key: Some("s3cret".into()),
        };
        let shown = format!("{id:?}");
        assert!(shown.contains("dev-pop"));
        assert!(
            !shown.contains("hunter2") && !shown.contains("s3cret"),
            "{shown}"
        );
    }
}
