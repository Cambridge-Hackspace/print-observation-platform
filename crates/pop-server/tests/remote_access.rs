//! The remote-access example (docs/remote/nginx.conf, #93) is held to POP's
//! route table: it forwards every route POP allows from anywhere, and no
//! route POP keeps to the space; it refuses API paths it does not name; and
//! it sets the forwarding header POP judges a request by, rather than adding
//! to one a client sent.
//!
//! What this does not prove: that nginx itself behaves as read here. The
//! test reads the file as nginx's documentation says nginx chooses a
//! location (an exact match, then the first regular expression that matches,
//! then the longest prefix), but no nginx runs in POP's tests. POP refuses
//! local-only routes from outside on its own whatever a proxy forwards
//! (tests/access.rs), so this file is a second line, not the only one.

use std::path::Path;

use pop_server::access::Reach;
use regex_automata::meta::Regex;

/// A location block: how it matches, what, and what it does.
#[derive(Debug)]
struct Location {
    modifier: String,
    pattern: String,
    body: String,
}

impl Location {
    fn forwards(&self) -> bool {
        self.body.contains("proxy_pass")
    }
}

/// Every location block in `conf`, in order.
fn locations(conf: &str) -> Vec<Location> {
    let head = Regex::new(r"location\s+(=|~\*?|\^~)?\s*([^\s{]+)\s*\{").unwrap();
    let mut out = Vec::new();
    // Comments out first, so a commented example is not read as one.
    let conf: String = conf
        .lines()
        .map(|l| l.split('#').next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    let mut at = 0;
    while let Some(m) = head.find(&conf[at..]) {
        let start = at + m.start();
        let open = at + m.end();
        let caps = head.create_captures();
        let mut caps = caps;
        head.captures(&conf[start..open], &mut caps);
        let modifier = caps
            .get_group(1)
            .map(|g| conf[start + g.start..start + g.end].to_owned())
            .unwrap_or_default();
        let g = caps.get_group(2).unwrap();
        let pattern = conf[start + g.start..start + g.end].to_owned();
        // Location blocks here hold no blocks of their own.
        let close = open + conf[open..].find('}').expect("a closing brace");
        out.push(Location {
            modifier,
            pattern,
            body: conf[open..close].to_owned(),
        });
        at = close + 1;
    }
    out
}

/// The location nginx chooses for `path`.
fn chosen<'a>(locs: &'a [Location], path: &str) -> Option<&'a Location> {
    if let Some(l) = locs.iter().find(|l| l.modifier == "=" && l.pattern == path) {
        return Some(l);
    }
    let longest = locs
        .iter()
        .filter(|l| (l.modifier.is_empty() || l.modifier == "^~") && path.starts_with(&l.pattern))
        .max_by_key(|l| l.pattern.len());
    if let Some(l) = longest
        && l.modifier == "^~"
    {
        return Some(l);
    }
    for l in locs.iter().filter(|l| l.modifier.starts_with('~')) {
        if Regex::new(&l.pattern).unwrap().is_match(path) {
            return Some(l);
        }
    }
    longest
}

/// A route's path with each parameter filled in.
fn concrete(path: &str) -> String {
    let p = Regex::new(r"\{[^}]+\}").unwrap();
    let mut out = String::new();
    let mut last = 0;
    for m in p.find_iter(path) {
        out.push_str(&path[last..m.start()]);
        out.push_str("x1");
        last = m.end();
    }
    out.push_str(&path[last..]);
    out
}

