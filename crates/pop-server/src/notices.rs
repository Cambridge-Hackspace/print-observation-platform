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
//!
//! What Discord took about a printer that needs a hand stands for its screen
//! to show (#55), until the printer no longer needs one.

use std::{
    collections::{BTreeMap, HashMap},
    time::{SystemTime, UNIX_EPOCH},
};

use pop_printers::PrinterStatus;
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

use crate::{
    api::{GroupNotice, ServerEvent, TowerPrinter},
    events::Events,
    notify::Notifier,
};

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

/// The printer as it would be with no one's print on it.
fn without_owner(p: &TowerPrinter) -> TowerPrinter {
    let mut p = p.clone();
    if let Some(j) = p.state.job.as_mut() {
        j.owner = None;
    }
    p
}

/// "Sam" from "Sam Lee".
pub fn first_name(owner: &str) -> Option<&str> {
    owner.split_whitespace().next()
}

/// The notice for a printer that needs a hand. A held print is "paused"; one
/// the printer has given up on "stopped" (#55), so nobody is told a print is
/// safe that is not.
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
        let what = if p.state.held { "is paused" } else { "stopped" };
        s.push_str(&format!(" {who}'s print {what}."));
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

/// A notice for the group, about one printer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// `without_owner` is the same notice with no one named in it, for the
    /// public stream when owners' names are not to go out on it (#134).
    NeedsHand {
        printer: String,
        text: String,
        without_owner: String,
    },
    Fixed {
        printer: String,
        text: String,
    },
}

impl Notice {
    pub fn text(&self) -> &str {
        match self {
            Self::NeedsHand { text, .. } | Self::Fixed { text, .. } => text,
        }
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
    pub fn see(&mut self, printers: &[TowerPrinter]) -> Vec<Notice> {
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
                Some(false) if now => out.push(Notice::NeedsHand {
                    printer: p.id.clone(),
                    text: needs_a_hand(p),
                    without_owner: needs_a_hand(&without_owner(p)),
                }),
                Some(true) if !now => out.push(Notice::Fixed {
                    printer: p.id.clone(),
                    text: fixed(p),
                }),
                // First seen, or no change.
                _ => {}
            }
        }
        out
    }
}

/// The needs-a-hand notices Discord has, while each trouble lasts.
///
/// Each trouble is numbered, so a notice that reaches Discord late (after
/// retries, say) stands only if its trouble is still the printer's current
/// one: never after the printer was fixed, nor over a newer trouble.
#[derive(Debug, Default)]
pub struct Told {
    trouble: HashMap<String, u64>,
    standing: BTreeMap<String, GroupNotice>,
}

impl Told {
    /// A printer's new trouble begins: its number, for [`Told::sent`].
    pub fn begin(&mut self, printer: &str) -> u64 {
        self.standing.remove(printer);
        let n = self.trouble.entry(printer.to_owned()).or_default();
        *n += 1;
        *n
    }

    /// A printer's trouble ends.
    pub fn end(&mut self, printer: &str) {
        self.standing.remove(printer);
        *self.trouble.entry(printer.to_owned()).or_default() += 1;
    }

    /// Discord has the notice for trouble `n`.
    pub fn sent(&mut self, n: u64, notice: GroupNotice) {
        if self.trouble.get(&notice.printer) == Some(&n) {
            self.standing.insert(notice.printer.clone(), notice);
        }
    }

    /// Forget printers the tower no longer has.
    pub fn keep(&mut self, printers: &[TowerPrinter]) {
        self.standing
            .retain(|id, _| printers.iter().any(|p| &p.id == id));
    }

