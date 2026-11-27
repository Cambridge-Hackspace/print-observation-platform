//! Reading a sliced `.gcode.3mf`, as Bambu Studio and OrcaSlicer write it.
//!
//! The archive holds one G-code per plate (`Metadata/plate_<n>.gcode`), each
//! read as plain G-code is; a picture of the plate (`Metadata/plate_<n>.png`)
//! when the slicer rendered one; and `Metadata/slice_info.config`, the
//! slicer's own summary of each plate. The summary gives what the G-code
//! does not: Bambu Studio writes each filament's grams there, and only
//! there.

use std::io::{Read, Seek};

use crate::{Metadata, Scanner, zip::Archive};

/// How much of an archive is read, at most. Every size is checked before
/// anything is decompressed, and again as it is.
#[derive(Debug, Clone)]
pub struct Limits {
    /// All plates' G-code together, decompressed.
    pub gcode_bytes: u64,
    /// One plate's picture. A larger one is not kept.
    pub thumbnail_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            gcode_bytes: 1 << 30,
            thumbnail_bytes: 1 << 20,
        }
    }
}

/// The most plates read from one archive. A slicer's plate count is the
/// member's, and a few is usual.
pub const MAX_PLATES: usize = 64;
/// The largest summary read.
const MAX_SLICE_INFO: u64 = 1 << 20;
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

/// One plate of a sliced 3MF.
#[derive(Debug, Clone, PartialEq)]
pub struct Plate {
    /// The plate's number, from 1, as the slicer numbers them.
    pub index: u32,
    pub metadata: Metadata,
    /// The slicer's picture of the plate, a PNG.
    pub thumbnail: Option<Vec<u8>>,
}

/// Why an archive was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not a zip archive at all.
    NotAnArchive,
    /// A zip archive using something a sliced 3MF does not.
    Unsupported(&'static str),
    /// Damaged, or not what it says it is.
    Corrupt(&'static str),
    /// Larger than the limits allow.
    TooLarge(&'static str),
    /// An archive with no sliced plate in it: a 3MF of a model, say, that
    /// has not been sliced.
    NoGcode,
    /// The file could not be read.
    Unreadable(std::io::ErrorKind),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnArchive => f.write_str("this is not a .gcode.3mf: it is not an archive"),
            Self::Unsupported(what) => {
                write!(f, "this .gcode.3mf uses {what}, which POP does not read")
            }
            Self::Corrupt(what) => write!(f, "this .gcode.3mf is damaged: {what}"),
            Self::TooLarge(what) => write!(f, "this .gcode.3mf is too large: {what}"),
            Self::NoGcode => f.write_str("this 3MF has not been sliced: it holds no G-code"),
            Self::Unreadable(kind) => write!(f, "the file could not be read: {kind}"),
        }
    }
}

impl std::error::Error for Error {}

/// Read every sliced plate in a `.gcode.3mf`.
pub fn read_3mf<R: Read + Seek>(r: R, limits: &Limits) -> Result<Vec<Plate>, Error> {
    let mut archive = Archive::open(r)?;
    let mut plates: Vec<(u32, usize)> = archive
        .entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| Some((plate_number(&e.name, ".gcode")?, i)))
        .collect();
    plates.sort();
    if plates.windows(2).any(|w| w[0].0 == w[1].0) {
        return Err(Error::Corrupt("two entries for one plate"));
    }
    if plates.is_empty() {
        return Err(Error::NoGcode);
    }
    if plates.len() > MAX_PLATES {
        return Err(Error::TooLarge("too many plates"));
    }

    let summary = match archive
        .entries
        .iter()
        .find(|e| e.name == "Metadata/slice_info.config")
    {
        Some(e) => {
            let e = e.clone();
            let mut xml = Vec::new();
            archive.read(&e, MAX_SLICE_INFO, |b| xml.extend_from_slice(b))?;
            summaries(&xml)
        }
        None => Vec::new(),
    };

    let mut budget = limits.gcode_bytes;
    let mut out = Vec::with_capacity(plates.len());
    for (index, i) in plates {
        let entry = archive.entries[i].clone();
        if entry.size > budget {
            return Err(Error::TooLarge("the G-code"));
        }
        budget -= entry.size;
        let mut scanner = Scanner::new();
        archive.read(&entry, entry.size, |b| scanner.push(b))?;
        let mut metadata = scanner.finish();
        if let Some(s) = summary.iter().find(|s| s.index == index) {
            s.fill(&mut metadata);
        }

        let picture = archive
            .entries
            .iter()
            .find(|e| plate_number(&e.name, ".png") == Some(index))
            .cloned();
        let thumbnail = match picture {
            Some(e) if e.size <= limits.thumbnail_bytes => {
                let mut png = Vec::new();
                archive.read(&e, limits.thumbnail_bytes, |b| png.extend_from_slice(b))?;
                png.starts_with(PNG).then_some(png)
            }
            _ => None,
        };
        out.push(Plate {
            index,
            metadata,
            thumbnail,
        });
    }
    Ok(out)
}

