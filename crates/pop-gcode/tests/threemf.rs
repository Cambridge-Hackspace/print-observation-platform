//! Reading a sliced `.gcode.3mf` (#39): real archives from Bambu Studio and
//! OrcaSlicer, archives built here to be hostile, and the reader fuzzed.
//!
//! The values expected of the real archives were read off them by hand
//! (Python's zipfile), not from this reader. The fuzzer is seeded:
//! `POP_FUZZ_SEED=<n>` replays a run, `POP_FUZZ_CASES` sets how many.

use std::io::{Cursor, Write};

use pop_gcode::{
    Filament, Limits, Metadata, Plate, read, read_3mf,
    threemf::{Error, MAX_PLATES},
};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn plates(bytes: &[u8]) -> Result<Vec<Plate>, Error> {
    read_3mf(Cursor::new(bytes), &Limits::default())
}

// ---- real archives --------------------------------------------------------------

#[test]
fn bambu_studio_writes_each_filaments_grams_only_in_its_summary() {
    let got = plates(&fixture("bambustudio-a1mini.gcode.3mf")).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].index, 1);
    assert_eq!(
        got[0].thumbnail, None,
        "made without a display to render one"
    );
    // The G-code inside is the plain .gcode fixture, byte for byte, so it
    // reads the same, with slice_info.config's used_g="0.98" added.
    let mut want = read(&fixture("bambustudio-a1mini.gcode"));
    assert_eq!(want.filaments[0].grams, None);
    want.filaments[0].grams = Some(0.98);
    assert_eq!(got[0].metadata, want);
}

#[test]
fn a_slot_the_gcode_shows_unused_stays_out() {
    let got = plates(&fixture("bambustudio-a1mini-two-filaments.gcode.3mf")).unwrap();
    assert_eq!(
        got[0].metadata.filaments,
        vec![Filament {
            slot: 0,
            material: Some("PLA".into()),
            color: Some("#00AE42".into()),
            grams: Some(0.98),
        }]
    );
}

#[test]
fn orcaslicer_with_its_picture_of_the_plate() {
    let got = plates(&fixture("orcaslicer-ender3v2.gcode.3mf")).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].metadata, read(&fixture("orcaslicer-ender3v2.gcode")));
    let png = got[0].thumbnail.as_deref().expect("plate_1.png");
    assert_eq!(png.len(), 2960);
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    // IHDR: 512 x 512.
    assert_eq!(&png[16..24], &[0, 0, 2, 0, 0, 0, 2, 0]);
}

#[test]
fn a_picture_larger_than_the_limit_is_not_kept() {
    let limits = Limits {
        thumbnail_bytes: 2959,
        ..Limits::default()
    };
    let got = read_3mf(
        Cursor::new(fixture("orcaslicer-ender3v2.gcode.3mf")),
        &limits,
    )
    .unwrap();
    assert_eq!(got[0].thumbnail, None);
    let limits = Limits {
        thumbnail_bytes: 2960,
        ..Limits::default()
    };
    let got = read_3mf(
        Cursor::new(fixture("orcaslicer-ender3v2.gcode.3mf")),
        &limits,
    )
    .unwrap();
    assert!(got[0].thumbnail.is_some());
}

// ---- archives built here ----------------------------------------------------------

/// One entry of an archive to build, and how to lie about it.
#[derive(Clone)]
struct Item {
    name: &'static str,
    data: Vec<u8>,
    deflate: bool,
    /// Say this size instead of the true one.
    says: Option<u32>,
    /// Say this CRC instead of the true one.
    crc: Option<u32>,
    /// Say this compressed size instead of the true one.
    compressed: Option<u32>,
    flags: u16,
    method: Option<u16>,
}

fn item(name: &'static str, data: &[u8]) -> Item {
    Item {
        name,
        data: data.to_vec(),
        deflate: true,
        says: None,
        crc: None,
        compressed: None,
        flags: 0,
        method: None,
    }
}

