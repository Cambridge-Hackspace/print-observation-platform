//! The fleet's supervision (#25), with drivers that misbehave on purpose.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use pop_printers::{
    Capabilities, Driver, DriverError, Fleet, Printer, PrinterState, PrinterStatus, Reporter,
    driver::DriverFuture,
    fleet::{Backoff, Factory},
};
use tokio::time::Instant;

/// How a test driver behaves once started.
#[derive(Clone, Copy)]
enum Act {
    /// Reports printing, with progress rising, every 5 ms, forever.
    Tick,
    /// Reports once, then panics.
    Panic,
    /// Reports once, then fails.
    Fail,
    /// Reports idle twice, the same state both times, then returns as if
    /// finished.
    End,
    /// Runs `ms` milliseconds, then fails.
    RunThenFail(u64),
}

struct Test(Act);

impl Driver for Test {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            status: true,
            ..Capabilities::default()
        }
    }

    fn run(self: Box<Self>, r: Reporter) -> DriverFuture {
        Box::pin(async move {
            let printing = |p: f32| PrinterState {
                status: PrinterStatus::Printing,
                progress: Some(p),
                ..PrinterState::idle()
            };
            match self.0 {
                Act::Tick => {
                    let mut p = 0.0;
                    loop {
                        p += 0.001;
                        r.report(printing(p));
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
                Act::Panic => {
                    r.report(PrinterState::idle());
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    panic!("a driver bug");
                }
                Act::Fail => {
                    r.report(PrinterState::idle());
                    Err(DriverError("the printer hung up".into()))
                }
                Act::End => {
                    r.report(PrinterState::idle());
                    r.report(PrinterState::idle());
                    Ok(())
                }
                Act::RunThenFail(ms) => {
                    r.report(PrinterState::idle());
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    Err(DriverError("gone".into()))
                }
            }
        })
    }
}

/// A factory for `act`, counting the drivers it makes and when.
fn factory(act: Act) -> (Factory, Arc<AtomicU32>, Arc<Mutex<Vec<Instant>>>) {
    let made = Arc::new(AtomicU32::new(0));
    let when = Arc::new(Mutex::new(Vec::new()));
    let (m, w) = (made.clone(), when.clone());
    let f: Factory = Arc::new(move || {
        m.fetch_add(1, Ordering::SeqCst);
        w.lock().unwrap().push(Instant::now());
        Box::new(Test(act))
    });
    (f, made, when)
}

fn quick() -> Backoff {
    Backoff {
        first: Duration::from_millis(10),
        max: Duration::from_millis(50),
        healthy_after: Duration::from_millis(500),
    }
}

fn progress(fleet: &Fleet, id: &str) -> f32 {
    fleet.state(id).and_then(|s| s.progress).unwrap_or(0.0)
}

/// The issue's "done when": a driver that panics is restarted, again and
/// again, while the rest of the fleet keeps reporting the whole time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_driver_is_restarted_and_the_fleet_keeps_reporting() {
    let mut fleet = Fleet::new(quick());
    let (tick_a, _, _) = factory(Act::Tick);
    let (tick_b, _, _) = factory(Act::Tick);
    let (panics, made, _) = factory(Act::Panic);
    fleet.add(Printer {
        id: "x1".into(),
        factory: tick_a,
    });
    fleet.add(Printer {
        id: "mk4".into(),
        factory: tick_b,
    });
    fleet.add(Printer {
        id: "broken".into(),
        factory: panics,
    });
    let mut updates = fleet.updates();

    // Watch for a while, sampling the healthy printers as the broken one
    // panics and restarts underneath them.
    let mut samples = Vec::new();
    let mut broken_offline = 0;
    let until = Instant::now() + Duration::from_millis(600);
    while Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(40)).await;
        samples.push((progress(&fleet, "x1"), progress(&fleet, "mk4")));
        while let Ok(u) = updates.try_recv() {
            if u.printer == "broken" && u.state.status == PrinterStatus::Offline {
                broken_offline += 1;
            }
        }
    }

    // `made` counts the one the fleet made to ask its capabilities, too.
    let restarts = made.load(Ordering::SeqCst) - 2;
    assert!(
        restarts >= 3,
        "the panicking driver was restarted {restarts} time(s)"
    );
    assert!(
        broken_offline >= 2,
        "it read as offline between drivers {broken_offline} time(s)"
    );
    for w in samples.windows(2) {
        assert!(
            w[1].0 > w[0].0 && w[1].1 > w[0].1,
            "a healthy printer stopped reporting: {samples:?}"
        );
    }
    assert_eq!(fleet.state("x1").unwrap().status, PrinterStatus::Printing);
}

