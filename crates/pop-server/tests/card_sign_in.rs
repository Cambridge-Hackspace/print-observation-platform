//! A card tap at a kiosk's reader signs the member in there (#18), where POP
//! is a CS:S device and the edge's sealed lease says who tapped.
//!
//! Taps reach POP two ways here: handed to `CardSignIns::heard` (most tests,
//! which are about what happens at the kiosk), and once through a real
//! broker and POP's device end, with a test playing the edge through CS:S's
//! own seal code (`a_sealed_lease_on_the_broker_reaches_its_kiosk`).
//!
//! WHAT THIS DOES NOT PROVE: that CS:S's edge leases POP when a member taps,
//! with `member` on it. That is the edge's half, pinned by CS:S's own e2e
//! (`e2e/drivers/signedwire.mjs`). Nor that a tap goes stale after
//! `TAP_FOR`: a minute is too long for this suite to wait out.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use css_wire::{LocalAliveRequest, ResourceLeasePayload};
use css_wire_sign::Peer;
use http_body_util::BodyExt;
use pop_integrations::{
    css_device::{self, Broker, LocalMember, Tap, Wire},
    fakes::broker::FakeBroker,
};
use pop_server::{
    AppState,
    api::Role,
    assets::Embedded,
    audit::Actor,
    card_sign_in::{CardSignIns, first_name},
    config::Provider,
    css_device::{Identity, load, save},
    db, router,
    secrets::Secrets,
    sign_in::SignIns,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

const PUBLIC: &str = "https://pop.example.org";
const PW: &str = "a long enough password";
const AT_KIOSK: &str = "192.168.1.5:4000";
const COMMAND_KEY: &str = "pop-command-key";
/// 32 bytes, base64: POP's secrets key.
const SECRETS_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    cards: CardSignIns,
    admin: String,
}

struct Res {
    code: StatusCode,
    headers: HeaderMap,
    body: Value,
}

fn provider() -> Provider {
    toml::from_str(
        "id = \"css\"\nlabel = \"CS:S\"\nissuer = \"https://css.example.org\"\nclient_id = \"pop\"\n",
    )
    .unwrap()
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
            password: PW,
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    let cards = CardSignIns::new("css").with_long_poll(Duration::from_millis(300));
    let (_tx, hearing) = tokio::sync::watch::channel(None);
    let state = AppState::new(db.clone(), Arc::new(Embedded))
        .with_sign_ins(SignIns::new(vec![provider()], PUBLIC))
        .with_card_sign_ins(cards.clone(), hearing);
    let mut pop = Pop {
        _dir: dir,
        db,
        app: router(state),
        cards,
        admin: String::new(),
    };
    let r = pop
        .send(
            Method::POST,
            "/api/session",
            None,
            Some(json!({"username": "ada", "password": PW})),
        )
        .await;
    assert_eq!(r.code, StatusCode::OK, "{}", r.body);
    pop.admin = set_cookie(&r.headers, "pop_session");
    pop
}

