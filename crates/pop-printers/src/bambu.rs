//! Bambu printers in LAN mode (#31): status over MQTT with TLS.
//!
//! The driver signs in to the printer's own MQTT broker on port 8883 as
//! `bblp`, with the printer's LAN access code, subscribes to
//! `device/<serial>/report`, and asks for the whole state (`pushall`). Some
//! models then send only what changed, so each report is merged into what
//! came before.
//!
//! The printer's certificate is self-signed. POP trusts it on first contact
//! and pins its SHA-256 fingerprint per printer ([`PinStore`]); a different
//! certificate later is refused, and the printer shows why rather than
//! reading as merely offline. So does an access code the printer refuses.
//!
//! Bambu does not document this protocol. The driver follows the community's
//! documentation of it (OpenBambuAPI), and [`TESTED_FIRMWARE`] records the
//! firmware it has been tested against.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use rumqttc::{
    AsyncClient, ConnectionError, Event, MqttOptions, Packet, QoS, TlsConfiguration, Transport,
};
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::CryptoProvider,
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    driver::{Capabilities, Driver, DriverError, DriverFuture, Reporter},
    state::{Job, Layer, PrinterState, PrinterStatus},
};

/// Firmware versions this driver has been run against on a real printer.
/// None yet: it is tested against POP's fake (`fakes::bambu`), which follows
/// OpenBambuAPI's documentation of the protocol as of 2026-11. Add a version
/// here, with the model, once a printer running it has been watched printing.
pub const TESTED_FIRMWARE: &[(&str, &str)] = &[];

/// Where printers' certificate fingerprints are kept, by serial number.
pub trait PinStore: Send + Sync {
    fn pinned(&self, serial: &str) -> Option<String>;
    fn pin(&self, serial: &str, fingerprint: &str);
}

/// Pins kept in memory: for tests, and until the server keeps them (#31).
#[derive(Default)]
pub struct MemoryPins(Mutex<HashMap<String, String>>);

impl PinStore for MemoryPins {
    fn pinned(&self, serial: &str) -> Option<String> {
        self.0.lock().unwrap().get(serial).cloned()
    }
    fn pin(&self, serial: &str, fingerprint: &str) {
        self.0
            .lock()
            .unwrap()
            .insert(serial.to_owned(), fingerprint.to_owned());
    }
}

/// One Bambu printer.
#[derive(Clone)]
pub struct Bambu {
    pub host: String,
    pub port: u16,
    pub serial: String,
    pub access_code: String,
    pub pins: Arc<dyn PinStore>,
    /// How long the printer may take to accept the connection.
    pub answer_within: Duration,
    /// How long to keep showing a refusal (a changed certificate, a wrong
    /// access code) before trying again: an admin has to act, so trying
    /// sooner only fills the log.
    pub hold_after_refusal: Duration,
}

impl Bambu {
    pub fn new(
        host: impl Into<String>,
        serial: impl Into<String>,
        access_code: impl Into<String>,
        pins: Arc<dyn PinStore>,
    ) -> Self {
        Self {
            host: host.into(),
            port: 8883,
            serial: serial.into(),
            access_code: access_code.into(),
            pins,
            answer_within: Duration::from_secs(15),
            hold_after_refusal: Duration::from_secs(300),
        }
    }
}

/// A certificate that does not match the pin, as the verifier found it.
#[derive(Debug, Clone)]
struct Mismatch {
    seen: String,
}

/// Trusts a printer's certificate on first contact, then only that one.
struct Pinning {
    serial: String,
    pins: Arc<dyn PinStore>,
    provider: Arc<CryptoProvider>,
    mismatch: Arc<Mutex<Option<Mismatch>>>,
}

impl fmt::Debug for Pinning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pinning")
            .field("serial", &self.serial)
            .finish()
    }
}

pub fn fingerprint(der: &[u8]) -> String {
    hex::encode(Sha256::digest(der))
}

impl ServerCertVerifier for Pinning {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let seen = fingerprint(end_entity.as_ref());
        match self.pins.pinned(&self.serial) {
            None => {
                self.pins.pin(&self.serial, &seen);
                Ok(ServerCertVerified::assertion())
            }
            Some(pinned) if pinned == seen => Ok(ServerCertVerified::assertion()),
            Some(_) => {
                *self.mismatch.lock().unwrap() = Some(Mismatch { seen });
                Err(rustls::Error::General(
                    "the printer's certificate is not the one pinned".into(),
                ))
            }
        }
    }

    // The printer must still prove it holds the certificate's key.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// What the printer has reported so far, merged.
#[derive(Debug, Default, Clone)]
pub struct Report(pub Map<String, Value>);

impl Report {
    /// Merge one report's `print` fields over what came before.
    pub fn merge(&mut self, report: &Value) {
        if let Some(print) = report.get("print").and_then(Value::as_object) {
            for (k, v) in print {
                self.0.insert(k.clone(), v.clone());
            }
        }
    }

