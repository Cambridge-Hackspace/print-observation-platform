//! The go2rtc client (#66), against the fake go2rtc; and the fake against
//! what a real go2rtc 1.9.15 answered (tests/recordings/go2rtc-1.9.15).

use futures_util::{SinkExt, StreamExt};
use pop_integrations::{
    fakes::go2rtc::{self as fake, FakeGo2rtc},
    go2rtc::{Go2rtc, Go2rtcError, Mjpeg, Mse, Picture},
};
use serde_json::Value;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

const RECORDING: &str = include_str!("recordings/go2rtc-1.9.15/recordings.json");

/// What POP reads of a stream list: each stream's sources. go2rtc also lists
/// who is watching, which POP never reads and the fake does not keep.
fn sources_only(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("consumers");
        for s in o.values_mut() {
            if let Some(s) = s.as_object_mut() {
                s.remove("consumers");
            }
        }
    }
    v
}

/// Every request the recorder made, made again of the fake, in order, gets
/// the answer the real go2rtc gave: status, content type and body.
#[tokio::test]
async fn the_fake_answers_every_recorded_request_as_go2rtc_did() {
    let rec: Value = serde_json::from_str(RECORDING).unwrap();
    let go2rtc = FakeGo2rtc::start_with(RECORDING, "pop", "recording-only").await;
    go2rtc.working(rec["camera"].as_str().unwrap(), true);
    let http = reqwest::Client::new();
    let exchanges = rec["exchanges"].as_array().unwrap();
    assert!(exchanges.len() >= 19, "the whole recording is played");
    for e in exchanges {
        let name = e["name"].as_str().unwrap();
        let mut url =
            reqwest::Url::parse(&format!("{}{}", go2rtc.url(), e["path"].as_str().unwrap()))
                .unwrap();
        for (k, v) in e["query"].as_object().unwrap() {
            url.query_pairs_mut().append_pair(k, v.as_str().unwrap());
        }
        // The configuration file is go2rtc's own; POP never reads it. The
        // streams are played below, once every stream is in.
        if name == "config" || name.starts_with("mse-") || name.starts_with("mjpeg-") {
            continue;
        }
        let mut req = http.request(e["method"].as_str().unwrap().parse().unwrap(), url);
        req = match &e["auth"] {
            Value::Bool(true) => req.basic_auth("pop", Some("recording-only")),
            Value::String(s) if s == "wrong" => req.basic_auth("pop", Some("wrong")),
            _ => req,
        };
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        let ctype = res
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let body = res.bytes().await.unwrap();
        assert_eq!(
            status,
            e["status"].as_u64().unwrap() as u16,
            "{name}: status"
        );
        assert_eq!(
            ctype,
            e["content_type"].as_str().unwrap(),
            "{name}: content type"
        );
        let want = &e["body"];
        if ctype.starts_with("application/json") {
            let got: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(sources_only(&got), sources_only(want), "{name}: body");
        } else if ctype.starts_with("image/jpeg") {
            assert!(
                want["jpeg_bytes"].as_u64().unwrap() > 0,
                "{name}: recorded a picture"
            );
            assert!(
                !body.is_empty() && body.starts_with(&[0xff, 0xd8]),
                "{name}: a JPEG"
            );
        } else {
            assert_eq!(
                std::str::from_utf8(&body).unwrap(),
                want.as_str().unwrap(),
                "{name}: body"
            );
        }
    }
}

fn client(go2rtc: &FakeGo2rtc) -> Go2rtc {
    Go2rtc::new(
        go2rtc.url(),
        Some((fake::USER.to_owned(), fake::PASSWORD.to_owned())),
    )
    .unwrap()
}

#[tokio::test]
async fn streams_are_added_replaced_listed_and_removed() {
    let go2rtc = FakeGo2rtc::start(RECORDING).await;
    let c = client(&go2rtc);
    assert_eq!(c.version().await.unwrap(), "1.9.15");
    assert!(c.streams().await.unwrap().is_empty());
    c.put("pop_a", "rtsp://cam-a/live").await.unwrap();
    c.put("pop_b", "http://cam-b/stream").await.unwrap();
    c.put("pop_a", "rtsp://cam-a/other").await.unwrap();
    let streams = c.streams().await.unwrap();
    assert_eq!(streams["pop_a"], ["rtsp://cam-a/other"]);
    assert_eq!(streams["pop_b"], ["http://cam-b/stream"]);
    c.delete("pop_a").await.unwrap();
    // Removing one go2rtc does not have is done already.
    c.delete("pop_a").await.unwrap();
    assert_eq!(
        c.streams().await.unwrap().keys().collect::<Vec<_>>(),
        ["pop_b"]
    );
}

