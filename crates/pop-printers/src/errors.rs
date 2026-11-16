//! Errors in plain words (#35): one catalog for every driver's errors.
//!
//! A driver reports what its printer said, as it said it ([`PrinterError::raw`]).
//! The fleet explains it before anyone sees it: [`explain`] finds the
//! catalog's words for it, adding the layer when there is one ("at layer
//! 112"). An error the catalog does not know is shown with what the printer
//! said, and logged, never hidden.
//!
//! **Every entry names its source**, and a test holds each to having one: a
//! catalog that guesses what an error means is worse than one that shows the
//! printer's own words. Klipper's entries are its own messages, from its
//! source. Bambu's codes have no entries yet: none has a source in hand that
//! says what it means, so each is shown as its code until one does. Linking an
//! error to the guide that fixes it is #73's.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What went wrong with a printer, for people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PrinterError {
    /// In plain words.
    pub words: String,
    /// What the printer said, as it said it.
    pub raw: String,
    /// Whether the catalog knew it. When it did not, `words` carry `raw`.
    pub known: bool,
}

impl PrinterError {
    /// An error as a driver read it, before the catalog has explained it.
    pub fn raw(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        Self {
            words: raw.clone(),
            raw,
            known: false,
        }
    }
}

/// How an entry recognizes an error.
#[derive(Debug, Clone, Copy)]
enum Match {
    StartsWith(&'static str),
    Contains(&'static str),
}

struct Entry {
    matches: Match,
    /// `{heater}` is replaced with the heater's name, for the one entry that
    /// has one.
    words: &'static str,
    /// Whether the layer belongs in the sentence: for what happens during a
    /// print, not for what happens to the printer.
    at_layer: bool,
    /// Where this meaning comes from.
    source: &'static str,
}

const KLIPPER: &str = "https://github.com/Klipper3d/klipper/blob/master/";

// Order matters: the first that matches is used, so a specific reason comes
// before the general shutdown it is part of.
const CATALOG: &[Entry] = &[
    Entry {
        matches: Match::Contains("Timer too close"),
        words: "The printer's controller fell behind its schedule and shut down",
        at_layer: true,
        source: "klippy/mcu.py: an MCU shutdown, reason \"Timer too close\"; docs/FAQ.md",
    },
    Entry {
        matches: Match::Contains("ADC out of range"),
        words: "A temperature reading went out of range and the printer shut down",
        at_layer: true,
        source: "klippy/mcu.py: an MCU shutdown, reason \"ADC out of range\"; docs/FAQ.md",
    },
    Entry {
        matches: Match::StartsWith("Lost communication with MCU"),
        words: "The printer lost contact with its controller board",
        at_layer: true,
        source: "klippy/mcu.py: \"Lost communication with MCU '%s'\"",
    },
    Entry {
        matches: Match::StartsWith("MCU '"),
        words: "The printer's controller shut down",
        at_layer: true,
        source: "klippy/mcu.py: \"MCU '%s' shutdown: %s\"",
    },
    Entry {
        matches: Match::StartsWith("Heater "),
        words: "The {heater} heater is not warming up as fast as it should",
        at_layer: true,
        source: "klippy/extras/verify_heater.py: \"Heater %s not heating at expected rate\"",
    },
    Entry {
        matches: Match::StartsWith("Move out of range"),
        words: "A move went past the edge of the printer's build area",
        at_layer: true,
        source: "klippy/kinematics: \"Move out of range\"",
    },
    Entry {
        matches: Match::StartsWith("Must home axis first"),
        words: "The printer was asked to move before finding its home position",
        at_layer: false,
        source: "klippy/kinematics: \"Must home axis first\"",
    },
    Entry {
        matches: Match::StartsWith("Extrude below minimum temp"),
        words: "The printer tried to push filament before the nozzle was hot enough",
        at_layer: true,
        source: "klippy/kinematics/extruder.py: \"Extrude below minimum temp\"",
    },
];

/// The heater's name in Klipper's "Heater extruder not heating at expected
/// rate", if that is what `raw` is.
fn heater(raw: &str) -> Option<&str> {
    raw.strip_prefix("Heater ")?
        .split_once(" not heating at expected rate")
        .map(|(name, _)| name)
}

/// Explain what a printer said, adding the layer it happened at when it
/// happened during a print.
pub fn explain(raw: &str, layer: Option<u32>) -> PrinterError {
    let found = CATALOG.iter().find(|e| match e.matches {
        Match::StartsWith(p) => raw.starts_with(p) && (p != "Heater " || heater(raw).is_some()),
        Match::Contains(p) => raw.contains(p),
    });
    let Some(entry) = found else {
        tracing::warn!("a printer error the catalog does not know: {raw}");
        return PrinterError {
            words: format!("The printer reported: {raw}"),
            raw: raw.to_owned(),
            known: false,
        };
    };
    let mut words = entry.words.to_owned();
    if let Some(h) = heater(raw) {
        words = words.replace("{heater}", h);
    }
    if let (true, Some(n)) = (entry.at_layer, layer) {
        words.push_str(&format!(" at layer {n}"));
    }
    words.push('.');
    PrinterError {
        words,
        raw: raw.to_owned(),
        known: true,
    }
}

/// Where each entry's meaning comes from, for the test that holds each to
/// having one, and for a person checking it.
pub fn sources() -> Vec<String> {
    CATALOG
        .iter()
        .map(|e| {
            if e.source.starts_with("klippy/") {
                format!("{KLIPPER}{}", e.source)
            } else {
                e.source.to_owned()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn klippers_own_messages_get_plain_words() {
        for (raw, words) in [
            (
                "Heater extruder not heating at expected rate",
                "The extruder heater is not warming up as fast as it should.",
            ),
            (
                "Heater heater_bed not heating at expected rate",
                "The heater_bed heater is not warming up as fast as it should.",
            ),
            (
                "Move out of range: 250.000 10.000 0.300 [12.000]",
                "A move went past the edge of the printer's build area.",
            ),
            (
                "Must home axis first: 10.000 10.000 5.000 [0.000]",
                "The printer was asked to move before finding its home position.",
            ),
            (
                "Extrude below minimum temp\nSee the 'min_extrude_temp' config option for details",
                "The printer tried to push filament before the nozzle was hot enough.",
            ),
            (
                "Lost communication with MCU 'mcu'",
                "The printer lost contact with its controller board.",
            ),
            (
                "MCU 'mcu' shutdown: Timer too close",
                "The printer's controller fell behind its schedule and shut down.",
            ),
            (
                "MCU 'mcu' shutdown: ADC out of range",
                "A temperature reading went out of range and the printer shut down.",
            ),
            (
                "MCU 'toolhead' shutdown: Rescheduled timer in the past",
                "The printer's controller shut down.",
            ),
        ] {
            let e = explain(raw, None);
            assert!(e.known, "{raw}");
            assert_eq!(e.words, words, "{raw}");
            assert_eq!(e.raw, raw, "the printer's own words are kept");
        }
    }

    #[test]
    fn what_happens_during_a_print_says_at_which_layer() {
        let e = explain("Heater extruder not heating at expected rate", Some(112));
        assert_eq!(
            e.words,
            "The extruder heater is not warming up as fast as it should at layer 112."
        );
        // Homing is not something a layer happens to.
        let h = explain("Must home axis first: 1 2 3 [0]", Some(112));
        assert!(!h.words.contains("layer"), "{}", h.words);
    }

    #[test]
    fn an_unknown_error_is_shown_with_the_printers_own_words() {
        for raw in [
            "Bambu error 0700_8011",
            "Bambu HMS 0300_0100_0001_0007",
            "Probe triggered prior to movement",
        ] {
            let e = explain(raw, Some(40));
            assert!(!e.known, "{raw}");
            assert_eq!(e.raw, raw);
            assert!(
                e.words.contains(raw),
                "the printer's words are hidden: {}",
                e.words
            );
        }
    }

    #[test]
    fn a_message_that_only_looks_like_a_heater_one_is_not_explained_as_one() {
        assert!(!explain("Heater extruder approaching target", None).known);
    }

    #[test]
    fn every_entry_names_its_source() {
        let s = sources();
        assert_eq!(s.len(), CATALOG.len());
        for (e, src) in CATALOG.iter().zip(&s) {
            assert!(
                src.len() > KLIPPER.len() || !src.starts_with("http"),
                "{}: {src}",
                e.words
            );
            assert!(
                e.source.contains('"') || e.source.contains("FAQ"),
                "{}: cites no message: {}",
                e.words,
                e.source
            );
        }
    }
}
