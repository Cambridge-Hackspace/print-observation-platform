//! The GitHub App client (#96), against the fake GitHub.

use base64::Engine as _;
use pop_integrations::{
    fakes::github::FakeGitHub,
    github::{AppConfig, GitHub, GitHubError},
};
use serde_json::Value;

async fn fake() -> FakeGitHub {
    FakeGitHub::start("acme", "pop", "12345", "678").await
}

fn client(g: &FakeGitHub, key: String) -> GitHub {
    GitHub::new(AppConfig {
        api: g.url(),
        app_id: "12345".into(),
        installation_id: "678".into(),
        private_key: key,
        owner: "acme".into(),
        repo: "pop".into(),
    })
    .unwrap()
}

#[tokio::test]
async fn the_jwt_says_it_is_the_app_for_under_ten_minutes() {
    let g = fake().await;
    let jwt = client(&g, g.private_key()).jwt().unwrap();
    let parts: Vec<&str> = jwt.split('.').collect();
    assert_eq!(parts.len(), 3);
    let decode = |p: &str| -> Value {
        serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(p)
                .unwrap(),
        )
        .unwrap()
    };
    assert_eq!(decode(parts[0])["alg"], "RS256");
    let claims = decode(parts[1]);
    assert_eq!(claims["iss"], "12345");
    let (iat, exp) = (
        claims["iat"].as_u64().unwrap(),
        claims["exp"].as_u64().unwrap(),
    );
    assert!(exp > iat && exp - iat <= 600, "{claims}");
}

#[tokio::test]
async fn a_screenshot_goes_on_its_branch_and_an_issue_shows_it() {
    let g = fake().await;
    let c = client(&g, g.private_key());
    assert!(!g.has_branch("feedback-attachments"));
    c.ensure_branch("feedback-attachments").await.unwrap();
    assert!(g.has_branch("feedback-attachments"));
    // Again: already there, nothing to do.
    c.ensure_branch("feedback-attachments").await.unwrap();
    let url = c
        .upload(
            "feedback-attachments",
            "feedback/r1.png",
            b"PNG BYTES",
            "Feedback r1",
        )
        .await
        .unwrap();
    assert_eq!(
        url,
        "https://raw.githubusercontent.com/acme/pop/feedback-attachments/feedback/r1.png"
    );
    assert_eq!(
        g.file("feedback-attachments", "feedback/r1.png").unwrap(),
        b"PNG BYTES"
    );
    let issue = c
        .create_issue(
            "The X1 reads offline",
            "Body",
            &["feedback".into(), "bug".into()],
        )
        .await
        .unwrap();
    assert_eq!(issue.html_url, "https://github.com/acme/pop/issues/1");
    let filed = g.issues();
    assert_eq!(filed[0].title, "The X1 reads offline");
    assert_eq!(filed[0].labels, ["feedback", "bug"]);
    // One installation token served every call.
    assert_eq!(g.token_requests(), 1);
}

/// A report retried after its picture went up finds it there, and does not
/// put it up again.
#[tokio::test]
async fn uploading_again_uses_the_file_already_there() {
    let g = fake().await;
    let c = client(&g, g.private_key());
    c.ensure_branch("b").await.unwrap();
    let first = c.upload("b", "f.png", b"ONE", "m").await.unwrap();
    let again = c.upload("b", "f.png", b"TWO", "m").await.unwrap();
    assert_eq!(first, again);
    assert_eq!(g.file("b", "f.png").unwrap(), b"ONE");
}

#[tokio::test]
async fn another_key_is_refused_and_a_bad_one_is_said() {
    let g = fake().await;
    let other = fake().await;
    let e = client(&g, other.private_key())
        .create_issue("t", "b", &[])
        .await
        .unwrap_err();
    assert!(matches!(e, GitHubError::Refused(_)), "{e:?}");
    assert!(!e.is_transient());
    assert!(g.issues().is_empty());
    let bad = GitHub::new(AppConfig {
        api: g.url(),
        app_id: "12345".into(),
        installation_id: "678".into(),
        private_key: "not a key".into(),
        owner: "acme".into(),
        repo: "pop".into(),
    })
    .err()
    .unwrap();
    assert!(matches!(bad, GitHubError::Key(_)), "{bad:?}");
}

#[tokio::test]
async fn github_down_is_worth_trying_again() {
    let g = fake().await;
    let c = client(&g, g.private_key());
    g.down(true);
    let e = c.create_issue("t", "b", &[]).await.unwrap_err();
    assert!(e.is_transient(), "{e:?}");
    g.down(false);
    c.create_issue("t", "b", &[]).await.unwrap();
    assert_eq!(g.issues().len(), 1);
}