#[tokio::test]
async fn a_picture_says_whether_the_camera_works() {
    let go2rtc = FakeGo2rtc::start(RECORDING).await;
    let c = client(&go2rtc);
    c.put("pop_a", "rtsp://cam-a/live").await.unwrap();
    // go2rtc answers 200 with nothing when it cannot get a picture.
    assert_eq!(c.picture("pop_a").await.unwrap(), Picture::Nothing);
    go2rtc.working("rtsp://cam-a/live", true);
    assert_eq!(
        c.picture("pop_a").await.unwrap(),
        Picture::Jpeg(fake::JPEG.len())
    );
    assert_eq!(c.picture("pop_nosuch").await.unwrap(), Picture::NoStream);
}

#[tokio::test]
async fn a_wrong_password_is_said_as_such() {
    let go2rtc = FakeGo2rtc::start(RECORDING).await;
    let c = Go2rtc::new(go2rtc.url(), Some(("pop".into(), "wrong".into()))).unwrap();
    assert_eq!(c.version().await, Err(Go2rtcError::Auth));
    let none = Go2rtc::new(go2rtc.url(), None).unwrap();
    assert_eq!(none.streams().await, Err(Go2rtcError::Auth));
    assert!(go2rtc.streams().is_empty());
}

#[tokio::test]
async fn a_source_go2rtc_refuses_is_refused_in_its_words() {
    let go2rtc = FakeGo2rtc::start(RECORDING).await;
    let c = client(&go2rtc);
    assert_eq!(
        c.put("pop_x", "exec:echo hi").await,
        Err(Go2rtcError::Refused(
            "streams: source from insecure producer".into()
        ))
    );
    assert!(go2rtc.streams().is_empty());
}

/// A camera's address can hold its password, and it travels in the request's
/// address; an error must not repeat it.
#[tokio::test]
async fn an_error_never_repeats_a_cameras_address() {
    // A port nothing listens on: bind one, then let it go.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let c = Go2rtc::new(format!("http://127.0.0.1:{port}"), None).unwrap();
    let e = c
        .put(
            "pop_a",
            "rtsps://bblp:s3cret-code@10.0.0.9:322/streaming/live/1",
        )
        .await
        .unwrap_err();
    assert!(matches!(e, Go2rtcError::Unreachable(_)), "{e:?}");
    let said = format!("{e} {e:?}");
    assert!(!said.contains("s3cret"), "{said}");
    assert!(!said.contains("bblp"), "{said}");
}

