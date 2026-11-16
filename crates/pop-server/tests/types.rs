//! The web app's types, in `web/src/lib/generated`, are generated from the
//! server's (ADR 0001). This fails when they no longer match, and names the
//! command that brings them back in line: `tools/gen-types.sh`, which runs this
//! with `POP_WRITE_TYPES=1`.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use pop_server::api::{
    AccessCode, AdminPrinter, ApiError, AuditEntry, AuditKind, DiscordWebhook, Guide, GuideImage,
    GuideProblems, GuideSummary, Health, NewGuideImage, NewPassword, NewRole, NewUser,
    NotifySettings, PublicGuide, PublicGuideList, Role, ServerEvent, SignIn, Snapshot, Status,
    TowerPrinter, TowerState, User,
};
use ts_rs::TS;

fn generated_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/src/lib/generated")
}

/// Every type the API carries. A type missing here has no TypeScript twin; the
/// web app then fails to build against it, which is the second oracle.
fn export(dir: &Path) {
    Status::export_all_to(dir).unwrap();
    ServerEvent::export_all_to(dir).unwrap();
    Snapshot::export_all_to(dir).unwrap();
    ApiError::export_all_to(dir).unwrap();
    Health::export_all_to(dir).unwrap();
    Role::export_all_to(dir).unwrap();
    AuditKind::export_all_to(dir).unwrap();
    AuditEntry::export_all_to(dir).unwrap();
    AdminPrinter::export_all_to(dir).unwrap();
    TowerState::export_all_to(dir).unwrap();
    TowerPrinter::export_all_to(dir).unwrap();
    AccessCode::export_all_to(dir).unwrap();
    // Guides (#71).
    Guide::export_all_to(dir).unwrap();
    GuideSummary::export_all_to(dir).unwrap();
    GuideProblems::export_all_to(dir).unwrap();
    GuideImage::export_all_to(dir).unwrap();
    NewGuideImage::export_all_to(dir).unwrap();
    PublicGuide::export_all_to(dir).unwrap();
    PublicGuideList::export_all_to(dir).unwrap();
    NotifySettings::export_all_to(dir).unwrap();
    DiscordWebhook::export_all_to(dir).unwrap();
    // The printer state model (#25), from pop-printers.
    pop_printers::PrinterState::export_all_to(dir).unwrap();
    pop_printers::Capabilities::export_all_to(dir).unwrap();
    User::export_all_to(dir).unwrap();
    SignIn::export_all_to(dir).unwrap();
    NewUser::export_all_to(dir).unwrap();
    NewPassword::export_all_to(dir).unwrap();
    NewRole::export_all_to(dir).unwrap();
}

fn read_dir(dir: &Path) -> BTreeMap<String, String> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| e.unwrap())
                .filter(|e| e.file_name().to_string_lossy().ends_with(".ts"))
                .map(|e| {
                    (
                        e.file_name().into_string().unwrap(),
                        fs::read_to_string(e.path()).unwrap(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_web_apps_types_match_the_servers() {
    let fresh = tempfile::tempdir().unwrap();
    export(fresh.path());
    let want = read_dir(fresh.path());
    assert!(!want.is_empty(), "nothing was exported");

    if std::env::var_os("POP_WRITE_TYPES").is_some() {
        let dir = generated_dir();
        for name in read_dir(&dir).keys() {
            if !want.contains_key(name) {
                fs::remove_file(dir.join(name)).unwrap();
            }
        }
        fs::create_dir_all(&dir).unwrap();
        for (name, body) in &want {
            fs::write(dir.join(name), body).unwrap();
        }
        return;
    }

    let have = read_dir(&generated_dir());
    assert_eq!(
        have.keys().collect::<Vec<_>>(),
        want.keys().collect::<Vec<_>>(),
        "web/src/lib/generated has a different set of files from what the server exports; run tools/gen-types.sh"
    );
    for (name, body) in &want {
        assert_eq!(
            &have[name], body,
            "web/src/lib/generated/{name} is stale; run tools/gen-types.sh"
        );
    }
}
