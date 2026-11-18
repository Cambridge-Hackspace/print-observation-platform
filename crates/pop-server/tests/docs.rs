//! The documentation names only what exists (#91): every `POP_*` variable a
//! document tells someone to set is one POP or its tools read, and every
//! file in the repository a document points to is there. A deployment is
//! validated by following the guide (docs/deployment.md), so a renamed
//! setting or a moved file must fail here first, not at the space.
//!
//! What this does not prove: that a variable means what the document says,
//! or that a step works. It proves only that the names are not dangling.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files(dir: &Path, keep: &dyn Fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if ![
                "target",
                "node_modules",
                "dist",
                ".git",
                "out",
                "test-results",
            ]
            .contains(&name.as_ref())
            {
                files(&p, keep, out);
            }
        } else if keep(&p) {
            out.push(p);
        }
    }
}

/// The documents: what an installer reads.
fn documents() -> Vec<PathBuf> {
    let r = root();
    let mut out = vec![r.join("README.md"), r.join("pop.example.toml")];
    files(
        &r.join("docs"),
        &|p| p.extension().is_some_and(|e| e == "md" || e == "conf"),
        &mut out,
    );
    files(&r.join("packaging"), &|_| true, &mut out);
    out
}

/// Every `POP_` name in `text`.
fn variables(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let b = text.as_bytes();
    let mut i = 0;
    while let Some(at) = text[i..].find("POP_") {
        let start = i + at;
        let before = start.checked_sub(1).map(|j| b[j]);
        let mut end = start + 4;
        while end < b.len()
            && (b[end].is_ascii_uppercase() || b[end].is_ascii_digit() || b[end] == b'_')
        {
            end += 1;
        }
        // A whole name, not the tail of another word.
        if !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') && end > start + 4 {
            out.insert(text[start..end].trim_end_matches('_').to_owned());
        }
        i = end;
    }
    out
}

/// Every name the code reads: as a string in Rust, or in a script or test.
fn read_by_code() -> String {
    let r = root();
    let mut src = Vec::new();
    files(
        &r.join("crates"),
        &|p| p.extension().is_some_and(|e| e == "rs"),
        &mut src,
    );
    files(&r.join("tools"), &|_| true, &mut src);
    files(&r.join("web/e2e"), &|_| true, &mut src);
    // Workflows read variables too: the release keys' secrets (#148).
    files(&r.join(".github/workflows"), &|_| true, &mut src);
    src.push(r.join("web/playwright.config.ts"));
    // Not this file: it names variables only to test the check.
    let me = r.join("crates/pop-server/tests/docs.rs");
    let mut text = src
        .iter()
        .filter(|p| p.canonicalize().ok() != me.canonicalize().ok())
        .filter_map(|p| fs::read_to_string(p).ok())
        .collect::<Vec<_>>()
        .join("\n");
    // A variable a service unit defines for what it starts is read there:
    // the `Environment=` lines of packaging's units, and nothing else in
    // them, so a unit cannot vouch for a name it only mentions.
    let mut units = Vec::new();
    files(
        &r.join("packaging"),
        &|p| p.extension().is_some_and(|e| e == "service"),
        &mut units,
    );
    for u in units {
        for line in fs::read_to_string(u).unwrap_or_default().lines() {
            if let Some(def) = line.strip_prefix("Environment=") {
                text.push('\n');
                text.push_str(def);
            }
        }
    }
    text
}

/// The variables in `docs` that `code` never reads.
fn unread(docs: &BTreeSet<String>, code: &str) -> Vec<String> {
    docs.iter()
        .filter(|v| !variables(code).contains(*v))
        .cloned()
        .collect()
}

/// Paths in the repository a document names, in backticks or bare:
/// `docs/...`, `packaging/...`, `tools/...`, `crates/...`, `web/...`.
fn paths(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for word in text.split(|c: char| c.is_whitespace() || "`()[],\"'<>".contains(c)) {
        let w = word.trim_end_matches(['.', ':', ';']);
        let named = ["docs/", "packaging/", "tools/", "crates/"]
            .iter()
            .any(|p| w.starts_with(p))
            || w == "pop.example.toml"
            || w == "README.md";
        // A path, not a pattern: no placeholders (`<version>`, a shell's
        // `$k`), globs or line numbers.
        if named && !w.contains(['*', '{', '<', '#', '$']) && !w.ends_with('/') && w.contains('.') {
            out.insert(w.split("::").next().unwrap_or(w).to_owned());
        }
    }
    out
}

#[test]
fn every_variable_a_document_names_is_read() {
    let code = read_by_code();
    let mut missing = Vec::new();
    let mut seen = BTreeSet::new();
    for d in documents() {
        let text = fs::read_to_string(&d).unwrap_or_default();
        seen.extend(variables(&text));
        for v in unread(&variables(&text), &code) {
            missing.push(format!(
                "{}: {v}",
                d.strip_prefix(root()).unwrap_or(&d).display()
            ));
        }
    }
    // A known name, so a scan that finds nothing does not pass for one that
    // found nothing wrong.
    assert!(
        seen.contains("POP_SECRETS_KEY") && seen.len() >= 5,
        "{seen:?}"
    );
    assert!(
        missing.is_empty(),
        "named but read by nothing:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn every_file_a_document_points_to_is_there() {
    let r = root();
    let mut missing = Vec::new();
    let mut seen = BTreeSet::new();
    for d in documents() {
        let text = fs::read_to_string(&d).unwrap_or_default();
        seen.extend(paths(&text));
        for p in paths(&text) {
            if !r.join(&p).exists() {
                missing.push(format!(
                    "{}: {p}",
                    d.strip_prefix(&r).unwrap_or(&d).display()
                ));
            }
        }
    }
    assert!(
        seen.contains("packaging/systemd/pop.service") && seen.len() > 10,
        "{seen:?}"
    );
    assert!(
        missing.is_empty(),
        "pointed to but not there:\n  {}",
        missing.join("\n  ")
    );
}

/// The checks, fed what a stale document would hold.
#[test]
fn the_checks_fail_a_document_that_names_what_is_gone() {
    let code = read_by_code();
    let doc = "Set POP_BAM_TOKEN and POP_NO_SUCH_SETTING, not MY_POP_THING.";
    assert_eq!(unread(&variables(doc), &code), ["POP_NO_SUCH_SETTING"]);
    // A unit mentioning a name it does not define does not vouch for it.
    assert!(!code.contains("POP_NOT_DEFINED_BY_A_UNIT"));
    assert!(
        code.contains("POP_KIOSK_URL=http"),
        "the kiosk unit's definition is read"
    );
    let r = root();
    let gone: Vec<String> = paths(
        "see `docs/deployment.md` and docs/not-a-guide.md, and packaging/systemd/pop.service.",
    )
    .into_iter()
    .filter(|p| !r.join(p).exists())
    .collect();
    assert_eq!(gone, ["docs/not-a-guide.md"]);
}
