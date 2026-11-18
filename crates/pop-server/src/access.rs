//! Who may do what, and from where (#7, #12): three classes of permission,
//! each including the one before it (cal, 2026-11-18, on #12).
//!
//! - **visibility**: see everything going on, names included;
//! - **printer**: control printers;
//! - **admin**: configure the system.
//!
//! | | not signed in | member | admin |
//! |---|---|---|---|
//! | local: the kiosk, or a device on the space's network | visibility | printer | admin |
//! | online | none | visibility | admin |
//!
//! A request is LOCAL when it began on one of the configured
//! `access.local_networks` AND an enrolled kiosk has checked in from that same
//! network within [`crate::kiosks::FRESH`] (crate::kiosks). The kiosk confirms
//! from its own side: POP believes the address its check-in came from, never
//! anything a request says about itself, so a mistyped network or a POP moved
//! elsewhere grants nothing on its own.
//!
//! Every route is declared in one table ([`Route`]) with what it does
//! ([`Effect`]) and what it needs ([`Need`]); the router is built from that
//! table, so a route cannot exist without being guarded. [`violations`] is the
//! check that no route which controls a printer needs less than printer-class.
//!
//! Forwarding headers are believed only from a configured
//! `access.trusted_proxies` peer; from anything else they make a request
//! remote, because a proxy on this machine would otherwise make every request
//! it passes on look local.

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
    /// Changes a tower-wide setting, such as where group notices go (#68).
    Settings,
    /// Writes guides (#71): nothing that reaches a printer.
    Content,
}

/// The classes of permission, each including the ones before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// See everything going on, names included.
    Visibility,
    /// Control printers.
    Printer,
    /// Configure the system.
    Admin,
}

/// A request's class: local or online, signed in or not, as cal's table
/// says. None is no class at all: a stranger online.
pub fn class(local: bool, role: Option<crate::api::Role>) -> Option<Class> {
    use crate::api::Role;
    match (local, role) {
        (_, Some(Role::Admin)) => Some(Class::Admin),
        (true, Some(Role::Member)) => Some(Class::Printer),
        (true, None) | (false, Some(Role::Member)) => Some(Class::Visibility),
        (false, None) => None,
    }
}

/// What a route needs of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Nothing: anyone, from anywhere.
    Anyone,
    /// A signed-in account, of any class: its own session and settings.
    Account,
    /// At least this class.
    Class(Class),
}

/// From where a route may be reached, beside its [`Need`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Wherever its need is met.
    Anywhere,
    /// From this machine only, with no proxy in between: the updater (#148).
    ThisMachine,
}

/// One route of the server.
pub struct Route {
    pub method: Method,
    pub path: &'static str,
    pub effect: Effect,
    pub need: Need,
    pub reach: Reach,
    pub handler: MethodRouter<AppState>,
}

/// A route of `table` that can change a printer from outside the space.
#[derive(Debug, PartialEq, Eq)]
pub struct Violation {
    pub method: Method,
    pub path: &'static str,
}

/// Every route in `table` that controls a printer and needs less than
/// printer-class, or changes a setting and needs less than admin-class.
/// Empty is the only acceptable answer.
pub fn violations(table: &[Route]) -> Vec<Violation> {
    let at_least = |r: &Route, c: Class| matches!(r.need, Need::Class(n) if n >= c);
    table
        .iter()
        .filter(|r| match r.effect {
            Effect::ControlsPrinter => !at_least(r, Class::Printer),
            Effect::Settings => !at_least(r, Class::Admin),
            _ => false,
        })
        .map(|r| Violation {
            method: r.method.clone(),
            path: r.path,
        })
        .collect()
}

/// Whether a stranger can use `r`: anyone, from anywhere.
pub fn is_public(r: &Route) -> bool {
    r.need == Need::Anyone && r.reach == Reach::Anywhere
}

/// Every public route of `table` that can change something. Signing in is
/// the one that may: it is how a stranger becomes someone, and it changes
/// nothing but their own session. Empty is the only other acceptable answer.
pub fn public_changes(table: &[Route]) -> Vec<Violation> {
    table
        .iter()
        .filter(|r| is_public(r) && r.effect != Effect::Read)
        .filter(|r| !(r.method == Method::POST && r.path == "/api/session"))
        .map(|r| Violation {
            method: r.method.clone(),
            path: r.path,
        })
        .collect()
}

