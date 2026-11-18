//! Keeping printers in sync with BAM! (#22).
//!
//! POP opens BAM!'s live connection (`GET /api/v1/events/live`, a
//! WebSocket), filtered to the printer category and the kinds that change a
//! printer: created, edited, archived, restored. BAM! does not replay what a
//! connection missed, so POP re-reads the whole category every time it
//! connects, and every so often as a backstop. It subscribes first and reads
//! second, so a change made between the two is seen.
//!
//! An event is only a cue to read again: BAM!'s category filter takes in
//! subcategories, so an event's category does not make its asset a printer.
//! What is a printer is what the category read says.

use std::{sync::Arc, time::Duration};

use futures_util::StreamExt;
use serde_json::Value;
use tokio::{net::TcpStream, sync::watch, task::JoinHandle};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest, http::HeaderValue},
};

use crate::bam::{Asset, Bam, BamError};

/// The kinds of event that change a printer.
pub const KINDS: &[&str] = &[
    "asset.created",
    "asset.edited",
    "asset.archived",
    "asset.restored",
];

/// What POP knows of the printer category.
#[derive(Debug, Clone, Default)]
pub struct View {
    /// The printers in use, as last read.
    pub printers: Vec<Asset>,
    /// The archived ones, as last read.
    pub archived: Vec<Asset>,
    /// Whether the live connection is open now.
    pub live: bool,
    /// How many times the category has been read.
    pub reads: u64,
    /// What went wrong last, if the last attempt did not work.
    pub error: Option<String>,
}

/// How often to read anyway, and how long to wait before connecting again.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub backstop: Duration,
    pub first_retry: Duration,
    pub max_retry: Duration,
    /// How long BAM! may take to say hello.
    pub hello_within: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            backstop: Duration::from_secs(300),
            first_retry: Duration::from_secs(1),
            max_retry: Duration::from_secs(60),
            hello_within: Duration::from_secs(10),
        }
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// An open live connection, past BAM!'s hello.
pub struct Live {
    ws: Socket,
}

impl Bam {
    /// Open the live connection for `category`'s printer events.
    pub async fn live(&self, category: &str, within: Duration) -> Result<Live, BamError> {
        let ws_base = if let Some(rest) = self.base().strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base().strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            return Err(BamError::Unexpected(format!(
                "{}: not an http(s) address",
                self.base()
            )));
        };
        let url = format!(
            "{ws_base}/api/v1/events/live?kinds={}&categories={category}",
            KINDS.join(",")
        );
        let mut req = url
            .as_str()
            .into_client_request()
            .map_err(|e| BamError::Unexpected(format!("{url}: {e}")))?;
        req.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {}", self.token()))
                .map_err(|_| BamError::Unexpected("the token is not a valid header".into()))?,
        );
        let (mut ws, _) =
            tokio_tungstenite::connect_async_tls_with_config(req, None, false, Some(connector()))
                .await
                .map_err(|e| match e {
                    tokio_tungstenite::tungstenite::Error::Http(r) if r.status() == 401 => {
                        BamError::Token("the live connection was refused".into())
                    }
                    tokio_tungstenite::tungstenite::Error::Http(r) if r.status() == 403 => {
                        BamError::Permission("the live connection needs events.read".into())
                    }
                    other => BamError::Unreachable(format!("the live connection: {other}")),
                })?;
        // BAM! says hello first; until it has, nothing is subscribed.
        let hello = tokio::time::timeout(within, next_json(&mut ws))
            .await
            .map_err(|_| {
                BamError::Unreachable("BAM! did not say hello on the live connection".into())
            })?;
        match hello {
            Some(v) if v["type"] == "hello" => Ok(Live { ws }),
            Some(v) => Err(BamError::Unexpected(format!(
                "the live connection opened with {v}"
            ))),
            None => Err(BamError::Unreachable(
                "the live connection closed before hello".into(),
            )),
        }
    }
}

impl Live {
    /// The next event's kind, or None when the connection has ended.
    pub async fn next_event(&mut self) -> Option<String> {
        loop {
            let v = next_json(&mut self.ws).await?;
            if v["type"] == "event" {
                return Some(v["event"]["kind"].as_str().unwrap_or("").to_owned());
            }
            // An error or anything else BAM! says is not an event: carry on.
        }
    }
}

/// The next JSON text message, or None when the connection ends.
async fn next_json(ws: &mut Socket) -> Option<Value> {
    loop {
        match ws.next().await? {
            Ok(Message::Text(t)) => match serde_json::from_str(&t) {
                Ok(v) => return Some(v),
                Err(_) => continue,
            },
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}

/// TLS with this build's one crypto provider, named rather than looked up:
/// a process with two providers compiled in cannot pick a default.
pub(crate) fn connector() -> Connector {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("TLS 1.2 and 1.3")
    .with_root_certificates(roots)
    .with_no_client_auth();
    Connector::Rustls(Arc::new(cfg))
}

/// Keep `category`'s printers in sync, until the returned task is aborted.
pub fn spawn(bam: Bam, category: String, opts: Options) -> (watch::Receiver<View>, JoinHandle<()>) {
    let (tx, rx) = watch::channel(View::default());
    let task = tokio::spawn(run(bam, category, opts, tx));
    (rx, task)
}

async fn read(bam: &Bam, category: &str, tx: &watch::Sender<View>) -> Result<(), BamError> {
    let printers = bam.assets(category, false).await?;
    let archived = bam.assets(category, true).await?;
    tx.send_modify(|v| {
        v.printers = printers;
        v.archived = archived;
        v.reads += 1;
        v.error = None;
    });
    Ok(())
}

async fn run(bam: Bam, category: String, opts: Options, tx: watch::Sender<View>) {
    let mut wait = opts.first_retry;
    loop {
        match bam.live(&category, opts.hello_within).await {
            Ok(mut live) => {
                // Subscribed: now read, so nothing between the two is missed.
                tx.send_modify(|v| v.live = true);
                if let Err(e) = read(&bam, &category, &tx).await {
                    tx.send_modify(|v| v.error = Some(e.to_string()));
                }
                wait = opts.first_retry;
                let mut backstop = tokio::time::interval(opts.backstop);
                backstop.tick().await;
                loop {
                    tokio::select! {
                        e = live.next_event() => {
                            if e.is_none() {
                                break;
                            }
                            if let Err(e) = read(&bam, &category, &tx).await {
                                tx.send_modify(|v| v.error = Some(e.to_string()));
                            }
                        }
                        _ = backstop.tick() => {
                            if let Err(e) = read(&bam, &category, &tx).await {
                                tx.send_modify(|v| v.error = Some(e.to_string()));
                            }
                        }
                    }
                }
                tx.send_modify(|v| v.live = false);
            }
            Err(e) => {
                // Not connected: what is known stays, said to be stale.
                tx.send_modify(|v| {
                    v.live = false;
                    v.error = Some(e.to_string());
                });
            }
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(opts.max_retry);
    }
}
