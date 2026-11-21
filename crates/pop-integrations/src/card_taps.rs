//! Card taps at a kiosk's reader, heard on the space's broker (#19), where
//! CS:S signs members in by card (Cambridge-Hackspace/cooperative-systems-spaces#241).
//!
//! The reader at a kiosk is a CS:S device. Its taps go to CS:S's edge, which
//! answers each on `resource/response/scan` with a [`LocalScanReply`] naming
//! the resource it answers about. POP listens to those answers and to
//! nothing else: it never asks, and never publishes.
//!
//! A reply is believed only as far as it can be checked. Its code or ticket
//! is redeemed with CS:S using POP's own client secret, so a reply forged on
//! the broker signs nobody in; at most it shows a refusal it made up. Which
//! resource is whose kiosk is POP's to say, not the reply's.

use std::{sync::Arc, time::Duration};

pub use css_wire::{LocalScanReply, LocalSignIn};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, TlsConfiguration, Transport};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

/// Where the scan replies are: `resource/response/scan`, unnamespaced, as the
/// edge publishes it.
pub const TOPIC: &str = css_wire::local::SCAN_RESPONSE;

/// The space's broker, as `[card_sign_in]` names it.
#[derive(Clone, PartialEq, Eq)]
pub struct Broker {
    pub host: String,
    pub port: u16,
    /// `mqtts://`: TLS, trusting what this machine trusts.
    pub tls: bool,
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Broker {
    // Never the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Broker")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

impl Broker {
    /// `mqtt://host[:port]` (1883) or `mqtts://host[:port]` (8883), with no
    /// path, query or login in it: the login is given apart.
    pub fn parse(url: &str, username: &str, password: &str) -> Result<Self, String> {
        let (tls, rest) = if let Some(r) = url.strip_prefix("mqtts://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("mqtt://") {
            (false, r)
        } else {
            return Err(format!(
                "{url:?}: expected mqtt://host[:port] or mqtts://host[:port]"
            ));
        };
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        if rest.is_empty() || rest.contains(['/', '?', '#', '@']) {
            return Err(format!(
                "{url:?}: expected just a host and port, as mqtt://192.168.1.20:1883 (the login goes in username and password)"
            ));
        }
        let (host, port) = match rest.rsplit_once(':') {
            // An IPv6 address in brackets has colons of its own.
            Some((h, p)) if !p.contains(']') => {
                let port = p
                    .parse::<u16>()
                    .ok()
                    .filter(|p| *p != 0)
                    .ok_or_else(|| format!("{url:?}: {p:?} is not a port"))?;
                (h, port)
            }
            _ => (rest, if tls { 8883 } else { 1883 }),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() {
            return Err(format!("{url:?}: no host"));
        }
        Ok(Self {
            host: host.to_owned(),
            port,
            tls,
            username: username.to_owned(),
            password: password.to_owned(),
        })
    }
}

/// What a tap at a reader came to, by the resource it was at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tap {
    /// The resource the reply names: its id or its external id, as the reader
    /// asked by.
    pub resource: String,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// CS:S's sign-in for this tap: a code, a ticket wanting the member's
    /// TOTP code, or a refusal in words.
    SignIn(LocalSignIn),
    /// No sign-in, and why, in words for the member: the tap was refused
    /// ("Unknown card"), or allowed with no sign-in on it, as an edge answers
    /// when it cannot reach CS:S.
    NotSignedIn(String),
}

/// The tap a scan reply reports, or None for one that is not a reply at all.
pub fn tap_from(payload: &[u8]) -> Option<Tap> {
    let r: LocalScanReply = serde_json::from_slice(payload).ok()?;
    let outcome = match (r.authorized, r.sign_in) {
        (true, Some(s)) => Outcome::SignIn(s),
        (true, None) => {
            Outcome::NotSignedIn("CS:S could not sign you in just now: try again".to_owned())
        }
        // A refusal never carries a sign-in; one that claims to is a refusal.
        (false, _) => Outcome::NotSignedIn(if r.reason.trim().is_empty() {
            "This card was refused".to_owned()
        } else {
            r.reason
        }),
    };
    Some(Tap {
        resource: r.resource,
        outcome,
    })
}

/// Whether POP is hearing the broker, for `/healthz`: None while it is, the
/// last error while it is not.
pub type Hearing = watch::Receiver<Option<String>>;

/// Listen until the task is aborted, sending every tap to `taps`, and
/// reconnecting after a dropped connection, waiting a little longer each time
/// up to half a minute.
pub fn spawn(broker: Broker, taps: mpsc::Sender<Tap>) -> (Hearing, JoinHandle<()>) {
    let (tx, rx) = watch::channel(Some("not connected yet".to_owned()));
    let task = tokio::spawn(run(broker, taps, tx));
    (rx, task)
}

async fn run(b: Broker, taps: mpsc::Sender<Tap>, hearing: watch::Sender<Option<String>>) {
    let mut opts = MqttOptions::new(format!("pop-cards-{}", std::process::id()), &b.host, b.port);
    opts.set_keep_alive(Duration::from_secs(30));
    if !b.username.is_empty() {
        opts.set_credentials(&b.username, &b.password);
    }
    if b.tls {
        match crate::sync::rustls_config() {
            Ok(cfg) => opts.set_transport(Transport::tls_with_config(TlsConfiguration::Rustls(
                Arc::new(cfg),
            ))),
            Err(e) => {
                let _ = hearing.send(Some(format!("TLS for {}: {e}", b.host)));
                return;
            }
        };
    }
    let (client, mut events) = AsyncClient::new(opts, 16);
    let mut wait = Duration::from_secs(1);
    loop {
        match events.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                // A clean session: subscribed again on every connection.
                if let Err(e) = client.subscribe(TOPIC, QoS::AtMostOnce).await {
                    let _ = hearing.send(Some(e.to_string()));
                }
            }
            // Hearing once the broker has the subscription, not when it is
            // asked for: until then a tap would be missed.
            Ok(Event::Incoming(Packet::SubAck(_))) => {
                wait = Duration::from_secs(1);
                let _ = hearing.send(None);
            }
            Ok(Event::Incoming(Packet::Publish(p))) if p.topic == TOPIC => {
                match tap_from(&p.payload) {
                    Some(t) => {
                        if taps.send(t).await.is_err() {
                            return;
                        }
                    }
                    None => tracing::warn!("card taps: a scan reply that is not one, ignored"),
                }
            }
            Ok(_) => {}
            Err(e) => {
                let why = format!("the broker at {}:{}: {e}", b.host, b.port);
                tracing::warn!("card taps: {why}");
                let _ = hearing.send(Some(why));
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(Duration::from_secs(30));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broker_is_a_host_and_port() {
        let b = Broker::parse("mqtt://192.168.1.20", "", "").unwrap();
        assert_eq!(
            (b.host.as_str(), b.port, b.tls),
            ("192.168.1.20", 1883, false)
        );
        let b = Broker::parse("mqtts://broker.space:9000/", "pop", "pw").unwrap();
        assert_eq!(
            (b.host.as_str(), b.port, b.tls),
            ("broker.space", 9000, true)
        );
        assert_eq!(Broker::parse("mqtts://h", "", "").unwrap().port, 8883);
        let b = Broker::parse("mqtt://[fd00::1]:1884", "", "").unwrap();
        assert_eq!((b.host.as_str(), b.port), ("fd00::1", 1884));
        assert_eq!(
            Broker::parse("mqtt://[fd00::1]", "", "").unwrap().port,
            1883
        );
        for bad in [
            "tcp://h:1883",
            "h:1883",
            "mqtt://",
            "mqtt://h:0",
            "mqtt://h:port",
            "mqtt://u:p@h",
            "mqtt://h/topic",
            "mqtt://:1883",
        ] {
            assert!(Broker::parse(bad, "", "").is_err(), "{bad}");
        }
    }

    #[test]
    fn a_password_is_never_shown() {
        let b = Broker::parse("mqtt://h", "pop", "hunter2").unwrap();
        assert!(!format!("{b:?}").contains("hunter2"));
    }

    fn tap(v: serde_json::Value) -> Option<Tap> {
        tap_from(v.to_string().as_bytes())
    }

    #[test]
    fn a_sign_in_is_read_from_a_yes() {
        let t = tap(
            serde_json::json!({"resource": "tower-1", "authorized": true, "reason": "authorized",
            "member": {"id": "u", "name": "Alex"}, "sign_in": {"kind": "code", "code": "c0de"}}),
        )
        .unwrap();
        assert_eq!(t.resource, "tower-1");
        assert_eq!(
            t.outcome,
            Outcome::SignIn(LocalSignIn::Code {
                code: "c0de".into()
            })
        );
        let t = tap(
            serde_json::json!({"resource": "r", "authorized": true, "reason": "",
            "sign_in": {"kind": "totp", "ticket": "t"}}),
        )
        .unwrap();
        assert_eq!(
            t.outcome,
            Outcome::SignIn(LocalSignIn::Totp { ticket: "t".into() })
        );
    }

    #[test]
    fn a_refusal_signs_nobody_in_even_claiming_to() {
        let t = tap(
            serde_json::json!({"resource": "r", "authorized": false, "reason": "Unknown card",
            "sign_in": {"kind": "code", "code": "c0de"}}),
        )
        .unwrap();
        assert_eq!(t.outcome, Outcome::NotSignedIn("Unknown card".into()));
        let t =
            tap(serde_json::json!({"resource": "r", "authorized": false, "reason": " "})).unwrap();
        assert_eq!(
            t.outcome,
            Outcome::NotSignedIn("This card was refused".into())
        );
    }

    #[test]
    fn a_yes_with_no_sign_in_says_so() {
        let t =
            tap(serde_json::json!({"resource": "r", "authorized": true, "reason": "authorized"}))
                .unwrap();
        assert!(
            matches!(t.outcome, Outcome::NotSignedIn(w) if w.contains("could not sign you in"))
        );
    }

    #[test]
    fn what_is_not_a_reply_is_nothing() {
        assert_eq!(tap_from(b"not json"), None);
        assert_eq!(tap(serde_json::json!({"resource": "r"})), None);
        assert_eq!(
            tap(
                serde_json::json!({"resource": "r", "authorized": true, "reason": "",
                "sign_in": {"kind": "sorcery"}})
            ),
            None
        );
    }
}
