//! Printer access: one driver interface, and a driver per kind of printer.
//!
//! Every printer is reached through one trait, [`Driver`] (ADR 0001,
//! "Printers: one driver interface"; #25). Each driver says what it can do
//! ([`Capabilities`]) and reports one state model ([`PrinterState`]). Each
//! printer runs as its own supervised task in a [`Fleet`]: a driver that
//! fails, panics or stops is replaced with a fresh one, after a pause that
//! grows while it keeps failing, and the printer reads as offline meanwhile.
//! No other printer notices. Drivers arrive in the ADR's order: Moonraker,
//! then OctoPrint and PrusaLink, then Bambu in LAN mode, then USB serial.

pub mod bambu;
pub mod driver;
pub mod errors;
#[cfg(feature = "fakes")]
pub mod fakes;
pub mod fleet;
pub mod moonraker;
pub mod state;

pub use driver::{Capabilities, Driver, DriverError, Reporter};
pub use errors::{CatalogEntry, PrinterError, catalog, explain};
pub use fleet::{Fleet, Printer, Update};
pub use state::{Job, Layer, PrinterState, PrinterStatus};
