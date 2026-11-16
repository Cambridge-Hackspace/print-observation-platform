//! Printers from BAM! fields (#21), on the printers a real BAM! recorded.

use pop_integrations::{
    bam::{Asset, Bam},
    fakes::bam::{FakeBam, TOKEN},
    printers::{Connection, FieldMap, Problem, printer_from},
};

const RECORDING: &str = include_str!("recordings/v0.20.0/recordings.json");

async fn recorded() -> (FakeBam, Vec<Asset>) {
    let fake = FakeBam::start(RECORDING).await;
    let bam = Bam::new(fake.url(), TOKEN).unwrap();
    let printers = bam.assets(fake.id("printers"), false).await.unwrap();
    (fake, printers)
}

fn named<'a>(all: &'a [Asset], name: &str) -> &'a Asset {
    all.iter().find(|a| a.asset.name == name).unwrap()
}

/// A recorded printer with some fields changed, for the cases a space's BAM!
/// did not happen to have.
fn with(base: &Asset, values: &[(&str, &str)]) -> Asset {
    let mut a = base.clone();
    for (k, v) in values {
        if v.is_empty() {
            a.field_values.remove(*k);
        } else {
            a.field_values.insert((*k).to_owned(), (*v).to_owned());
        }
    }
    a
}

const NO_SECRETS: fn(&str) -> bool = |_| false;
const ALL_SECRETS: fn(&str) -> bool = |_| true;

fn problems(a: &Asset) -> Vec<Problem> {
    printer_from(a, &FieldMap::default(), &ALL_SECRETS).unwrap_err()
}

// ---- the issue's "done when": named, never half-added -------------------------

#[tokio::test]
async fn a_printer_with_no_connection_is_its_problem_naming_the_field() {
    let (_f, all) = recorded().await;
    let p = printer_from(named(&all, "Ender 3"), &FieldMap::default(), &ALL_SECRETS).unwrap_err();
    assert_eq!(p.len(), 1, "{p:?}");
    assert_eq!(p[0].field, "Connection (connection)");
    assert!(p[0].problem.starts_with("not set"), "{p:?}");
}

#[tokio::test]
async fn every_problem_is_named_at_once_not_the_first_alone() {
    let (_f, all) = recorded().await;
    let bad = with(
        named(&all, "Voron 2.4"),
        &[("connection", "smoke signals"), ("camera", "ftp://cam")],
    );
    let fields: Vec<_> = problems(&bad).into_iter().map(|p| p.field).collect();
    assert_eq!(fields, ["Connection (connection)", "Camera (camera)"]);
}

#[tokio::test]
async fn an_unknown_connection_says_which_ones_pop_knows() {
    let (_f, all) = recorded().await;
    let p = problems(&with(
        named(&all, "MK4"),
        &[("connection", "smoke signals")],
    ));
    assert!(
        p[0].problem
            .contains("moonraker, octoprint, prusalink, bambu, usb_serial"),
        "{p:?}"
    );
}

// ---- the recorded printers -------------------------------------------------------------

#[tokio::test]
async fn a_moonraker_printer_maps_to_its_websocket() {
    let (_f, all) = recorded().await;
    let p = printer_from(named(&all, "Voron 2.4"), &FieldMap::default(), &NO_SECRETS).unwrap();
    assert_eq!(
        p.connection,
        Connection::Moonraker {
            url: "ws://voron.local:7125/websocket".into()
        }
    );
    assert_eq!(p.materials, ["PLA", "PETG", "ABS", "ASA"]);
    assert_eq!(p.training.as_deref(), Some("advanced"));
    assert_eq!(p.model.as_deref(), Some("Voron 2.4 R2"));
}

#[tokio::test]
async fn a_prusalink_printer_maps_to_its_http_address() {
    let (_f, all) = recorded().await;
    let p = printer_from(named(&all, "MK4"), &FieldMap::default(), &NO_SECRETS).unwrap();
    assert_eq!(
        p.connection,
        Connection::PrusaLink {
            url: "http://192.168.1.52".into()
        }
    );
    assert!(p.materials.is_empty() && p.camera.is_none());
}

