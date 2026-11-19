//! The live connection trusts what this machine trusts (#180): a BAM! whose
//! certificate the space's own authority signed, as on an OPNsense network,
//! is reached once that authority is installed, as POP's HTTP requests do.
//!
//! The connection is opened in a child process started with SSL_CERT_FILE
//! naming a test authority: the variable is the whole process's, and Linux
//! reads the system's certificates through it, so it stands in for
//! update-ca-certificates without touching this machine's.
//!
//! What it does not prove: that Debian's own store is where POP looks. That
//! is rustls-platform-verifier's, and the same reqwest's HTTP requests use.

use std::{sync::Arc, time::Duration};

use futures_util::SinkExt;
use pop_integrations::bam::{Bam, BamError};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
use tokio::net::TcpListener;
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    },
};
use tokio_tungstenite::tungstenite::Message;

/// An authority of the space's own, and a certificate it signed for `host`.
fn authority_and_server(host: &str) -> (String, CertificateDer<'static>, PrivateKeyDer<'static>) {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.distinguished_name
        .push(rcgen::DnType::CommonName, "The space's own authority");
    let ca = ca.self_signed(&ca_key).unwrap();
    let key = KeyPair::generate().unwrap();
    let cert = CertificateParams::new(vec![host.to_owned()])
        .unwrap()
        .signed_by(&key, &ca, &ca_key)
        .unwrap();
    (
        ca.pem(),
        cert.der().clone(),
        PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
    )
}

/// A BAM! over TLS that says hello on every live connection, and its address.
async fn bam_over_tls(cert: CertificateDer<'static>, key: PrivateKeyDer<'static>) -> String {
    let cfg = ServerConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert], key)
    .unwrap();
    let tls = TlsAcceptor::from(Arc::new(cfg));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let tls = tls.clone();
            tokio::spawn(async move {
                let Ok(stream) = tls.accept(tcp).await else {
                    return;
                };
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                let _ = ws.send(Message::Text(r#"{"type":"hello"}"#.into())).await;
                tokio::time::sleep(Duration::from_secs(5)).await;
            });
        }
    });
    format!("https://localhost:{port}")
}

/// Open the live connection to the BAM! at POP_TRUST_URL, in a process whose
/// SSL_CERT_FILE was set before it started, and say what happened on stdout.
/// Run only as the child of the test below; on its own it does nothing.
#[tokio::test]
async fn a_bam_that_never_finishes_the_handshake_is_given_up_on() {
    // It takes the connection, then says nothing at all.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((tcp, _)) = listener.accept().await {
            held.push(tcp);
        }
    });
    let bam = Bam::new(format!("https://localhost:{port}"), "token").unwrap();
    let gave_up = tokio::time::timeout(
        Duration::from_secs(10),
        bam.live("printers", Duration::from_millis(500)),
    )
    .await
    .expect("the live connection waited for ever");
    assert!(matches!(gave_up, Err(BamError::Unreachable(_))));
}

#[tokio::test]
async fn child_opens_the_live_connection() {
    let Ok(url) = std::env::var("POP_TRUST_URL") else {
        return;
    };
    let bam = Bam::new(url, "token").unwrap();
    match bam.live("printers", Duration::from_secs(5)).await {
        Ok(_) => println!("TRUST:opened"),
        Err(BamError::Unreachable(e)) => println!("TRUST:refused {e}"),
        Err(e) => println!("TRUST:other {e:?}"),
    }
}

/// What the live connection does in a child trusting only `authority`.
fn live_trusting(url: &str, authority: &std::path::Path) -> String {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_opens_the_live_connection", "--nocapture"])
        .env("POP_TRUST_URL", url)
        .env("SSL_CERT_FILE", authority)
        .env_remove("SSL_CERT_DIR")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "the child failed: {stdout}");
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("TRUST:"))
        .unwrap_or_else(|| panic!("the child said nothing: {stdout}"))
        .to_owned()
}

/// The same, waited for off the runtime: the BAM! the child connects to is
/// served on this one, which must not be blocked while it does.
async fn trusting(url: &str, authority: &std::path::Path) -> String {
    let (url, authority) = (url.to_owned(), authority.to_owned());
    tokio::task::spawn_blocking(move || live_trusting(&url, &authority))
        .await
        .unwrap()
}

#[tokio::test]
async fn the_live_connection_trusts_the_authority_this_machine_trusts() {
    let dir = std::env::temp_dir().join(format!("pop-trust-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (theirs, cert, key) = authority_and_server("localhost");
    let (other, _, _) = authority_and_server("localhost");
    let url = bam_over_tls(cert, key).await;
    std::fs::write(dir.join("other.pem"), other).unwrap();
    std::fs::write(dir.join("theirs.pem"), theirs).unwrap();

    // Trusting some other authority, the connection is refused: what is
    // trusted is what the machine says, not anything that answers.
    let refused = trusting(&url, &dir.join("other.pem")).await;
    assert!(
        refused.starts_with("refused"),
        "a server the machine does not trust was reached: {refused}"
    );

    // Trusting the authority that signed BAM!'s certificate, it opens.
    let opened = trusting(&url, &dir.join("theirs.pem")).await;
    assert_eq!(
        opened, "opened",
        "the authority this machine trusts was not trusted"
    );
    std::fs::remove_dir_all(&dir).ok();
}
