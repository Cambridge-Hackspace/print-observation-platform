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
    /// The CS:S tool whose training this printer needs, by its id or its
    /// external id, where training comes from CS:S (#17).
    pub css_tool: String,
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
            css_tool: "css_tool".into(),
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

/// A printer's camera, as BAM! gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Camera {
    /// An address go2rtc plays: rtsp, rtsps, http or https.
    Address { url: String },
    /// A Bambu printer's own camera, RTSPS on port 322: the X1 series, P2S,
    /// and the H2 series with LAN Only Liveview on (OpenBambuAPI `video.md`).
    BambuRtsps,
    /// A Bambu printer's own camera, JPEG pictures on port 6000: the A1 and
    /// P1 series (the same source).
    BambuJpeg,
}

/// What a Bambu model's own camera is, from its model as BAM! has it, by
/// OpenBambuAPI's `video.md`. `None` for a model POP does not know.
pub fn bambu_camera(model: &str) -> Option<Camera> {
    let m: String = model
        .to_ascii_uppercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    let m = m.strip_prefix("BAMBULAB").unwrap_or(&m);
    if ["X1", "P2S", "H2"].iter().any(|p| m.starts_with(p)) {
        Some(Camera::BambuRtsps)
    } else if ["A1", "P1"].iter().any(|p| m.starts_with(p)) {
        Some(Camera::BambuJpeg)
    } else {
        None
    }
}

/// The value of the camera field that means a Bambu printer's own camera.
pub const BUILT_IN: &str = "built-in";

/// A printer as POP runs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrinterConfig {
    pub bam_id: String,
    pub name: String,
    pub connection: Connection,
    pub camera: Option<Camera>,
    pub model: Option<String>,
    pub materials: Vec<String>,
    pub training: Option<String>,
    /// The CS:S tool whose training it needs (#17).
    pub css_tool: Option<String>,
}

/// What training a printer needs (#17): its training level, and its CS:S
/// tool, from BAM! alone. A printer whose other fields are wrong still needs
/// what these say.
pub fn needs(asset: &Asset, map: &FieldMap) -> (Option<String>, Option<String>) {
    let value = |key: &str| {
        asset
            .field_values
            .get(key)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    (value(&map.training), value(&map.css_tool))
}

/// One thing wrong with a printer, naming the field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    /// The field as BAM! labels it, with its key: "Connection (connection)".
    pub field: String,
    pub problem: String,
    /// Fixed in POP, not BAM!: a secret POP keeps (#150).
    pub in_pop: bool,
}

/// What a missing access code is called: not a BAM! field, so the label is
/// the name itself, and it is fixed in POP.
pub const ACCESS_CODE: &str = "LAN access code";

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
            in_pop: key == ACCESS_CODE,
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
                            ACCESS_CODE,
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
    // A Bambu printer's own address, when it is one.
    let bambu_host = match &connection {
        Some(Connection::Bambu { host, .. }) => Some(host.clone()),
        _ => None,
    };
    let model = value(&map.model);
    let camera = match value(&map.camera) {
        None => None,
        Some(c) if c.eq_ignore_ascii_case(BUILT_IN) => match (&bambu_host, model) {
            (None, _) => {
                problem(
                    &map.camera,
                    format!(
                        "{BUILT_IN:?} is for a Bambu printer's own camera; give this printer's camera its address"
                    ),
                );
                None
            }
            (Some(_), m) => match m.and_then(bambu_camera) {
                Some(kind) => Some(kind),
                None => {
                    problem(
                        &map.camera,
                        format!(
                            "{BUILT_IN:?} needs a Bambu model POP knows in {}: X1, P2S, H2S, H2D, A1 or P1",
                            label(&map.model)
                        ),
                    );
                    None
                }
            },
        },
        // A camera's password is kept sealed in POP (#150), never in BAM!.
        Some(c) if url_password(c) => {
            problem(
                &map.camera,
                "holds the camera's password: give the address without it here, and enter the camera's login in POP".to_owned(),
            );
            None
        }
        // A Bambu printer's own RTSPS camera, by its address: POP signs in
        // to it with the access code, and checks its certificate.
        Some(c)
            if c.starts_with("rtsps://")
                && bambu_host.as_deref().is_some_and(|h| url_host(c) == h) =>
        {
            Some(Camera::BambuRtsps)
        }
        Some(c)
            if ["rtsp://", "rtsps://", "http://", "https://"]
                .iter()
                .any(|s| c.starts_with(s)) =>
        {
            Some(Camera::Address { url: c.to_owned() })
        }
        Some(c) => {
            problem(
                &map.camera,
                format!(
                    "{c:?} is not a camera address POP can show: rtsp, rtsps, http or https, or {BUILT_IN:?} for a Bambu printer's own"
                ),
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
        model: model.map(str::to_owned),
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
        css_tool: value(&map.css_tool).map(str::to_owned),
    })
}

/// Whether `url` carries a password: `scheme://user:password@host/...`.
fn url_password(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    authority
        .rsplit_once('@')
        .is_some_and(|(user, _)| user.contains(':'))
}

/// The host of `url`, without its user, port or path.
fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let rest = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
    match rest.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => rest,
    }
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
