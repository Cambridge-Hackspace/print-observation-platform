//! POP as a CS:S device (#18): a module bound as `power` to each resource
//! whose reader is at a kiosk, acting on the leases CS:S's edge sends it, as a
//! ToolGuard acts on its own (Cambridge-Hackspace/cooperative-systems-spaces#249).
//!
//! A member taps their card at the tower's reader. The reader asks the edge;
//! on a yes, the edge leases every `power` device on that resource, POP among
//! them, and on a resource set to identify its member (#228) the lease says
//! who: `member`, their CS:S user id (the OpenID Connect `sub`) and name. A
//! lease that starts a session for a member is a [`Tap`] for the kiosk whose
//! reader that resource is.
//!
//! **Only a sealed lease counts.** The local wire is signed with the command
//! key CS:S gave POP when it registered: the edge seals what it sends POP, and
//! POP seals what it sends the edge ([`Wire`]). An unsigned lease, or one
//! sealed with another key, or one already seen, signs nobody in -- it is
//! exactly what a forged sign-in would look like. So a CS:S edge older than
//! 0.6.1, which seals nothing, signs nobody in at POP either.
//!
//! POP says it is alive, sealed, every second ([`ALIVE_EVERY`]): an edge
//! leases no device it has not heard from, and the alive is also how the edge
//! learns POP's nonce.

use std::{collections::HashMap, sync::Arc, time::Duration};

pub use css_wire::LocalMember;
use css_wire::{LocalAliveRequest, LocalHello, ResourceLeasePayload, local, seal::Sealed};
use css_wire_sign::{Peer, Refused};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, TlsConfiguration, Transport};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

/// What POP listens to: leases, and the edge's hello.
pub const TOPICS: [&str; 2] = [local::RESOURCE_LEASE, local::HELLO];
/// Where POP says it is alive.
pub const ALIVE: &str = local::RESOURCE_ALIVE_REQUEST;
/// How often POP says so: as often as a ToolGuard does.
pub const ALIVE_EVERY: Duration = Duration::from_secs(1);

/// The space's broker, as `[card_sign_in]` names it: the edge's.
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

/// A member's card opened a session on a resource: a sign-in for the kiosk
/// whose card reader that resource is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tap {
    /// The resource, by its CS:S id: a lease always names it so.
    pub resource: String,
    pub member: LocalMember,
}

/// POP's end of the local wire: what it believes, and what it seals. Pure:
/// time is the caller's milliseconds.
pub struct Wire {
    device_id: String,
    key: Vec<u8>,
    peer: Peer,
    nonces: Box<dyn FnMut() -> String + Send>,
    /// Each resource's session as the leases have told it: the member's id
    /// (empty for a session that names nobody) and when its lease runs out.
    sessions: HashMap<String, (String, u64)>,
}

/// What one message from the edge came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heard {
    /// A session began for a member: sign them in at that kiosk.
    Tap(Tap),
    /// The edge is newly known: say alive now, so the edge hears POP sealed
    /// to it without waiting a second.
    NewEdge,
    /// For the log: a message to POP that was not believed, or a session that
    /// names nobody.
    Note(String),
}

impl Wire {
    /// `key` is POP's command key from its registration; `nonces` draws a
    /// fresh nonce, at start and whenever the seal code asks for another.
    pub fn new(
        device_id: &str,
        key: &str,
        mut nonces: impl FnMut() -> String + Send + 'static,
    ) -> Self {
        Self {
            device_id: device_id.to_owned(),
            key: key.as_bytes().to_vec(),
            peer: Peer::new(nonces()),
            nonces: Box::new(nonces),
            sessions: HashMap::new(),
        }
    }

    /// The alive pulse, sealed.
    pub fn alive(&mut self) -> Vec<u8> {
        let mut m = LocalAliveRequest {
            device_id: self.device_id.clone(),
            seal: None,
        };
        self.peer.seal(&self.key, &mut m);
        serde_json::to_vec(&m).expect("an alive pulse serializes")
    }

