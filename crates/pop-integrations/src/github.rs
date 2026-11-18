//! A client for GitHub as a GitHub App (#96): POP files feedback as issues
//! on its own repository, with the screenshot on a branch of it.
//!
//! The App's private key stays on the server. POP signs a short-lived JWT
//! with it (RS256, `iss` the App's id), exchanges that for an installation
//! token (`POST /app/installations/{id}/access_tokens`), and uses the token
//! for the rest, renewing it before it lapses. The App needs Issues: write
//! and Contents: write on the repository, and nothing else.
//!
//! What it relies on, from GitHub's REST documentation and, for the reads,
//! from GitHub's own answers on this repository (2026-11-17):
//!
//! - `GET /repos/{o}/{r}` gives `default_branch`.
//! - `GET /repos/{o}/{r}/git/ref/heads/{b}` gives `object.sha`, and is a
//!   404 for a branch that does not exist.
//! - `GET /repos/{o}/{r}/contents/{path}?ref={b}` gives a file's
//!   `download_url` (on raw.githubusercontent.com), and is a 404 for none.
//! - `POST /repos/{o}/{r}/git/refs` makes a branch; `PUT .../contents/{path}`
//!   adds a file to one; `POST /repos/{o}/{r}/issues` files an issue. These
//!   writes are from the documentation only: POP has not run them against
//!   GitHub itself, and the fake (`fakes::github`) follows the documentation.
//!
//! Uploading is made safe to repeat: a file already on the branch is used as
//! it is, so a report retried after its picture went up but before its issue
//! did is not uploaded twice.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use aws_lc_rs::{rand::SystemRandom, signature::RsaKeyPair};
use base64::Engine as _;
use reqwest::{Method, StatusCode, header};
use serde::Deserialize;
use serde_json::{Value, json};

const B64URL: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// How POP reaches GitHub as its App.
#[derive(Clone)]
pub struct AppConfig {
    /// GitHub's API: `https://api.github.com`.
    pub api: String,
    pub app_id: String,
    pub installation_id: String,
    /// The App's private key, as GitHub gives it (PEM).
    pub private_key: String,
    pub owner: String,
    pub repo: String,
}

/// Why a request to GitHub failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubError {
    /// The App's key is not one POP can sign with.
    Key(String),
    /// GitHub refused the App: a wrong id, installation or key, or it lacks a
    /// permission.
    Refused(String),
    /// GitHub could not be reached, or failed; worth trying again.
    Unreachable(String),
    /// GitHub answered, but not with what POP expected.
    Unexpected(String),
}

impl std::fmt::Display for GitHubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Key(m) => write!(f, "the GitHub App's private key: {m}"),
            Self::Refused(m) => write!(
                f,
                "GitHub refused POP's App ({m}); check its id, installation and permissions (Issues and Contents: write)"
            ),
            Self::Unreachable(m) => write!(f, "GitHub cannot be reached: {m}"),
            Self::Unexpected(m) => write!(f, "GitHub answered unexpectedly: {m}"),
        }
    }
}

impl std::error::Error for GitHubError {}

impl GitHubError {
    /// Whether trying again later could work.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Unreachable(_))
    }
}

/// An issue GitHub filed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub html_url: String,
}

/// The DER inside a PEM block, and whether it is PKCS#8.
fn pem_der(pem: &str) -> Result<(Vec<u8>, bool), GitHubError> {
    let pkcs8 = pem.contains("BEGIN PRIVATE KEY");
    let pkcs1 = pem.contains("BEGIN RSA PRIVATE KEY");
    if !pkcs8 && !pkcs1 {
        return Err(GitHubError::Key(
            "not a PEM private key (BEGIN RSA PRIVATE KEY or BEGIN PRIVATE KEY)".into(),
        ));
    }
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .flat_map(|l| l.trim().chars())
        .collect();
    let der = B64
        .decode(body)
        .map_err(|_| GitHubError::Key("its PEM is not base64".into()))?;
    Ok((der, pkcs8))
}