/// The recorded MSE and MJPEG streams, played against the fake: each begins
/// as go2rtc's did.
#[tokio::test]
async fn the_fake_streams_as_go2rtc_did() {
    let rec: Value = serde_json::from_str(RECORDING).unwrap();
    let go2rtc = FakeGo2rtc::start_with(RECORDING, "pop", "recording-only").await;
    let exchanges = rec["exchanges"].as_array().unwrap();
    // The streams the recording had when it watched, as it added them.
    for e in exchanges
        .iter()
        .filter(|e| e["method"] == "PUT" && e["status"] == 200)
    {
        let q = &e["query"];
        go2rtc.add(q["name"].as_str().unwrap(), q["src"].as_str().unwrap());
    }
    go2rtc.working(rec["camera"].as_str().unwrap(), true);
    // What the recording published into is an H.264 camera.
    let h264 = exchanges.iter().find(|e| e["name"] == "add-h264").unwrap()["query"]["src"]
        .as_str()
        .unwrap()
        .to_owned();
    go2rtc.h264(&h264, true);
    let streams: Vec<&Value> = exchanges
        .iter()
        .filter(|e| {
            let n = e["name"].as_str().unwrap();
            n.starts_with("mse-") || n.starts_with("mjpeg-")
        })
        .collect();
    assert_eq!(streams.len(), 9, "every recorded stream is played");
    for e in streams {
        let name = e["name"].as_str().unwrap();
        let src = e["query"]["src"].as_str().unwrap();
        let auth = e["auth"] == true;
        if e["path"] == "/api/ws" {
            let url = format!(
                "{}/api/ws?src={src}",
                go2rtc.url().replace("http://", "ws://")
            );
            let mut req = url.into_client_request().unwrap();
            if auth {
                req.headers_mut().insert(
                    "Authorization",
                    "Basic cG9wOnJlY29yZGluZy1vbmx5".parse().unwrap(), // pop:recording-only
                );
            }
            let opened = tokio_tungstenite::connect_async(req).await;
            let want_status = e["status"].as_u64().unwrap();
            let mut ws = match opened {
                Ok((ws, _)) => {
                    assert_eq!(want_status, 101, "{name}: opened");
                    ws
                }
                Err(tokio_tungstenite::tungstenite::Error::Http(r)) => {
                    assert_eq!(
                        u64::from(r.status().as_u16()),
                        want_status,
                        "{name}: refused"
                    );
                    continue;
                }
                Err(other) => panic!("{name}: {other}"),
            };
            ws.send(Message::Text(e["body"]["sent"].to_string().into()))
                .await
                .unwrap();
            for (i, want) in e["body"]["frames"].as_array().unwrap().iter().enumerate() {
                let got = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
                    .await
                    .unwrap_or_else(|_| panic!("{name}: frame {i} never came"))
                    .unwrap()
                    .unwrap();
                match got {
                    Message::Text(t) => {
                        let got: Value = serde_json::from_str(&t).unwrap();
                        assert_eq!(&got, &want["text"], "{name}: frame {i}");
                    }
                    Message::Binary(b) => {
                        assert_eq!(
                            std::str::from_utf8(&b[4..8]).unwrap(),
                            want["binary"].as_str().unwrap_or("not binary"),
                            "{name}: frame {i}"
                        );
                    }
                    other => panic!("{name}: frame {i}: {other:?}"),
                }
            }
        } else {
            let mut res = reqwest::Client::new()
                .get(format!("{}/api/stream.mjpeg?src={src}", go2rtc.url()))
                .basic_auth("pop", Some("recording-only"))
                .send()
                .await
                .unwrap();
            assert_eq!(
                u64::from(res.status().as_u16()),
                e["status"].as_u64().unwrap(),
                "{name}"
            );
            let ctype = res
                .headers()
                .get("content-type")
                .map(|v| v.to_str().unwrap().to_owned())
                .unwrap_or_default();
            assert_eq!(
                ctype,
                e["content_type"].as_str().unwrap(),
                "{name}: content type"
            );
            let want = e["body"]["first_part"].as_str().unwrap();
            let mut got = Vec::new();
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                match res.chunk().await.unwrap() {
                    Some(c) => got.extend_from_slice(&c),
                    None => break,
                }
            }
            let got = String::from_utf8_lossy(&got);
            let head = got.split("\r\n\r\n").next().unwrap_or_default();
            // The length is the picture's, which is the camera's and not go2rtc's.
            let shape = |h: &str| {
                h.split("\r\n")
                    .map(|l| {
                        if l.starts_with("Content-Length: ") {
                            "Content-Length: N"
                        } else {
                            l
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\r\n")
            };
            assert_eq!(shape(head), shape(want), "{name}: the first part's head");
        }
    }
}

#[tokio::test]
async fn mse_plays_an_h264_camera_and_says_why_another_does_not() {
    let go2rtc = FakeGo2rtc::start(RECORDING).await;
    let c = client(&go2rtc);
    for (name, src) in [
        ("pop_h", "rtsp://h264-cam/live"),
        ("pop_j", "http://jpeg-cam/mjpeg"),
        ("pop_d", "rtsp://dead/"),
    ] {
        c.put(name, src).await.unwrap();
    }
    go2rtc.h264("rtsp://h264-cam/live", true);
    go2rtc.working("http://jpeg-cam/mjpeg", true);
    match c.mse("pop_h", "avc1.640029").await.unwrap() {
        Mse::Playing { mime, mut media } => {
            assert_eq!(mime, "video/mp4; codecs=\"avc1.420029\"");
            assert_eq!(&media.next().await.unwrap()[4..8], b"ftyp");
            assert_eq!(&media.next().await.unwrap()[4..8], b"moof");
        }
        _ => panic!("the H.264 camera did not play"),
    }
    assert!(matches!(
        c.mse("pop_j", "avc1.640029").await.unwrap(),
        Mse::Unsupported
    ));
    assert!(matches!(
        c.mse("pop_d", "avc1.640029").await.unwrap(),
        Mse::NoPicture
    ));
    assert!(matches!(
        c.mse("pop_nosuch", "avc1.640029").await.unwrap(),
        Mse::NoPicture
    ));
    // What reached go2rtc: only the codecs asked for, once per stream.
    assert_eq!(
        go2rtc.sent(),
        vec![r#"{"type":"mse","value":"avc1.640029"}"#; 4]
    );
    let wrong = Go2rtc::new(go2rtc.url(), Some(("pop".into(), "wrong".into()))).unwrap();
    assert!(matches!(
        wrong.mse("pop_h", "avc1.640029").await,
        Err(Go2rtcError::Auth)
    ));
}

#[tokio::test]
async fn mjpeg_plays_a_jpeg_camera_only() {
    let go2rtc = FakeGo2rtc::start(RECORDING).await;
    let c = client(&go2rtc);
    c.put("pop_h", "rtsp://h264-cam/live").await.unwrap();
    c.put("pop_j", "http://jpeg-cam/mjpeg").await.unwrap();
    go2rtc.h264("rtsp://h264-cam/live", true);
    go2rtc.working("http://jpeg-cam/mjpeg", true);
    match c.mjpeg("pop_j").await.unwrap() {
        Mjpeg::Playing {
            content_type,
            mut body,
        } => {
            assert_eq!(content_type, "multipart/x-mixed-replace; boundary=frame");
            let first = Mjpeg::chunk(&mut body).await.unwrap();
            assert!(first.starts_with(b"--frame\r\nContent-Type: image/jpeg"));
        }
        _ => panic!("the JPEG camera did not play"),
    }
    assert!(matches!(c.mjpeg("pop_h").await.unwrap(), Mjpeg::NoPicture));
    assert!(matches!(
        c.mjpeg("pop_nosuch").await.unwrap(),
        Mjpeg::NoStream
    ));
}
