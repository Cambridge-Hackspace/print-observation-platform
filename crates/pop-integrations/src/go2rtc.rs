//! A client for go2rtc's API (#66), as of go2rtc 1.9.15.
//!
//! go2rtc runs as its own service beside POP (ADR 0001). POP tells it which
//! cameras to serve, one stream per printer, and asks it for a picture from
//! each to say whether the camera works. Viewers never reach go2rtc directly
//! (#67).
//!
//! What go2rtc does that the client works with, recorded from a real one
//! (tests/recordings/go2rtc-1.9.15, made by tools/go2rtc/record.py):
//!
//! - `PUT /api/streams?name=&src=` adds a stream, or replaces one of the same
//!   name. go2rtc writes it into its own configuration file, so it outlives a
//!   restart, and `DELETE /api/streams?src=<name>` takes it out again. A
//!   delete of a stream go2rtc does not have still answers 200.
//! - A source that runs a program (`exec:`) is refused with a 400.
//! - The user name and password are not asked of requests from go2rtc's own
//!   machine unless `local_auth: true` is set (docs/integrations/go2rtc.md).
//! - `GET /api/frame.jpeg?src=<name>` answers 200 with a JPEG when the camera
//!   works, 200 with *nothing* when it does not, and 404 when there is no
//!   such stream. The status alone does not say whether the camera works.
//!
//! Watching (#67), recorded the same way:
//!
//! - MSE is a WebSocket, `/api/ws?src=<name>`: the client sends
//!   `{"type":"mse","value":"<codecs it can play>"}`, and go2rtc answers
//!   `{"type":"mse","value":"<MIME type>"}`, then the video as binary MP4
//!   fragments (`ftyp`, then `moof` after `moof`). Or it answers
//!   `{"type":"error","value":...}`: a camera that sends JPEG cannot be played
//!   this way ("codecs not matched"), and the other errors can hold the
//!   camera's address.
//! - `/api/stream.mjpeg?src=<name>` is the MJPEG stream: a JPEG camera's
//!   pictures, `multipart/x-mixed-replace`. A camera that sends H.264, or none
//!   at all, is a 200 with nothing; no such stream is a 404.
//!
//! A camera's address may carry its password, so no error this client makes
//! includes a request's address, or what go2rtc said went wrong in a stream.

use std::{collections::BTreeMap, time::Duration};

use futures_util::{SinkExt, StreamExt};
use reqwest::{Method, StatusCode, header};
use serde::Deserialize;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest, http::HeaderValue},
};

/// Why a request to go2rtc failed, in words that say what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Go2rtcError {
    /// go2rtc refused POP's user name or password.
    Auth,
    /// go2rtc would not take what POP asked of it, and said why.
    Refused(String),
    /// go2rtc could not be reached at all.
    Unreachable(String),
    /// go2rtc answered, but not with what POP expected.
    Unexpected(String),
}

impl std::fmt::Display for Go2rtcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth => write!(
                f,
                "go2rtc refused POP's user name and password; set [go2rtc] username and POP_GO2RTC_PASSWORD to go2rtc's api username and password"
            ),
            Self::Refused(m) => write!(f, "go2rtc refused: {m}"),
            Self::Unreachable(m) => write!(f, "go2rtc cannot be reached: {m}"),
            Self::Unexpected(m) => write!(f, "go2rtc answered unexpectedly: {m}"),
        }
    }
}

impl std::error::Error for Go2rtcError {}

/// What go2rtc gave when asked for a picture from a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picture {
    /// A JPEG, this many bytes: the camera works.
    Jpeg(usize),
    /// Nothing: go2rtc has the stream but could not get a picture from it.
    Nothing,
    /// go2rtc has no such stream.
    NoStream,
}

#[derive(Deserialize)]
struct Info {
    version: String,
}

#[derive(Deserialize)]
struct Stream {
    #[serde(default)]
    producers: Option<Vec<Producer>>,
}

#[derive(Deserialize)]
struct Producer {
    #[serde(default)]
    url: Option<String>,
}

/// How an MSE stream began.
pub enum Mse {
    /// The video, in this MIME type, follows.
    Playing { mime: String, media: Media },
    /// The camera sends what MSE cannot carry (JPEG): watch it as MJPEG.
    Unsupported,
    /// go2rtc could not get a picture from the camera, or has no such stream.
    NoPicture,
}

