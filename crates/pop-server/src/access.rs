//! Control is local (ADR 0001, #7): anyone with access can watch from
//! anywhere, but nothing outside the space can start, pause, stop or resume a
//! printer, or change one. That is a property of the server, not of which
//! buttons a screen happens to hide.
//!
//! Every route is declared in one table ([`Route`]), with what it does
//! ([`Effect`]) and from where it may be reached ([`Reach`]). The router is
//! built from that table, and every local-only route is guarded: a remote
//! request gets 403. [`violations`] is the check that no route which controls
//! a printer is reachable from outside; a test runs it on the real table.
//!
//! A request is local when it comes from this machine or one of the
//! configured `access.local_networks` (and, once kiosk sessions exist (#12),
//! from a kiosk session). Forwarding headers are believed only from a
//! configured `access.trusted_proxies` peer; from anything else they make a
//! request remote, because a proxy on this machine would otherwise make every
//! request it passes on look local.

use std::net::{IpAddr, SocketAddr};

use axum::{
    Json, Router,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::MethodRouter,
};
use ipnet::IpNet;

use crate::{AppState, api::ApiError, config::Access};

/// What a route does, which decides from where it may be reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Reads, and changes nothing.
    Read,
    /// Reports a problem with a print or a printer.
    Report,
    /// Uploads a file to print later.
    Upload,
    /// Sends a print, confirms a cleared bed, starts, pauses, stops or resumes
    /// a printer, or changes a printer's settings.
    ControlsPrinter,
    /// Signs in or out, or changes an account.
    Account,
}

/// Who may use a route (#11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    /// Anyone, signed in or not.
    Anyone,
    /// Anyone signed in.
    Member,
    /// An admin.
    Admin,
}

/// From where a route may be reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    Anywhere,
    LocalOnly,
}

/// One route of the server.
pub struct Route {
    pub method: Method,
    pub path: &'static str,
    pub effect: Effect,
    pub reach: Reach,
    pub who: Who,
    pub handler: MethodRouter<AppState>,
}

/// A route of `table` that can change a printer from outside the space.
#[derive(Debug, PartialEq, Eq)]
pub struct Violation {
    pub method: Method,
    pub path: &'static str,
}

/// Every route in `table` that controls a printer but is reachable from
/// anywhere. Empty is the only acceptable answer.
pub fn violations(table: &[Route]) -> Vec<Violation> {
    table
        .iter()
        .filter(|r| r.effect == Effect::ControlsPrinter && r.reach != Reach::LocalOnly)
        .map(|r| Violation {
            method: r.method.clone(),
            path: r.path,
        })
        .collect()
}

/// A router with every route of `table`, each guarded as it declares: only
/// JSON for a request that changes something, then local-only, then who.
pub fn build(table: Vec<Route>, state: &AppState) -> Router<AppState> {
    let mut router = Router::new();
    for r in table {
        // Layers run outermost first: the last added is checked first.
        let who = r.who;
        let mut handler = match who {
            Who::Anyone => r.handler,
            _ => r.handler.route_layer(middleware::from_fn_with_state(
                state.clone(),
                move |s: State<AppState>, req: Request, next: Next| require(s, who, req, next),
            )),
        };
        if r.reach == Reach::LocalOnly {
            handler =
                handler.route_layer(middleware::from_fn_with_state(state.clone(), local_only));
        }
        if matches!(r.method, Method::POST | Method::PUT | Method::PATCH) {
            handler = handler.route_layer(middleware::from_fn(json_only));
        }
        router = router.route(r.path, handler);
    }
    router
}

fn refuse(code: StatusCode, error: &str) -> Response {
    (
        code,
        Json(ApiError {
            error: error.to_owned(),
        }),
    )
        .into_response()
}

/// A request that changes something must be JSON. A form on another site can
/// post to POP without asking, but cannot send JSON without the browser first
/// asking POP whether it may, which it never grants: so this, with the
/// session cookie's SameSite, is what stops another site acting as a member.
async fn json_only(req: Request, next: Next) -> Response {
    let json = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
    if json {
        next.run(req).await
    } else {
        refuse(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "send this as JSON (Content-Type: application/json)",
        )
    }
}

/// The session cookie's name.
pub const SESSION_COOKIE: &str = "pop_session";

/// The session token from a request's cookies, if it carries one.
pub fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == SESSION_COOKIE)
        .map(|(_, v)| v.to_owned())
}

