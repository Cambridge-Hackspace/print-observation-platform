//! docs/threat-model.md names a test for every claim it makes (#92). This
//! holds it to that: every test it names must exist, so renaming or deleting
//! one cannot leave a claim standing that nothing enforces.
//!
//! What this does not prove: that the named test asserts the claim beside
//! it. That is the reviewer's to read. It proves only that the reference is
//! not dangling.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A test the document names: the file it is in, and the name to find there.
#[derive(Debug, PartialEq)]
enum Ref {
    /// `path/to/file.rs::name` or `path/to/file.rs::tests::name`.
    Rust { file: String, name: String },
    /// `path/to/file.test.ts › "what it does"`.
    Web { file: String, name: String },
}

/// Every test reference in backticks.
fn refs(doc: &str) -> Vec<Ref> {
    let mut out = Vec::new();
    for (i, span) in doc.split('`').enumerate() {
        if i % 2 == 0 {
            continue; // outside backticks
        }
        if let Some((file, rest)) = span.split_once(".rs::") {
            let name = rest.rsplit("::").next().unwrap_or_default();
            out.push(Ref::Rust {
                file: format!("{file}.rs"),
                name: name.to_string(),
            });
        } else if let Some((file, rest)) = span.split_once(" › ") {
            out.push(Ref::Web {
                file: file.to_string(),
                name: rest.trim_matches('"').to_string(),
            });
        }
    }
    out
}

/// The references that point at nothing.
fn dangling(refs: &[Ref], read: impl Fn(&str) -> Option<String>) -> Vec<String> {
    refs.iter()
        .filter_map(|r| {
            let (file, found) = match r {
                Ref::Rust { file, name } => (
                    file,
                    read(file).is_some_and(|s| s.contains(&format!("fn {name}("))),
                ),
                Ref::Web { file, name } => (
                    file,
                    read(file).is_some_and(|s| {
                        s.contains(&format!("'{name}'"))
                            || s.contains(&format!("\"{name}\""))
                            || s.contains(&format!("`{name}`"))
                    }),
                ),
            };
            (!found).then(|| format!("{r:?} (in {file})"))
        })
        .collect()
}

fn read_from_root(file: &str) -> Option<String> {
    std::fs::read_to_string(root().join(file)).ok()
}

#[test]
fn every_test_the_threat_model_names_exists() {
    let doc = std::fs::read_to_string(root().join("docs/threat-model.md"))
        .expect("docs/threat-model.md is readable");
    let found = refs(&doc);
    // Not vacuous: the document names dozens of tests, in both languages.
    let rust = found
        .iter()
        .filter(|r| matches!(r, Ref::Rust { .. }))
        .count();
    let web = found
        .iter()
        .filter(|r| matches!(r, Ref::Web { .. }))
        .count();
    assert!(
        rust >= 50 && web >= 3,
        "found only {rust} Rust and {web} web test references"
    );
    let missing = dangling(&found, read_from_root);
    assert!(
        missing.is_empty(),
        "docs/threat-model.md names tests that do not exist:\n{}",
        missing.join("\n")
    );
}

#[test]
fn the_check_finds_a_reference_that_points_at_nothing() {
    let doc = "Real: `crates/pop-server/tests/access.rs::no_route_needs_less_than_what_it_does`. \
               Renamed: `crates/pop-server/tests/access.rs::no_route_controls_anything`. \
               Unit: `crates/pop-server/src/access.rs::tests::this_machine_is_not_the_space_by_itself`. \
               Gone: `crates/pop-server/tests/nowhere.rs::anything`. \
               Web: `web/src/lib/tower/HandView.test.ts › \"never offers to resume a print that stopped\"`. \
               Web, renamed: `web/src/lib/tower/HandView.test.ts › \"offers to resume\"`. \
               Not a reference: `access.local_networks`.";
    let found = refs(doc);
    assert_eq!(found.len(), 6, "{found:?}");
    let missing = dangling(&found, read_from_root);
    assert_eq!(missing.len(), 3, "{missing:#?}");
    assert!(missing[0].contains("no_route_controls_anything"));
    assert!(missing[1].contains("nowhere.rs"));
    assert!(missing[2].contains("offers to resume"));
}