/// A router with every route of `table`, each guarded as it declares: only
/// JSON for a request that changes something, then its reach, then its need.
pub fn build(table: Vec<Route>, state: &AppState) -> Router<AppState> {
    let mut router = Router::new();
    for r in table {
        // Layers run outermost first: the last added is checked first.
        let need = r.need;
        let mut handler = match need {
            Need::Anyone => r.handler,
            _ => r.handler.route_layer(middleware::from_fn_with_state(
                state.clone(),
                move |s: State<AppState>, req: Request, next: Next| require(s, need, req, next),
            )),
        };
        if r.reach == Reach::ThisMachine {
            handler = handler.route_layer(middleware::from_fn(this_machine));
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

/// The peer address of a request, as the socket gives it.
pub fn peer(req: &Request) -> Option<IpAddr> {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
}

/// Lets through what `need` names, with the account, if any, as an
/// extension for the handler; 401 when signing in would help, 403 when it
/// would not.
async fn require(
    State(state): State<AppState>,
    need: Need,
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
    let local = is_local(&state, peer(&req), req.headers());
    let has = class(local, user.as_ref().map(|u| u.role));
    let refused = match need {
        Need::Anyone => None,
        Need::Account => user
            .is_none()
            .then_some((StatusCode::UNAUTHORIZED, "sign in first")),
        Need::Class(c) if has >= Some(c) => None,
        Need::Class(Class::Admin) if user.is_some() => {
            Some((StatusCode::FORBIDDEN, "only an admin can do this"))
        }
        Need::Class(Class::Printer) if user.is_some() => Some((
            StatusCode::FORBIDDEN,
            "this is done at the space: at the print station, or on the space's network",
        )),
        Need::Class(_) => Some((StatusCode::UNAUTHORIZED, "sign in first")),
    };
    if let Some((code, why)) = refused {
        return refuse(code, why);
    }
    if let Some(user) = user {
        req.extensions_mut().insert(user);
    }
    next.run(req).await
}

/// From this machine, directly: a loopback peer and no forwarding header,
/// since a proxy on this machine would otherwise pass anyone off as it.
async fn this_machine(req: Request, next: Next) -> Response {
    let direct = peer(&req).is_some_and(|p| canonical(p).is_loopback())
        && forwarded_for(req.headers()).is_none();
    if direct {
        next.run(req).await
    } else {
        refuse(StatusCode::FORBIDDEN, "this is for POP's own machine")
    }
}

/// Whether a request from `peer`, with `headers`, is LOCAL: it began on one of
/// the space's networks, and an enrolled kiosk has checked in from that same
/// network lately. No peer at all is not local: the check fails closed.
pub fn is_local(state: &AppState, peer: Option<IpAddr>, headers: &HeaderMap) -> bool {
    let Some(client) = space_network_client(&state.access, peer, headers) else {
        return false;
    };
    state
        .access
        .local_networks
        .iter()
        .filter(|n| n.contains(&client))
        .any(|n| state.kiosks.seen_on(n))
}

/// Where a request began, when that is one of the space's networks: the
/// client's address, the forwarding chain believed only from a trusted proxy.
/// None is outside, or unknowable, and both are treated as outside. This alone
/// is not "local": that needs a kiosk's word too ([`is_local`]).
pub fn space_network_client(
    access: &Access,
    peer: Option<IpAddr>,
    headers: &HeaderMap,
) -> Option<IpAddr> {
    client_address(access, peer, headers).filter(|c| within(&access.local_networks, *c))
}

/// Where a request began: the peer, or the forwarding chain's client when the
/// peer is a trusted proxy. None when that cannot be known.
pub fn client_address(
    access: &Access,
    peer: Option<IpAddr>,
    headers: &HeaderMap,
) -> Option<IpAddr> {
    let peer = peer.map(canonical)?;
    let forwarded = forwarded_for(headers);
    let client = match forwarded {
        None => peer,
        Some(_) if !within(&access.trusted_proxies, peer) => return None,
        Some(chain) => {
            // The client is the rightmost address no trusted proxy added: any
            // to its left came from the client, and can say anything.
            // Every hop a trusted proxy, or an address that would not parse:
            // nothing says where the request began.
            chain
                .into_iter()
                .rev()
                .find(|ip| !within(&access.trusted_proxies, *ip))?
        }
    };
    Some(canonical(client))
}

/// The addresses a forwarding header names, oldest first, or None if the
/// request carries no forwarding header. A header present but unreadable is
/// an empty chain, which no request passes as local.
///
/// So is a request carrying both `Forwarded` and `X-Forwarded-For`. Nothing
/// says how the two interleave, so neither order can be trusted: a proxy that
/// adds `Forwarded` but passes on the client's own `X-Forwarded-For` would
/// otherwise let a client outside the space name an address inside it, and
/// have that taken as where it came from. A proxy in front of POP should set
/// one of them.
fn forwarded_for(headers: &HeaderMap) -> Option<Vec<IpAddr>> {
    let xff: Vec<_> = headers.get_all("x-forwarded-for").iter().collect();
    let fwd: Vec<_> = headers.get_all("forwarded").iter().collect();
    if xff.is_empty() && fwd.is_empty() {
        return None;
    }
    if !xff.is_empty() && !fwd.is_empty() {
        return Some(Vec::new());
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

    /// Whether a request began on the space's network: the address half of
    /// "local", which a kiosk's check-in completes (tests/access.rs).
    fn on(a: &Access, peer: Option<IpAddr>, h: &HeaderMap) -> bool {
        space_network_client(a, peer, h).is_some()
    }

    /// POP's own machine is not the space unless it is on a configured
    /// network: a tunnel to it (SSH, a tunnel daemon) once counted as local
    /// (threat model gap 2), and under #12 nothing does by its address alone.
    #[test]
    fn this_machine_is_not_the_space_by_itself() {
        let a = access(&[], &[]);
        assert!(!on(&a, ip("127.0.0.1"), &HeaderMap::new()));
        assert!(!on(&a, ip("::1"), &HeaderMap::new()));
        let a = access(&["127.0.0.0/8"], &[]);
        assert!(on(&a, ip("127.0.0.1"), &HeaderMap::new()), "named, it is");
    }

    #[test]
    fn the_spaces_network_is_local_and_nowhere_else_is() {
        let a = access(SHOP, &[]);
        assert!(on(&a, ip("192.168.1.40"), &HeaderMap::new()));
        assert!(!on(&a, ip("192.168.2.40"), &HeaderMap::new()));
        assert!(!on(&a, ip("203.0.113.9"), &HeaderMap::new()));
    }

    #[test]
    fn an_ipv4_address_carried_in_ipv6_matches_its_network() {
        let a = access(SHOP, &[]);
        assert!(on(&a, ip("::ffff:192.168.1.40"), &HeaderMap::new()));
    }

    #[test]
    fn no_peer_address_is_remote() {
        assert!(!on(&access(SHOP, &[]), None, &HeaderMap::new()));
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
            assert!(!on(&a, ip("127.0.0.1"), &h), "{h:?}");
        }
    }

    #[test]
    fn a_trusted_proxy_passes_on_where_the_request_began() {
        let a = access(SHOP, &["127.0.0.1/32"]);
        assert!(on(
            &a,
            ip("127.0.0.1"),
            &headers(&[("x-forwarded-for", "192.168.1.40")])
        ));
        assert!(!on(
            &a,
            ip("127.0.0.1"),
            &headers(&[("x-forwarded-for", "203.0.113.9")])
        ));
        assert!(on(
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
        assert!(!on(&a, ip("127.0.0.1"), &h));
    }

    #[test]
    fn a_request_with_both_forwarding_headers_is_remote() {
        // The proxy wrote where the request really came from in `Forwarded`
        // and passed on the `X-Forwarded-For` the client wrote itself.
        let a = access(SHOP, &["127.0.0.1/32"]);
        let forged = headers(&[
            ("forwarded", "for=203.0.113.9"),
            ("x-forwarded-for", "192.168.1.40"),
        ]);
        assert!(!on(&a, ip("127.0.0.1"), &forged));
        // Even when both name an address inside the space.
        let both_local = headers(&[
            ("forwarded", "for=192.168.1.40"),
            ("x-forwarded-for", "192.168.1.40"),
        ]);
        assert!(!on(&a, ip("127.0.0.1"), &both_local));
        // Either one alone still passes the space on.
        for h in [
            headers(&[("forwarded", "for=192.168.1.40")]),
            headers(&[("x-forwarded-for", "192.168.1.40")]),
        ] {
            assert!(on(&a, ip("127.0.0.1"), &h));
        }
    }

    #[test]
    fn a_chain_of_trusted_proxies_is_walked_back_to_the_client() {
        let a = access(SHOP, &["127.0.0.1/32", "10.0.0.0/8"]);
        let h = headers(&[("x-forwarded-for", "203.0.113.9, 192.168.1.40, 10.1.2.3")]);
        assert!(on(&a, ip("127.0.0.1"), &h));
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
            assert!(!on(&a, ip("127.0.0.1"), &h), "{name}: {v}");
        }
    }

    #[test]
    fn a_chain_of_nothing_but_trusted_proxies_is_remote() {
        let a = access(SHOP, &["127.0.0.1/32", "10.0.0.0/8"]);
        assert!(!on(
            &a,
            ip("127.0.0.1"),
            &headers(&[("x-forwarded-for", "10.0.0.1")])
        ));
    }
}
