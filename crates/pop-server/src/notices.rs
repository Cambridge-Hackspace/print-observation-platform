//! Which events notify the group (#69), and the words: one plain sentence,
//! naming the printer and the person, as the round 6 mockup has them.
//!
//! - A printer needs a hand: "Ender 3 needs a hand: the filament ran out at
//!   layer 112. Sam's print is paused."
//! - It is fixed: "Ender 3 is printing again."
//!
//! Someone reporting a problem (#63) and a communal request being funded post
//! from where those happen, when they are built.
//!
//! A printer's trouble is noticed once. Going offline and coming back (a
//! driver restarting, POP restarting) is not a new trouble, and a printer
//! first seen already needing a hand, as after POP restarts, is not
//! announced again.

use std::collections::HashMap;

use pop_printers::PrinterStatus;
use tokio::{sync::watch, task::JoinHandle};

use crate::{api::TowerPrinter, notify::Notifier};

/// "the filament ran out at layer 112" from "The filament ran out at layer
/// 112.": the catalog's words, to go mid-sentence. A leading acronym ("PLA")
/// keeps its capitals.
fn mid_sentence(words: &str) -> String {
    let w = words.trim().trim_end_matches('.');
    let mut chars = w.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && second.is_lowercase() => first
            .to_lowercase()
            .chain(w[first.len_utf8()..].chars())
            .collect(),
        _ => w.to_owned(),
    }
}

/// "Sam" from "Sam Lee".
fn first_name(owner: &str) -> Option<&str> {
    owner.split_whitespace().next()
}

/// The notice for a printer that needs a hand.
pub fn needs_a_hand(p: &TowerPrinter) -> String {
    let mut s = match &p.state.error {
        Some(e) => format!("{} needs a hand: {}.", p.name, mid_sentence(&e.words)),
        None => format!("{} needs a hand.", p.name),
    };
    if let Some(who) = p
        .state
        .job
        .as_ref()
        .and_then(|j| j.owner.as_deref())
        .and_then(first_name)
    {
        s.push_str(&format!(" {who}'s print is paused."));
    }
    s
}

/// The notice for a printer that no longer needs a hand.
pub fn fixed(p: &TowerPrinter) -> String {
    match p.state.status {
        PrinterStatus::Printing => format!("{} is printing again.", p.name),
        _ => format!("{} is fixed and ready.", p.name),
    }
}

/// What each printer last needed, to notice changes once.
#[derive(Debug, Default)]
pub struct Watcher {
    /// By id: whether it needs a hand, as last seen online.
    needs_hand: HashMap<String, bool>,
}

impl Watcher {
    /// The notices for the change from what was seen before to `printers`.
    pub fn see(&mut self, printers: &[TowerPrinter]) -> Vec<String> {
        let mut out = Vec::new();
        for p in printers {
            let now = match p.state.status {
                // Not known: neither a trouble nor a fix.
                PrinterStatus::Offline => continue,
                PrinterStatus::NeedsHand => true,
                // Paused by someone, or waiting to resume: not fixed yet.
                PrinterStatus::Paused => match self.needs_hand.get(&p.id) {
                    Some(true) => continue,
                    _ => false,
                },
                PrinterStatus::Idle | PrinterStatus::Printing | PrinterStatus::Done => false,
            };
            match self.needs_hand.insert(p.id.clone(), now) {
                Some(false) if now => out.push(needs_a_hand(p)),
                Some(true) if !now => out.push(fixed(p)),
                // First seen, or no change.
                _ => {}
            }
        }
        out
    }
}