/// A GitHub App installed on one repository.
#[derive(Clone)]
pub struct GitHub {
    cfg: AppConfig,
    key: Arc<RsaKeyPair>,
    http: reqwest::Client,
    token: Arc<Mutex<Option<(String, Instant)>>>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl GitHub {
    pub fn new(cfg: AppConfig) -> Result<Self, GitHubError> {
        let (der, pkcs8) = pem_der(&cfg.private_key)?;
        let key = if pkcs8 {
            RsaKeyPair::from_pkcs8(&der)
        } else {
            RsaKeyPair::from_der(&der)
        }
        .map_err(|e| GitHubError::Key(format!("not an RSA key POP can sign with: {e}")))?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("POP/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| GitHubError::Unexpected(format!("cannot make an HTTP client: {e}")))?;
        Ok(Self {
            cfg: AppConfig {
                api: cfg.api.trim_end_matches('/').to_owned(),
                ..cfg
            },
            key: Arc::new(key),
            http,
            token: Arc::default(),
        })
    }

    /// A JWT that says this is the App, good for nine minutes.
    pub fn jwt(&self) -> Result<String, GitHubError> {
        let now = unix_now();
        let header = B64URL.encode(json!({"alg": "RS256", "typ": "JWT"}).to_string());
        // A minute back, for a clock a little ahead of GitHub's.
        let claims = B64URL.encode(
            json!({"iat": now.saturating_sub(60), "exp": now + 540, "iss": self.cfg.app_id})
                .to_string(),
        );
        let signed = format!("{header}.{claims}");
        let mut sig = vec![0u8; self.key.public_modulus_len()];
        self.key
            .sign(
                &aws_lc_rs::signature::RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                signed.as_bytes(),
                &mut sig,
            )
            .map_err(|_| GitHubError::Key("signing failed".into()))?;
        Ok(format!("{signed}.{}", B64URL.encode(sig)))
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        auth: &str,
        body: Option<&Value>,
    ) -> Result<(StatusCode, Value), GitHubError> {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.cfg.api))
            .header(header::AUTHORIZATION, auth)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(b) = body {
            req = req.json(b);
        }
        let res = req
            .send()
            .await
            .map_err(|e| GitHubError::Unreachable(e.without_url().to_string()))?;
        let status = res.status();
        let body = res
            .bytes()
            .await
            .map_err(|e| GitHubError::Unreachable(e.without_url().to_string()))?;
        let v = serde_json::from_slice(&body).unwrap_or(Value::Null);
        Ok((status, v))
    }

    /// An installation token, renewed when it has five minutes or less left.
    async fn token(&self) -> Result<String, GitHubError> {
        if let Some((t, until)) = self.token.lock().unwrap().clone()
            && Instant::now() + Duration::from_secs(300) < until
        {
            return Ok(t);
        }
        let path = format!(
            "/app/installations/{}/access_tokens",
            self.cfg.installation_id
        );
        let (status, v) = self
            .send(
                Method::POST,
                &path,
                &format!("Bearer {}", self.jwt()?),
                None,
            )
            .await?;
        match status {
            StatusCode::CREATED | StatusCode::OK => {}
            s if s.is_server_error() => {
                return Err(GitHubError::Unreachable(format!("{path}: HTTP {s}")));
            }
            s => {
                return Err(GitHubError::Refused(format!(
                    "{path}: HTTP {s}: {}",
                    said(&v)
                )));
            }
        }
        let token = v["token"]
            .as_str()
            .ok_or_else(|| GitHubError::Unexpected("no token in the answer".into()))?
            .to_owned();
        // GitHub's tokens last an hour; POP keeps it for less than that,
        // whatever the answer says, and so does not need to read its time.
        let until = Instant::now() + Duration::from_secs(55 * 60);
        *self.token.lock().unwrap() = Some((token.clone(), until));
        Ok(token)
    }

