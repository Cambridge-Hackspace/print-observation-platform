//! The go2rtc client (#66), against the fake go2rtc; and the fake against
//! what a real go2rtc 1.9.15 answered (tests/recordings/go2rtc-1.9.15).

use pop_integrations::{
    fakes::go2rtc::{self as fake, FakeGo2rtc},
    go2rtc::{Go2rtc, Go2rtcError, Picture},
};
use serde_json::Value;

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
        // The configuration file is go2rtc's own; POP never reads it.
        if name == "config" {
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