#[tokio::test]
async fn a_bambu_printer_waits_for_its_access_code_in_pop() {
    let (_f, all) = recorded().await;
    let x1 = named(&all, "X1 Carbon");
    let waiting = printer_from(x1, &FieldMap::default(), &NO_SECRETS).unwrap_err();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].field, "LAN access code");
    let ready = printer_from(x1, &FieldMap::default(), &|id: &str| id == x1.asset.id).unwrap();
    assert_eq!(
        ready.connection,
        Connection::Bambu {
            host: "192.168.1.50".into(),
            serial: "01S00C123456789".into()
        }
    );
    assert_eq!(
        ready.camera.as_deref(),
        Some("rtsps://192.168.1.50/streaming/live/1")
    );
}

// ---- addresses as an admin is likely to type them ---------------------------------

#[tokio::test]
async fn a_moonraker_address_is_made_a_websocket_however_it_is_typed() {
    let (_f, all) = recorded().await;
    let voron = named(&all, "Voron 2.4");
    for (typed, url) in [
        ("voron.local", "ws://voron.local:7125/websocket"),
        ("192.168.1.60:7130", "ws://192.168.1.60:7130/websocket"),
        ("http://voron.local", "ws://voron.local/websocket"),
        (
            "https://voron.example.org/",
            "wss://voron.example.org/websocket",
        ),
        (
            "wss://voron.example.org/websocket",
            "wss://voron.example.org/websocket",
        ),
    ] {
        let p = printer_from(
            &with(voron, &[("address", typed)]),
            &FieldMap::default(),
            &NO_SECRETS,
        )
        .unwrap();
        assert_eq!(
            p.connection,
            Connection::Moonraker { url: url.into() },
            "{typed}"
        );
    }
}

#[tokio::test]
async fn an_address_that_cannot_be_used_names_the_field_and_why() {
    let (_f, all) = recorded().await;
    let voron = named(&all, "Voron 2.4");
    for (conn, typed, says) in [
        ("moonraker", "ftp://voron.local", "not ftp://"),
        ("octoprint", "gopher://host", "use http:// or https://"),
        ("bambu", "http://192.168.1.50", "IP address or name alone"),
        ("usb_serial", "ttyACM0", "/dev/ttyACM0"),
    ] {
        let p = problems(&with(
            voron,
            &[
                ("connection", conn),
                ("address", typed),
                ("serial", "01S00C1"),
            ],
        ));
        assert_eq!(p[0].field, "Address (address)", "{conn} {typed}");
        assert!(
            p[0].problem.contains(says),
            "{conn} {typed}: {}",
            p[0].problem
        );
    }
}

#[tokio::test]
async fn a_printer_with_a_connection_and_no_address_names_the_address() {
    let (_f, all) = recorded().await;
    let p = problems(&with(named(&all, "MK4"), &[("address", "")]));
    assert_eq!(p[0].field, "Address (address)");
}

#[tokio::test]
async fn a_bambu_printer_needs_its_serial_number() {
    let (_f, all) = recorded().await;
    let p = problems(&with(named(&all, "X1 Carbon"), &[("serial", "")]));
    assert_eq!(p[0].field, "Serial number (serial)");
}

#[tokio::test]
async fn the_fields_can_be_called_anything_in_bam() {
    let (_f, all) = recorded().await;
    // A space whose BAM! calls its connection field "link".
    let mut renamed = named(&all, "MK4").clone();
    let v = renamed.field_values.remove("connection").unwrap();
    renamed.field_values.insert("link".into(), v);
    let map = FieldMap {
        connection: "link".into(),
        ..FieldMap::default()
    };
    assert!(printer_from(&renamed, &map, &NO_SECRETS).is_ok());
    assert!(printer_from(&renamed, &FieldMap::default(), &NO_SECRETS).is_err());
}