/// An MSE stream's video: binary MP4 fragments, as go2rtc sends them.
pub struct Media {
    ws: Box<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>>,
}

impl Media {
    /// The next fragment; `None` when go2rtc ends the stream.
    pub async fn next(&mut self) -> Option<Vec<u8>> {
        while let Some(m) = self.ws.next().await {
            match m.ok()? {
                Message::Binary(b) => return Some(b.to_vec()),
                Message::Close(_) => return None,
                // A later error from go2rtc ends the video; what it says may
                // hold the camera's address, so it goes no further.
                Message::Text(_) => return None,
                _ => {}
            }
        }
        None
    }
}

/// How an MJPEG stream began.
pub enum Mjpeg {
    /// The pictures follow: a `multipart/x-mixed-replace` body of this type.
    Playing {
        content_type: String,
        body: reqwest::Response,
    },
    /// go2rtc has the stream, but no JPEG pictures from it.
    NoPicture,
    /// go2rtc has no such stream.
    NoStream,
}

impl Mjpeg {
    /// The next piece of a playing stream's body.
    pub async fn chunk(body: &mut reqwest::Response) -> Option<Vec<u8>> {
        body.chunk().await.ok().flatten().map(|b| b.to_vec())
    }
}

/// The longest go2rtc may be silent in a stream before POP gives up on it.
const STREAM_SILENCE: Duration = Duration::from_secs(20);

/// A client for one go2rtc.
#[derive(Clone)]
pub struct Go2rtc {
    base: String,
    auth: Option<(String, String)>,
    http: reqwest::Client,
    /// For streams, which have no end: only a connect and a silence limit.
    streams: reqwest::Client,
}

