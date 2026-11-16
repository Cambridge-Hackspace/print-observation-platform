//! Errors link to the right guide (#73): an admin links a guide to an error
//! the catalog knows, by its stable key, and anyone can look up which
//! published guide fixes an error, for "Show me how".

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use pop_server::{
    AppState,
    api::{Guide, GuideStatus, GuideStep, Role, Then},
    assets::Embedded,
    audit::Actor,
    db, guides, router,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

const LOCAL: &str = "127.0.0.1:4000";
const STRANGER: &str = "203.0.113.9:4000";

fn guide(slug: &str, status: GuideStatus) -> Guide {
    Guide {
        slug: slug.into(),
        title: format!("Guide {slug}"),
        status,
        summary: String::new(),
        start: "do-it".into(),
        steps: vec![GuideStep {
            id: "do-it".into(),
            title: "Home the printer from its screen, then start again.".into(),
            body: String::new(),
            image: None,
            then: Then::Done,
        }],
    }
}

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    admin: String,
}

async fn pop() -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    users::create(
        &db,
        NewAccount {
            username: "ada",
            name: "Ada",
            role: Role::Admin,
            password: "ada's password",
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    let app = router(AppState::new(db.clone(), Arc::new(Embedded)));
    let mut pop = Pop {
        _dir: dir,
        db,
        app,
        admin: String::new(),
    };
    let (res, _) = pop
        .send(
            Method::POST,
            "/api/session",
            Some(json!({"username": "ada", "password": "ada's password"})),
            LOCAL,
        )
        .await;
    pop.admin = res.split(';').next().unwrap().to_owned();
    pop
}

impl Pop {
    /// (Set-Cookie or "", (status, body)).
    async fn send(
        &self,
        method: Method,
        uri: &str,
        body: Option<Value>,
        from: &str,
    ) -> (String, (StatusCode, Value)) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json");
        if !self.admin.is_empty() && from == LOCAL {
            req = req.header(header::COOKIE, &self.admin);
        }
        let mut req = req
            .body(body.map(|b| Body::from(b.to_string())).unwrap_or_default())
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(from.parse::<SocketAddr>().unwrap()));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let cookie = res
            .headers()
            .get(header::SET_COOKIE)
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let code = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            cookie,
            (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null)),
        )
    }
    async fn admin(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.send(method, uri, body, LOCAL).await.1
    }
    async fn public(&self) -> Value {
        let (_, (c, b)) = self
            .send(Method::GET, "/api/public/error-guides", None, STRANGER)
            .await;
        assert_eq!(c, StatusCode::OK);
        b
    }
}

#[tokio::test]
async fn an_admin_sees_every_error_the_catalog_knows() {
    let pop = pop().await;
    let (c, list) = pop
        .admin(Method::GET, "/api/admin/error-guides", None)
        .await;
    assert_eq!(c, StatusCode::OK);
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), pop_printers::catalog().len());
    let home = list
        .iter()
        .find(|e| e["key"] == "klipper-must-home")
        .unwrap();
    assert_eq!(
        home["words"],
        "The printer was asked to move before finding its home position"
    );
    assert_eq!(home["guide"], Value::Null);
    assert!(
        home["source"]
            .as_str()
            .unwrap()
            .contains("Must home axis first")
    );
}

#[tokio::test]
async fn a_linked_published_guide_is_what_show_me_how_opens() {
    let pop = pop().await;
    guides::create(&pop.db, &guide("homing", GuideStatus::Published))
        .await
        .unwrap();
    let (c, b) = pop
        .admin(
            Method::PUT,
            "/api/admin/error-guides/klipper-must-home",
            Some(json!({"guide": "homing"})),
        )
        .await;
    assert_eq!(c, StatusCode::NO_CONTENT, "{b}");
    assert_eq!(pop.public().await, json!({"klipper-must-home": "homing"}));
    let (_, list) = pop
        .admin(Method::GET, "/api/admin/error-guides", None)
        .await;
    let home = list
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "klipper-must-home")
        .unwrap()
        .clone();
    assert_eq!(home["guide"], "homing");
}

