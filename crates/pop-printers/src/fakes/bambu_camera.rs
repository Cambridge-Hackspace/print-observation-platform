//! A fake Bambu camera (#33): the A1 and P1 series' picture stream on port
//! 6000, over TLS with a self-signed certificate, on a real port, as
//! `bambu_camera`'s sources describe it.
//!
//! It takes the 80-byte sign-in, closes at once with nothing sent when the
//! user name or access code is wrong (as the printer does), and then sends a
//! picture every `every`, each a 16-byte header and the JPEG, split into
//! pieces of `split` bytes as a network may split them. Faults: a header
//! claiming a picture too large, a picture that is not a JPEG, the
//! connection cut mid-picture, and a different certificate.
//!
//! Started with [`FakeBambuCamera::start_rtsps`], it is instead an X1's
//! RTSPS port: TLS, answering each RTSP request with a 200 that names it,
//! and keeping the requests, so a test can see what came through a relay.

use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};
use tokio_rustls::TlsAcceptor;

use crate::bambu::fingerprint;

/// A small, well-formed JPEG: what the fake camera sends.
pub const JPEG: &[u8] = &[
    0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00, 0x01,
    0x00, 0x01, 0x00, 0x00, 0xff, 0xd9,
];

/// What the fake sends after signing in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sends {
    /// Pictures, as a working camera does.
    Pictures,
    /// A header claiming this many bytes, then bytes without end.
    TooLarge(u32),
    /// A header and a body that is not a JPEG.
    NotJpeg,
    /// Half a picture, then the connection closed.
    CutOff,
}

struct Shared {
    access_code: String,
    acceptor: Mutex<(TlsAcceptor, String)>,
    sends: Mutex<Sends>,
    split: Mutex<usize>,
    every: Duration,
    signed_in: AtomicU32,
    watching: AtomicU32,
    rtsp: bool,
    rtsp_requests: Mutex<Vec<String>>,
}

/// A fake Bambu camera, listening until dropped.
pub struct FakeBambuCamera {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Drop for FakeBambuCamera {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn certificate(serial: &str) -> (TlsAcceptor, String) {
    let ck = rcgen::generate_simple_self_signed(vec![serial.to_owned()]).expect("a certificate");
    let der: CertificateDer<'static> = ck.cert.der().clone();
    let fp = fingerprint(der.as_ref());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ck.key_pair.serialize_der()));
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("protocol versions")
    .with_no_client_auth()
    .with_single_cert(vec![der], key)
    .expect("a server config");
    (TlsAcceptor::from(Arc::new(cfg)), fp)
}

/// One picture as the camera frames it.
pub fn framed(jpeg: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(16 + jpeg.len());
    f.extend_from_slice(&u32::try_from(jpeg.len()).unwrap_or(u32::MAX).to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&1u32.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(jpeg);
    f
}

impl FakeBambuCamera {
    /// Start one on a free port, sending a picture every `every`.
    pub async fn start(serial: &str, access_code: &str, every: Duration) -> Self {
        Self::start_as(serial, access_code, every, false).await
    }

    /// Start an X1's RTSPS port instead, on a free port.
    pub async fn start_rtsps(serial: &str) -> Self {
        Self::start_as(serial, "", Duration::ZERO, true).await
    }

    /// Every RTSP request's first line, oldest first.
    pub fn rtsp_requests(&self) -> Vec<String> {
        self.shared.rtsp_requests.lock().unwrap().clone()
    }

    async fn start_as(serial: &str, access_code: &str, every: Duration, rtsp: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let addr = listener.local_addr().expect("its address");
        let shared = Arc::new(Shared {
            access_code: access_code.to_owned(),
            acceptor: Mutex::new(certificate(serial)),
            sends: Mutex::new(Sends::Pictures),
            split: Mutex::new(4096),
            every,
            signed_in: AtomicU32::new(0),
            watching: AtomicU32::new(0),
            rtsp,
            rtsp_requests: Mutex::new(Vec::new()),
        });
        let s = shared.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let acceptor = s.acceptor.lock().unwrap().0.clone();
                let s = s.clone();
                tokio::spawn(async move {
                    if let Ok(tls) = acceptor.accept(tcp).await {
                        if s.rtsp {
                            serve_rtsp(tls, s).await;
                        } else {
                            serve(tls, s).await;
                        }
                    }
                });
            }
        });
        Self { addr, shared, task }
    }

    pub fn host(&self) -> String {
        self.addr.ip().to_string()
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// The fingerprint of the certificate it presents now.
    pub fn fingerprint(&self) -> String {
        self.shared.acceptor.lock().unwrap().1.clone()
    }

    /// Present a new certificate from now on.
    pub fn new_certificate(&self, serial: &str) {
        *self.shared.acceptor.lock().unwrap() = certificate(serial);
    }

    /// What it sends to whoever signs in next.
    pub fn sends(&self, what: Sends) {
        *self.shared.sends.lock().unwrap() = what;
    }

    /// Split what it sends into pieces of this many bytes.
    pub fn split(&self, bytes: usize) {
        *self.shared.split.lock().unwrap() = bytes.max(1);
    }

    /// How many sign-ins it has accepted.
    pub fn signed_in(&self) -> u32 {
        self.shared.signed_in.load(Ordering::SeqCst)
    }

    /// How many are connected and signed in now.
    pub fn watching(&self) -> u32 {
        self.shared.watching.load(Ordering::SeqCst)
    }
}