impl Go2rtc {
    /// `base` is go2rtc's API address, `http://127.0.0.1:1984`; `auth` its
    /// user name and password, when it has them.
    pub fn new(
        base: impl Into<String>,
        auth: Option<(String, String)>,
    ) -> Result<Self, Go2rtcError> {
        let http = reqwest::Client::builder()
            // A picture can take a camera a few seconds to give.
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("POP/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Go2rtcError::Unexpected(format!("cannot make an HTTP client: {e}")))?;
        let streams = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(STREAM_SILENCE)
            .user_agent(concat!("POP/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Go2rtcError::Unexpected(format!("cannot make an HTTP client: {e}")))?;
        Ok(Self {
            base: base.into().trim_end_matches('/').to_owned(),
            auth,
            http,
            streams,
        })
    }

    /// Ask go2rtc something: its status, content type and body.
    async fn call(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<(StatusCode, String, Vec<u8>), Go2rtcError> {
        let mut url = reqwest::Url::parse(&format!("{}{path}", self.base)).map_err(|e| {
            Go2rtcError::Unreachable(format!("{} is not an address: {e}", self.base))
        })?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        let mut req = self.http.request(method, url);
        if let Some((user, password)) = &self.auth {
            req = req.basic_auth(user, Some(password));
        }
        // The request's address carries the camera's, so errors leave it out.
        let res = req
            .send()
            .await
            .map_err(|e| Go2rtcError::Unreachable(format!("{}: {}", self.base, e.without_url())))?;
        let status = res.status();
        let ctype = res
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = res
            .bytes()
            .await
            .map_err(|e| {
                Go2rtcError::Unreachable(format!("the answer was cut off: {}", e.without_url()))
            })?
            .to_vec();
        if status == StatusCode::UNAUTHORIZED {
            return Err(Go2rtcError::Auth);
        }
        Ok((status, ctype, body))
    }

    fn said(body: &[u8]) -> String {
        String::from_utf8_lossy(body).trim().to_owned()
    }

    /// Whether POP gives go2rtc a user name and password.
    pub fn has_password(&self) -> bool {
        self.auth.is_some()
    }

    /// Whether go2rtc answers a request with no password at all: as it does
    /// for its own machine when `local_auth` is off, whatever password it has
    /// (#149). Then anything on the box can change its streams.
    pub async fn answers_without_password(&self) -> Result<bool, Go2rtcError> {
        let bare = Self {
            auth: None,
            ..self.clone()
        };
        match bare.call(Method::GET, "/api", &[]).await {
            Ok((s, _, _)) if s.is_success() => Ok(true),
            Ok((s, _, _)) => Err(Go2rtcError::Unexpected(format!("/api: HTTP {s}"))),
            Err(Go2rtcError::Auth) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// go2rtc's version: whether it answers, and lets POP in.
    pub async fn version(&self) -> Result<String, Go2rtcError> {
        let (status, _, body) = self.call(Method::GET, "/api", &[]).await?;
        if !status.is_success() {
            return Err(Go2rtcError::Unexpected(format!("/api: HTTP {status}")));
        }
        serde_json::from_slice::<Info>(&body)
            .map(|i| i.version)
            .map_err(|e| Go2rtcError::Unexpected(format!("/api: not the JSON POP expects: {e}")))
    }

    /// Every stream go2rtc has, with its sources.
    pub async fn streams(&self) -> Result<BTreeMap<String, Vec<String>>, Go2rtcError> {
        let (status, _, body) = self.call(Method::GET, "/api/streams", &[]).await?;
        if !status.is_success() {
            return Err(Go2rtcError::Unexpected(format!(
                "/api/streams: HTTP {status}"
            )));
        }
        let streams: BTreeMap<String, Stream> = serde_json::from_slice(&body).map_err(|e| {
            Go2rtcError::Unexpected(format!("/api/streams: not the JSON POP expects: {e}"))
        })?;
        Ok(streams
            .into_iter()
            .map(|(name, s)| {
                let sources = s
                    .producers
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|p| p.url)
                    .collect();
                (name, sources)
            })
            .collect())
    }

    /// Serve `source` as the stream `name`, in place of whatever it was.
    pub async fn put(&self, name: &str, source: &str) -> Result<(), Go2rtcError> {
        let (status, _, body) = self
            .call(
                Method::PUT,
                "/api/streams",
                &[("name", name), ("src", source)],
            )
            .await?;
        match status {
            s if s.is_success() => Ok(()),
            StatusCode::BAD_REQUEST => Err(Go2rtcError::Refused(Self::said(&body))),
            s => Err(Go2rtcError::Unexpected(format!("adding {name}: HTTP {s}"))),
        }
    }

    /// Stop serving the stream `name`; done already if go2rtc has no such stream.
    pub async fn delete(&self, name: &str) -> Result<(), Go2rtcError> {
        let (status, _, _) = self
            .call(Method::DELETE, "/api/streams", &[("src", name)])
            .await?;
        if status.is_success() {
            Ok(())
        } else {
            Err(Go2rtcError::Unexpected(format!(
                "removing {name}: HTTP {status}"
            )))
        }
    }

    /// A picture from the stream `name`, which says whether its camera works.
    pub async fn picture(&self, name: &str) -> Result<Picture, Go2rtcError> {
        let (status, ctype, body) = self
            .call(Method::GET, "/api/frame.jpeg", &[("src", name)])
            .await?;
        match status {
            StatusCode::NOT_FOUND => Ok(Picture::NoStream),
            s if s.is_success() => {
                if body.is_empty() {
                    Ok(Picture::Nothing)
                } else if ctype.starts_with("image/jpeg") && body.starts_with(&[0xff, 0xd8]) {
                    Ok(Picture::Jpeg(body.len()))
                } else {
                    Err(Go2rtcError::Unexpected(format!(
                        "a picture from {name} that is not a JPEG ({ctype:?}, {} bytes)",
                        body.len()
                    )))
                }
            }
            s => Err(Go2rtcError::Unexpected(format!(
                "a picture from {name}: HTTP {s}"
            ))),
        }
    }

    /// Watch the stream `name` as MSE, offering the codecs the viewer can play.
    pub async fn mse(&self, name: &str, codecs: &str) -> Result<Mse, Go2rtcError> {
        let mut url = reqwest::Url::parse(&format!("{}/api/ws", self.base)).map_err(|e| {
            Go2rtcError::Unreachable(format!("{} is not an address: {e}", self.base))
        })?;
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme)
            .map_err(|()| Go2rtcError::Unexpected("cannot make a WebSocket address".into()))?;
        url.query_pairs_mut().append_pair("src", name);
        let mut req = url
            .as_str()
            .into_client_request()
            .map_err(|e| Go2rtcError::Unexpected(format!("the WebSocket request: {e}")))?;
        if let Some((user, password)) = &self.auth {
            use base64::Engine as _;
            let token =
                base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
            req.headers_mut().insert(
                "Authorization",
                HeaderValue::from_str(&format!("Basic {token}")).map_err(|_| {
                    Go2rtcError::Unexpected("the password is not a valid header".into())
                })?,
            );
        }
        let connect = tokio_tungstenite::connect_async_tls_with_config(
            req,
            None,
            false,
            Some(crate::sync::connector()),
        );
        let (mut ws, _) = tokio::time::timeout(Duration::from_secs(10), connect)
            .await
            .map_err(|_| Go2rtcError::Unreachable(format!("{}: no answer", self.base)))?
            .map_err(|e| match e {
                tokio_tungstenite::tungstenite::Error::Http(r) if r.status() == 401 => {
                    Go2rtcError::Auth
                }
                tokio_tungstenite::tungstenite::Error::Http(r) => {
                    Go2rtcError::Unexpected(format!("watching {name}: HTTP {}", r.status()))
                }
                _ => Go2rtcError::Unreachable(format!("{}: the WebSocket did not open", self.base)),
            })?;
        let ask = serde_json::json!({"type": "mse", "value": codecs}).to_string();
        ws.send(Message::Text(ask.into()))
            .await
            .map_err(|_| Go2rtcError::Unreachable("go2rtc closed the WebSocket".into()))?;
        let first = tokio::time::timeout(STREAM_SILENCE, async {
            while let Some(m) = ws.next().await {
                match m {
                    Ok(Message::Text(t)) => return Some(t.to_string()),
                    Ok(Message::Close(_)) | Err(_) => return None,
                    Ok(_) => {}
                }
            }
            None
        })
        .await
        .ok()
        .flatten();
        let Some(first) = first else {
            return Ok(Mse::NoPicture);
        };
        let v: serde_json::Value = serde_json::from_str(&first).map_err(|_| {
            Go2rtcError::Unexpected(format!("watching {name}: not the JSON POP expects"))
        })?;
        match (v["type"].as_str(), v["value"].as_str()) {
            (Some("mse"), Some(mime)) => Ok(Mse::Playing {
                mime: mime.to_owned(),
                media: Media { ws: Box::new(ws) },
            }),
            (Some("error"), Some(why)) if why.contains("codecs not matched") => {
                Ok(Mse::Unsupported)
            }
            (Some("error"), _) => Ok(Mse::NoPicture),
            _ => Err(Go2rtcError::Unexpected(format!(
                "watching {name}: go2rtc began with {:?}",
                v["type"]
            ))),
        }
    }

    /// Watch the stream `name` as MJPEG.
    pub async fn mjpeg(&self, name: &str) -> Result<Mjpeg, Go2rtcError> {
        let mut url =
            reqwest::Url::parse(&format!("{}/api/stream.mjpeg", self.base)).map_err(|e| {
                Go2rtcError::Unreachable(format!("{} is not an address: {e}", self.base))
            })?;
        url.query_pairs_mut().append_pair("src", name);
        let mut req = self.streams.get(url);
        if let Some((user, password)) = &self.auth {
            req = req.basic_auth(user, Some(password));
        }
        let res = req
            .send()
            .await
            .map_err(|e| Go2rtcError::Unreachable(format!("{}: {}", self.base, e.without_url())))?;
        let ctype = res
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        match res.status() {
            StatusCode::UNAUTHORIZED => Err(Go2rtcError::Auth),
            StatusCode::NOT_FOUND => Ok(Mjpeg::NoStream),
            s if s.is_success() && ctype.starts_with("multipart/x-mixed-replace") => {
                Ok(Mjpeg::Playing {
                    content_type: ctype,
                    body: res,
                })
            }
            s if s.is_success() => Ok(Mjpeg::NoPicture),
            s => Err(Go2rtcError::Unexpected(format!(
                "watching {name}: HTTP {s}"
            ))),
        }
    }
}
