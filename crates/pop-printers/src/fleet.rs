//! The fleet (#25): every printer as its own supervised task.
//!
//! A printer's supervisor makes a driver, runs it, and when it returns,
//! fails or panics, reports the printer offline and makes a fresh one after a
//! pause that doubles while it keeps failing (up to a limit), and resets once
//! a driver has run well for a while. A panic in one printer's driver is
//! caught by its own task and touches no other printer.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use tokio::{
    sync::{broadcast, mpsc, watch},
    task::JoinHandle,
    time::Instant,
};

use crate::{
    driver::{Capabilities, Driver, Reporter},
    state::PrinterState,
};

/// Makes a fresh driver for a printer, each time its supervisor needs one.
pub type Factory = Arc<dyn Fn() -> Box<dyn Driver> + Send + Sync>;

/// A printer for the fleet to run.
pub struct Printer {
    /// POP's id for it.
    pub id: String,
    pub factory: Factory,
}

/// One printer's state changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub printer: String,
    pub state: PrinterState,
}

/// How long the supervisor waits before a fresh driver, at first and at
/// most; and how long a driver must have run to count as having recovered.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub first: Duration,
    pub max: Duration,
    pub healthy_after: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(1),
            max: Duration::from_secs(60),
            healthy_after: Duration::from_secs(60),
        }
    }
}

struct Entry {
    state: watch::Receiver<PrinterState>,
    capabilities: Capabilities,
    supervisor: JoinHandle<()>,
}

/// Every printer POP runs, each under its own supervisor.
pub struct Fleet {
    printers: BTreeMap<String, Entry>,
    updates: broadcast::Sender<Update>,
    backoff: Backoff,
}

impl Default for Fleet {
    fn default() -> Self {
        Self::new(Backoff::default())
    }
}

impl Fleet {
    pub fn new(backoff: Backoff) -> Self {
        Self {
            printers: BTreeMap::new(),
            updates: broadcast::channel(1024).0,
            backoff,
        }
    }

    /// Every state change of every printer, as it happens.
    pub fn updates(&self) -> broadcast::Receiver<Update> {
        self.updates.subscribe()
    }

    /// Start running `printer`. It reads as offline until its driver says
    /// otherwise.
    pub fn add(&mut self, printer: Printer) {
        let capabilities = (printer.factory)().capabilities();
        let (state_tx, state_rx) = watch::channel(PrinterState::offline());
        let supervisor = tokio::spawn(supervise(
            printer.id.clone(),
            printer.factory,
            state_tx,
            self.updates.clone(),
            self.backoff,
        ));
        if let Some(old) = self.printers.insert(
            printer.id,
            Entry {
                state: state_rx,
                capabilities,
                supervisor,
            },
        ) {
            old.supervisor.abort();
        }
    }

    /// Stop running a printer.
    pub fn remove(&mut self, id: &str) {
        if let Some(e) = self.printers.remove(id) {
            e.supervisor.abort();
        }
    }

    pub fn state(&self, id: &str) -> Option<PrinterState> {
        self.printers.get(id).map(|e| e.state.borrow().clone())
    }

    pub fn capabilities(&self, id: &str) -> Option<Capabilities> {
        self.printers.get(id).map(|e| e.capabilities)
    }

    /// Every printer and its state, by id.
    pub fn states(&self) -> BTreeMap<String, PrinterState> {
        self.printers
            .iter()
            .map(|(id, e)| (id.clone(), e.state.borrow().clone()))
            .collect()
    }
}

impl Drop for Fleet {
    fn drop(&mut self) {
        for e in self.printers.values() {
            e.supervisor.abort();
        }
    }
}

async fn supervise(
    id: String,
    factory: Factory,
    state: watch::Sender<PrinterState>,
    updates: broadcast::Sender<Update>,
    backoff: Backoff,
) {
    let set = |mut s: PrinterState| {
        // Every error explained before anyone sees it (#35), whichever
        // driver reported it.
        if let Some(e) = &s.error {
            s.error = Some(crate::errors::explain(&e.raw, s.layer.map(|l| l.current)));
        }
        if *state.borrow() != s {
            state.send_replace(s.clone());
            let _ = updates.send(Update {
                printer: id.clone(),
                state: s,
            });
        }
    };
    let mut wait = backoff.first;
    loop {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let driver = factory();
        let started = Instant::now();
        // The driver runs as a task of its own, so a panic in it ends that
        // task and nothing else: its JoinHandle says so.
        let mut task = tokio::spawn(driver.run(Reporter::new(tx)));
        let outcome = loop {
            tokio::select! {
                Some(s) = rx.recv() => set(s),
                r = &mut task => break r,
            }
        };
        // Reports sent just before the end still count.
        while let Ok(s) = rx.try_recv() {
            set(s);
        }
        match outcome {
            Ok(Ok(())) => tracing::warn!("printer {id}: the driver stopped; starting it again"),
            Ok(Err(e)) => tracing::warn!("printer {id}: the driver failed: {e}; starting it again"),
            Err(e) if e.is_panic() => {
                tracing::error!("printer {id}: the driver panicked; starting it again")
            }
            Err(_) => return, // aborted: the printer was removed
        }
        set(PrinterState::offline());
        if started.elapsed() >= backoff.healthy_after {
            wait = backoff.first;
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(backoff.max);
    }
}