#[tokio::test(start_paused = true)]
async fn the_wait_between_drivers_doubles_up_to_a_limit() {
    let mut fleet = Fleet::new(quick());
    let (f, _, when) = factory(Act::Fail);
    fleet.add(Printer {
        id: "p".into(),
        factory: f,
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    let w = when.lock().unwrap().clone();
    // The first is the capabilities driver; the rest are runs.
    let gaps: Vec<u128> = w[1..]
        .windows(2)
        .map(|p| (p[1] - p[0]).as_millis())
        .collect();
    assert_eq!(&gaps[..5], &[10, 20, 40, 50, 50], "{gaps:?}");
}

#[tokio::test(start_paused = true)]
async fn a_driver_that_ran_well_resets_the_wait() {
    let mut fleet = Fleet::new(quick());
    let (f, _, when) = factory(Act::RunThenFail(600));
    fleet.add(Printer {
        id: "p".into(),
        factory: f,
    });
    tokio::time::sleep(Duration::from_millis(2_000)).await;
    let w = when.lock().unwrap().clone();
    let gaps: Vec<u128> = w[1..]
        .windows(2)
        .map(|p| (p[1] - p[0]).as_millis())
        .collect();
    // Each ran 600 ms, past healthy_after, so each wait is the first again.
    assert!(gaps.len() >= 2, "{gaps:?}");
    assert!(gaps.iter().all(|g| *g == 610), "{gaps:?}");
}

#[tokio::test(start_paused = true)]
async fn a_driver_that_fails_or_just_ends_is_started_again() {
    for act in [Act::Fail, Act::End] {
        let mut fleet = Fleet::new(quick());
        let (f, made, _) = factory(act);
        fleet.add(Printer {
            id: "p".into(),
            factory: f,
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            made.load(Ordering::SeqCst) >= 4,
            "{}",
            made.load(Ordering::SeqCst)
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_removed_printer_is_not_started_again() {
    let mut fleet = Fleet::new(quick());
    let (f, made, _) = factory(Act::Fail);
    fleet.add(Printer {
        id: "p".into(),
        factory: f,
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    fleet.remove("p");
    let at = made.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(made.load(Ordering::SeqCst), at);
    assert!(fleet.state("p").is_none());
}

#[tokio::test(start_paused = true)]
async fn only_a_change_of_state_is_published() {
    let mut fleet = Fleet::new(Backoff {
        first: Duration::from_secs(3600),
        ..quick()
    });
    let (f, _, _) = factory(Act::End);
    let mut updates = fleet.updates();
    fleet.add(Printer {
        id: "p".into(),
        factory: f,
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut seen = Vec::new();
    while let Ok(u) = updates.try_recv() {
        seen.push(u.state.status);
    }
    // Offline at first (not published: nothing changed), idle (reported
    // twice, published once), then offline when it ended; then a long wait
    // with nothing new.
    assert_eq!(seen, vec![PrinterStatus::Idle, PrinterStatus::Offline]);
}

#[tokio::test]
async fn a_new_printer_reads_as_offline_and_says_what_it_can_do() {
    let mut fleet = Fleet::new(Backoff {
        first: Duration::from_secs(3600),
        ..quick()
    });
    let (f, _, _) = factory(Act::Panic);
    fleet.add(Printer {
        id: "p".into(),
        factory: f,
    });
    assert_eq!(fleet.state("p").unwrap().status, PrinterStatus::Offline);
    assert_eq!(
        fleet.capabilities("p").unwrap(),
        Capabilities {
            status: true,
            ..Capabilities::default()
        }
    );
}