/// Lets through who `who` names, with their account as an extension for the
/// handler; 401 for no one signed in, 403 for a member on an admin route.
async fn require(
    State(state): State<AppState>,
    who: Who,
    mut req: Request,
    next: Next,
) -> Response {
    let user = match session_token(req.headers()) {
        None => None,
        Some(t) => match crate::users::session_user(&state.db, &t).await {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("reading a session: {e:?}");
                return refuse(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "sign-in is not working right now",
                );
            }
        },
    };
    let Some(user) = user else {
        return refuse(StatusCode::UNAUTHORIZED, "sign in first");
    };
    if who == Who::Admin && user.role != crate::api::Role::Admin {
        return refuse(StatusCode::FORBIDDEN, "only an admin can do this");
    }
    req.extensions_mut().insert(user);
    next.run(req).await
}

async fn local_only(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    if is_local(&state.access, peer, req.headers()) {
        next.run(req).await
    } else {
        refuse(StatusCode::FORBIDDEN, "this can only be done at the space")
    }
}

/// Whether a request from `peer`, with `headers`, is from inside the space.
/// No peer address at all is remote: the check fails closed.
pub fn is_local(access: &Access, peer: Option<IpAddr>, headers: &HeaderMap) -> bool {
    let Some(peer) = peer.map(canonical) else {
        return false;
    };
    let forwarded = forwarded_for(headers);
    let client = match forwarded {
        None => peer,
        Some(_) if !within(&access.trusted_proxies, peer) => return false,
        Some(chain) => {
            // The client is the rightmost address no trusted proxy added: any
            // to its left came from the client, and can say anything.
            match chain
                .into_iter()
                .rev()
                .find(|ip| !within(&access.trusted_proxies, *ip))
            {
                Some(ip) => ip,
                // Every hop a trusted proxy, or an address that would not
                // parse: nothing says where the request began.
                None => return false,
            }
        }
    };
    client.is_loopback() || within(&access.local_networks, client)
}

/// The addresses a forwarding header names, oldest first, or None if the
/// request carries no forwarding header. A header present but unreadable is
/// an empty chain, which no request passes as local.
fn forwarded_for(headers: &HeaderMap) -> Option<Vec<IpAddr>> {
    let xff: Vec<_> = headers.get_all("x-forwarded-for").iter().collect();
    let fwd: Vec<_> = headers.get_all("forwarded").iter().collect();
    if xff.is_empty() && fwd.is_empty() {
        return None;
    }
    let mut chain = Vec::new();
    let mut unreadable = false;
    for v in fwd {
        let Ok(v) = v.to_str() else {
            unreadable = true;
            continue;
        };
        for element in v.split(',') {
            for pair in element.split(';') {
                let Some((k, val)) = pair.trim().split_once('=') else {
                    continue;
                };
                if k.trim().eq_ignore_ascii_case("for") {
                    match parse_node(val.trim().trim_matches('"')) {
                        Some(ip) => chain.push(ip),
                        None => unreadable = true,
                    }
                }
            }
        }
    }
    for v in xff {
        let Ok(v) = v.to_str() else {
            unreadable = true;
            continue;
        };
        for part in v.split(',') {
            match parse_node(part.trim()) {
                Some(ip) => chain.push(ip),
                None => unreadable = true,
            }
        }
    }
    Some(if unreadable { Vec::new() } else { chain })
}

/// An address from a forwarding header: `1.2.3.4`, `1.2.3.4:5678`,
/// `[2001:db8::1]` or `[2001:db8::1]:5678`.
fn parse_node(s: &str) -> Option<IpAddr> {
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Some(canonical(ip));
    }
    if let Ok(sa) = s.parse::<SocketAddr>() {
        return Some(canonical(sa.ip()));
    }
    let inner = s.strip_prefix('[')?.split(']').next()?;
    inner.parse::<IpAddr>().ok().map(canonical)
}

/// An IPv4 address carried in IPv6 (`::ffff:192.168.1.5`) as the IPv4
/// address it is, so the configured networks match it.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