    /// One message from the edge, on `topic`.
    pub fn heard(&mut self, topic: &str, payload: &[u8], now: u64) -> Vec<Heard> {
        let before = self.peer.theirs().map(str::to_owned);
        let mut out = Vec::new();
        if topic == local::HELLO {
            if let Some(why) = self.open::<LocalHello>(payload, |m| &m.device_id) {
                out.push(Heard::Note(format!("a hello not believed: {why:?}")));
            }
        } else if topic == local::RESOURCE_LEASE {
            match serde_json::from_slice::<ResourceLeasePayload>(payload) {
                Ok(l) if l.device_id == self.device_id => match self.believe(&l) {
                    Ok(()) => out.extend(self.lease(l, now)),
                    Err(Refused::Stale) => {}
                    Err(why) => out.push(Heard::Note(format!(
                        "a lease for {} not believed: {why:?}",
                        l.resource
                    ))),
                },
                _ => {}
            }
        }
        if self.peer.theirs().map(str::to_owned) != before {
            out.push(Heard::NewEdge);
        }
        out
    }

    fn believe<M: Sealed>(&mut self, m: &M) -> Result<(), Refused> {
        self.peer.open(&self.key, m, &mut self.nonces)
    }

    /// Open a message for POP to learn the edge's nonce. `Some` only when it
    /// was for POP and is unsigned or badly signed.
    fn open<M: Sealed + serde::de::DeserializeOwned>(
        &mut self,
        payload: &[u8],
        device: impl Fn(&M) -> &String,
    ) -> Option<Refused> {
        let m: M = serde_json::from_slice(payload).ok()?;
        if *device(&m) != self.device_id {
            return None;
        }
        match self.believe(&m) {
            Ok(()) | Err(Refused::Stale) => None,
            Err(why) => Some(why),
        }
    }

    /// A believed lease. A grant renews a session; one that starts a session,
    /// or names another member than the session had, is a tap. A refusal
    /// ends the session.
    fn lease(&mut self, l: ResourceLeasePayload, now: u64) -> Vec<Heard> {
        if !l.grant {
            self.sessions.remove(&l.resource);
            return vec![];
        }
        let until = now.saturating_add(u64::try_from(l.ttl_ms).unwrap_or(0));
        let who = l.member.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        let new = match self.sessions.get(&l.resource) {
            Some((was, lapses)) => *was != who || now >= *lapses,
            None => true,
        };
        self.sessions.insert(l.resource.clone(), (who, until));
        match (new, l.member) {
            (true, Some(member)) => vec![Heard::Tap(Tap {
                resource: l.resource,
                member,
            })],
            (true, None) => vec![Heard::Note(format!(
                "a session on {} names no member: switch on \"Identify the member on a scan\" for that resource in CS:S",
                l.resource
            ))],
            (false, _) => vec![],
        }
    }
}

/// Whether POP is hearing the broker, for `/healthz`: None while it is, the
/// last error while it is not.
pub type Hearing = watch::Receiver<Option<String>>;

/// Be POP's device on `broker` until the task is aborted, sending every tap
/// to `taps`, and reconnecting after a dropped connection, waiting a little
/// longer each time up to half a minute.
pub fn spawn(broker: Broker, wire: Wire, taps: mpsc::Sender<Tap>) -> (Hearing, JoinHandle<()>) {
    let (tx, rx) = watch::channel(Some("not connected yet".to_owned()));
    let task = tokio::spawn(run(broker, wire, taps, tx));
    (rx, task)
}

