//! A fake GitHub (#96), for one repository with POP's App installed on it,
//! over real HTTP on a real port. It answers as GitHub's REST documentation
//! says, and as GitHub itself answered the reads (`github.rs` says which).
//!
//! It makes the App's key pair itself ([`FakeGitHub::private_key`]) and
//! checks every JWT against the public half: its signature, that it is the
//! App's, and that it has not lapsed. Only the installation token it gave
//! opens the repository. Faults: GitHub down (every request a 503), and an
//! issue that fails once after the picture went up.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use aws_lc_rs::{
    encoding::AsDer,
    rsa::{KeyPair, KeySize},
    signature::{KeyPair as _, RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::Engine as _;
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

const B64URL: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// An issue the fake was asked to file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filed {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

#[derive(Default)]
struct Repo {
    branches: BTreeMap<String, String>,
    /// (branch, path) to the file's bytes.
    files: BTreeMap<(String, String), Vec<u8>>,
    issues: Vec<Filed>,
    down: bool,
    fail_next_issue: bool,
    token_requests: u32,
}

struct Shared {
    owner: String,
    repo: String,
    app_id: String,
    installation_id: String,
    public_key: Vec<u8>,
    token: String,
    state: Mutex<Repo>,
}

/// A fake GitHub, listening until dropped.
pub struct FakeGitHub {
    addr: SocketAddr,
    shared: Arc<Shared>,
    private_key: String,
    task: JoinHandle<()>,
}

impl Drop for FakeGitHub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn pem(der: &[u8]) -> String {
    let b64 = B64.encode(der);
    let lines: Vec<&str> = b64
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        lines.join("\n")
    )
}

