//! The BAM! client (#20) against a fake BAM! serving answers recorded from a
//! real BAM! v0.20.0.

use pop_integrations::{
    bam::{Bam, BamError},
    fakes::bam::{FakeBam, TOKEN},
};

const RECORDING: &str = include_str!("recordings/v0.20.0/recordings.json");

async fn fake() -> (FakeBam, Bam) {
    let fake = FakeBam::start(RECORDING).await;
    let bam = Bam::new(fake.url(), TOKEN).unwrap();
    (fake, bam)
}

#[tokio::test]
async fn the_printers_are_those_directly_in_the_category_and_in_use() {
    let (f, bam) = fake().await;
    let mut names: Vec<_> = bam
        .assets(f.id("printers"), false)
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.asset.name)
        .collect();
    names.sort();
    // Not Form 3, in a subcategory; not the archived Old Prusa; not filament.
    assert_eq!(names, ["Ender 3", "MK4", "Voron 2.4", "X1 Carbon"]);
}

#[tokio::test]
async fn archived_printers_are_read_on_their_own() {
    let (f, bam) = fake().await;
    let archived = bam.assets(f.id("printers"), true).await.unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].asset.name, "Old Prusa");
    assert!(archived[0].asset.archived_at.is_some());
}

/// What the fake sends for a listing, raw: the precondition of the two tests
/// below is that it is BAM!'s `null`, not a `[]` POP would have read anyway.
async fn raw_listing(f: &FakeBam, archived: bool) -> serde_json::Value {
    let q = if archived { "archived=true&" } else { "" };
    reqwest::Client::new()
        .get(format!(
            "{}/api/v1/assets?{q}category_id={}",
            f.url(),
            f.id("printers")
        ))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_category_with_nothing_archived_lists_none_not_an_error() {
    let (f, bam) = fake().await;
    f.restore(f.id("old"));
    assert_eq!(
        raw_listing(&f, true).await["assets"],
        serde_json::Value::Null
    );
    assert_eq!(bam.assets(f.id("printers"), true).await.unwrap(), []);
}

#[tokio::test]
async fn a_new_empty_printer_category_lists_none_not_an_error() {
    let (f, bam) = fake().await;
    for p in ["mk4", "voron", "x1", "incomplete"] {
        f.archive(f.id(p));
    }
    assert_eq!(
        raw_listing(&f, false).await["assets"],
        serde_json::Value::Null
    );
    assert_eq!(bam.assets(f.id("printers"), false).await.unwrap(), []);
}

#[tokio::test]
async fn an_answer_without_its_list_is_unexpected_not_empty() {
    // null is BAM!'s "none"; a missing list is something else answering.
    let (f, bam) = fake().await;
    f.send_body("{}");
    assert!(matches!(
        bam.assets(f.id("printers"), false).await.unwrap_err(),
        BamError::Unexpected(_)
    ));
    assert!(matches!(
        bam.categories().await.unwrap_err(),
        BamError::Unexpected(_)
    ));
}

#[tokio::test]
async fn a_printers_fields_are_strings_by_key() {
    let (f, bam) = fake().await;
    let x1 = bam.asset(f.id("x1")).await.unwrap();
    assert_eq!(x1.asset.name, "X1 Carbon");
    assert_eq!(x1.field_values["connection"], "bambu");
    assert_eq!(x1.field_values["serial"], "01S00C123456789");
    assert!(x1.categories.iter().any(|c| c.id == f.id("printers")));
}

#[tokio::test]
async fn the_category_tree_comes_flat_with_parents() {
    let (f, bam) = fake().await;
    let cats = bam.categories().await.unwrap();
    let resin = cats.iter().find(|c| c.id == f.id("resin")).unwrap();
    assert_eq!(resin.parent_id.as_deref(), Some(f.id("printers")));
    assert!(
        cats.iter()
            .any(|c| c.name == "Filament" && c.parent_id.is_none())
    );
}

#[tokio::test]
async fn the_printer_categorys_fields_say_their_types_and_choices() {
    let (f, bam) = fake().await;
    let fields = bam.fields(f.id("printers")).await.unwrap();
    let connection = fields.iter().find(|e| e.field.key == "connection").unwrap();
    assert_eq!(connection.field.validation_type, "select");
    assert_eq!(
        connection.field.choices(),
        ["moonraker", "octoprint", "prusalink", "bambu", "usb_serial"]
    );
    assert_eq!(
        fields
            .iter()
            .find(|e| e.field.key == "camera")
            .unwrap()
            .field
            .validation_type,
        "url"
    );
}

#[tokio::test]
async fn events_read_in_order_including_the_subcategorys() {
    let (f, bam) = fake().await;
    let kinds = [
        "asset.created",
        "asset.edited",
        "asset.archived",
        "asset.restored",
    ];
    let (events, more) = bam.events(None, &kinds, f.id("printers")).await.unwrap();
    assert!(!more);
    let ks: Vec<_> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(&ks[ks.len() - 2..], ["asset.edited", "asset.archived"]);
    // BAM!'s category filter takes in subcategories: Form 3's creation is
    // here, though Form 3 is not a printer POP runs.
    let created: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "asset.created")
        .collect();
    assert_eq!(created.len(), 6, "{created:?}");
}

#[tokio::test]
async fn a_missing_asset_is_not_found() {
    let (_f, bam) = fake().await;
    assert_eq!(
        bam.asset("00000000-0000-4000-8000-000000000000")
            .await
            .unwrap_err(),
        BamError::NotFound
    );
}

#[tokio::test]
async fn a_bad_token_says_so_in_bams_words() {
    let f = FakeBam::start(RECORDING).await;
    let bam = Bam::new(f.url(), "bam_not_a_real_token").unwrap();
    let e = bam.assets(f.id("printers"), false).await.unwrap_err();
    assert_eq!(
        e,
        BamError::Token("Invalid, expired or revoked API token".into())
    );
    assert!(e.to_string().contains("new one"), "{e}");
}

#[tokio::test]
async fn an_answer_that_is_not_json_is_unexpected_not_a_crash() {
    let (f, bam) = fake().await;
    f.send_garbage(true);
    assert!(matches!(
        bam.categories().await.unwrap_err(),
        BamError::Unexpected(_)
    ));
}

#[tokio::test]
async fn a_bam_that_is_not_there_is_unreachable() {
    let bam = Bam::new("http://127.0.0.1:9", TOKEN).unwrap();
    assert!(matches!(
        bam.categories().await.unwrap_err(),
        BamError::Unreachable(_)
    ));
}

#[tokio::test]
async fn the_check_finds_the_printer_category_or_says_it_is_missing() {
    let (f, bam) = fake().await;
    bam.check(f.id("printers")).await.unwrap();
    let e = bam.check("no-such-category").await.unwrap_err();
    assert!(e.to_string().contains("no-such-category"), "{e}");
}

#[tokio::test]
async fn every_request_carries_the_token_and_asks_only_what_was_recorded() {
    let (f, bam) = fake().await;
    bam.assets(f.id("printers"), false).await.unwrap();
    bam.fields(f.id("printers")).await.unwrap();
    // The fake answers an unrecorded request with a 404 naming it; none here.
    assert!(
        f.requests().iter().all(|r| r.starts_with("GET /api/v1/")),
        "{:?}",
        f.requests()
    );
}