async fn run(
    b: Broker,
    mut wire: Wire,
    taps: mpsc::Sender<Tap>,
    hearing: watch::Sender<Option<String>>,
) {
    let mut opts = MqttOptions::new(format!("pop-{}", std::process::id()), &b.host, b.port);
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
    let start = tokio::time::Instant::now();
    let now = || start.elapsed().as_millis() as u64;
    let mut alive = tokio::time::interval(ALIVE_EVERY);
    alive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut wait = Duration::from_secs(1);
    let mut connected = false;
    loop {
        let event = tokio::select! {
            e = events.poll() => e,
            _ = alive.tick(), if connected => {
                let _ = client.try_publish(ALIVE, QoS::AtMostOnce, false, wire.alive());
                continue;
            }
        };
        match event {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                // A clean session: subscribed again on every connection.
                for t in TOPICS {
                    if let Err(e) = client.try_subscribe(t, QoS::AtMostOnce) {
                        let _ = hearing.send(Some(e.to_string()));
                    }
                }
                connected = true;
            }
            // Hearing once the broker has the subscriptions, not when they are
            // asked for: until then a lease would be missed.
            Ok(Event::Incoming(Packet::SubAck(_))) => {
                wait = Duration::from_secs(1);
                let _ = hearing.send(None);
            }
            Ok(Event::Incoming(Packet::Publish(p))) => {
                for h in wire.heard(&p.topic, &p.payload, now()) {
                    match h {
                        Heard::Tap(t) => {
                            if taps.send(t).await.is_err() {
                                return;
                            }
                        }
                        Heard::NewEdge => {
                            let _ = client.try_publish(ALIVE, QoS::AtMostOnce, false, wire.alive());
                        }
                        Heard::Note(n) => tracing::warn!("CS:S device: {n}"),
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                connected = false;
                let why = format!("the broker at {}:{}: {e}", b.host, b.port);
                tracing::warn!("CS:S device: {why}");
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

    // --- the wire (#18) ----------------------------------------------------

    const KEY: &str = "pop-command-key";
    const ME: &str = "dev-pop";

    fn counter() -> impl FnMut() -> String + Send + 'static {
        let mut n = 0;
        move || {
            n += 1;
            format!("pop-{n}")
        }
    }

    /// POP, and an edge that has heard POP's alive and so seals to it.
    fn pair() -> (Wire, Peer) {
        let mut pop = Wire::new(ME, KEY, counter());
        let mut edge = Peer::new("edge-1".into());
        let a: LocalAliveRequest = serde_json::from_slice(&pop.alive()).unwrap();
        assert!(a.seal.is_some(), "the alive went out unsigned");
        // Stale, as a first message always is, but it teaches the edge POP's nonce.
        assert_eq!(
            edge.open(KEY.as_bytes(), &a, || "edge-2".into()),
            Err(Refused::Stale)
        );
        (pop, edge)
    }

    fn lease(
        device: &str,
        grant: bool,
        ttl_ms: i64,
        member: Option<(&str, &str)>,
    ) -> ResourceLeasePayload {
        ResourceLeasePayload {
            resource: "tower-uuid".into(),
            device_id: device.into(),
            grant,
            ttl_ms,
            reason: None,
            member: member.map(|(id, name)| LocalMember {
                id: id.into(),
                name: name.into(),
            }),
            seal: None,
        }
    }

    fn sealed(edge: &mut Peer, key: &str, mut l: ResourceLeasePayload) -> Vec<u8> {
        edge.seal(key.as_bytes(), &mut l);
        serde_json::to_vec(&l).unwrap()
    }

    fn taps(h: &[Heard]) -> Vec<&Tap> {
        h.iter()
            .filter_map(|h| match h {
                Heard::Tap(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    const ALEX: Option<(&str, &str)> = Some(("u-alex", "Alex Doe"));
    const SAM: Option<(&str, &str)> = Some(("u-sam", "Sam Roe"));

    #[test]
    fn a_sealed_lease_that_starts_a_session_is_one_tap() {
        let (mut pop, mut edge) = pair();
        let h = pop.heard(
            local::RESOURCE_LEASE,
            &sealed(&mut edge, KEY, lease(ME, true, 3000, ALEX)),
            100,
        );
        let t = taps(&h);
        assert_eq!(t.len(), 1, "{h:?}");
        assert_eq!(t[0].resource, "tower-uuid");
        assert_eq!(
            (t[0].member.id.as_str(), t[0].member.name.as_str()),
            ("u-alex", "Alex Doe")
        );
        // Renewals within the lease are the same session, not more sign-ins.
        for at in [1_000, 2_500, 4_000] {
            let h = pop.heard(
                local::RESOURCE_LEASE,
                &sealed(&mut edge, KEY, lease(ME, true, 3000, ALEX)),
                at,
            );
            assert!(
                taps(&h).is_empty(),
                "a renewal at {at} signed them in again: {h:?}"
            );
        }
    }

    #[test]
    fn a_new_session_is_a_new_tap() {
        let (mut pop, mut edge) = pair();
        let mut l = |pop: &mut Wire, grant, m, at| {
            let p = sealed(&mut edge, KEY, lease(ME, grant, 3000, m));
            taps(&pop.heard(local::RESOURCE_LEASE, &p, at)).len()
        };
        assert_eq!(l(&mut pop, true, ALEX, 100), 1);
        // The session ends, and the same member taps again.
        assert_eq!(l(&mut pop, false, None, 500), 0);
        assert_eq!(l(&mut pop, true, ALEX, 600), 1, "after an end");
        // Another member's session.
        assert_eq!(l(&mut pop, true, SAM, 700), 1, "another member");
        // A lease that lapsed unrenewed: what comes after is a new session.
        assert_eq!(l(&mut pop, true, SAM, 3_701), 1, "after a lapse");
    }

    #[test]
    fn an_unsigned_or_wrongly_sealed_or_replayed_lease_signs_nobody_in() {
        let (mut pop, mut edge) = pair();
        // Unsigned: anyone on the broker can publish it.
        let forged = serde_json::to_vec(&lease(ME, true, 3000, ALEX)).unwrap();
        let h = pop.heard(local::RESOURCE_LEASE, &forged, 100);
        assert!(
            taps(&h).is_empty(),
            "an unsigned lease signed Alex in: {h:?}"
        );
        assert!(
            h.iter()
                .any(|h| matches!(h, Heard::Note(n) if n.contains("Unsigned"))),
            "the forgery went unremarked: {h:?}"
        );
        // Sealed with another key.
        let other = sealed(&mut edge, "another-key", lease(ME, true, 3000, ALEX));
        let h = pop.heard(local::RESOURCE_LEASE, &other, 200);
        assert!(taps(&h).is_empty(), "{h:?}");
        assert!(
            h.iter()
                .any(|h| matches!(h, Heard::Note(n) if n.contains("BadSignature"))),
            "{h:?}"
        );
        // The edge's own, once: a tap. The same bytes again, after the
        // session ended: a replay, and nothing.
        let real = sealed(&mut edge, KEY, lease(ME, true, 3000, ALEX));
        assert_eq!(taps(&pop.heard(local::RESOURCE_LEASE, &real, 300)).len(), 1);
        let end = sealed(&mut edge, KEY, lease(ME, false, 0, None));
        pop.heard(local::RESOURCE_LEASE, &end, 400);
        let h = pop.heard(local::RESOURCE_LEASE, &real, 500);
        assert!(
            taps(&h).is_empty(),
            "a replayed lease signed Alex in again: {h:?}"
        );
    }

    #[test]
    fn a_lease_for_another_device_is_nothing_at_all() {
        let (mut pop, mut edge) = pair();
        let theirs = sealed(&mut edge, KEY, lease("dev-plug", true, 3000, ALEX));
        assert_eq!(pop.heard(local::RESOURCE_LEASE, &theirs, 100), vec![]);
        let unsigned = serde_json::to_vec(&lease("dev-plug", true, 3000, ALEX)).unwrap();
        assert_eq!(
            pop.heard(local::RESOURCE_LEASE, &unsigned, 100),
            vec![],
            "nor a note"
        );
    }

    #[test]
    fn a_session_that_names_nobody_says_how_to_fix_it() {
        let (mut pop, mut edge) = pair();
        let h = pop.heard(
            local::RESOURCE_LEASE,
            &sealed(&mut edge, KEY, lease(ME, true, 3000, None)),
            100,
        );
        assert!(taps(&h).is_empty());
        assert!(
            h.iter()
                .any(|h| matches!(h, Heard::Note(n) if n.contains("Identify the member"))),
            "{h:?}"
        );
    }

    #[test]
    fn the_edges_hello_makes_it_known_and_pop_says_alive_to_it() {
        let mut pop = Wire::new(ME, KEY, counter());
        let mut edge = Peer::new("edge-1".into());
        let mut hello = LocalHello {
            device_id: ME.into(),
            seal: None,
        };
        edge.seal(KEY.as_bytes(), &mut hello);
        let h = pop.heard(local::HELLO, &serde_json::to_vec(&hello).unwrap(), 100);
        assert_eq!(h, vec![Heard::NewEdge]);
        // The alive POP sends then names the edge, so the edge believes it.
        let a: LocalAliveRequest = serde_json::from_slice(&pop.alive()).unwrap();
        edge.open(KEY.as_bytes(), &a, || "edge-2".into())
            .expect("a fresh alive");
        // The same hello again teaches nothing new.
        assert!(
            !pop.heard(local::HELLO, &serde_json::to_vec(&hello).unwrap(), 200)
                .contains(&Heard::NewEdge)
        );
    }
}