    /// A request as the installation.
    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(StatusCode, Value), GitHubError> {
        let token = self.token().await?;
        let (status, v) = self
            .send(method, path, &format!("Bearer {token}"), body)
            .await?;
        if status == StatusCode::UNAUTHORIZED {
            // A token GitHub no longer takes: forget it, so the next call asks again.
            *self.token.lock().unwrap() = None;
            return Err(GitHubError::Unreachable(format!(
                "{path}: the token was refused"
            )));
        }
        if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            return Err(GitHubError::Unreachable(format!("{path}: HTTP {status}")));
        }
        if status == StatusCode::FORBIDDEN {
            return Err(GitHubError::Refused(format!("{path}: {}", said(&v))));
        }
        Ok((status, v))
    }

    fn repo_path(&self, rest: &str) -> String {
        format!("/repos/{}/{}{rest}", self.cfg.owner, self.cfg.repo)
    }

    /// Make `branch` if the repository has none, from its default branch.
    pub async fn ensure_branch(&self, branch: &str) -> Result<(), GitHubError> {
        let (status, _) = self
            .call(
                Method::GET,
                &self.repo_path(&format!("/git/ref/heads/{branch}")),
                None,
            )
            .await?;
        if status.is_success() {
            return Ok(());
        }
        if status != StatusCode::NOT_FOUND {
            return Err(GitHubError::Unexpected(format!(
                "reading {branch}: HTTP {status}"
            )));
        }
        let (status, repo) = self.call(Method::GET, &self.repo_path(""), None).await?;
        let default = repo["default_branch"]
            .as_str()
            .filter(|_| status.is_success())
            .ok_or_else(|| GitHubError::Unexpected(format!("the repository: HTTP {status}")))?
            .to_owned();
        let (status, head) = self
            .call(
                Method::GET,
                &self.repo_path(&format!("/git/ref/heads/{default}")),
                None,
            )
            .await?;
        let sha = head["object"]["sha"]
            .as_str()
            .filter(|_| status.is_success())
            .ok_or_else(|| GitHubError::Unexpected(format!("{default}: HTTP {status}")))?
            .to_owned();
        let (status, v) = self
            .call(
                Method::POST,
                &self.repo_path("/git/refs"),
                Some(&json!({"ref": format!("refs/heads/{branch}"), "sha": sha})),
            )
            .await?;
        match status {
            StatusCode::CREATED => Ok(()),
            // Made meanwhile, by another report.
            StatusCode::UNPROCESSABLE_ENTITY => Ok(()),
            s => Err(GitHubError::Unexpected(format!(
                "making {branch}: HTTP {s}: {}",
                said(&v)
            ))),
        }
    }

    /// Put `bytes` at `path` on `branch`, unless a file is there already;
    /// either way, the file's address for an issue to show it by.
    pub async fn upload(
        &self,
        branch: &str,
        path: &str,
        bytes: &[u8],
        message: &str,
    ) -> Result<String, GitHubError> {
        let at = self.repo_path(&format!("/contents/{path}"));
        let (status, v) = self
            .call(Method::GET, &format!("{at}?ref={branch}"), None)
            .await?;
        if status.is_success() {
            return download_url(&v);
        }
        if status != StatusCode::NOT_FOUND {
            return Err(GitHubError::Unexpected(format!(
                "reading {path}: HTTP {status}"
            )));
        }
        let (status, v) = self
            .call(
                Method::PUT,
                &at,
                Some(&json!({"message": message, "content": B64.encode(bytes), "branch": branch})),
            )
            .await?;
        match status {
            StatusCode::CREATED | StatusCode::OK => download_url(&v["content"]),
            s => Err(GitHubError::Unexpected(format!(
                "adding {path}: HTTP {s}: {}",
                said(&v)
            ))),
        }
    }

    /// File an issue.
    pub async fn create_issue(
        &self,
        title: &str,
        body: &str,
        labels: &[String],
    ) -> Result<Issue, GitHubError> {
        let (status, v) = self
            .call(
                Method::POST,
                &self.repo_path("/issues"),
                Some(&json!({"title": title, "body": body, "labels": labels})),
            )
            .await?;
        if status != StatusCode::CREATED {
            return Err(GitHubError::Unexpected(format!(
                "filing the issue: HTTP {status}: {}",
                said(&v)
            )));
        }
        serde_json::from_value(v)
            .map_err(|e| GitHubError::Unexpected(format!("the filed issue: {e}")))
    }
}

fn download_url(v: &Value) -> Result<String, GitHubError> {
    v["download_url"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| GitHubError::Unexpected("no download_url for the file".into()))
}

fn said(v: &Value) -> String {
    v["message"].as_str().unwrap_or("no message").to_owned()
}