/// What is wrong with `conf`, as POP's routes see it.
fn problems(conf: &str) -> Vec<String> {
    let locs = locations(conf);
    let mut out = Vec::new();
    for r in pop_server::routes() {
        let path = concrete(r.path);
        let forwarded = chosen(&locs, &path).is_some_and(Location::forwards);
        match (r.reach, forwarded) {
            (Reach::Anywhere, false) => {
                out.push(format!(
                    "{} {} is POP's from anywhere, but not forwarded",
                    r.method, r.path
                ));
            }
            (Reach::ThisMachine, true) => {
                out.push(format!(
                    "{} {} is for POP's own machine only, but forwarded",
                    r.method, r.path
                ));
            }
            _ => {}
        }
    }
    for path in [
        "/",
        "/kiosk",
        "/admin",
        "/guides",
        "/guides/a-guide",
        "/assets/index-1a2b.js",
        "/design",
    ] {
        if !chosen(&locs, path).is_some_and(Location::forwards) {
            out.push(format!("the web app's {path} is not forwarded"));
        }
    }
    for path in ["/api/something-new", "/api/admin/notify/discord/anything"] {
        if chosen(&locs, path).is_some_and(Location::forwards) {
            out.push(format!("{path}, which POP does not have, is forwarded"));
        }
    }
    // An API location no route uses is a stale entry, read as a promise.
    let used: Vec<String> = pop_server::routes()
        .iter()
        .filter_map(|r| chosen(&locs, &concrete(r.path)).map(|l| l.pattern.clone()))
        .collect();
    for l in locs.iter().filter(|l| l.forwards()) {
        let api = l.pattern.trim_start_matches('^').starts_with("/api/");
        if api && !used.contains(&l.pattern) {
            out.push(format!("location {} forwards no route POP has", l.pattern));
        }
    }
    if !conf.contains("proxy_set_header X-Forwarded-For $remote_addr;") {
        out.push("the forwarding header is not set to the client's own address".into());
    }
    if conf.contains("$proxy_add_x_forwarded_for") {
        out.push("the forwarding header is added to, so a client can forge where it is".into());
    }
    if !conf.contains("proxy_set_header Forwarded \"\";") {
        out.push("a Forwarded header from the client is passed on".into());
    }
    let at = |p: &str| chosen(&locs, p).map(|l| l.body.clone()).unwrap_or_default();
    if !at("/api/events").contains("proxy_buffering off") {
        out.push("the live stream is buffered, so screens see nothing until it fills".into());
    }
    let upload = at("/api/files/local");
    if !upload.contains("client_max_body_size 0;")
        || !upload.contains("proxy_request_buffering off;")
    {
        out.push(
            "the slicer's upload is held to nginx's body limit, or buffered whole, before POP's own limit".into(),
        );
    }
    if !at("/api/printers/x1/camera/mse").contains("Upgrade $http_upgrade") {
        out.push("the camera's WebSocket is not upgraded".into());
    }
    out
}

fn example() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/remote/nginx.conf");
    std::fs::read_to_string(p).expect("docs/remote/nginx.conf")
}

#[test]
fn the_example_forwards_what_pop_allows_from_anywhere_and_nothing_else() {
    let conf = example();
    assert!(
        locations(&conf).len() > 10,
        "the example's locations were read"
    );
    let found = problems(&conf);
    assert!(
        found.is_empty(),
        "docs/remote/nginx.conf:\n  {}",
        found.join("\n  ")
    );
}

/// The check, fed the mistakes it exists to catch.
#[test]
fn the_check_fails_an_example_that_forwards_too_much_or_forges_nothing() {
    let conf = example();
    let leaky = conf.replacen(
        "location /api/ {\n        return 404;\n    }",
        "location /api/ {\n        proxy_pass http://127.0.0.1:8080;\n    }",
        1,
    );
    assert_ne!(leaky, conf, "the deny was found to change");
    let found = problems(&leaky);
    assert!(
        found
            .iter()
            .any(|p| p.contains("/api/admin/notify/discord") && p.contains("forwarded")),
        "{found:#?}"
    );
    let added = conf.replacen(
        "X-Forwarded-For $remote_addr;",
        "X-Forwarded-For $proxy_add_x_forwarded_for;",
        1,
    );
    let found = problems(&added);
    assert!(found.iter().any(|p| p.contains("forge")), "{found:#?}");
    let missing = conf.replacen(
        "    location = /api/me { proxy_pass http://127.0.0.1:8080; }\n",
        "",
        1,
    );
    assert_ne!(missing, conf);
    let found = problems(&missing);
    assert!(
        found.iter().any(|p| p.contains("GET /api/me ")),
        "{found:#?}"
    );
    let stale = conf.replacen(
        "    # ---- feedback (#96)",
        "    location = /api/gone { proxy_pass http://127.0.0.1:8080; }\n    # ---- feedback (#96)",
        1,
    );
    assert_ne!(stale, conf);
    let found = problems(&stale);
    assert!(found.iter().any(|p| p.contains("/api/gone")), "{found:#?}");
    let limited = conf.replacen("        client_max_body_size 0;\n", "", 1);
    assert_ne!(limited, conf);
    let found = problems(&limited);
    assert!(
        found.iter().any(|p| p.contains("slicer's upload")),
        "{found:#?}"
    );
}

#[test]
fn locations_are_chosen_as_nginx_chooses_them() {
    let conf = "
        location /api/ { return 404; }
        location / { proxy_pass x; }
        location = /api/me { proxy_pass x; }
        location ~ ^/api/a/[^/]+$ { proxy_pass x; }
        location ~ ^/api/a/ { return 404; }
    ";
    let l = locations(conf);
    let pick = |p: &str| chosen(&l, p).map(|l| l.pattern.clone());
    assert_eq!(pick("/api/me").as_deref(), Some("/api/me"), "exact first");
    assert_eq!(
        pick("/api/a/b").as_deref(),
        Some("^/api/a/[^/]+$"),
        "then the first regex"
    );
    assert_eq!(
        pick("/api/zzz").as_deref(),
        Some("/api/"),
        "then the longest prefix"
    );
    assert_eq!(pick("/kiosk").as_deref(), Some("/"));
}