impl FakeGitHub {
    /// `owner/repo`, with POP's App (`app_id`) installed as `installation_id`.
    pub async fn start(owner: &str, repo: &str, app_id: &str, installation_id: &str) -> Self {
        let key = KeyPair::generate(KeySize::Rsa2048).expect("an RSA key");
        let private_key = pem(key.as_der().expect("its DER").as_ref());
        let public_key = key.public_key().as_ref().to_vec();
        let mut state = Repo::default();
        state.branches.insert(
            "main".into(),
            "0123456789abcdef0123456789abcdef01234567".into(),
        );
        let shared = Arc::new(Shared {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            app_id: app_id.to_owned(),
            installation_id: installation_id.to_owned(),
            public_key,
            token: "ghs_fake_installation_token".to_owned(),
            state: Mutex::new(state),
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let addr = listener.local_addr().expect("its address");
        let app = Router::new().fallback(answer).with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            addr,
            shared,
            private_key,
            task,
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The App's private key, as GitHub would give it.
    pub fn private_key(&self) -> String {
        self.private_key.clone()
    }

    /// Every issue filed, in order.
    pub fn issues(&self) -> Vec<Filed> {
        self.shared.state.lock().unwrap().issues.clone()
    }

    /// The file at `path` on `branch`.
    pub fn file(&self, branch: &str, path: &str) -> Option<Vec<u8>> {
        self.shared
            .state
            .lock()
            .unwrap()
            .files
            .get(&(branch.to_owned(), path.to_owned()))
            .cloned()
    }

    /// Every file on `branch`, by path.
    pub fn files(&self, branch: &str) -> Vec<String> {
        self.shared
            .state
            .lock()
            .unwrap()
            .files
            .keys()
            .filter(|(b, _)| b == branch)
            .map(|(_, p)| p.clone())
            .collect()
    }

    pub fn has_branch(&self, branch: &str) -> bool {
        self.shared
            .state
            .lock()
            .unwrap()
            .branches
            .contains_key(branch)
    }

    /// GitHub down, or back.
    pub fn down(&self, down: bool) {
        self.shared.state.lock().unwrap().down = down;
    }

    /// The next issue fails, as GitHub failing mid-report.
    pub fn fail_next_issue(&self) {
        self.shared.state.lock().unwrap().fail_next_issue = true;
    }

    /// How many installation tokens were asked for.
    pub fn token_requests(&self) -> u32 {
        self.shared.state.lock().unwrap().token_requests
    }
}

fn message(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({"message": msg}))).into_response()
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// Whether `jwt` is the App's own, signed by its key and not lapsed.
fn app_jwt(s: &Shared, jwt: &str) -> bool {
    let mut parts = jwt.split('.');
    let (Some(h), Some(c), Some(sig), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let Ok(sig) = B64URL.decode(sig) else {
        return false;
    };
    let signed = format!("{h}.{c}");
    if UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, &s.public_key)
        .verify(signed.as_bytes(), &sig)
        .is_err()
    {
        return false;
    }
    let Some(claims) = B64URL
        .decode(c)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    claims["iss"] == s.app_id
        && claims["exp"]
            .as_u64()
            .is_some_and(|e| e > now && e <= now + 600)
        && claims["iat"].as_u64().is_some_and(|i| i <= now)
}

async fn answer(State(s): State<Arc<Shared>>, req: Request) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let query = req.uri().query().unwrap_or_default().to_owned();
    let headers = req.headers().clone();
    let body: Bytes = axum::body::to_bytes(req.into_body(), 16 << 20)
        .await
        .unwrap_or_default();
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if s.state.lock().unwrap().down {
        return message(StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable");
    }
    let tokens = format!("/app/installations/{}/access_tokens", s.installation_id);
    if method == Method::POST && path == tokens {
        s.state.lock().unwrap().token_requests += 1;
        if !bearer(&headers).is_some_and(|j| app_jwt(&s, j)) {
            return message(
                StatusCode::UNAUTHORIZED,
                "A JSON web token could not be decoded",
            );
        }
        return (
            StatusCode::CREATED,
            Json(json!({"token": s.token, "expires_at": "2099-01-01T00:00:00Z"})),
        )
            .into_response();
    }
    if bearer(&headers) != Some(s.token.as_str()) {
        return message(StatusCode::UNAUTHORIZED, "Bad credentials");
    }
    let base = format!("/repos/{}/{}", s.owner, s.repo);
    let Some(rest) = path.strip_prefix(&base) else {
        return message(StatusCode::NOT_FOUND, "Not Found");
    };
    let mut st = s.state.lock().unwrap();
    match (method, rest) {
        (Method::GET, "") => Json(json!({
            "full_name": format!("{}/{}", s.owner, s.repo),
            "default_branch": "main",
            "private": false,
        }))
        .into_response(),
        (Method::GET, r) if r.starts_with("/git/ref/heads/") => {
            let b = &r["/git/ref/heads/".len()..];
            match st.branches.get(b) {
                Some(sha) => Json(json!({
                    "ref": format!("refs/heads/{b}"),
                    "object": {"sha": sha, "type": "commit"},
                }))
                .into_response(),
                None => message(StatusCode::NOT_FOUND, "Not Found"),
            }
        }
        (Method::POST, "/git/refs") => {
            let (Some(r), Some(sha)) = (json["ref"].as_str(), json["sha"].as_str()) else {
                return message(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request");
            };
            let Some(b) = r.strip_prefix("refs/heads/") else {
                return message(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Reference name is invalid",
                );
            };
            if st.branches.contains_key(b) {
                return message(StatusCode::UNPROCESSABLE_ENTITY, "Reference already exists");
            }
            st.branches.insert(b.to_owned(), sha.to_owned());
            (
                StatusCode::CREATED,
                Json(json!({"ref": r, "object": {"sha": sha}})),
            )
                .into_response()
        }
        (m, r) if r.starts_with("/contents/") => {
            let file = r["/contents/".len()..].to_owned();
            let download = |b: &str| {
                format!(
                    "https://raw.githubusercontent.com/{}/{}/{b}/{file}",
                    s.owner, s.repo
                )
            };
            if m == Method::GET {
                let b = query
                    .split('&')
                    .find_map(|p| p.strip_prefix("ref="))
                    .unwrap_or("main")
                    .to_owned();
                return if st.files.contains_key(&(b.clone(), file.clone())) {
                    Json(json!({"path": file, "type": "file", "download_url": download(&b)}))
                        .into_response()
                } else {
                    message(StatusCode::NOT_FOUND, "Not Found")
                };
            }
            if m != Method::PUT {
                return message(StatusCode::NOT_FOUND, "Not Found");
            }
            let b = json["branch"].as_str().unwrap_or("main").to_owned();
            if !st.branches.contains_key(&b) {
                return message(StatusCode::NOT_FOUND, &format!("Branch {b} not found"));
            }
            if st.files.contains_key(&(b.clone(), file.clone())) {
                return message(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Invalid request.\n\n\"sha\" wasn't supplied.",
                );
            }
            let Some(bytes) = json["content"].as_str().and_then(|c| B64.decode(c).ok()) else {
                return message(StatusCode::UNPROCESSABLE_ENTITY, "content is not base64");
            };
            st.files.insert((b.clone(), file.clone()), bytes);
            (
                StatusCode::CREATED,
                Json(
                    json!({"content": {"path": file, "download_url": download(&b)}, "commit": {}}),
                ),
            )
                .into_response()
        }
        (Method::POST, "/issues") => {
            if std::mem::take(&mut st.fail_next_issue) {
                return message(StatusCode::BAD_GATEWAY, "Server Error");
            }
            let number = st.issues.len() as u64 + 1;
            st.issues.push(Filed {
                number,
                title: json["title"].as_str().unwrap_or_default().to_owned(),
                body: json["body"].as_str().unwrap_or_default().to_owned(),
                labels: json["labels"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|l| l.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
            });
            (
                StatusCode::CREATED,
                Json(json!({
                    "number": number,
                    "html_url": format!("https://github.com/{}/{}/issues/{number}", s.owner, s.repo),
                })),
            )
                .into_response()
        }
        _ => message(StatusCode::NOT_FOUND, "Not Found"),
    }
}
