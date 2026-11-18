//! A Bambu printer's own camera, on the models that send framed JPEG
//! pictures over TLS on port 6000 (#33): the A1 and P1 series. The X1 and
//! P2S series, and the H2 series once "LAN Only Liveview" is on, serve RTSPS
//! instead, which go2rtc plays (#66).
//!
//! The protocol, from two independent descriptions that agree on it:
//! OpenBambuAPI's `video.md` (Doridian/OpenBambuAPI at cc383a2c) and
//! ha-bambulab's `ChamberImageThread` (greghesp/ha-bambulab at e3c73139,
//! `pybambu/bambu_client.py`, MIT):
//!
//! - The client connects with TLS and sends 80 bytes: `0x40`, `0x3000`, `0`,
//!   `0` as little-endian `u32`s, then the user name `bblp` and the LAN
//!   access code, each right-padded with zero bytes to 32.
//! - The printer then sends each picture as a 16-byte header, whose first
//!   four bytes are the picture's size (little-endian), followed by that
//!   many bytes of JPEG, from `FF D8` to `FF D9`. ha-bambulab reads the size
//!   from three bytes, OpenBambuAPI from four; the fourth is zero for any
//!   picture under 16 MB, so reading four agrees with both.
//! - A wrong access code is a connection closed with nothing sent.
//! - Pictures come every second or two (ha-bambulab).
//!
//! The decoder ([`Decoder`]) is the only part that reads what the printer
//! sends, so it is the part held to never panic and never hold more than
//! [`MAX_PICTURE`] bytes, whatever arrives (tests/bambu_camera.rs).
//!
//! The certificate is pinned as the driver pins the printer's MQTT one, but
//! under its own key (`<serial>#camera`): neither source says whether port
//! 6000 presents the same certificate as port 8883.

use std::{collections::VecDeque, fmt, sync::Arc, time::Duration};

use rustls::pki_types::ServerName;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{TlsConnector, client::TlsStream};

use crate::bambu::{Mismatches, PinStore, pinned_tls};

/// The port the A1 and P1 series serve their camera on.
pub const PORT: u16 = 6000;

/// The largest picture POP takes from a printer. The printers send
/// 1280x720 JPEGs (OpenBambuAPI), a small fraction of this; anything larger
/// is a broken or hostile stream, and the connection is dropped.
pub const MAX_PICTURE: usize = 4 << 20;

/// The header before each picture.
pub const HEADER: usize = 16;

/// How long a printer may send nothing before POP gives up on the camera.
const SILENCE: Duration = Duration::from_secs(10);

/// Why a picture could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// The header claims a picture larger than [`MAX_PICTURE`].
    TooLarge(usize),
    /// The header claims a picture of no bytes.
    Empty,
    /// What came is not a JPEG: no `FF D8` at its start or `FF D9` at its end.
    NotJpeg,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge(n) => write!(f, "the camera sent a picture of {n} bytes, too large"),
            Self::Empty => write!(f, "the camera sent an empty picture"),
            Self::NotJpeg => write!(f, "the camera sent something that is not a JPEG"),
        }
    }
}

/// Reads pictures out of what the printer sends, however it is split.
#[derive(Debug, Default)]
pub struct Decoder {
    header: Vec<u8>,
    /// The picture being read, and the size its header gave.
    body: Option<(usize, Vec<u8>)>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many bytes the decoder holds now. Never more than
    /// [`HEADER`] + [`MAX_PICTURE`].
    pub fn held(&self) -> usize {
        self.header.len() + self.body.as_ref().map_or(0, |(_, b)| b.len())
    }

    /// Take what arrived; the whole pictures it finished. After an error the
    /// stream cannot be trusted again, and the decoder is reset.
    pub fn push(&mut self, mut bytes: &[u8]) -> Result<Vec<Vec<u8>>, FrameError> {
        let mut done = Vec::new();
        while !bytes.is_empty() {
            match &mut self.body {
                None => {
                    let want = HEADER - self.header.len();
                    let take = want.min(bytes.len());
                    self.header.extend_from_slice(&bytes[..take]);
                    bytes = &bytes[take..];
                    if self.header.len() == HEADER {
                        let size = u32::from_le_bytes([
                            self.header[0],
                            self.header[1],
                            self.header[2],
                            self.header[3],
                        ]) as usize;
                        self.header.clear();
                        if size == 0 {
                            *self = Self::default();
                            return Err(FrameError::Empty);
                        }
                        if size > MAX_PICTURE {
                            *self = Self::default();
                            return Err(FrameError::TooLarge(size));
                        }
                        // Grown as bytes arrive, not to what the header claims.
                        self.body = Some((size, Vec::new()));
                    }
                }
                Some((size, body)) => {
                    let take = (*size - body.len()).min(bytes.len());
                    body.extend_from_slice(&bytes[..take]);
                    bytes = &bytes[take..];
                    if body.len() == *size {
                        let picture = std::mem::take(body);
                        self.body = None;
                        if !(picture.starts_with(&[0xff, 0xd8]) && picture.ends_with(&[0xff, 0xd9]))
                        {
                            *self = Self::default();
                            return Err(FrameError::NotJpeg);
                        }
                        done.push(picture);
                    }
                }
            }
        }
        Ok(done)
    }
}

