//! Structured logs (#4): every request is logged, and the JSON format is one
//! JSON object per line.

use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use axum::{body::Body, http::Request};
use pop_server::{AppState, assets::Embedded, db, router};
use tower::ServiceExt;

/// A log destination the test can read back.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

#[tokio::test]
async fn every_request_is_logged_as_one_json_object_per_line() {
    let logs = Captured::default();
    let sink = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter("info")
        .json()
        .with_writer(move || sink.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("pop.db")).await.unwrap();
    let app = router(AppState::new(pool, Arc::new(Embedded)));
    let res = app
        .oneshot(Request::get("/api/status").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let text = logs.text();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect();
    let response = lines
        .iter()
        .find(|l| l["fields"]["message"] == "finished processing request")
        .unwrap_or_else(|| panic!("no request was logged: {text}"));
    assert_eq!(response["level"], "INFO");
    assert_eq!(response["fields"]["status"], 200);
    assert_eq!(response["span"]["uri"], "/api/status", "{response}");
}
