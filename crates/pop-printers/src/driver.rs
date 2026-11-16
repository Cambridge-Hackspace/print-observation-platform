//! The interface every printer driver implements (#25).

use std::{fmt, future::Future, pin::Pin};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use ts_rs::TS;

use crate::state::PrinterState;

/// What a driver can do. POP offers only what a printer's driver supports:
/// no start button for a printer it can only watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
pub struct Capabilities {
    pub status: bool,
    pub upload: bool,
    pub start: bool,
    pub camera: bool,
    pub errors: bool,
}

/// Why a driver stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverError(pub String);

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DriverError {}

/// Where a driver reports its printer's state. Cheap to clone.
#[derive(Clone)]
pub struct Reporter {
    tx: mpsc::UnboundedSender<PrinterState>,
}

impl Reporter {
    pub(crate) fn new(tx: mpsc::UnboundedSender<PrinterState>) -> Self {
        Self { tx }
    }

    /// Report the printer's state as it is now.
    pub fn report(&self, state: PrinterState) {
        // The fleet outlives its drivers; if it has gone, so has everyone
        // who would read this.
        let _ = self.tx.send(state);
    }
}

pub type DriverFuture = Pin<Box<dyn Future<Output = Result<(), DriverError>> + Send>>;

/// A printer driver: a task that stays connected to one printer, reporting
/// its state, until it fails.
///
/// A driver need not reconnect on its own: when `run` returns, with an error
/// or not, or panics, its supervisor reports the printer offline and starts
/// a fresh driver, waiting longer between tries while it keeps failing. A
/// driver that can reconnect more cleverly (resuming a session, say) may do
/// so inside `run`.
///
/// Making a driver must not connect to anything: the fleet makes one to ask
/// its capabilities, and a fresh one each time it starts it.
pub trait Driver: Send + 'static {
    fn capabilities(&self) -> Capabilities;
    fn run(self: Box<Self>, reporter: Reporter) -> DriverFuture;
}
