//! One state model for every printer (#25), whatever its driver.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where a printer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PrinterStatus {
    /// Not reachable: switched off, unplugged, or its driver is restarting.
    Offline,
    /// Ready for a print.
    Idle,
    Printing,
    Paused,
    /// Stopped for a person: an error, a jam, filament run out.
    NeedsHand,
    /// Finished; the print is still on the bed.
    Done,
}

/// How far through its layers a print is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Layer {
    pub current: u32,
    /// The total, when the printer says.
    pub total: Option<u32>,
}

/// The print a printer is working on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Job {
    /// The file's name, as the slicer named it.
    pub name: String,
    /// Whose print it is, when POP sent it (#45); unknown for one started at
    /// the printer itself.
    pub owner: Option<String>,
}

/// Everything POP shows about a printer, as its driver last reported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct PrinterState {
    pub status: PrinterStatus,
    /// 0 to 1, while there is a print.
    pub progress: Option<f32>,
    pub layer: Option<Layer>,
    /// Seconds until done, as the printer estimates.
    #[ts(type = "number | null")]
    pub seconds_left: Option<u64>,
    pub job: Option<Job>,
    /// What is wrong, as the driver read it. Plain words come from the error
    /// catalog (#35).
    pub error: Option<String>,
}

impl PrinterState {
    /// A printer POP cannot reach.
    pub fn offline() -> Self {
        Self {
            status: PrinterStatus::Offline,
            progress: None,
            layer: None,
            seconds_left: None,
            job: None,
            error: None,
        }
    }

    /// An idle printer with nothing to report.
    pub fn idle() -> Self {
        Self {
            status: PrinterStatus::Idle,
            ..Self::offline()
        }
    }
}