    fn u64(&self, k: &str) -> Option<u64> {
        self.0.get(k).and_then(Value::as_u64)
    }

    /// The printer's error, by Bambu's code, or None. Plain words come from
    /// the error catalog (#35).
    pub fn error(&self) -> Option<String> {
        let code = self.u64("print_error").filter(|c| *c != 0);
        let hms = self
            .0
            .get("hms")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|h| Some((h.get("attr")?.as_u64()?, h.get("code")?.as_u64()?)));
        match (code, hms) {
            (Some(c), _) => Some(format!("Bambu error {:04X}_{:04X}", c >> 16, c & 0xFFFF)),
            (None, Some((a, c))) => Some(format!(
                "Bambu HMS {:04X}_{:04X}_{:04X}_{:04X}",
                a >> 16,
                a & 0xFFFF,
                c >> 16,
                c & 0xFFFF
            )),
            _ => None,
        }
    }

    pub fn state(&self) -> PrinterState {
        let error = self.error();
        let status = match self.0.get("gcode_state").and_then(Value::as_str) {
            Some("PREPARE" | "SLICING" | "RUNNING") => PrinterStatus::Printing,
            // A pause with an error is the printer waiting for a person.
            Some("PAUSE") if self.u64("print_error").is_some_and(|c| c != 0) => {
                PrinterStatus::NeedsHand
            }
            Some("PAUSE") => PrinterStatus::Paused,
            Some("FINISH") => PrinterStatus::Done,
            Some("FAILED") => PrinterStatus::NeedsHand,
            // IDLE, or not said yet
            _ => PrinterStatus::Idle,
        };
        let has_print = status != PrinterStatus::Idle;
        let progress = self
            .u64("mc_percent")
            .filter(|_| has_print)
            .map(|p| (p.min(100) as f32) / 100.0);
        let layer = self.u64("layer_num").filter(|_| has_print).map(|n| Layer {
            current: n as u32,
            total: self
                .u64("total_layer_num")
                .filter(|t| *t > 0)
                .map(|t| t as u32),
        });
        let seconds_left = self
            .u64("mc_remaining_time")
            .filter(|_| {
                matches!(
                    status,
                    PrinterStatus::Printing | PrinterStatus::Paused | PrinterStatus::NeedsHand
                )
            })
            .map(|m| m * 60);
        let job = self
            .0
            .get("subtask_name")
            .and_then(Value::as_str)
            .filter(|n| !n.is_empty() && has_print)
            .map(|n| Job {
                name: n.to_owned(),
                owner: None,
            });
        PrinterState {
            status,
            progress,
            layer,
            seconds_left,
            job,
            error: error.filter(|_| status == PrinterStatus::NeedsHand),
        }
    }
}

fn fail(m: impl Into<String>) -> DriverError {
    DriverError(m.into())
}

