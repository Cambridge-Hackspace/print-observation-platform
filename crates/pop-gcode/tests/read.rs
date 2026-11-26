//! Slicer metadata read from real files (#38), and the reader fuzzed.
//!
//! Every file in fixtures/ was written by the slicer it is named for, from
//! its vendor's own presets, by tools/slicer-fixtures/make.sh; fixtures/
//! README.md says which version and presets. The values expected here were
//! read off each file by eye, not from this reader.
//!
//! The fuzzer is seeded: it prints its seed, and `POP_FUZZ_SEED=<n>` replays
//! a run exactly. `POP_FUZZ_CASES` sets how many files it tries.

use pop_gcode::{Filament, MAX_LINE, Metadata, Scanner, Slicer, SlicerKind, read};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn slicer(kind: SlicerKind, version: &str) -> Option<Slicer> {
    Some(Slicer {
        kind,
        version: Some(version.into()),
    })
}

fn filament(material: Option<&str>, color: Option<&str>, grams: Option<f64>) -> Filament {
    Filament {
        slot: 0,
        material: material.map(Into::into),
        color: color.map(Into::into),
        grams,
    }
}

#[test]
fn prusaslicer() {
    assert_eq!(
        read(&fixture("prusaslicer-mk4s.gcode")),
        Metadata {
            slicer: slicer(SlicerKind::PrusaSlicer, "2.9.4"),
            printer_model: Some("MK4S".into()),
            nozzles_mm: vec![0.4],
            estimated_seconds: Some(2 * 60 + 12),
            filaments: vec![filament(Some("PLA"), Some("#FF8000"), Some(0.96))],
            total_grams: Some(0.96),
        }
    );
}

#[test]
fn superslicer() {
    assert_eq!(
        read(&fixture("superslicer-ender3v2.gcode")),
        Metadata {
            slicer: slicer(SlicerKind::SuperSlicer, "2.5.59.13"),
            printer_model: Some("ENDER3V2".into()),
            nozzles_mm: vec![0.4],
            estimated_seconds: Some(7 * 60 + 5),
            filaments: vec![filament(Some("PLA"), Some("#DDDDDD"), Some(0.96))],
            total_grams: Some(0.96),
        }
    );
}

#[test]
fn orcaslicer() {
    assert_eq!(
        read(&fixture("orcaslicer-ender3v2.gcode")),
        Metadata {
            slicer: slicer(SlicerKind::OrcaSlicer, "2.5.0"),
            printer_model: Some("Creality Ender-3 V2".into()),
            nozzles_mm: vec![0.4],
            estimated_seconds: Some(6 * 60 + 14),
            filaments: vec![filament(Some("PETG"), Some("#F2754E"), Some(0.88))],
            total_grams: Some(0.88),
        }
    );
}

#[test]
fn bambu_studio() {
    assert_eq!(
        read(&fixture("bambustudio-a1mini.gcode")),
        Metadata {
            slicer: slicer(SlicerKind::BambuStudio, "02.09.01.70"),
            printer_model: Some("Bambu Lab A1 mini".into()),
            nozzles_mm: vec![0.4],
            // The total estimate, not the model's alone (2m 9s).
            estimated_seconds: Some(2 * 60 + 28),
            // Bambu Studio writes no grams per filament, only the total.
            filaments: vec![filament(Some("PLA"), Some("#00AE42"), None)],
            total_grams: Some(0.98),
        }
    );
}

#[test]
fn a_slot_loaded_and_not_used_is_not_a_filament_the_print_needs() {
    // PETG HF sits in slot 2; the header says only slot 1 prints.
    let m = read(&fixture("bambustudio-a1mini-two-filaments.gcode"));
    assert_eq!(
        m.filaments,
        vec![filament(Some("PLA"), Some("#00AE42"), None)]
    );
    assert_eq!(m.total_grams, Some(0.98));
}

#[test]
fn cura_marlin_flavor_names_no_material_and_no_weight() {
    assert_eq!(
        read(&fixture("cura-ender3.gcode")),
        Metadata {
            slicer: slicer(SlicerKind::Cura, "5.14.0"),
            printer_model: Some("Creality Ender-3".into()),
            nozzles_mm: vec![],
            estimated_seconds: Some(650),
            // One extruder used (0.41 m of filament), of nothing it names.
            filaments: vec![filament(None, None, None)],
            total_grams: None,
        }
    );
}

