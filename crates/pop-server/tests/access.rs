//! Who may do what, and from where (#7, #12), through the router: three
//! classes, local only on a network a kiosk has lately checked in from.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
    routing::{get, post, put},
};
use http_body_util::BodyExt;
use pop_server::{
    AppState,
    access::{
        Class, Effect, Need, Reach, Route, Violation, class, is_public, public_changes, violations,
    },
    api::Role,
    assets::Embedded,
    audit::Actor,
    config::Access,
    db, router_with, routes,
    users::{self, NewAccount},
};
use tower::ServiceExt;

async fn ok() -> &'static str {
    "done"
}

fn route(method: Method, path: &'static str, effect: Effect, need: Need, reach: Reach) -> Route {
    let handler = match method {
        Method::GET => get(ok),
        Method::PUT => put(ok),
        _ => post(ok),
    };
    Route {
        method,
        path,
        effect,
        need,
        reach,
        handler,
    }
}

/// A route that would start a print, standing in for the real ones until
/// they exist, so the guard is proven on a route of the same kind.
fn start_print(need: Need) -> Route {
    route(
        Method::POST,
        "/api/printers/{id}/start",
        Effect::ControlsPrinter,
        need,
        Reach::Anywhere,
    )
}

/// A route that shows names: visibility-class, the class "local" grants.
fn look() -> Route {
    route(
        Method::GET,
        "/api/printers/{id}/look",
        Effect::Read,
        Need::Class(Class::Visibility),
        Reach::Anywhere,
    )
}

fn watch() -> Route {
    route(
        Method::GET,
        "/api/printers/{id}/watch",
        Effect::Read,
        Need::Anyone,
        Reach::Anywhere,
    )
}

// ---- the classes, as cal's table has them (on #12) -----------------------------------

/// The table, written out here rather than taken from the code it checks.
#[test]
fn the_classes_are_cals_table() {
    use Class::*;
    let cells = [
        (true, None, Some(Visibility)),
        (true, Some(Role::Member), Some(Printer)),
        (true, Some(Role::Admin), Some(Admin)),
        (false, None, None),
        (false, Some(Role::Member), Some(Visibility)),
        (false, Some(Role::Admin), Some(Admin)),
    ];
    for (local, role, want) in cells {
        assert_eq!(class(local, role), want, "local {local}, {role:?}");
    }
    // Each includes the ones before it.
    assert!(Visibility < Printer && Printer < Admin);
}

// ---- the table's own check, on the real table and on tables built to fail it ---------

/// No route that controls a printer needs less than printer-class, and none
/// that changes a setting less than admin-class.
#[test]
fn no_route_needs_less_than_what_it_does() {
    assert_eq!(violations(&routes()), vec![]);
}

#[test]
fn the_check_fails_a_printer_route_that_needs_too_little() {
    for need in [Need::Anyone, Need::Account, Need::Class(Class::Visibility)] {
        let v = violations(&[watch(), start_print(need)]);
        assert_eq!(
            v,
            vec![Violation {
                method: Method::POST,
                path: "/api/printers/{id}/start"
            }],
            "{need:?}"
        );
    }
    for need in [Need::Class(Class::Printer), Need::Class(Class::Admin)] {
        assert_eq!(
            violations(&[watch(), start_print(need)]),
            vec![],
            "{need:?}"
        );
    }
    let setting = route(
        Method::PUT,
        "/api/admin/thing",
        Effect::Settings,
        Need::Class(Class::Printer),
        Reach::Anywhere,
    );
    assert_eq!(
        violations(&[setting]).len(),
        1,
        "a setting a member could change"
    );
}

#[test]
fn no_public_route_changes_anything_but_signing_in() {
    assert_eq!(public_changes(&routes()), []);
    // The public guides (#94) are in the table, and public.
    let public: Vec<&str> = routes()
        .iter()
        .filter(|r| is_public(r))
        .map(|r| r.path)
        .collect();
    for p in [
        "/api/public/guides",
        "/api/public/guides/{slug}",
        "/api/public/guides/{slug}/images/{id}",
    ] {
        assert!(public.contains(&p), "{p} is not public: {public:?}");
    }
}

#[test]
fn the_check_fails_a_public_route_that_writes() {
    // The oracle, fed what a broken table would hold: a public route that
    // writes a guide.
    let writes = route(
        Method::PUT,
        "/api/public/guides/{slug}",
        Effect::Content,
        Need::Anyone,
        Reach::Anywhere,
    );
    assert_eq!(
        public_changes(&[watch(), writes]),
        [Violation {
            method: Method::PUT,
            path: "/api/public/guides/{slug}"
        }]
    );
}

// ---- through a real router -----------------------------------------------------------