impl Driver for Bambu {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            status: true,
            errors: true,
            ..Capabilities::default()
        }
    }

    fn run(self: Box<Self>, r: Reporter) -> DriverFuture {
        Box::pin(async move {
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let mismatch = Arc::new(Mutex::new(None));
            let verifier = Pinning {
                serial: self.serial.clone(),
                pins: self.pins.clone(),
                provider: provider.clone(),
                mismatch: mismatch.clone(),
            };
            let tls = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| fail(e.to_string()))?
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(verifier))
                .with_no_client_auth();
            let mut opts =
                MqttOptions::new(format!("pop-{}", self.serial), self.host.clone(), self.port);
            opts.set_credentials("bblp", self.access_code.clone());
            opts.set_keep_alive(Duration::from_secs(30));
            // A whole report from some models runs to tens of kilobytes.
            opts.set_max_packet_size(1 << 20, 1 << 16);
            opts.set_transport(Transport::tls_with_config(TlsConfiguration::Rustls(
                Arc::new(tls),
            )));
            let (client, mut events) = AsyncClient::new(opts, 16);
            let report_topic = format!("device/{}/report", self.serial);
            let request_topic = format!("device/{}/request", self.serial);

            // Shows a refusal an admin must act on, for a while, then lets
            // the fleet try again.
            let refused = |why: String| {
                let r = r.clone();
                let hold = self.hold_after_refusal;
                async move {
                    r.report(PrinterState {
                        error: Some(why.clone()),
                        ..PrinterState::offline()
                    });
                    tokio::time::sleep(hold).await;
                    Err(fail(why))
                }
            };

            let mut report = Report::default();
            let mut connected = false;
            loop {
                let event = if connected {
                    events.poll().await
                } else {
                    match tokio::time::timeout(self.answer_within, events.poll()).await {
                        Ok(e) => e,
                        Err(_) => return Err(fail(format!("{} did not answer", self.host))),
                    }
                };
                match event {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        connected = true;
                        client
                            .subscribe(&report_topic, QoS::AtMostOnce)
                            .await
                            .map_err(|e| fail(e.to_string()))?;
                        let pushall =
                            json!({"pushing": {"sequence_id": "0", "command": "pushall"}});
                        client
                            .publish(&request_topic, QoS::AtMostOnce, false, pushall.to_string())
                            .await
                            .map_err(|e| fail(e.to_string()))?;
                    }
                    Ok(Event::Incoming(Packet::Publish(p))) if p.topic == report_topic => {
                        match serde_json::from_slice::<Value>(&p.payload) {
                            Ok(v) => {
                                report.merge(&v);
                                if report.0.contains_key("gcode_state") {
                                    r.report(report.state());
                                }
                            }
                            Err(_) => tracing::warn!(
                                "bambu {}: an unreadable report, ignored",
                                self.serial
                            ),
                        }
                    }
                    Ok(_) => {}
                    Err(ConnectionError::ConnectionRefused(code)) => {
                        return refused(format!(
                            "the printer refused POP's sign-in ({code:?}): check its LAN access code"
                        ))
                        .await;
                    }
                    Err(e) => {
                        // Taken before awaiting: the lock must not be held across it.
                        let found = mismatch.lock().unwrap().take();
                        if let Some(m) = found {
                            return refused(format!(
                                "the printer's certificate changed (now {}): an admin must confirm it is the same printer",
                                &m.seen[..16]
                            ))
                            .await;
                        }
                        return Err(fail(format!(
                            "the connection to {} dropped: {e}",
                            self.host
                        )));
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(v: Value) -> Report {
        let mut r = Report::default();
        r.merge(&json!({ "print": v }));
        r
    }

    #[test]
    fn each_bambu_state_reads_as_pop_says() {
        for (bambu, pop) in [
            ("IDLE", PrinterStatus::Idle),
            ("PREPARE", PrinterStatus::Printing),
            ("RUNNING", PrinterStatus::Printing),
            ("PAUSE", PrinterStatus::Paused),
            ("FINISH", PrinterStatus::Done),
            ("FAILED", PrinterStatus::NeedsHand),
        ] {
            assert_eq!(
                report(json!({"gcode_state": bambu})).state().status,
                pop,
                "{bambu}"
            );
        }
    }

    #[test]
    fn a_pause_with_an_error_needs_a_hand_and_names_the_code() {
        // Some print error; what each code means is the catalog's (#35).
        let s = report(json!({"gcode_state": "PAUSE", "print_error": 0x0700_8011u64})).state();
        assert_eq!(s.status, PrinterStatus::NeedsHand);
        assert_eq!(s.error.as_deref(), Some("Bambu error 0700_8011"));
    }

    #[test]
    fn an_hms_entry_names_the_error_when_there_is_no_print_error() {
        let s = report(json!({"gcode_state": "FAILED", "print_error": 0,
            "hms": [{"attr": 0x0300_0100u64, "code": 0x0001_0007u64}]}))
        .state();
        assert_eq!(s.error.as_deref(), Some("Bambu HMS 0300_0100_0001_0007"));
    }

    #[test]
    fn a_print_reads_with_progress_layers_time_left_and_name() {
        let s = report(
            json!({"gcode_state": "RUNNING", "mc_percent": 37, "layer_num": 41,
            "total_layer_num": 120, "mc_remaining_time": 52, "subtask_name": "lamp_shade"}),
        )
        .state();
        assert_eq!(s.progress, Some(0.37));
        let l = s.layer.unwrap();
        assert_eq!((l.current, l.total), (41, Some(120)));
        assert_eq!(s.seconds_left, Some(52 * 60));
        assert_eq!(s.job.unwrap().name, "lamp_shade");
        assert_eq!(s.error, None);
    }

    #[test]
    fn an_idle_printer_shows_nothing_left_over_from_its_last_print() {
        let s = report(
            json!({"gcode_state": "IDLE", "mc_percent": 100, "layer_num": 120,
            "mc_remaining_time": 0, "subtask_name": "lamp_shade", "print_error": 0x0700_8011u64}),
        )
        .state();
        assert_eq!(
            (s.progress, s.layer, s.seconds_left, s.job, s.error),
            (None, None, None, None, None)
        );
    }

    #[test]
    fn a_partial_report_keeps_the_rest() {
        let mut r = report(
            json!({"gcode_state": "RUNNING", "mc_percent": 10, "subtask_name": "lamp_shade"}),
        );
        r.merge(&json!({"print": {"mc_percent": 11}}));
        let s = r.state();
        assert_eq!(s.progress, Some(0.11));
        assert_eq!(s.job.unwrap().name, "lamp_shade");
    }

    #[test]
    fn a_progress_over_100_is_kept_at_100() {
        assert_eq!(
            report(json!({"gcode_state": "RUNNING", "mc_percent": 130}))
                .state()
                .progress,
            Some(1.0)
        );
    }
}
