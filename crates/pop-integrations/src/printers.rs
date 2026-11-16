//! Printers from BAM! fields (#21): which field of the printer category
//! gives each printer's connection, address, camera, model, materials and
//! training level, and what a printer is when they are all right.
//!
//! A printer whose fields are missing or wrong is never half-added: it comes
//! back as its problems, each naming the field, for an admin to put right in
//! BAM!. Secrets (a Bambu printer's LAN access code) never live in BAM!: they
//! are entered in POP, and a printer that needs one reads as a problem until
//! it is there.

use serde::{Deserialize, Serialize};

use crate::bam::Asset;

/// Which BAM! field, by key, gives each of a printer's settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct FieldMap {
    pub connection: String,
    pub address: String,
    pub camera: String,
    pub model: String,
    pub materials: String,
    pub training: String,
    /// A Bambu printer's serial number.
    pub serial: String,
}

impl Default for FieldMap {
    fn default() -> Self {
        Self {
            connection: "connection".into(),
            address: "address".into(),
            camera: "camera".into(),
            model: "model".into(),
            materials: "materials".into(),
            training: "training".into(),
            serial: "serial".into(),
        }
    }
}

/// How POP reaches a printer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Connection {
    /// Klipper, through Moonraker's WebSocket.
    Moonraker {
        url: String,
    },
    OctoPrint {
        url: String,
    },
    PrusaLink {
        url: String,
    },
    /// Bambu in LAN mode: its address and serial number. The access code is
    /// POP's, never BAM!'s.
    Bambu {
        host: String,
        serial: String,
    },
    /// A printer on the kiosk box's USB port.
    UsbSerial {
        device: String,
    },
}

/// The values the connection field may have.
pub const CONNECTIONS: &[&str] = &["moonraker", "octoprint", "prusalink", "bambu", "usb_serial"];

/// A printer as POP runs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrinterConfig {
    pub bam_id: String,
    pub name: String,
    pub connection: Connection,
    pub camera: Option<String>,
    pub model: Option<String>,
    pub materials: Vec<String>,
    pub training: Option<String>,
}

/// One thing wrong with a printer, naming the field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    /// The field as BAM! labels it, with its key: "Connection (connection)".
    pub field: String,
    pub problem: String,
}

/// Whether POP holds a secret a printer needs, by its BAM! id.
pub trait Secrets {
    fn has_access_code(&self, bam_id: &str) -> bool;
}

impl<F: Fn(&str) -> bool> Secrets for F {
    fn has_access_code(&self, bam_id: &str) -> bool {
        self(bam_id)
    }
}