const SHOP: &str = "192.168.1.0/24";
const WORKSHOP: &str = "192.168.2.0/24";
const KIOSK_AT: &str = "192.168.1.5";
const IN_SHOP: &str = "192.168.1.40:4000";
const OUTSIDE: &str = "203.0.113.9:4000";

struct Pop {
    _dir: tempfile::TempDir,
    state: AppState,
    app: Router,
    member: String,
    admin: String,
}

fn shop() -> Access {
    Access {
        local_networks: vec![SHOP.parse().unwrap(), WORKSHOP.parse().unwrap()],
        trusted_proxies: vec![],
    }
}

async fn pop(table: Vec<Route>, access: Access) -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let cookie = |role, username: &'static str| {
        let db = db.clone();
        async move {
            let u = users::create(
                &db,
                NewAccount {
                    username,
                    name: username,
                    role,
                    password: "a long enough password",
                },
                &Actor::command_line(),
            )
            .await
            .unwrap();
            let t = users::start_session(&db, &u, false).await.unwrap();
            format!("pop_session={t}")
        }
    };
    let member = cookie(Role::Member, "grace").await;
    let admin = cookie(Role::Admin, "ada").await;
    let state = AppState::new(db, Arc::new(Embedded)).with_access(access);
    let app = router_with(table, state.clone());
    Pop {
        _dir: dir,
        state,
        app,
        member,
        admin,
    }
}

impl Pop {
    /// An enrolled kiosk checked in from `at`, `ago` ago.
    fn kiosk_seen(&self, at: &str, ago: Duration) {
        self.state
            .kiosks
            .saw_at(1, at.parse().unwrap(), Instant::now() - ago);
    }

    async fn send(
        &self,
        method: Method,
        uri: &str,
        peer: Option<&str>,
        cookie: Option<&str>,
        headers: &[(&str, &str)],
    ) -> StatusCode {
        let json = matches!(method, Method::POST | Method::PUT | Method::PATCH);
        let mut req = Request::builder().method(method).uri(uri);
        if json {
            req = req.header(header::CONTENT_TYPE, "application/json");
        }
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let mut req = req
            .body(if json {
                Body::from("{}")
            } else {
                Body::empty()
            })
            .unwrap();
        if let Some(p) = peer {
            req.extensions_mut()
                .insert(ConnectInfo(p.parse::<SocketAddr>().unwrap()));
        }
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let _ = res.into_body().collect().await;
        code
    }

    async fn look(&self, peer: Option<&str>, headers: &[(&str, &str)]) -> StatusCode {
        self.send(Method::GET, "/api/printers/x1/look", peer, None, headers)
            .await
    }
}

/// The space's network is local only with a kiosk's word for it: the same
/// request from the same address, with and without a check-in.
#[tokio::test]
async fn the_spaces_network_is_local_only_while_a_kiosk_vouches_for_it() {
    let pop = pop(vec![look()], shop()).await;
    assert_eq!(
        pop.look(Some(IN_SHOP), &[]).await,
        StatusCode::UNAUTHORIZED,
        "no kiosk yet"
    );
    pop.kiosk_seen(KIOSK_AT, Duration::from_secs(10));
    assert_eq!(
        pop.look(Some(IN_SHOP), &[]).await,
        StatusCode::OK,
        "a kiosk checked in"
    );
    // A kiosk two minutes and more silent vouches for nothing.
    pop.kiosk_seen(KIOSK_AT, Duration::from_secs(121));
    assert_eq!(
        pop.look(Some(IN_SHOP), &[]).await,
        StatusCode::UNAUTHORIZED,
        "a stale check-in"
    );
}

