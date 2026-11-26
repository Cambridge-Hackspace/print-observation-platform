//! Slicer metadata from G-code and 3MF files.
//!
//! POP is not a slicer (ADR 0001). It reads what the slicer wrote into a file:
//! the printer model, the material, the estimated time and filament. Bambu
//! printers take a sliced `.gcode.3mf`, so metadata is read from the G-code
//! inside the 3MF as well as from plain G-code.
//!
//! What a file says is the uploader's claim, not a measurement: anyone can
//! write a comment line. It is good for offering the right printers and
//! saying what a print needs, and no more.
//!
//! A value a file does not state, or states in a form this does not read, is
//! missing (`None`, or an empty list). Nothing is worked out from something
//! else: grams are never computed from length and a guessed density, and a
//! file that says it weighs 0 g while using filament has no weight.

mod read;
pub mod threemf;
mod zip;

pub use read::{MAX_LINE, Scanner};
pub use threemf::{Limits, Plate, read_3mf};

/// What a slicer wrote about a print.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    /// The slicer that wrote the file.
    pub slicer: Option<Slicer>,
    /// The printer model the file was sliced for, in the slicer's own words
    /// ("MK4S", "Bambu Lab A1 mini", "Creality Ender-3"). Slicers name the same
    /// printer differently, so this is matched through aliases (#24).
    pub printer_model: Option<String>,
    /// Each extruder's nozzle diameter in millimetres, by extruder.
    pub nozzles_mm: Vec<f64>,
    /// How long the slicer estimates the print takes, in seconds.
    pub estimated_seconds: Option<u64>,
    /// The filaments the print uses, by extruder or slot. A slot the file
    /// shows unused is left out; where it shows no use at all, every slot it
    /// names is listed.
    pub filaments: Vec<Filament>,
    /// The whole print's filament, in grams.
    pub total_grams: Option<f64>,
}

/// One filament a print uses.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filament {
    /// The extruder or slot, from 0.
    pub slot: usize,
    /// The material, as the slicer names it ("PLA", "PETG").
    pub material: Option<String>,
    /// `#RRGGBB` or `#RRGGBBAA`, upper case.
    pub color: Option<String>,
    /// Grams of it the print uses.
    pub grams: Option<f64>,
}

/// Which slicer wrote a file, and its version as the file gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slicer {
    pub kind: SlicerKind,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlicerKind {
    PrusaSlicer,
    SuperSlicer,
    OrcaSlicer,
    BambuStudio,
    /// UltiMaker Cura, whose engine signs itself `Cura_SteamEngine`.
    Cura,
    /// A slicer this does not know, by the name it gave.
    Other(String),
}

/// Read a whole file's metadata. For a large file, feed a [`Scanner`] in
/// pieces instead, as it is read.
pub fn read(gcode: &[u8]) -> Metadata {
    let mut s = Scanner::new();
    s.push(gcode);
    s.finish()
}
