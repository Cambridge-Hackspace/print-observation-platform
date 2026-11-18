//! A fake Bambu printer (#26): its MQTT broker, over TLS with a self-signed
//! certificate, on a real port.
//!
//! It takes a sign-in as `bblp` with the printer's access code, answers a
//! subscription to `device/<serial>/report`, sends the whole state when asked
//! (`pushall`), and sends what changed when a test changes its printer, as
//! the printers that report partially do. Faults, each as a real printer or a
//! real network can show them: the connection dropped, a report that is not
//! JSON, the access code refused, and a different certificate (a printer
//! replaced, or something pretending to be it).

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use bytes::BytesMut;
use rumqttc::{
    ConnAck, ConnectReturnCode, Packet, PingResp, Publish, QoS, SubAck, SubscribeReasonCode,
    mqttbytes::Error as CodecError,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::broadcast,
    task::JoinHandle,
};
use tokio_rustls::TlsAcceptor;

use crate::bambu::fingerprint;

#[derive(Clone)]
enum Push {
    Report(Value),
    Raw(Vec<u8>),
    Drop,
}

struct Shared {
    serial: String,
    access_code: String,
    state: Mutex<Map<String, Value>>,
    acceptor: Mutex<(TlsAcceptor, String)>,
    connections: Mutex<u32>,
    requests: Mutex<Vec<Value>>,
    pushes: broadcast::Sender<Push>,
}

/// A fake Bambu printer, listening until dropped.
pub struct FakeBambu {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Drop for FakeBambu {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A fresh self-signed certificate for `serial`, as a TLS acceptor, and its
/// fingerprint.
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

impl FakeBambu {
    /// Start one on a free port, idle.
    pub async fn start(serial: &str, access_code: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let addr = listener.local_addr().expect("its address");
        let shared = Arc::new(Shared {
            serial: serial.to_owned(),
            access_code: access_code.to_owned(),
            state: Mutex::new(
                json!({"gcode_state": "IDLE", "mc_percent": 0, "layer_num": 0, "total_layer_num": 0,
                       "mc_remaining_time": 0, "subtask_name": "", "print_error": 0, "hms": []})
                .as_object()
                .cloned()
                .expect("an object"),
            ),
            acceptor: Mutex::new(certificate(serial)),
            connections: Mutex::new(0),
            requests: Mutex::new(Vec::new()),
            pushes: broadcast::channel(256).0,
        });
        let s = shared.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let acceptor = s.acceptor.lock().unwrap().0.clone();
                let s = s.clone();
                tokio::spawn(async move {
                    if let Ok(tls) = acceptor.accept(tcp).await {
                        serve(tls, s).await;
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

    /// How many sign-ins it has accepted.
    pub fn connections(&self) -> u32 {
        *self.shared.connections.lock().unwrap()
    }

    /// Every request a client has published, oldest first.
    pub fn requests(&self) -> Vec<Value> {
        self.shared.requests.lock().unwrap().clone()
    }

    /// Change the printer: these `print` fields replace what it had, and
    /// every subscriber is sent just these, as a partial report.
    pub fn set(&self, fields: Value) {
        if let Some(f) = fields.as_object() {
            let mut st = self.shared.state.lock().unwrap();
            for (k, v) in f {
                st.insert(k.clone(), v.clone());
            }
        }
        let mut print = fields.as_object().cloned().unwrap_or_default();
        print.insert("command".into(), json!("push_status"));
        let _ = self
            .shared
            .pushes
            .send(Push::Report(json!({ "print": print })));
    }

    /// Send every subscriber a report that is not JSON.
    pub fn send_garbage(&self) {
        let _ = self.shared.pushes.send(Push::Raw(b"{not json".to_vec()));
    }

    /// Drop every connection, as a printer switched off does.
    pub fn drop_connections(&self) {
        let _ = self.shared.pushes.send(Push::Drop);
    }

    /// Present a different certificate from the next connection on.
    pub fn rotate_certificate(&self) {
        *self.shared.acceptor.lock().unwrap() = certificate(&self.shared.serial);
    }
}

async fn serve<S>(mut tls: S, s: Arc<Shared>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let report_topic = format!("device/{}/report", s.serial);
    let request_topic = format!("device/{}/request", s.serial);
    let mut pushes = s.pushes.subscribe();
    let mut buf = BytesMut::with_capacity(4096);
    let mut out = BytesMut::new();
    let mut subscribed = false;
    loop {
        // Answer every whole packet read so far.
        loop {
            match Packet::read(&mut buf, 1 << 20) {
                Ok(Packet::Connect(c)) => {
                    let ok = c
                        .login
                        .as_ref()
                        .is_some_and(|l| l.username == "bblp" && l.password == s.access_code);
                    let code = if ok {
                        ConnectReturnCode::Success
                    } else {
                        ConnectReturnCode::BadUserNamePassword
                    };
                    let _ = ConnAck::new(code, false).write(&mut out);
                    if !ok {
                        let _ = tls.write_all(&out).await;
                        return;
                    }
                    *s.connections.lock().unwrap() += 1;
                }
                Ok(Packet::Subscribe(sub)) => {
                    subscribed |= sub.filters.iter().any(|f| f.path == report_topic);
                    let codes = sub
                        .filters
                        .iter()
                        .map(|_| SubscribeReasonCode::Success(QoS::AtMostOnce))
                        .collect();
                    let _ = SubAck::new(sub.pkid, codes).write(&mut out);
                }
                Ok(Packet::Publish(p)) if p.topic == request_topic => {
                    if let Ok(v) = serde_json::from_slice::<Value>(&p.payload) {
                        let pushall = v.pointer("/pushing/command").and_then(Value::as_str)
                            == Some("pushall");
                        s.requests.lock().unwrap().push(v);
                        if pushall && subscribed {
                            let mut full = s.state.lock().unwrap().clone();
                            full.insert("command".into(), json!("push_status"));
                            let body = json!({ "print": full }).to_string();
                            let _ =
                                Publish::new(&report_topic, QoS::AtMostOnce, body).write(&mut out);
                        }
                    }
                }
                Ok(Packet::PingReq) => {
                    let _ = PingResp.write(&mut out);
                }
                Ok(Packet::Disconnect) => return,
                Ok(_) => {}
                Err(CodecError::InsufficientBytes(_)) => break,
                Err(_) => return,
            }
        }
        if !out.is_empty() {
            if tls.write_all(&out).await.is_err() {
                return;
            }
            out.clear();
        }
        tokio::select! {
            n = tls.read_buf(&mut buf) => {
                if !matches!(n, Ok(n) if n > 0) {
                    return;
                }
            }
            p = pushes.recv() => {
                let payload = match p {
                    Ok(Push::Report(v)) if subscribed => v.to_string().into_bytes(),
                    Ok(Push::Report(_)) => continue,
                    Ok(Push::Raw(r)) if subscribed => r,
                    Ok(Push::Raw(_)) => continue,
                    Ok(Push::Drop) | Err(_) => return,
                };
                let _ = Publish::new(&report_topic, QoS::AtMostOnce, payload).write(&mut out);
            }
        }
    }
}