/// A zip archive of `items`, written the way miniz (which both slicers use)
/// writes one.
fn archive(items: &[Item]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for it in items {
        let body = if it.deflate {
            let mut e =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(&it.data).unwrap();
            e.finish().unwrap()
        } else {
            it.data.clone()
        };
        let method = it.method.unwrap_or(if it.deflate { 8 } else { 0 });
        let crc = it.crc.unwrap_or_else(|| crc32fast::hash(&it.data));
        let size = it.says.unwrap_or(it.data.len() as u32);
        let offset = out.len() as u32;
        let fields = |b: &mut Vec<u8>| {
            b.extend_from_slice(&20u16.to_le_bytes());
            b.extend_from_slice(&it.flags.to_le_bytes());
            b.extend_from_slice(&method.to_le_bytes());
            b.extend_from_slice(&[0; 4]);
            b.extend_from_slice(&crc.to_le_bytes());
            b.extend_from_slice(&it.compressed.unwrap_or(body.len() as u32).to_le_bytes());
            b.extend_from_slice(&size.to_le_bytes());
            b.extend_from_slice(&(it.name.len() as u16).to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes());
        };
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        fields(&mut out);
        out.extend_from_slice(it.name.as_bytes());
        out.extend_from_slice(&body);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        fields(&mut central);
        // Comment length, disk, internal attributes; then external.
        central.extend_from_slice(&[0; 6]);
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(it.name.as_bytes());
    }
    let at = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(items.len() as u16).to_le_bytes());
    out.extend_from_slice(&(items.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

const GCODE: &[u8] = b"; generated by OrcaSlicer 2.5.0 on today\n; printer_model = X1\n; filament_type = PLA\n; filament used [g] = 1.5\n";

#[test]
fn the_archive_built_here_reads_as_a_slicers_does() {
    // The builder is right, or every refusal below proves nothing.
    let got = plates(&archive(&[item("Metadata/plate_1.gcode", GCODE)])).unwrap();
    assert_eq!(got[0].metadata.printer_model.as_deref(), Some("X1"));
    let stored = Item {
        deflate: false,
        ..item("Metadata/plate_1.gcode", GCODE)
    };
    assert_eq!(plates(&archive(&[stored])).unwrap(), got);
}

#[test]
fn a_zip_bomb_is_refused_when_it_says_too_much_and_when_it_lies() {
    let zeros = vec![0u8; 16 << 20];
    // It says how large it is, and that is past the limit: refused before a
    // byte is decompressed.
    let limits = Limits {
        gcode_bytes: 1 << 20,
        ..Limits::default()
    };
    let bomb = archive(&[item("Metadata/plate_1.gcode", &zeros)]);
    assert!(
        bomb.len() < 64 << 10,
        "16 MiB of zeros compress to {} bytes",
        bomb.len()
    );
    assert_eq!(
        read_3mf(Cursor::new(&bomb), &limits),
        Err(Error::TooLarge("the G-code"))
    );
    // It says it is small, and is not: refused one byte past what it said.
    let liar = archive(&[Item {
        says: Some(100),
        ..item("Metadata/plate_1.gcode", &zeros)
    }]);
    assert_eq!(
        plates(&liar),
        Err(Error::Corrupt("an entry is longer than it says"))
    );
}

#[test]
fn the_gcode_limit_is_for_all_plates_together() {
    let one = vec![b'\n'; 600];
    let two = archive(&[
        item("Metadata/plate_1.gcode", &one),
        item("Metadata/plate_2.gcode", &one),
    ]);
    let limits = |n| Limits {
        gcode_bytes: n,
        ..Limits::default()
    };
    assert_eq!(
        read_3mf(Cursor::new(&two), &limits(1200)).map(|p| p.len()),
        Ok(2)
    );
    assert_eq!(
        read_3mf(Cursor::new(&two), &limits(1199)),
        Err(Error::TooLarge("the G-code"))
    );
}

#[test]
fn plates_come_in_order_and_only_by_their_own_names() {
    let got = plates(&archive(&[
        item("Metadata/plate_10.gcode", b"; printer_model = ten\n"),
        item("Metadata/plate_2.gcode", b"; printer_model = two\n"),
        item("Metadata/plate_02.gcode", b"; printer_model = zero-two\n"),
        item("Metadata/plate_0.gcode", b"; printer_model = zero\n"),
        item("Metadata/plate_.gcode", b"; printer_model = none\n"),
        item("plate_3.gcode", b"; printer_model = elsewhere\n"),
    ]))
    .unwrap();
    assert_eq!(
        got.iter()
            .map(|p| (p.index, p.metadata.printer_model.clone().unwrap()))
            .collect::<Vec<_>>(),
        [(2, "two".to_owned()), (10, "ten".to_owned())]
    );
}

#[test]
fn too_many_plates_are_refused() {
    let names: Vec<&'static str> = (1..=MAX_PLATES + 1)
        .map(|n| &*Box::leak(format!("Metadata/plate_{n}.gcode").into_boxed_str()))
        .collect();
    let items: Vec<Item> = names.iter().map(|n| item(n, b"\n")).collect();
    assert_eq!(
        plates(&archive(&items)),
        Err(Error::TooLarge("too many plates"))
    );
    assert_eq!(
        plates(&archive(&items[..MAX_PLATES])).map(|p| p.len()),
        Ok(MAX_PLATES)
    );
}

#[test]
fn what_a_sliced_3mf_does_not_use_is_refused() {
    let g = || item("Metadata/plate_1.gcode", GCODE);
    assert_eq!(
        plates(&archive(&[Item { flags: 1, ..g() }])),
        Err(Error::Unsupported("an encrypted entry"))
    );
    assert_eq!(
        plates(&archive(&[Item {
            method: Some(12),
            ..g()
        }])),
        Err(Error::Unsupported(
            "an entry compressed other than by deflate"
        ))
    );
    assert_eq!(
        plates(&archive(&[Item {
            says: Some(u32::MAX),
            ..g()
        }])),
        Err(Error::Unsupported("a zip64 archive"))
    );
}

#[test]
fn a_damaged_archive_is_refused() {
    let g = || item("Metadata/plate_1.gcode", GCODE);
    assert_eq!(
        plates(&archive(&[Item {
            crc: Some(0xdead_beef),
            ..g()
        }])),
        Err(Error::Corrupt("an entry's checksum does not match"))
    );
    assert_eq!(
        plates(&archive(&[Item {
            says: Some(GCODE.len() as u32 + 1),
            ..g()
        }])),
        Err(Error::Corrupt("an entry is shorter than it says"))
    );

    // Its local header names another entry.
    let mut a = archive(&[g()]);
    let at = a.windows(5).position(|w| w == b"Metad").unwrap();
    a[at] = b'm';
    assert_eq!(
        plates(&a),
        Err(Error::Corrupt(
            "an entry's header does not match the directory"
        ))
    );

    // Truncated anywhere.
    let whole = archive(&[g()]);
    for cut in [0, 1, 21, whole.len() / 2, whole.len() - 1] {
        assert!(plates(&whole[..cut]).is_err(), "cut at {cut}");
    }
    assert_eq!(plates(b"not a zip at all"), Err(Error::NotAnArchive));
    assert_eq!(
        plates(GCODE),
        Err(Error::NotAnArchive),
        "plain G-code is not a 3MF"
    );
}

/// The end record's place in an archive this file built: its last 22 bytes.
fn end(a: &[u8]) -> usize {
    a.len() - 22
}

#[test]
fn the_end_record_is_the_one_whose_comment_reaches_the_end() {
    let mut a = archive(&[item("Metadata/plate_1.gcode", GCODE)]);
    // A comment holding what looks like another end record.
    let comment = b"PK\x05\x06 not the end";
    let e = end(&a);
    a[e + 20..e + 22].copy_from_slice(&(comment.len() as u16).to_le_bytes());
    a.extend_from_slice(comment);
    assert_eq!(plates(&a).map(|p| p.len()), Ok(1));
}

#[test]
fn an_archive_split_across_disks_or_with_its_directory_elsewhere_is_refused() {
    let whole = archive(&[item("Metadata/plate_1.gcode", GCODE)]);
    let mut a = whole.clone();
    let e = end(&a);
    a[e + 4] = 1;
    assert_eq!(
        plates(&a),
        Err(Error::Unsupported("an archive split across disks"))
    );

    let mut a = whole.clone();
    let e = end(&a);
    let at = u32::from_le_bytes(a[e + 16..e + 20].try_into().unwrap());
    a[e + 16..e + 20].copy_from_slice(&(at - 1).to_le_bytes());
    assert_eq!(
        plates(&a),
        Err(Error::Corrupt(
            "the central directory is not where the archive says"
        ))
    );
}

#[test]
fn an_entry_that_says_it_runs_into_the_directory_is_refused() {
    let a = archive(&[Item {
        compressed: Some(1 << 20),
        ..item("Metadata/plate_1.gcode", GCODE)
    }]);
    assert_eq!(
        plates(&a),
        Err(Error::Corrupt("an entry runs into the central directory"))
    );
}

#[test]
fn a_stored_entry_whose_sizes_differ_is_refused() {
    let a = archive(&[Item {
        deflate: false,
        says: Some(GCODE.len() as u32 - 1),
        ..item("Metadata/plate_1.gcode", GCODE)
    }]);
    assert_eq!(
        plates(&a),
        Err(Error::Corrupt("a stored entry's sizes differ"))
    );
}

#[test]
fn a_summary_past_its_limit_is_refused() {
    let big = vec![b' '; (1 << 20) + 1];
    let a = archive(&[
        item("Metadata/plate_1.gcode", GCODE),
        item("Metadata/slice_info.config", &big),
    ]);
    assert_eq!(plates(&a), Err(Error::TooLarge("an entry")));
    let a = archive(&[
        item("Metadata/plate_1.gcode", GCODE),
        item("Metadata/slice_info.config", &big[1..]),
    ]);
    assert!(plates(&a).is_ok());
}

#[test]
fn a_directory_past_its_limit_is_refused() {
    // 40,000 empty entries with 100-byte names: a directory of about 5.8 MiB.
    let names: Vec<&'static str> = (0..40_000)
        .map(|n| &*Box::leak(format!("{n:0>100}").into_boxed_str()))
        .collect();
    let items: Vec<Item> = names
        .iter()
        .map(|n| Item {
            deflate: false,
            ..item(n, b"")
        })
        .collect();
    assert_eq!(
        plates(&archive(&items)),
        Err(Error::TooLarge("the archive's central directory"))
    );
}

#[test]
fn two_entries_for_one_plate_are_refused() {
    let a = archive(&[
        item("Metadata/plate_1.gcode", GCODE),
        item("Metadata/plate_1.gcode", b"; printer_model = other\n"),
    ]);
    assert_eq!(plates(&a), Err(Error::Corrupt("two entries for one plate")));
}

#[test]
fn a_picture_that_is_not_a_png_is_not_kept() {
    let a = archive(&[
        item("Metadata/plate_1.gcode", GCODE),
        item("Metadata/plate_1.png", b"GIF89a not a png"),
    ]);
    assert_eq!(plates(&a).unwrap()[0].thumbnail, None);
    let png = b"\x89PNG\r\n\x1a\nthe rest";
    let a = archive(&[
        item("Metadata/plate_1.gcode", GCODE),
        item("Metadata/plate_1.png", png),
    ]);
    assert_eq!(plates(&a).unwrap()[0].thumbnail.as_deref(), Some(&png[..]));
}

#[test]
fn a_signature_in_the_comment_is_not_the_end_record() {
    let mut a = archive(&[item("Metadata/plate_1.gcode", GCODE)]);
    // A comment holding a whole end record, saying it has no comment of its
    // own: only one whose comment runs exactly to the end is the end.
    let mut comment = b"PK\x05\x06".to_vec();
    comment.extend_from_slice(&[0; 18]);
    comment.extend_from_slice(b"and more");
    let e = end(&a);
    a[e + 20..e + 22].copy_from_slice(&(comment.len() as u16).to_le_bytes());
    a.extend_from_slice(&comment);
    assert_eq!(plates(&a).map(|p| p.len()), Ok(1));
}

#[test]
fn an_entry_whose_own_header_is_not_one_is_refused() {
    let mut a = archive(&[item("Metadata/plate_1.gcode", GCODE)]);
    a[0] = b'X';
    assert_eq!(
        plates(&a),
        Err(Error::Corrupt(
            "an entry's header does not match the directory"
        ))
    );
    // Nor one compressed differently from what the directory says.
    let mut a = archive(&[item("Metadata/plate_1.gcode", GCODE)]);
    a[8] = 0;
    assert_eq!(
        plates(&a),
        Err(Error::Corrupt(
            "an entry's header does not match the directory"
        ))
    );
}

#[test]
fn a_zip64_archive_is_refused() {
    let mut a = archive(&[item("Metadata/plate_1.gcode", GCODE)]);
    let e = end(&a);
    a[e + 16..e + 20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(plates(&a), Err(Error::Unsupported("a zip64 archive")));
}

#[test]
fn a_summary_with_nothing_measured_adds_nothing() {
    let timed = b"; estimated printing time (normal mode) = 2m\n; filament_type = PLA\n";
    let info = br#"<config><plate>
  <metadata key="index" value="1"/>
  <metadata key="prediction" value="600"/>
  <metadata key="weight" value="0"/>
  <filament id="1" type="PLA" used_g="0.00"/>
</plate></config>"#;
    let got = plates(&archive(&[
        item("Metadata/plate_1.gcode", timed),
        item("Metadata/slice_info.config", info),
    ]))
    .unwrap();
    let m = &got[0].metadata;
    assert_eq!(m.estimated_seconds, Some(120), "the G-code's time stands");
    assert_eq!(m.total_grams, None);
    assert_eq!(m.filaments[0].grams, None);
}

#[test]
fn an_unsliced_3mf_holds_no_gcode() {
    assert_eq!(
        plates(&archive(&[item("3D/3dmodel.model", b"<model/>")])),
        Err(Error::NoGcode)
    );
}

#[test]
fn a_summary_adds_what_the_gcode_lacks_and_overrules_nothing() {
    let info = br##"<?xml version="1.0" encoding="UTF-8"?>
<config><plate>
  <metadata key="index" value="1"/>
  <metadata key="prediction" value="600"/>
  <metadata key="weight" value="9.5"/>
  <filament id="1" type="PETG" color="#ffffff" used_g="7.25"/>
  <filament id="2" type="TPU" color="#000000" used_g="2.25"/>
</plate></config>"##;
    let got = plates(&archive(&[
        item("Metadata/plate_1.gcode", GCODE),
        item("Metadata/slice_info.config", info),
    ]))
    .unwrap();
    let m = &got[0].metadata;
    assert_eq!(m.estimated_seconds, Some(600), "the G-code gives no time");
    assert_eq!(m.total_grams, Some(9.5), "nor a total");
    assert_eq!(
        m.filaments,
        vec![Filament {
            slot: 0,
            material: Some("PLA".into()),
            color: Some("#FFFFFF".into()),
            grams: Some(1.5),
        }],
        "the G-code's material and grams stand; slot 2 the G-code does not name"
    );
}

#[test]
fn a_summary_that_does_not_parse_says_nothing() {
    let bomb = br#"<?xml version="1.0"?>
<!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">]>
<config><plate><metadata key="index" value="1"/><metadata key="prediction" value="&lol2;"/></plate></config>"#;
    for info in [&bomb[..], b"<config><plate>", b"\xff\xfe"] {
        let got = plates(&archive(&[
            item("Metadata/plate_1.gcode", GCODE),
            item("Metadata/slice_info.config", info),
        ]))
        .unwrap();
        assert_eq!(got[0].metadata, read(GCODE));
    }
}

// ---- the fuzzer ----------------------------------------------------------------

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

fn sound(m: &Metadata) {
    assert!(
        m.total_grams.is_none_or(|g| g.is_finite() && g > 0.0),
        "{m:?}"
    );
    assert!(m.filaments.len() <= 32, "{m:?}");
    for f in &m.filaments {
        assert!(f.grams.is_none_or(|g| g.is_finite() && g > 0.0), "{f:?}");
    }
}

#[test]
fn the_reader_survives_whatever_an_archive_holds() {
    let real: Vec<Vec<u8>> = [
        "bambustudio-a1mini.gcode.3mf",
        "bambustudio-a1mini-two-filaments.gcode.3mf",
        "orcaslicer-ender3v2.gcode.3mf",
    ]
    .iter()
    .map(|n| fixture(n))
    .collect();
    let limits = Limits {
        gcode_bytes: 4 << 20,
        thumbnail_bytes: 1 << 20,
    };
    let mut rng = Rng(seed());
    let cases: usize = std::env::var("POP_FUZZ_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(400);
    let (mut read_ok, mut refused) = (0, 0);
    for case in 0..cases {
        let mut a = real[rng.below(real.len())].clone();
        match rng.below(4) {
            // Flip bytes: in the headers and directory as often as the data.
            0 | 1 => {
                for _ in 0..1 + rng.below(8) {
                    let at = if rng.below(2) == 0 {
                        a.len() - 1 - rng.below(a.len().min(2048))
                    } else {
                        rng.below(a.len())
                    };
                    a[at] = rng.next() as u8;
                }
            }
            // Cut it short.
            2 => a.truncate(rng.below(a.len())),
            // Overwrite a run with a little-endian 32-bit number: a size, an
            // offset or a count.
            _ => {
                let at = rng.below(a.len() - 4);
                let n = [0u32, 1, 0xffff, u32::MAX, rng.next() as u32][rng.below(5)];
                a[at..at + 4].copy_from_slice(&n.to_le_bytes());
            }
        }
        match read_3mf(Cursor::new(&a), &limits) {
            Ok(plates) => {
                read_ok += 1;
                assert!(
                    !plates.is_empty() && plates.len() <= MAX_PLATES,
                    "case {case}"
                );
                for p in &plates {
                    sound(&p.metadata);
                    if let Some(t) = &p.thumbnail {
                        assert!(t.starts_with(b"\x89PNG\r\n\x1a\n"), "case {case}");
                        assert!(t.len() as u64 <= limits.thumbnail_bytes, "case {case}");
                    }
                }
            }
            Err(_) => refused += 1,
        }
    }
    // A fuzzer that only ever sees refusals tests the first check alone.
    if cases >= 100 {
        assert!(
            read_ok > 0 && refused > 0,
            "read {read_ok}, refused {refused}"
        );
    }
}