    /// Every notice standing, by printer id.
    pub fn standing(&self) -> Vec<GroupNotice> {
        self.standing.values().cloned().collect()
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// Post the group's notices as the tower's printers change. The receiver has
/// the needs-a-hand notices Discord took, as every screen is sent them.
pub fn spawn(
    mut tower: watch::Receiver<Vec<TowerPrinter>>,
    notifier: Notifier,
    events: Events,
) -> (watch::Receiver<Vec<GroupNotice>>, JoinHandle<()>) {
    let (tx, rx) = watch::channel(Vec::new());
    let task = tokio::spawn(async move {
        let mut watcher = Watcher::default();
        let mut told = Told::default();
        let (sent_tx, mut sent_rx) = mpsc::unbounded_channel::<(u64, GroupNotice)>();
        loop {
            let printers = tower.borrow_and_update().clone();
            for notice in watcher.see(&printers) {
                match notice {
                    Notice::NeedsHand {
                        printer,
                        text,
                        without_owner,
                    } => {
                        let n = told.begin(&printer);
                        let (sent, taken) = oneshot::channel();
                        notifier.post_then(text.clone(), sent);
                        let back = sent_tx.clone();
                        tokio::spawn(async move {
                            if taken.await.is_ok() {
                                let at = unix_now();
                                let _ = back.send((
                                    n,
                                    GroupNotice {
                                        printer,
                                        text,
                                        at,
                                        without_owner,
                                    },
                                ));
                            }
                        });
                    }
                    Notice::Fixed { printer, text } => {
                        told.end(&printer);
                        notifier.post(text);
                    }
                }
            }
            told.keep(&printers);
            publish(&events, &tx, told.standing());
            loop {
                tokio::select! {
                    changed = tower.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        break;
                    }
                    Some((n, notice)) = sent_rx.recv() => {
                        told.sent(n, notice);
                        publish(&events, &tx, told.standing());
                    }
                }
            }
        }
    });
    (rx, task)
}

/// Send every screen what the group was told, if it changed.
fn publish(events: &Events, tx: &watch::Sender<Vec<GroupNotice>>, standing: Vec<GroupNotice>) {
    if *tx.borrow() == standing {
        return;
    }
    tx.send_replace(standing.clone());
    // After the receiver has them, so a snapshot taken at this event's id
    // includes them.
    events.publish(ServerEvent::Told(standing));
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
            camera: false,
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
            key: None,
        });
        p.state.held = true;
        p
    }

    fn texts(n: Vec<Notice>) -> Vec<String> {
        n.iter().map(|n| n.text().to_owned()).collect()
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
    fn a_print_the_printer_gave_up_on_is_stopped_not_paused() {
        let mut p = stuck("Ender 3");
        p.state.held = false;
        assert_eq!(
            needs_a_hand(&p),
            "Ender 3 needs a hand: the filament ran out at layer 112. Sam's print stopped."
        );
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
            [Notice::Fixed {
                printer: "id-Ender 3".into(),
                text: "Ender 3 is printing again.".into()
            }]
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
            texts(w.see(&[printer("Ender 3", PrinterStatus::Idle)])),
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
        assert!(matches!(&n[0], Notice::NeedsHand { printer, text, .. }
            if printer == "id-A" && text.starts_with("A needs a hand")));
    }

    fn told(printer: &str, text: &str) -> GroupNotice {
        GroupNotice {
            printer: printer.into(),
            text: text.into(),
            at: 1,
            without_owner: text.into(),
        }
    }

    #[test]
    fn what_discord_took_stands_while_the_trouble_lasts() {
        let mut t = Told::default();
        let n = t.begin("a");
        assert!(t.standing().is_empty(), "nothing until Discord has it");
        t.sent(n, told("a", "A needs a hand."));
        assert_eq!(t.standing(), [told("a", "A needs a hand.")]);
        t.end("a");
        assert!(t.standing().is_empty(), "fixed: nothing stands");
    }

    #[test]
    fn a_notice_taken_late_does_not_outlive_its_trouble() {
        let mut t = Told::default();
        let first = t.begin("a");
        t.end("a");
        t.sent(first, told("a", "late"));
        assert!(t.standing().is_empty(), "not after the fix");
        let first = t.begin("a");
        t.end("a");
        let second = t.begin("a");
        t.sent(first, told("a", "the old trouble"));
        assert!(t.standing().is_empty(), "not over a newer trouble");
        t.sent(second, told("a", "the new trouble"));
        assert_eq!(t.standing(), [told("a", "the new trouble")]);
    }

    #[test]
    fn a_new_trouble_clears_the_last_ones_notice() {
        let mut t = Told::default();
        let n = t.begin("a");
        t.sent(n, told("a", "the first"));
        t.begin("a");
        assert!(t.standing().is_empty());
    }

    #[test]
    fn a_printer_the_tower_no_longer_has_is_forgotten() {
        let mut t = Told::default();
        let (a, b) = (t.begin("id-A"), t.begin("id-B"));
        t.sent(a, told("id-A", "A"));
        t.sent(b, told("id-B", "B"));
        t.keep(&[printer("A", PrinterStatus::NeedsHand)]);
        assert_eq!(t.standing(), [told("id-A", "A")]);
    }
}