/// A kiosk vouches for ITS network: another configured network is not local
/// because a kiosk is on this one.
#[tokio::test]
async fn a_kiosk_vouches_for_its_own_network_only() {
    let pop = pop(vec![look()], shop()).await;
    pop.kiosk_seen("192.168.2.7", Duration::ZERO);
    assert_eq!(pop.look(Some(IN_SHOP), &[]).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        pop.look(Some("192.168.2.40:4000"), &[]).await,
        StatusCode::OK
    );
    // A kiosk heard from outside every configured network vouches for none.
    let pop = self::pop(vec![look()], shop()).await;
    pop.kiosk_seen("203.0.113.50", Duration::ZERO);
    assert_eq!(
        pop.look(Some("203.0.113.51:4000"), &[]).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn outside_this_machine_and_no_address_are_not_local() {
    let pop = pop(vec![look()], shop()).await;
    pop.kiosk_seen(KIOSK_AT, Duration::ZERO);
    for peer in [
        Some(OUTSIDE),
        Some("127.0.0.1:4000"),
        Some("[::1]:4000"),
        None,
    ] {
        assert_eq!(
            pop.look(peer, &[]).await,
            StatusCode::UNAUTHORIZED,
            "{peer:?}"
        );
    }
}

#[tokio::test]
async fn forwarding_headers_count_only_from_a_trusted_proxy_and_only_one_kind() {
    // An unnamed proxy on this machine: what it passes on is not local.
    let pop = pop(vec![look()], shop()).await;
    pop.kiosk_seen(KIOSK_AT, Duration::ZERO);
    let unnamed = pop
        .look(
            Some("127.0.0.1:4000"),
            &[("x-forwarded-for", "192.168.1.40")],
        )
        .await;
    assert_eq!(unnamed, StatusCode::UNAUTHORIZED);
    // A trusted one: where the request began, as it says.
    let mut behind = shop();
    behind.trusted_proxies = vec!["127.0.0.1/32".parse().unwrap()];
    let pop = self::pop(vec![look()], behind).await;
    pop.kiosk_seen(KIOSK_AT, Duration::ZERO);
    let via = |h: &'static [(&'static str, &'static str)]| pop.look(Some("127.0.0.1:4000"), h);
    assert_eq!(
        via(&[("forwarded", "for=192.168.1.40")]).await,
        StatusCode::OK
    );
    assert_eq!(
        via(&[("x-forwarded-for", "203.0.113.9")]).await,
        StatusCode::UNAUTHORIZED
    );
    // A client forging one header beside the proxy's other: refused.
    assert_eq!(
        via(&[
            ("forwarded", "for=203.0.113.9"),
            ("x-forwarded-for", "192.168.1.40")
        ])
        .await,
        StatusCode::UNAUTHORIZED
    );
}

/// Printer-class, by where and who: a member at the space, any admin; not a
/// member online, nobody signed out.
#[tokio::test]
async fn printer_control_is_for_members_at_the_space_and_admins() {
    let pop = pop(vec![start_print(Need::Class(Class::Printer))], shop()).await;
    pop.kiosk_seen(KIOSK_AT, Duration::ZERO);
    let start = |peer, cookie| {
        pop.send(
            Method::POST,
            "/api/printers/x1/start",
            Some(peer),
            cookie,
            &[],
        )
    };
    assert_eq!(start(IN_SHOP, Some(&pop.member)).await, StatusCode::OK);
    assert_eq!(
        start(OUTSIDE, Some(&pop.member)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(start(OUTSIDE, Some(&pop.admin)).await, StatusCode::OK);
    assert_eq!(
        start(IN_SHOP, None).await,
        StatusCode::UNAUTHORIZED,
        "visibility is not control"
    );
    assert_eq!(start(OUTSIDE, None).await, StatusCode::UNAUTHORIZED);
}

/// Every admin route of the real table refuses a member even at the space,
/// where a member holds printer-class: admin is a class above.
#[tokio::test]
async fn every_admin_route_refuses_a_member_at_the_space() {
    let pop = pop(routes(), shop()).await;
    pop.kiosk_seen(KIOSK_AT, Duration::ZERO);
    let admin: Vec<_> = routes()
        .into_iter()
        .filter(|r| r.need == Need::Class(Class::Admin))
        .map(|r| {
            (
                r.method.clone(),
                r.path
                    .replace("{id}", "1")
                    .replace("{bam_id}", "x1")
                    .replace("{slug}", "a")
                    .replace("{key}", "k"),
            )
        })
        .collect();
    assert!(admin.len() > 20, "{admin:?}");
    for (method, path) in admin {
        let code = pop
            .send(method.clone(), &path, Some(IN_SHOP), Some(&pop.member), &[])
            .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{method} {path}");
    }
}

/// The updater's route is this machine's, directly: not through a proxy on
/// it, and not from the space (#148).
#[tokio::test]
async fn a_this_machine_route_is_for_this_machine_directly() {
    let quiet = route(
        Method::GET,
        "/quiet",
        Effect::Read,
        Need::Anyone,
        Reach::ThisMachine,
    );
    let mut behind = shop();
    behind.trusted_proxies = vec!["127.0.0.1/32".parse().unwrap()];
    let pop = pop(vec![quiet], behind).await;
    pop.kiosk_seen(KIOSK_AT, Duration::ZERO);
    let get = |peer, h| pop.send(Method::GET, "/quiet", Some(peer), None, h);
    assert_eq!(get("127.0.0.1:4000", &[]).await, StatusCode::OK);
    assert_eq!(get("[::1]:4000", &[]).await, StatusCode::OK);
    assert_eq!(
        get("127.0.0.1:4000", &[("x-forwarded-for", "127.0.0.1")]).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(get(IN_SHOP, &[]).await, StatusCode::FORBIDDEN);
    assert_eq!(get(OUTSIDE, &[]).await, StatusCode::FORBIDDEN);
}