/// Post the group's notices as the tower's printers change.
pub fn spawn(mut tower: watch::Receiver<Vec<TowerPrinter>>, notifier: Notifier) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut watcher = Watcher::default();
        loop {
            let printers = tower.borrow_and_update().clone();
            for notice in watcher.see(&printers) {
                notifier.post(notice);
            }
            if tower.changed().await.is_err() {
                return;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use pop_printers::{Job, Layer, PrinterError, PrinterState};

    use super::*;

    fn printer(name: &str, status: PrinterStatus) -> TowerPrinter {
        TowerPrinter {
            id: format!("id-{name}"),
            name: name.to_owned(),
            state: PrinterState {
                status,
                ..PrinterState::offline()
            },
            watched: true,
            materials: vec![],
            training: None,
        }
    }

    fn stuck(name: &str) -> TowerPrinter {
        let mut p = printer(name, PrinterStatus::NeedsHand);
        p.state.layer = Some(Layer {
            current: 112,
            total: Some(300),
        });
        p.state.job = Some(Job {
            name: "phone_stand.gcode".into(),
            owner: Some("Sam Lee".into()),
        });
        p.state.error = Some(PrinterError {
            words: "The filament ran out at layer 112.".into(),
            raw: "Filament runout".into(),
            known: true,
        });
        p
    }

    #[test]
    fn the_words_are_the_mockups() {
        assert_eq!(
            needs_a_hand(&stuck("Ender 3")),
            "Ender 3 needs a hand: the filament ran out at layer 112. Sam's print is paused."
        );
        assert_eq!(
            fixed(&printer("Ender 3", PrinterStatus::Printing)),
            "Ender 3 is printing again."
        );
        assert_eq!(
            fixed(&printer("Ender 3", PrinterStatus::Idle)),
            "Ender 3 is fixed and ready."
        );
        assert_eq!(
            needs_a_hand(&printer("X1", PrinterStatus::NeedsHand)),
            "X1 needs a hand."
        );
        assert_eq!(mid_sentence("PLA jammed."), "PLA jammed");
    }

    #[test]
    fn a_trouble_is_noticed_once_and_its_fix_once() {
        let mut w = Watcher::default();
        assert!(
            w.see(&[printer("Ender 3", PrinterStatus::Printing)])
                .is_empty()
        );
        assert_eq!(w.see(&[stuck("Ender 3")]).len(), 1);
        assert!(
            w.see(&[stuck("Ender 3")]).is_empty(),
            "not again while it waits"
        );
        assert_eq!(
            w.see(&[printer("Ender 3", PrinterStatus::Printing)]),
            ["Ender 3 is printing again."]
        );
        assert!(
            w.see(&[printer("Ender 3", PrinterStatus::Printing)])
                .is_empty()
        );
    }

    #[test]
    fn going_offline_and_back_is_not_a_new_trouble_or_a_fix() {
        let mut w = Watcher::default();
        w.see(&[printer("Ender 3", PrinterStatus::Printing)]);
        w.see(&[stuck("Ender 3")]);
        assert!(
            w.see(&[printer("Ender 3", PrinterStatus::Offline)])
                .is_empty()
        );
        assert!(w.see(&[stuck("Ender 3")]).is_empty(), "the same trouble");
        assert!(
            w.see(&[printer("Ender 3", PrinterStatus::Offline)])
                .is_empty()
        );
        assert_eq!(
            w.see(&[printer("Ender 3", PrinterStatus::Idle)]),
            ["Ender 3 is fixed and ready."]
        );
    }

    #[test]
    fn a_printer_first_seen_needing_a_hand_is_not_announced_again() {
        // As after POP restarts: the group was told before.
        let mut w = Watcher::default();
        assert!(w.see(&[stuck("Ender 3")]).is_empty());
        // But its fix is news.
        assert_eq!(
            w.see(&[printer("Ender 3", PrinterStatus::Printing)]).len(),
            1
        );
    }

    #[test]
    fn a_pause_after_a_trouble_is_not_yet_a_fix() {
        let mut w = Watcher::default();
        w.see(&[printer("Ender 3", PrinterStatus::Printing)]);
        w.see(&[stuck("Ender 3")]);
        assert!(
            w.see(&[printer("Ender 3", PrinterStatus::Paused)])
                .is_empty()
        );
        assert_eq!(
            w.see(&[printer("Ender 3", PrinterStatus::Printing)]).len(),
            1
        );
        // A pause on its own is not a trouble.
        assert!(
            w.see(&[printer("Ender 3", PrinterStatus::Paused)])
                .is_empty()
        );
    }

    #[test]
    fn each_printer_is_its_own() {
        let mut w = Watcher::default();
        w.see(&[
            printer("A", PrinterStatus::Printing),
            printer("B", PrinterStatus::Printing),
        ]);
        let n = w.see(&[stuck("A"), printer("B", PrinterStatus::Done)]);
        assert_eq!(n.len(), 1);
        assert!(n[0].starts_with("A needs a hand"));
    }
}
