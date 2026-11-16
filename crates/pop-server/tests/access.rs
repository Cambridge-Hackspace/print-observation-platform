//! Control is local (#7), through the router.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode},
    routing::{get, post},
};
use http_body_util::BodyExt;
use pop_server::{
    AppState,
    access::{Effect, Reach, Route, Who, violations},
    assets::Embedded,
    config::Access,
    db, router_with, routes,
};
use tower::ServiceExt;

async fn ok() -> &'static str {
    "done"
}

/// A route that would start a print, standing in for the real ones until
/// they exist, so the guard is proven on a route of the same kind.
fn start_print(reach: Reach) -> Route {
    Route {
        method: Method::POST,
        path: "/api/printers/{id}/start",
        effect: Effect::ControlsPrinter,
        reach,
        who: Who::Anyone,
        handler: post(ok),
    }
}

fn watch() -> Route {
    Route {
        method: Method::GET,
        path: "/api/printers/{id}/watch",
        effect: Effect::Read,
        reach: Reach::Anywhere,
        who: Who::Anyone,
        handler: get(ok),
    }
}

// ---- the issue's check, on the real table, and on tables built to fail it -----

/// No route that can change a printer is reachable from outside the space.
///
/// Today no route controls a printer, so on the real table this passes
/// trivially: what gives it teeth until one does is the self-test below, on a
/// table with such a route marked both ways, and the guard tests after it.
#[test]
fn no_route_that_controls_a_printer_is_reachable_from_outside() {
    assert_eq!(violations(&routes()), vec![]);
}

#[test]
fn the_check_fails_a_printer_route_marked_reachable_from_anywhere() {
    let table = vec![watch(), start_print(Reach::Anywhere)];
    let v = violations(&table);
    assert_eq!(v.len(), 1, "{v:?}");
    assert_eq!(v[0].path, "/api/printers/{id}/start");
    assert_eq!(v[0].method, Method::POST);
    assert_eq!(
        violations(&[watch(), start_print(Reach::LocalOnly)]),
        vec![]
    );
}

// ---- the guard, through a real router ---------------------------------------------

async fn app(table: Vec<Route>, access: Access) -> (tempfile::TempDir, Router) {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("pop.db")).await.unwrap();
    let state = AppState::new(pool, Arc::new(Embedded)).with_access(access);
    (dir, router_with(table, state))
}

fn shop() -> Access {
    Access {
        local_networks: vec!["192.168.1.0/24".parse().unwrap()],
        trusted_proxies: vec![],
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    peer: Option<&str>,
    headers: &[(&str, &str)],
) -> (StatusCode, String) {
    // A request that changes something must be JSON (#11), as every real
    // client's is.
    let json = matches!(method, Method::POST | Method::PUT | Method::PATCH);
    let mut req = Request::builder().method(method).uri(uri);
    if json {
        req = req.header("content-type", "application/json");
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
    let res = app.clone().oneshot(req).await.unwrap();
    let code = res.status();
    let body =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    (code, body)
}

#[tokio::test]
async fn a_local_only_route_refuses_a_remote_request() {
    let (_d, app) = app(vec![start_print(Reach::LocalOnly)], shop()).await;
    let (code, body) = send(
        &app,
        Method::POST,
        "/api/printers/x1/start",
        Some("203.0.113.9:4000"),
        &[],
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert!(body.contains("only be done at the space"), "{body}");
}

#[tokio::test]
async fn a_local_only_route_serves_the_space_and_this_machine() {
    let (_d, app) = app(vec![start_print(Reach::LocalOnly)], shop()).await;
    for peer in ["192.168.1.40:4000", "127.0.0.1:4000", "[::1]:4000"] {
        let (code, body) = send(
            &app,
            Method::POST,
            "/api/printers/x1/start",
            Some(peer),
            &[],
        )
        .await;
        assert_eq!((code, body.as_str()), (StatusCode::OK, "done"), "{peer}");
    }
}

#[tokio::test]
async fn a_local_only_route_refuses_a_request_with_no_peer_address() {
    let (_d, app) = app(vec![start_print(Reach::LocalOnly)], shop()).await;
    let (code, _) = send(&app, Method::POST, "/api/printers/x1/start", None, &[]).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_local_only_route_refuses_what_an_unnamed_proxy_passes_on() {
    let (_d, app) = app(vec![start_print(Reach::LocalOnly)], shop()).await;
    let (code, _) = send(
        &app,
        Method::POST,
        "/api/printers/x1/start",
        Some("127.0.0.1:4000"),
        &[("x-forwarded-for", "192.168.1.40")],
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_route_reachable_from_anywhere_serves_a_remote_request() {
    let (_d, app) = app(vec![watch(), start_print(Reach::LocalOnly)], shop()).await;
    let (code, body) = send(
        &app,
        Method::GET,
        "/api/printers/x1/watch",
        Some("203.0.113.9:4000"),
        &[],
    )
    .await;
    assert_eq!((code, body.as_str()), (StatusCode::OK, "done"));
}

/// Every local-only route in the real table refuses a remote request. Empty
/// today, like the table's printer routes; it covers each as it arrives.
#[tokio::test]
async fn every_local_only_route_of_the_real_table_refuses_a_remote_request() {
    let local: Vec<_> = routes()
        .into_iter()
        .filter(|r| r.reach == Reach::LocalOnly)
        .map(|r| (r.method.clone(), r.path.replace("{id}", "x1")))
        .collect();
    let (_d, app) = app(routes(), shop()).await;
    for (method, path) in local {
        let (code, _) = send(&app, method.clone(), &path, Some("203.0.113.9:4000"), &[]).await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{method} {path}");
    }
}

#[tokio::test]
async fn the_real_tables_routes_reachable_from_anywhere_serve_a_remote_request() {
    let (_d, app) = app(routes(), shop()).await;
    let (code, _) = send(
        &app,
        Method::GET,
        "/api/status",
        Some("203.0.113.9:4000"),
        &[],
    )
    .await;
    assert_eq!(code, StatusCode::OK);
}