#[test]
fn cura_griffin_flavor() {
    assert_eq!(
        read(&fixture("cura-s5.gcode")),
        Metadata {
            slicer: slicer(SlicerKind::Cura, "5.14.0"),
            printer_model: Some("Ultimaker S5".into()),
            nozzles_mm: vec![0.4],
            estimated_seconds: Some(658),
            filaments: vec![filament(None, None, None)],
            total_grams: None,
        }
    );
}

#[test]
fn what_a_file_does_not_say_is_missing_never_guessed() {
    // PrusaSlicer with no presets chosen: `printer_model =` is empty, and
    // the filament has no density, so the file says 0 g for 322 mm of it.
    assert_eq!(
        read(&fixture("prusaslicer-no-presets.gcode")),
        Metadata {
            slicer: slicer(SlicerKind::PrusaSlicer, "2.9.4"),
            printer_model: None,
            nozzles_mm: vec![0.4],
            estimated_seconds: Some(4 * 60 + 51),
            filaments: vec![filament(Some("PLA"), Some("#29B2B2"), None)],
            total_grams: None,
        }
    );
    assert_eq!(read(b"G28\nG1 X10 Y10\n"), Metadata::default());
    assert_eq!(read(b""), Metadata::default());
}

#[test]
fn a_value_that_does_not_read_is_missing_and_moves_nothing() {
    let m = read(
        b"; generated by PrusaSlicer 2.9.4 on today\n\
          ; nozzle_diameter = 0.4,wide\n\
          ; filament_type = PLA;;PETG\n\
          ; filament_colour = orange;#00ff00;#0000FF\n\
          ; filament used [g] = 1.5, lots, 2\n\
          ; total filament used [g] = NaN\n\
          ; estimated printing time (normal mode) = soon\n",
    );
    // An unreadable nozzle would shift the rest onto the wrong extruders.
    assert_eq!(m.nozzles_mm, Vec::<f64>::new());
    assert_eq!(
        m.filaments,
        vec![
            Filament {
                slot: 0,
                material: Some("PLA".into()),
                color: None,
                grams: Some(1.5),
            },
            Filament {
                slot: 2,
                material: Some("PETG".into()),
                color: Some("#0000FF".into()),
                grams: Some(2.0),
            },
        ],
        "slot 1's grams do not read, so nothing says it is used"
    );
    assert_eq!(m.total_grams, None);
    assert_eq!(m.estimated_seconds, None);
}

#[test]
fn without_any_sign_of_use_every_slot_named_is_listed() {
    let m = read(b"; filament_type = PLA;PETG\n");
    assert_eq!(
        m.filaments.iter().map(|f| f.slot).collect::<Vec<_>>(),
        [0, 1]
    );
}

#[test]
fn line_endings_and_piece_boundaries_change_nothing() {
    for name in FIXTURES {
        let unix = fixture(name);
        let dos: Vec<u8> = String::from_utf8(unix.clone())
            .unwrap()
            .replace('\n', "\r\n")
            .into_bytes();
        assert_eq!(read(&dos), read(&unix), "{name}");

        let mut s = Scanner::new();
        for b in &unix {
            s.push(std::slice::from_ref(b));
        }
        assert_eq!(s.finish(), read(&unix), "{name}");
    }
}

const FIXTURES: &[&str] = &[
    "bambustudio-a1mini-two-filaments.gcode",
    "bambustudio-a1mini.gcode",
    "cura-ender3.gcode",
    "cura-s5.gcode",
    "orcaslicer-ender3v2.gcode",
    "prusaslicer-mk4s.gcode",
    "prusaslicer-no-presets.gcode",
    "superslicer-ender3v2.gcode",
];

#[test]
fn only_a_line_that_is_a_comment_is_read() {
    // A command's trailing comment is the start G-code's, not the slicer's.
    assert_eq!(read(b"M117 ; printer_model = MK4S\n").printer_model, None);
    assert_eq!(
        read(b"; generated by SomeSlicer on 2026-11-26\n").slicer,
        Some(Slicer {
            kind: SlicerKind::Other("SomeSlicer".into()),
            version: None,
        }),
        "a slicer that gives no version has none, not \"on\""
    );
}