#[tokio::test]
async fn a_link_to_a_draft_is_never_shown_to_anyone() {
    let pop = pop().await;
    guides::create(&pop.db, &guide("homing", GuideStatus::Draft))
        .await
        .unwrap();
    pop.admin(
        Method::PUT,
        "/api/admin/error-guides/klipper-must-home",
        Some(json!({"guide": "homing"})),
    )
    .await;
    assert_eq!(pop.public().await, json!({}));
    // Published, it is.
    guides::update(&pop.db, "homing", &guide("homing", GuideStatus::Published))
        .await
        .unwrap();
    assert_eq!(pop.public().await, json!({"klipper-must-home": "homing"}));
}

#[tokio::test]
async fn a_link_follows_its_guide_through_a_rename() {
    let pop = pop().await;
    guides::create(&pop.db, &guide("homing", GuideStatus::Published))
        .await
        .unwrap();
    pop.admin(
        Method::PUT,
        "/api/admin/error-guides/klipper-must-home",
        Some(json!({"guide": "homing"})),
    )
    .await;
    guides::update(
        &pop.db,
        "homing",
        &guide("homing-first", GuideStatus::Published),
    )
    .await
    .unwrap();
    assert_eq!(
        pop.public().await,
        json!({"klipper-must-home": "homing-first"})
    );
    // Linked by its old slug, it is still that guide, shown by its slug now.
    let (c, _) = pop
        .admin(
            Method::PUT,
            "/api/admin/error-guides/klipper-move-out-of-range",
            Some(json!({"guide": "homing"})),
        )
        .await;
    assert_eq!(c, StatusCode::NO_CONTENT);
    assert_eq!(
        pop.public().await,
        json!({"klipper-must-home": "homing-first", "klipper-move-out-of-range": "homing-first"})
    );
}

#[tokio::test]
async fn unknown_errors_and_guides_are_refused_by_name() {
    let pop = pop().await;
    guides::create(&pop.db, &guide("homing", GuideStatus::Published))
        .await
        .unwrap();
    let (c, _) = pop
        .admin(
            Method::PUT,
            "/api/admin/error-guides/no-such-error",
            Some(json!({"guide": "homing"})),
        )
        .await;
    assert_eq!(c, StatusCode::NOT_FOUND);
    let (c, b) = pop
        .admin(
            Method::PUT,
            "/api/admin/error-guides/klipper-must-home",
            Some(json!({"guide": "nope"})),
        )
        .await;
    assert_eq!(c, StatusCode::BAD_REQUEST);
    assert!(
        b["error"]
            .as_str()
            .unwrap()
            .contains("no guide has the slug \"nope\""),
        "{b}"
    );
    assert_eq!(pop.public().await, json!({}));
}

#[tokio::test]
async fn linking_and_unlinking_are_recorded() {
    let pop = pop().await;
    guides::create(&pop.db, &guide("homing", GuideStatus::Published))
        .await
        .unwrap();
    pop.admin(
        Method::PUT,
        "/api/admin/error-guides/klipper-must-home",
        Some(json!({"guide": "homing"})),
    )
    .await;
    let (c, _) = pop
        .admin(
            Method::PUT,
            "/api/admin/error-guides/klipper-must-home",
            Some(json!({"guide": null})),
        )
        .await;
    assert_eq!(c, StatusCode::NO_CONTENT);
    assert_eq!(pop.public().await, json!({}));
    let (_, log) = pop
        .admin(
            Method::GET,
            "/api/admin/audit?kind=error_guide_linked",
            None,
        )
        .await;
    let summaries: Vec<&str> = log
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["summary"].as_str().unwrap())
        .collect();
    assert_eq!(
        summaries,
        [
            "Ada unlinked the guide from the error \"The printer was asked to move before finding its home position\"",
            "Ada linked the guide homing to the error \"The printer was asked to move before finding its home position\"",
        ]
    );
}