/// The printer `asset` is, or everything wrong with it.
pub fn printer_from(
    asset: &Asset,
    map: &FieldMap,
    secrets: &impl Secrets,
) -> Result<PrinterConfig, Vec<Problem>> {
    let label = |key: &str| {
        asset
            .effective_fields
            .iter()
            .find(|e| e.field.key == key)
            .map(|e| format!("{} ({key})", e.field.label))
            .unwrap_or_else(|| key.to_owned())
    };
    let value = |key: &str| {
        asset
            .field_values
            .get(key)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
    };
    let mut problems = Vec::new();
    let mut problem = |key: &str, p: String| {
        problems.push(Problem {
            field: label(key),
            problem: p,
        })
    };

    let address = value(&map.address);
    let connection = match value(&map.connection) {
        None => {
            problem(
                &map.connection,
                "not set: say how POP reaches this printer".into(),
            );
            None
        }
        Some(kind) => {
            let kind = kind.to_ascii_lowercase();
            let addressed = |problem: &mut dyn FnMut(&str, String), what: &str| {
                if address.is_none() {
                    problem(&map.address, format!("not set: a {what} printer needs one"));
                }
                address
            };
            match kind.as_str() {
                "moonraker" => {
                    addressed(&mut problem, "Moonraker").and_then(|a| match moonraker_url(a) {
                        Ok(url) => Some(Connection::Moonraker { url }),
                        Err(e) => {
                            problem(&map.address, e);
                            None
                        }
                    })
                }
                "octoprint" | "prusalink" => {
                    let what = if kind == "octoprint" {
                        "OctoPrint"
                    } else {
                        "PrusaLink"
                    };
                    addressed(&mut problem, what).and_then(|a| match http_url(a) {
                        Ok(url) if kind == "octoprint" => Some(Connection::OctoPrint { url }),
                        Ok(url) => Some(Connection::PrusaLink { url }),
                        Err(e) => {
                            problem(&map.address, e);
                            None
                        }
                    })
                }
                "bambu" => {
                    let host = addressed(&mut problem, "Bambu").and_then(|a| {
                        if a.contains("://") || a.contains('/') || a.contains(' ') {
                            problem(
                                &map.address,
                                format!("{a:?}: a Bambu printer's address is its IP address or name alone"),
                            );
                            None
                        } else {
                            Some(a.to_owned())
                        }
                    });
                    let serial = match value(&map.serial) {
                        None => {
                            problem(
                                &map.serial,
                                "not set: a Bambu printer needs its serial number".into(),
                            );
                            None
                        }
                        Some(s) if s.contains(char::is_whitespace) => {
                            problem(&map.serial, format!("{s:?} has spaces in it"));
                            None
                        }
                        Some(s) => Some(s.to_owned()),
                    };
                    if !secrets.has_access_code(&asset.asset.id) {
                        // Not a BAM! field: the label is the name itself.
                        problem(
                            "LAN access code",
                            "not entered in POP yet: it lives in POP, never in BAM!".into(),
                        );
                    }
                    match (host, serial) {
                        (Some(host), Some(serial)) => Some(Connection::Bambu { host, serial }),
                        _ => None,
                    }
                }
                "usb_serial" => addressed(&mut problem, "USB").and_then(|a| {
                    if a.starts_with('/') {
                        Some(Connection::UsbSerial {
                            device: a.to_owned(),
                        })
                    } else {
                        problem(
                            &map.address,
                            format!(
                                "{a:?}: a USB printer's address is its device, such as /dev/ttyACM0"
                            ),
                        );
                        None
                    }
                }),
                other => {
                    problem(
                        &map.connection,
                        format!(
                            "{other:?} is not one POP knows: use one of {}",
                            CONNECTIONS.join(", ")
                        ),
                    );
                    None
                }
            }
        }
    };
    let camera = match value(&map.camera) {
        None => None,
        Some(c)
            if ["rtsp://", "rtsps://", "http://", "https://"]
                .iter()
                .any(|s| c.starts_with(s)) =>
        {
            Some(c.to_owned())
        }
        Some(c) => {
            problem(
                &map.camera,
                format!("{c:?} is not a camera address POP can show: rtsp, rtsps, http or https"),
            );
            None
        }
    };
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(PrinterConfig {
        bam_id: asset.asset.id.clone(),
        name: asset.asset.name.clone(),
        connection: connection.expect("no problems means a connection"),
        camera,
        model: value(&map.model).map(str::to_owned),
        materials: value(&map.materials)
            .map(|m| {
                m.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        training: value(&map.training).map(str::to_owned),
    })
}

/// A Moonraker WebSocket address from what an admin is likely to type.
fn moonraker_url(a: &str) -> Result<String, String> {
    let (scheme, rest) = match a.split_once("://") {
        Some(("ws" | "wss", _)) => return Ok(a.to_owned()),
        Some(("http", r)) => ("ws", r),
        Some(("https", r)) => ("wss", r),
        Some((s, _)) => {
            return Err(format!(
                "{a:?}: a Moonraker address is ws://, wss://, http:// or https://, not {s}://"
            ));
        }
        None => ("ws", a),
    };
    let host = rest.trim_end_matches('/');
    if host.is_empty() || host.contains(' ') {
        return Err(format!("{a:?} is not an address"));
    }
    let with_port = if host.contains(':') || a.contains("://") {
        host.to_owned()
    } else {
        format!("{host}:7125")
    };
    Ok(if with_port.ends_with("/websocket") {
        format!("{scheme}://{with_port}")
    } else {
        format!("{scheme}://{with_port}/websocket")
    })
}

/// An HTTP address, with http:// assumed when none is given.
fn http_url(a: &str) -> Result<String, String> {
    match a.split_once("://") {
        Some(("http" | "https", rest)) if !rest.is_empty() && !rest.contains(' ') => {
            Ok(a.trim_end_matches('/').to_owned())
        }
        Some((s, _)) => Err(format!("{a:?}: use http:// or https://, not {s}://")),
        None if !a.contains(' ') => Ok(format!("http://{}", a.trim_end_matches('/'))),
        None => Err(format!("{a:?} is not an address")),
    }
}