/// The 80 bytes that sign in to the camera.
pub fn sign_in(access_code: &str) -> Result<[u8; 80], CameraError> {
    if access_code.len() > 32 || !access_code.is_ascii() {
        return Err(CameraError::Refused(
            "the access code is not one a printer gives: at most 32 letters and digits".into(),
        ));
    }
    let mut p = [0u8; 80];
    p[0..4].copy_from_slice(&0x40u32.to_le_bytes());
    p[4..8].copy_from_slice(&0x3000u32.to_le_bytes());
    p[16..20].copy_from_slice(b"bblp");
    p[48..48 + access_code.len()].copy_from_slice(access_code.as_bytes());
    Ok(p)
}

/// Why the camera could not be watched, in words that say what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraError {
    /// The printer cannot be reached, or stopped answering.
    Unreachable(String),
    /// The printer closed the connection before sending anything: the access
    /// code is wrong, or the printer is not in LAN mode.
    Refused(String),
    /// The printer's certificate is not the one pinned.
    Certificate { seen: String },
    /// The camera sent something POP will not read.
    Stream(FrameError),
}

impl fmt::Display for CameraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(m) => write!(f, "the camera cannot be reached: {m}"),
            Self::Refused(m) => write!(f, "the camera refused POP: {m}"),
            Self::Certificate { seen } => write!(
                f,
                "the camera's certificate is not the one pinned (it presented {seen}); an admin confirms a replaced printer"
            ),
            Self::Stream(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CameraError {}

/// A Bambu printer's camera on port 6000.
#[derive(Clone)]
pub struct BambuCamera {
    pub host: String,
    pub port: u16,
    pub serial: String,
    pub access_code: String,
    pub pins: Arc<dyn PinStore>,
}

/// Pictures from a camera, as they come.
pub struct Pictures {
    tls: Box<TlsStream<TcpStream>>,
    decoder: Decoder,
    ready: VecDeque<Vec<u8>>,
    got_any: bool,
}

impl BambuCamera {
    /// The key its certificate is pinned under.
    pub fn pin_key(serial: &str) -> String {
        format!("{serial}#camera")
    }

    /// Connect and sign in.
    pub async fn connect(&self) -> Result<Pictures, CameraError> {
        let hello = sign_in(&self.access_code)?;
        let (tls, mismatch) = pinned_tls(&Self::pin_key(&self.serial), self.pins.clone())
            .map_err(|e| CameraError::Unreachable(e.to_string()))?;
        let tcp =
            tokio::time::timeout(SILENCE, TcpStream::connect((self.host.as_str(), self.port)))
                .await
                .map_err(|_| CameraError::Unreachable(format!("{} did not answer", self.host)))?
                .map_err(|e| CameraError::Unreachable(format!("{}: {e}", self.host)))?;
        let name = ServerName::try_from(self.host.clone())
            .map_err(|_| CameraError::Unreachable(format!("{} is not a host name", self.host)))?;
        let mut tls = match tokio::time::timeout(
            SILENCE,
            TlsConnector::from(Arc::new(tls)).connect(name, tcp),
        )
        .await
        {
            Err(_) => {
                return Err(CameraError::Unreachable(format!(
                    "{} did not answer",
                    self.host
                )));
            }
            Ok(Err(e)) => return Err(refused_by_pin(&mismatch, e)),
            Ok(Ok(t)) => t,
        };
        tls.write_all(&hello)
            .await
            .map_err(|e| CameraError::Unreachable(e.to_string()))?;
        Ok(Pictures {
            tls: Box::new(tls),
            decoder: Decoder::new(),
            ready: VecDeque::new(),
            got_any: false,
        })
    }
}

fn refused_by_pin(mismatch: &Mismatches, e: std::io::Error) -> CameraError {
    match mismatch.lock().unwrap().take() {
        Some(m) => CameraError::Certificate { seen: m.seen },
        None => CameraError::Unreachable(e.to_string()),
    }
}

impl Pictures {
    /// The next whole picture, a JPEG.
    pub async fn next(&mut self) -> Result<Vec<u8>, CameraError> {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            if let Some(p) = self.ready.pop_front() {
                return Ok(p);
            }
            let n = tokio::time::timeout(SILENCE, self.tls.read(&mut buf))
                .await
                .map_err(|_| CameraError::Unreachable("the camera stopped sending".into()))?
                .map_err(|e| CameraError::Unreachable(e.to_string()))?;
            if n == 0 {
                return Err(if self.got_any {
                    CameraError::Unreachable("the camera closed the connection".into())
                } else {
                    CameraError::Refused(
                        "it closed the connection at once: check the access code, and that the printer is in LAN mode".into(),
                    )
                });
            }
            self.got_any = true;
            let pictures = self.decoder.push(&buf[..n]).map_err(CameraError::Stream)?;
            self.ready.extend(pictures);
        }
    }
}
