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
//! A camera's address may carry its password, so no error this client makes
//! includes a request's address.

use std::{collections::BTreeMap, time::Duration};

use reqwest::{Method, StatusCode, header};
use serde::Deserialize;

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

/// A client for one go2rtc.
#[derive(Clone)]
pub struct Go2rtc {
    base: String,
    auth: Option<(String, String)>,
    http: reqwest::Client,
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
        Ok(Self {
            base: base.into().trim_end_matches('/').to_owned(),
            auth,
            http,
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
}