/// `n` for `Metadata/plate_<n><ext>`, where `n` is a plate's number from 1.
fn plate_number(name: &str, ext: &str) -> Option<u32> {
    let n = name.strip_prefix("Metadata/plate_")?.strip_suffix(ext)?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) || n.starts_with('0') {
        return None;
    }
    n.parse().ok()
}

/// What `slice_info.config` says about one plate.
#[derive(Debug, Default)]
struct Summary {
    index: u32,
    seconds: Option<u64>,
    grams: Option<f64>,
    filaments: Vec<SummaryFilament>,
}

#[derive(Debug)]
struct SummaryFilament {
    slot: usize,
    material: Option<String>,
    color: Option<String>,
    grams: Option<f64>,
}

impl Summary {
    /// Fill in what the G-code left missing. What the G-code says stands:
    /// the summary adds to it and overrules nothing, and names no filament
    /// the G-code shows unused.
    fn fill(&self, m: &mut Metadata) {
        m.estimated_seconds = m.estimated_seconds.or(self.seconds);
        m.total_grams = m.total_grams.or(self.grams);
        for f in &mut m.filaments {
            let Some(s) = self.filaments.iter().find(|s| s.slot == f.slot) else {
                continue;
            };
            f.material = f.material.take().or_else(|| s.material.clone());
            f.color = f.color.take().or_else(|| s.color.clone());
            f.grams = f.grams.or(s.grams);
        }
    }
}

/// Each plate's summary. A summary that does not parse says nothing.
fn summaries(xml: &[u8]) -> Vec<Summary> {
    let Ok(text) = std::str::from_utf8(xml) else {
        return Vec::new();
    };
    let Ok(doc) = roxmltree::Document::parse(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for plate in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("plate"))
    {
        let meta = |key: &str| {
            plate
                .children()
                .find(|n| n.has_tag_name("metadata") && n.attribute("key") == Some(key))
                .and_then(|n| n.attribute("value"))
        };
        let Some(index) = meta("index").and_then(|i| i.parse::<u32>().ok()) else {
            continue;
        };
        let filaments = plate
            .children()
            .filter(|n| n.has_tag_name("filament"))
            .filter_map(|n| {
                let id: usize = n.attribute("id")?.parse().ok()?;
                Some(SummaryFilament {
                    slot: id.checked_sub(1)?,
                    material: n.attribute("type").and_then(crate::read::text),
                    color: n.attribute("color").and_then(crate::read::color),
                    grams: n
                        .attribute("used_g")
                        .and_then(crate::read::amount)
                        .filter(|&g| g > 0.0),
                })
            })
            .collect();
        out.push(Summary {
            index,
            seconds: meta("prediction").and_then(|s| s.parse().ok()),
            grams: meta("weight")
                .and_then(crate::read::amount)
                .filter(|&g| g > 0.0),
            filaments,
        });
    }
    out
}