struct Watching(Arc<Shared>);

impl Drop for Watching {
    fn drop(&mut self) {
        self.0.watching.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn serve(mut tls: tokio_rustls::server::TlsStream<tokio::net::TcpStream>, s: Arc<Shared>) {
    let mut hello = [0u8; 80];
    if tls.read_exact(&mut hello).await.is_err() {
        return;
    }
    let mut want = [0u8; 80];
    want[0..4].copy_from_slice(&0x40u32.to_le_bytes());
    want[4..8].copy_from_slice(&0x3000u32.to_le_bytes());
    want[16..20].copy_from_slice(b"bblp");
    let code = s.access_code.as_bytes();
    want[48..48 + code.len()].copy_from_slice(code);
    if hello != want {
        // As the printer does: closed, nothing sent.
        let _ = tls.shutdown().await;
        return;
    }
    s.signed_in.fetch_add(1, Ordering::SeqCst);
    s.watching.fetch_add(1, Ordering::SeqCst);
    let _watching = Watching(s.clone());
    loop {
        let sends = *s.sends.lock().unwrap();
        let bytes = match sends {
            Sends::Pictures => framed(JPEG),
            Sends::NotJpeg => framed(b"this is not a picture"),
            Sends::CutOff => {
                let f = framed(JPEG);
                let _ = tls.write_all(&f[..f.len() / 2]).await;
                let _ = tls.shutdown().await;
                return;
            }
            Sends::TooLarge(n) => {
                let mut f = n.to_le_bytes().to_vec();
                f.extend_from_slice(&[0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
                if tls.write_all(&f).await.is_err() {
                    return;
                }
                // Bytes without end, until the reader gives up.
                let junk = vec![0xab; 64 * 1024];
                loop {
                    if tls.write_all(&junk).await.is_err() {
                        return;
                    }
                }
            }
        };
        let split = *s.split.lock().unwrap();
        for piece in bytes.chunks(split) {
            if tls.write_all(piece).await.is_err() || tls.flush().await.is_err() {
                return;
            }
        }
        tokio::time::sleep(s.every).await;
    }
}

async fn serve_rtsp(
    mut tls: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    s: Arc<Shared>,
) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let Ok(n) = tls.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        while let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let request = String::from_utf8_lossy(&buf[..end]).into_owned();
            buf.drain(..end + 4);
            let first = request.lines().next().unwrap_or_default().to_owned();
            let cseq = request
                .lines()
                .find_map(|l| l.strip_prefix("CSeq: "))
                .unwrap_or("0")
                .to_owned();
            s.rtsp_requests.lock().unwrap().push(first.clone());
            let reply =
                format!("RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nServer: fake-bambu ({first})\r\n\r\n");
            if tls.write_all(reply.as_bytes()).await.is_err() {
                return;
            }
        }
    }
}
