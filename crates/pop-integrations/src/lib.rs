//! The outside systems POP talks to, each behind an adapter chosen in
//! configuration (ADR 0001, "Integrations: interchangeable adapters"): sign-in
//! providers, card readers, training, group notices, personal messages, BAM!
//! for inventory, and Throne for funding.

pub mod bam;
pub mod github;
pub mod go2rtc;
pub mod printers;
pub mod sync;

#[cfg(feature = "fakes")]
pub mod fakes;