/// The `name=value` of the cookie `name` a response sets.
fn set_cookie(h: &HeaderMap, name: &str) -> String {
    h.get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .find(|v| v.starts_with(&format!("{name}=")))
        .unwrap_or_else(|| panic!("no {name} cookie in {h:?}"))
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

fn member(id: &str, name: &str) -> LocalMember {
    LocalMember {
        id: id.into(),
        name: name.into(),
    }
}

impl Pop {
    async fn send(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> Res {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        let mut req = match body {
            Some(b) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(AT_KIOSK.parse::<SocketAddr>().unwrap()));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        Res {
            code,
            headers,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    /// A kiosk, enrolled, with its card reader at `resource`: its cookie.
    async fn kiosk(&self, name: &str, resource: &str) -> String {
        let r = self
            .send(
                Method::POST,
                "/api/admin/kiosks",
                Some(&self.admin),
                Some(json!({"name": name})),
            )
            .await;
        assert_eq!(r.code, StatusCode::CREATED, "{}", r.body);
        let cookie = set_cookie(&r.headers, "pop_kiosk");
        let id = r.body["id"].as_i64().unwrap();
        let r = self.reader(id, resource).await;
        assert_eq!(r.code, StatusCode::NO_CONTENT, "{}", r.body);
        cookie
    }

    async fn reader(&self, id: i64, resource: &str) -> Res {
        self.send(
            Method::PUT,
            &format!("/api/admin/kiosks/{id}/card-reader"),
            Some(&self.admin),
            Some(json!({"resource": resource})),
        )
        .await
    }

    async fn tap(&self, resource: &str, m: LocalMember) {
        self.cards
            .heard(
                &self.db,
                Tap {
                    resource: resource.into(),
                    member: m,
                },
            )
            .await;
    }

    async fn waiting(&self, kiosk: &str) -> Res {
        self.send(Method::GET, "/api/kiosk/tap", Some(kiosk), None)
            .await
    }

    async fn take(&self, kiosk: &str) -> Res {
        // As the kiosk sends it: JSON, as every POST to POP must be.
        self.send(Method::POST, "/api/kiosk/tap", Some(kiosk), Some(json!({})))
            .await
    }

    async fn audit(&self, kind: &str) -> Vec<String> {
        sqlx::query_scalar("SELECT summary FROM audit WHERE kind = ? ORDER BY id")
            .bind(kind)
            .fetch_all(&self.db)
            .await
            .unwrap()
    }
}

// ---- the tests ----

#[tokio::test]
async fn a_tap_at_a_kiosks_reader_signs_the_member_in_there() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    pop.tap("tower-1", member("u-alex", "Alex Doe")).await;

    // The kiosk is told whom to welcome, by their first name.
    let w = pop.waiting(&kiosk).await;
    assert_eq!(w.code, StatusCode::OK);
    assert_eq!(w.body, json!({"tap": {"kind": "sign_in", "name": "Alex"}}));

    let r = pop.take(&kiosk).await;
    assert_eq!(r.code, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["name"], "Alex Doe");
    assert_eq!(r.body["role"], "member", "{}", r.body);
    let session = set_cookie(&r.headers, "pop_session");

    // A kiosk session, as any sign-in at an enrolled kiosk is (#12).
    let s = pop
        .send(Method::GET, "/api/session", Some(&session), None)
        .await;
    assert_eq!(s.code, StatusCode::OK);
    assert!(
        s.body["idle_secs"].is_number(),
        "a kiosk session: {}",
        s.body
    );
    let rec = pop.audit("signed_in").await;
    assert!(
        rec.iter()
            .any(|r| r == "Alex Doe signed in with a CS:S card at the kiosk"),
        "{rec:?}"
    );
    // And the tap is spent.
    assert_eq!(pop.waiting(&kiosk).await.body, json!({"tap": null}));
    assert_eq!(pop.take(&kiosk).await.code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_same_member_is_the_same_account_by_card_and_by_browser() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    // Alex signed in in a browser once: the account their CS:S sub links to.
    let browser = users::create_from_provider(&pop.db, "css", "CS:S", "u-alex", "alex", "Alex Doe")
        .await
        .unwrap();
    pop.tap("tower-1", member("u-alex", "Alex Doe")).await;
    let r = pop.take(&kiosk).await;
    assert_eq!(r.code, StatusCode::OK, "{}", r.body);
    assert_eq!(
        r.body["id"],
        json!(browser.id),
        "the account a browser sign-in made"
    );
    // And one made by a first tap is the one a later tap reaches.
    pop.tap("tower-1", member("u-sam", "Sam Roe")).await;
    let first = pop.take(&kiosk).await.body["id"].clone();
    pop.tap("tower-1", member("u-sam", "Sam Roe")).await;
    let again = pop.take(&kiosk).await.body["id"].clone();
    assert!(first.is_number());
    assert_eq!(first, again, "a second tap is the same account");
    let linked: i64 = sqlx::query_scalar(
        "SELECT user_id FROM user_identities WHERE provider = 'css' AND subject = 'u-sam'",
    )
    .fetch_one(&pop.db)
    .await
    .unwrap();
    assert_eq!(json!(linked), first, "linked by the provider's sub");
}

#[tokio::test]
async fn a_card_sign_in_never_makes_or_unmakes_an_admin() {
    // A lease names no roles: the mutant is a card sign-in that applied the
    // provider's role mapping with nothing to map, and took the admin away.
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    let u = users::create_from_provider(&pop.db, "css", "CS:S", "u-boss", "boss", "Bo Ss")
        .await
        .unwrap();
    users::apply_provider_roles(&pop.db, u, "css", "CS:S", true)
        .await
        .unwrap();
    pop.tap("tower-1", member("u-boss", "Bo Ss")).await;
    let r = pop.take(&kiosk).await;
    assert_eq!(r.code, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["role"], "admin", "{}", r.body);
}

#[tokio::test]
async fn a_tap_is_for_its_own_kiosk_and_for_no_other() {
    let pop = pop().await;
    let one = pop.kiosk("Tower one", "tower-1").await;
    let two = pop.kiosk("Tower two", "tower-2").await;
    // A reader no kiosk has: no sign-in anywhere.
    pop.tap("tower-9", member("u-x", "Ex")).await;
    assert_eq!(pop.waiting(&one).await.body, json!({"tap": null}));
    assert_eq!(pop.waiting(&two).await.body, json!({"tap": null}));
    pop.tap("tower-1", member("u-alex", "Alex Doe")).await;
    // The control first: the right kiosk has it...
    assert_eq!(
        pop.waiting(&one).await.body,
        json!({"tap": {"kind": "sign_in", "name": "Alex"}})
    );
    // ...and the other, waiting its while, does not, and cannot take it.
    assert_eq!(pop.waiting(&two).await.body, json!({"tap": null}));
    assert_eq!(pop.take(&two).await.code, StatusCode::NOT_FOUND);
    assert_eq!(pop.take(&one).await.code, StatusCode::OK);
}

#[tokio::test]
async fn a_newer_tap_at_a_kiosk_replaces_the_one_waiting() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    pop.tap("tower-1", member("u-alex", "Alex Doe")).await;
    pop.tap("tower-1", member("u-sam", "Sam Roe")).await;
    assert_eq!(
        pop.waiting(&kiosk).await.body,
        json!({"tap": {"kind": "sign_in", "name": "Sam"}})
    );
    assert_eq!(pop.take(&kiosk).await.body["name"], "Sam Roe");
    assert_eq!(
        pop.take(&kiosk).await.code,
        StatusCode::NOT_FOUND,
        "one tap, once"
    );
}

#[tokio::test]
async fn a_tap_set_aside_signs_nobody_in() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    pop.tap("tower-1", member("u-alex", "Alex Doe")).await;
    let r = pop
        .send(Method::DELETE, "/api/kiosk/tap", Some(&kiosk), None)
        .await;
    assert_eq!(r.code, StatusCode::NO_CONTENT);
    assert_eq!(pop.waiting(&kiosk).await.body, json!({"tap": null}));
    assert_eq!(pop.take(&kiosk).await.code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_kiosk_waiting_hears_a_tap_as_it_happens() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    let cards = pop.cards.clone();
    let db = pop.db.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cards
            .heard(
                &db,
                Tap {
                    resource: "tower-1".into(),
                    member: member("u-alex", "Alex Doe"),
                },
            )
            .await;
    });
    // Asked before the tap; answered when it comes, inside the wait.
    let started = std::time::Instant::now();
    let w = pop.waiting(&kiosk).await;
    assert_eq!(w.body, json!({"tap": {"kind": "sign_in", "name": "Alex"}}));
    assert!(
        started.elapsed() < Duration::from_millis(290),
        "answered at the tap, not at the end of the wait"
    );
}

#[tokio::test]
async fn a_deactivated_account_is_not_signed_in_by_card() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-1").await;
    pop.tap("tower-1", member("u-alex", "Alex Doe")).await;
    let id = pop.take(&kiosk).await.body["id"].as_i64().unwrap();
    let r = pop
        .send(
            Method::PUT,
            &format!("/api/admin/users/{id}/active"),
            Some(&pop.admin),
            Some(json!({"active": false})),
        )
        .await;
    assert!(r.code.is_success(), "{}", r.body);
    pop.tap("tower-1", member("u-alex", "Alex Doe")).await;
    let r = pop.take(&kiosk).await;
    assert_eq!(r.code, StatusCode::FORBIDDEN);
    assert!(r.headers.get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn a_card_reader_belongs_to_one_kiosk_and_is_recorded() {
    let pop = pop().await;
    let _one = pop.kiosk("Tower one", "tower-1").await;
    let r = pop
        .send(
            Method::POST,
            "/api/admin/kiosks",
            Some(&pop.admin),
            Some(json!({"name": "Tower two"})),
        )
        .await;
    let two = r.body["id"].as_i64().unwrap();
    let taken = pop.reader(two, "tower-1").await;
    assert_eq!(taken.code, StatusCode::CONFLICT, "{}", taken.body);
    assert_eq!(
        pop.reader(two, "x".repeat(129).as_str()).await.code,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(pop.reader(999, "tower-3").await.code, StatusCode::NOT_FOUND);
    assert_eq!(
        pop.reader(two, "tower-2").await.code,
        StatusCode::NO_CONTENT
    );
    // The desk's list shows each kiosk's reader.
    let list = pop
        .send(Method::GET, "/api/admin/kiosks", Some(&pop.admin), None)
        .await;
    let readers: Vec<Value> = list
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["card_resource"].clone())
        .collect();
    assert_eq!(readers, [json!("tower-1"), json!("tower-2")]);
    // Taken off, it is free for another; and each change is recorded.
    assert_eq!(pop.reader(two, "").await.code, StatusCode::NO_CONTENT);
    assert_eq!(
        pop.audit("kiosk_reader_set").await,
        [
            "Ada set the kiosk Tower one's card reader to tower-1",
            "Ada set the kiosk Tower two's card reader to tower-2",
            "Ada took the card reader off the kiosk Tower two",
        ]
    );
    // A revoked kiosk's reader is free too.
    let r = pop
        .send(
            Method::DELETE,
            "/api/admin/kiosks/1",
            Some(&pop.admin),
            None,
        )
        .await;
    assert_eq!(r.code, StatusCode::NO_CONTENT);
    assert_eq!(
        pop.reader(two, "tower-1").await.code,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn a_sealed_lease_on_the_broker_reaches_its_kiosk_and_a_forged_one_does_not() {
    let pop = pop().await;
    let kiosk = pop.kiosk("The tower", "tower-uuid").await;
    let broker = FakeBroker::start(None).await;
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let mut n = 0;
    let wire = Wire::new("dev-pop", COMMAND_KEY, move || {
        n += 1;
        format!("pop-{n}")
    });
    let (mut hearing, device) =
        css_device::spawn(Broker::parse(&broker.url(), "", "").unwrap(), wire, tx);
    let keeper = pop.cards.clone().spawn(pop.db.clone(), rx);
    tokio::time::timeout(Duration::from_secs(10), hearing.wait_for(Option::is_none))
        .await
        .unwrap()
        .unwrap();
    // The edge hears POP's sealed alive, and so can seal to it.
    let mut edge = Peer::new("edge-1".into());
    let alive = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(p) = broker.published(css_device::ALIVE).first() {
                return p.clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("an alive pulse");
    let a: LocalAliveRequest = serde_json::from_slice(&alive).unwrap();
    let _ = edge.open(COMMAND_KEY.as_bytes(), &a, || "edge-2".into());
    let lease = |m: &str| ResourceLeasePayload {
        resource: "tower-uuid".into(),
        device_id: "dev-pop".into(),
        grant: true,
        ttl_ms: 3000,
        reason: None,
        member: Some(member(m, "Alex Doe")),
        seal: None,
    };

    // A forged lease first, many times and with time to arrive: no tap.
    let forged = serde_json::to_vec(&lease("u-mallory")).unwrap();
    for _ in 0..5 {
        broker.publish("resource/lease", forged.clone());
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        pop.waiting(&kiosk).await.body,
        json!({"tap": null}),
        "an unsigned lease on the broker signed someone in"
    );

    // The edge's own, sealed: the kiosk is told.
    let mut real = lease("u-alex");
    edge.seal(COMMAND_KEY.as_bytes(), &mut real);
    let real = serde_json::to_vec(&real).unwrap();
    let seen = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            broker.publish("resource/lease", real.clone());
            let w = pop.waiting(&kiosk).await;
            if !w.body["tap"].is_null() {
                return w.body;
            }
        }
    })
    .await
    .expect("the tap to reach the kiosk");
    assert_eq!(seen, json!({"tap": {"kind": "sign_in", "name": "Alex"}}));
    let r = pop.take(&kiosk).await;
    assert_eq!(r.code, StatusCode::OK, "{}", r.body);
    let linked: Vec<String> =
        sqlx::query_scalar("SELECT subject FROM user_identities WHERE provider = 'css'")
            .fetch_all(&pop.db)
            .await
            .unwrap();
    assert_eq!(linked, ["u-alex"], "and never the forgery's member");
    device.abort();
    keeper.abort();
}

#[tokio::test]
async fn pops_identity_is_kept_sealed_and_never_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let secrets = Secrets::new(SECRETS_KEY);
    assert_eq!(load(&db, &secrets).await.unwrap(), None);
    let id = Identity {
        device_id: "dev-pop".into(),
        auth_token: "tok".into(),
        command_key: Some(COMMAND_KEY.into()),
    };
    save(&db, &secrets, &id).await.unwrap();
    assert_eq!(load(&db, &secrets).await.unwrap(), Some(id.clone()));
    // Sealed: the key is nowhere in the database as it is written.
    let raw: Vec<u8> =
        sqlx::query_scalar("SELECT sealed FROM sealed_settings WHERE key = 'css_device'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(
        !String::from_utf8_lossy(&raw).contains(COMMAND_KEY),
        "the command key is stored in the clear"
    );
    // A second registration would orphan CS:S's bindings to the first.
    let other = Identity {
        device_id: "dev-other".into(),
        ..id.clone()
    };
    let e = save(&db, &secrets, &other).await.unwrap_err();
    assert!(e.contains("already registered"), "{e}");
    assert_eq!(load(&db, &secrets).await.unwrap(), Some(id));
}

#[test]
fn a_kiosk_greets_a_member_by_the_first_word_of_their_name() {
    assert_eq!(first_name("Alex Doe"), "Alex");
    assert_eq!(first_name("  Cal  "), "Cal");
    assert_eq!(first_name("Mary Ann Smith"), "Mary");
    assert_eq!(first_name(""), "");
}
