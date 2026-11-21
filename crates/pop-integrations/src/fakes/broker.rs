//! A fake of the space's broker (#19), as CS:S's edge uses it: plain MQTT on
//! a real port, a login it checks when given one, and the scan replies a test
//! tells it to send to everyone subscribed to them.
//!
//! What it does not do: retain, QoS above 0, or wildcards. POP subscribes to
//! one topic exactly, so a subscription to anything else is recorded and
//! answered but delivers nothing.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use bytes::BytesMut;
use rumqttc::{
    ConnAck, ConnectReturnCode, Packet, PingResp, Publish, QoS, SubAck, SubscribeReasonCode,
    mqttbytes::Error as CodecError,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::broadcast,
    task::JoinHandle,
};

#[derive(Clone)]
enum Push {
    Publish(String, Vec<u8>),
    Drop,
}

struct Shared {
    /// The login it takes, or None for none at all.
    login: Option<(String, String)>,
    connections: Mutex<u32>,
    subscriptions: Mutex<Vec<String>>,
    publishes: Mutex<Vec<String>>,
    pushes: broadcast::Sender<Push>,
}

/// A fake broker, listening until dropped.
pub struct FakeBroker {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Drop for FakeBroker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeBroker {
    /// One on a free port, taking `login` (username, password), or anyone.
    pub async fn start(login: Option<(&str, &str)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let addr = listener.local_addr().expect("its address");
        let shared = Arc::new(Shared {
            login: login.map(|(u, p)| (u.to_owned(), p.to_owned())),
            connections: Mutex::new(0),
            subscriptions: Mutex::new(Vec::new()),
            publishes: Mutex::new(Vec::new()),
            pushes: broadcast::channel(256).0,
        });
        let s = shared.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                tokio::spawn(serve(tcp, s.clone()));
            }
        });
        Self { addr, shared, task }
    }

    /// `mqtt://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("mqtt://{}", self.addr)
    }

    /// Connections it accepted the login of.
    pub fn connections(&self) -> u32 {
        *self.shared.connections.lock().unwrap()
    }

    /// Every topic filter a client subscribed to, in order.
    pub fn subscriptions(&self) -> Vec<String> {
        self.shared.subscriptions.lock().unwrap().clone()
    }

    /// The topics of everything a client published: POP should publish
    /// nothing.
    pub fn publishes(&self) -> Vec<String> {
        self.shared.publishes.lock().unwrap().clone()
    }

    /// Send `payload` on `topic` to every client subscribed to it.
    pub fn publish(&self, topic: &str, payload: impl Into<Vec<u8>>) {
        let _ = self
            .shared
            .pushes
            .send(Push::Publish(topic.to_owned(), payload.into()));
    }

    /// Drop every connection, as a restarted broker would.
    pub fn drop_connections(&self) {
        let _ = self.shared.pushes.send(Push::Drop);
    }
}

async fn serve(mut tcp: tokio::net::TcpStream, s: Arc<Shared>) {
    let mut pushes = s.pushes.subscribe();
    let mut buf = BytesMut::with_capacity(4096);
    let mut out = BytesMut::new();
    let mut subscribed: Vec<String> = Vec::new();
    loop {
        loop {
            match Packet::read(&mut buf, 1 << 20) {
                Ok(Packet::Connect(c)) => {
                    let ok = match &s.login {
                        None => true,
                        Some((u, p)) => c
                            .login
                            .as_ref()
                            .is_some_and(|l| &l.username == u && &l.password == p),
                    };
                    let code = if ok {
                        ConnectReturnCode::Success
                    } else {
                        ConnectReturnCode::BadUserNamePassword
                    };
                    let _ = ConnAck::new(code, false).write(&mut out);
                    if !ok {
                        let _ = tcp.write_all(&out).await;
                        return;
                    }
                    *s.connections.lock().unwrap() += 1;
                }
                Ok(Packet::Subscribe(sub)) => {
                    for f in &sub.filters {
                        s.subscriptions.lock().unwrap().push(f.path.clone());
                        subscribed.push(f.path.clone());
                    }
                    let codes = sub
                        .filters
                        .iter()
                        .map(|_| SubscribeReasonCode::Success(QoS::AtMostOnce))
                        .collect();
                    let _ = SubAck::new(sub.pkid, codes).write(&mut out);
                }
                Ok(Packet::Publish(p)) => s.publishes.lock().unwrap().push(p.topic.clone()),
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
            if tcp.write_all(&out).await.is_err() {
                return;
            }
            out.clear();
        }
        tokio::select! {
            n = tcp.read_buf(&mut buf) => {
                if !matches!(n, Ok(n) if n > 0) {
                    return;
                }
            }
            p = pushes.recv() => match p {
                Ok(Push::Publish(topic, payload)) if subscribed.contains(&topic) => {
                    let _ = Publish::new(&topic, QoS::AtMostOnce, payload).write(&mut out);
                }
                Ok(Push::Publish(..)) => {}
                Ok(Push::Drop) | Err(_) => return,
            }
        }
    }
}