#[test]
fn a_quoted_value_is_unquoted_and_an_empty_one_is_missing() {
    let m = read(b"; printer_model = \"\"\n; filament_type = \"PLA Silk\";PETG\n");
    assert_eq!(m.printer_model, None);
    assert_eq!(
        m.filaments
            .iter()
            .map(|f| f.material.as_deref())
            .collect::<Vec<_>>(),
        [Some("PLA Silk"), Some("PETG")]
    );
}

#[test]
fn a_line_that_is_not_utf8_is_not_read() {
    assert_eq!(
        read(b"; printer_model = Caf\xe9 Printer\n").printer_model,
        None
    );
    assert_eq!(
        read("; printer_model = Café Printer\n".as_bytes())
            .printer_model
            .as_deref(),
        Some("Café Printer")
    );
}

#[test]
fn a_used_filament_weighing_nothing_has_no_weight() {
    // Slot 0 used with no density known; slot 1 not used at all.
    let m = read(b"; filament used [mm] = 300.5, 0.00\n; filament used [g] = 0.00, 0.00\n");
    assert_eq!(
        m.filaments,
        vec![Filament {
            slot: 0,
            ..Filament::default()
        }]
    );
}

#[test]
fn cura_trains_with_a_gap_name_no_nozzles() {
    let m = read(b";EXTRUDER_TRAIN.1.NOZZLE.DIAMETER:0.4\n");
    assert_eq!(m.nozzles_mm, Vec::<f64>::new());
    let m = read(b";EXTRUDER_TRAIN.0.NOZZLE.DIAMETER:0.4\n;EXTRUDER_TRAIN.1.NOZZLE.DIAMETER:0.6\n");
    assert_eq!(m.nozzles_mm, vec![0.4, 0.6]);
}

#[test]
fn the_last_line_is_read_without_a_newline() {
    assert_eq!(
        read(b"; printer_model = MK4S").printer_model.as_deref(),
        Some("MK4S")
    );
}

#[test]
fn an_overlong_line_is_skipped_whole_and_the_next_is_read() {
    let mut file = b"; printer_model = ".to_vec();
    file.extend(std::iter::repeat_n(b'X', MAX_LINE));
    file.extend_from_slice(b"\n; filament_type = PETG\n");
    let mut s = Scanner::new();
    for piece in file.chunks(4096) {
        s.push(piece);
        assert!(s.held() <= MAX_LINE, "holds {}", s.held());
    }
    let m = s.finish();
    assert_eq!(m.printer_model, None);
    assert_eq!(m.filaments[0].material.as_deref(), Some("PETG"));

    // And one exactly at the limit is still read.
    let mut file = b"; printer_model = ".to_vec();
    file.extend(std::iter::repeat_n(b'Y', MAX_LINE - file.len()));
    assert_eq!(file.len(), MAX_LINE);
    assert!(read(&file).printer_model.is_some());
}

// ---- the fuzzer ----------------------------------------------------------------

/// xorshift64*: enough randomness to fuzz with, and replayable from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn seed() -> u64 {
    let s = std::env::var("POP_FUZZ_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64
                | 1
        });
    eprintln!("SEED={s} (replay with POP_FUZZ_SEED={s})");
    s
}

/// Pieces of what slicers write, for the fuzzer to splice in, so that it
/// reaches the parsing of values and not only of lines.
const WORDS: &[&[u8]] = &[
    b"; ",
    b";",
    b" = ",
    b": ",
    b"\n",
    b"\r\n",
    b",",
    b";",
    b"#",
    b"generated by ",
    b"BambuStudio ",
    b"Generated with Cura_SteamEngine ",
    b"printer_model",
    b"nozzle_diameter",
    b"filament_type",
    b"filament_colour",
    b"filament used [g]",
    b"filament used [mm]",
    b"total filament used [g]",
    b"total filament weight [g]",
    b"estimated printing time (normal mode)",
    b"total estimated time",
    b"filament",
    b"TIME:",
    b"PRINT.TIME:",
    b"Filament used:",
    b"TARGET_MACHINE.NAME:",
    b"EXTRUDER_TRAIN.",
    b".NOZZLE.DIAMETER:",
    b".MATERIAL.VOLUME_USED:",
    b"18446744073709551615",
    b"99999999999999999999d",
    b"-1",
    b"NaN",
    b"inf",
    b"1e308",
    b"0",
    b"0.4",
    b"31",
    b"32",
    b"4294967295",
    b"1d 2h 3m 4s",
    b"m",
    b"\xff\xfe",
    b"\0",
];