fn within(nets: &[IpNet], ip: IpAddr) -> bool {
    nets.iter().any(|n| n.contains(&ip))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access(local: &[&str], proxies: &[&str]) -> Access {
        Access {
            local_networks: local.iter().map(|s| s.parse().unwrap()).collect(),
            trusted_proxies: proxies.iter().map(|s| s.parse().unwrap()).collect(),
        }
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, v.parse().unwrap());
        }
        h
    }

    fn ip(s: &str) -> Option<IpAddr> {
        Some(s.parse().unwrap())
    }

    const SHOP: &[&str] = &["192.168.1.0/24"];

    #[test]
    fn this_machine_is_local() {
        let a = access(&[], &[]);
        assert!(is_local(&a, ip("127.0.0.1"), &HeaderMap::new()));
        assert!(is_local(&a, ip("::1"), &HeaderMap::new()));
    }

    #[test]
    fn the_spaces_network_is_local_and_nowhere_else_is() {
        let a = access(SHOP, &[]);
        assert!(is_local(&a, ip("192.168.1.40"), &HeaderMap::new()));
        assert!(!is_local(&a, ip("192.168.2.40"), &HeaderMap::new()));
        assert!(!is_local(&a, ip("203.0.113.9"), &HeaderMap::new()));
    }

    #[test]
    fn an_ipv4_address_carried_in_ipv6_matches_its_network() {
        let a = access(SHOP, &[]);
        assert!(is_local(&a, ip("::ffff:192.168.1.40"), &HeaderMap::new()));
    }

    #[test]
    fn no_peer_address_is_remote() {
        assert!(!is_local(&access(SHOP, &[]), None, &HeaderMap::new()));
    }

    #[test]
    fn forwarding_headers_from_an_untrusted_peer_make_a_request_remote() {
        // A proxy on this machine nobody named: what it passes on is not local,
        // even though it connects from loopback.
        let a = access(SHOP, &[]);
        for h in [
            headers(&[("x-forwarded-for", "192.168.1.40")]),
            headers(&[("forwarded", "for=192.168.1.40")]),
        ] {
            assert!(!is_local(&a, ip("127.0.0.1"), &h), "{h:?}");
        }
    }

    #[test]
    fn a_trusted_proxy_passes_on_where_the_request_began() {
        let a = access(SHOP, &["127.0.0.1/32"]);
        assert!(is_local(
            &a,
            ip("127.0.0.1"),
            &headers(&[("x-forwarded-for", "192.168.1.40")])
        ));
        assert!(!is_local(
            &a,
            ip("127.0.0.1"),
            &headers(&[("x-forwarded-for", "203.0.113.9")])
        ));
        assert!(is_local(
            &a,
            ip("127.0.0.1"),
            &headers(&[(
                "forwarded",
                "for=\"[::ffff:192.168.1.40]:5050\";proto=https"
            )])
        ));
    }

    #[test]
    fn a_client_cannot_claim_to_be_local_through_a_trusted_proxy() {
        // The client sent "X-Forwarded-For: 192.168.1.40" itself; the proxy
        // appended the address it really came from.
        let a = access(SHOP, &["127.0.0.1/32"]);
        let h = headers(&[("x-forwarded-for", "192.168.1.40, 203.0.113.9")]);
        assert!(!is_local(&a, ip("127.0.0.1"), &h));
    }

    #[test]
    fn a_chain_of_trusted_proxies_is_walked_back_to_the_client() {
        let a = access(SHOP, &["127.0.0.1/32", "10.0.0.0/8"]);
        let h = headers(&[("x-forwarded-for", "203.0.113.9, 192.168.1.40, 10.1.2.3")]);
        assert!(is_local(&a, ip("127.0.0.1"), &h));
    }

    #[test]
    fn a_forwarding_header_that_cannot_be_read_is_remote() {
        let a = access(SHOP, &["127.0.0.1/32"]);
        for v in ["not-an-address", "for=_hidden", "192.168.1.40, garbage"] {
            let name = if v.starts_with("for=") {
                "forwarded"
            } else {
                "x-forwarded-for"
            };
            let mut h = HeaderMap::new();
            h.append(name, v.parse().unwrap());
            assert!(!is_local(&a, ip("127.0.0.1"), &h), "{name}: {v}");
        }
    }

    #[test]
    fn a_chain_of_nothing_but_trusted_proxies_is_remote() {
        let a = access(SHOP, &["127.0.0.1/32", "10.0.0.0/8"]);
        assert!(!is_local(
            &a,
            ip("127.0.0.1"),
            &headers(&[("x-forwarded-for", "10.0.0.1")])
        ));
    }
}