/// A file to try: a real one cut, spliced and corrupted, or noise.
fn mangle(rng: &mut Rng, fixtures: &[Vec<u8>]) -> Vec<u8> {
    let mut f = if rng.below(8) == 0 {
        Vec::new()
    } else {
        let src = &fixtures[rng.below(fixtures.len())];
        let a = rng.below(src.len() + 1);
        let b = a + rng.below(src.len() - a + 1);
        src[a..b].to_vec()
    };
    for _ in 0..rng.below(200) {
        let at = rng.below(f.len() + 1);
        match rng.below(4) {
            0 => {
                let w = WORDS[rng.below(WORDS.len())];
                f.splice(at..at, w.iter().copied());
            }
            1 if at < f.len() => f[at] = rng.next() as u8,
            2 if at < f.len() => {
                let end = (at + rng.below(64)).min(f.len());
                f.drain(at..end);
            }
            _ => {
                let n = rng.below(16);
                f.splice(at..at, (0..n).map(|_| rng.next() as u8));
            }
        }
    }
    if rng.below(32) == 0 {
        // A line far past the limit.
        let at = rng.below(f.len() + 1);
        f.splice(
            at..at,
            std::iter::repeat_n(b'x', MAX_LINE + rng.below(3 * MAX_LINE)),
        );
    }
    f
}

/// What any reading must be, whatever it was fed.
fn sound(m: &Metadata) {
    let quantity = |n: f64| n.is_finite() && n >= 0.0;
    assert!(
        m.nozzles_mm.iter().all(|&d| quantity(d) && d > 0.0),
        "{m:?}"
    );
    assert!(
        m.total_grams.is_none_or(|g| quantity(g) && g > 0.0),
        "{m:?}"
    );
    assert!(m.filaments.len() <= 32, "{} filaments", m.filaments.len());
    assert!(
        m.filaments.windows(2).all(|w| w[0].slot < w[1].slot),
        "slots out of order: {m:?}"
    );
    for f in &m.filaments {
        assert!(f.slot < 32, "{f:?}");
        assert!(f.grams.is_none_or(|g| quantity(g) && g > 0.0), "{f:?}");
        if let Some(c) = &f.color {
            let hex = c.strip_prefix('#').expect("a color starts with #");
            assert!(
                (hex.len() == 6 || hex.len() == 8)
                    && hex
                        .chars()
                        .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c)),
                "{c:?}"
            );
        }
        assert!(
            f.material.as_ref().is_none_or(|s| !s.trim().is_empty()),
            "{f:?}"
        );
    }
    assert!(
        m.printer_model
            .as_ref()
            .is_none_or(|s| !s.trim().is_empty()),
        "{m:?}"
    );
}

#[test]
fn the_reader_survives_whatever_a_file_holds() {
    let fixtures: Vec<Vec<u8>> =
        std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "gcode"))
            .map(|p| std::fs::read(p).unwrap())
            .collect();
    assert!(fixtures.len() >= 8, "{} fixtures", fixtures.len());

    let mut rng = Rng(seed());
    let cases: usize = std::env::var("POP_FUZZ_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(2000);
    for case in 0..cases {
        let file = mangle(&mut rng, &fixtures);
        let whole = read(&file);
        sound(&whole);

        // Fed in random pieces, it reads exactly the same, and never holds
        // more than one line's limit.
        let mut s = Scanner::new();
        let mut rest = &file[..];
        while !rest.is_empty() {
            let n = 1 + if rng.below(4) == 0 {
                rng.below(1 << 18)
            } else {
                rng.below(300)
            };
            let (piece, r) = rest.split_at(n.min(rest.len()));
            rest = r;
            s.push(piece);
            assert!(s.held() <= MAX_LINE, "case {case}: holds {}", s.held());
        }
        assert_eq!(
            s.finish(),
            whole,
            "case {case}: the pieces changed what was read"
        );
    }
}
