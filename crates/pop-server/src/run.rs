//! Running the tower's printers (#49): every printer added to the tower (#23)
//! that BAM! still has in use, with its fields right (#21), runs in the fleet
//! (#25), and every screen is sent the tower's printers whenever one changes.
//!
//! The set is worked out again whenever BAM!'s view changes (#22) or an admin
//! decides about a printer. A printer whose settings change in BAM! gets a
//! fresh driver; one archived, declined, or whose fields go wrong leaves the
//! fleet. A printer POP has no driver for yet is listed, offline, so the
//! tower does not pretend it is not there.

use std::{collections::BTreeMap, sync::Arc};

use pop_integrations::{
    printers::{Connection, FieldMap, PrinterConfig, printer_from},
    sync::View,
};
use pop_printers::{
    Driver, Fleet, PrinterState,
    bambu::{Bambu, PinStore},
    fleet::{Backoff, Factory, Printer},
    moonraker::Moonraker,
    octoprint::OctoPrint,
    prusalink::PrusaLink,
};
use sqlx::SqlitePool;
use tokio::{
    sync::{Notify, broadcast::error::RecvError, watch},
    task::JoinHandle,
};

use crate::{
    api::{ServerEvent, TowerPrinter, TowerState},
    events::Events,
    secrets::Secrets,
};

/// What the runner needs.
pub struct Runner {
    pub db: SqlitePool,
    pub secrets: Secrets,
    pub fields: FieldMap,
    pub view: watch::Receiver<View>,
    pub events: Events,
    /// Notified when an admin decides about a printer.
    pub decided: Arc<Notify>,
    pub pins: Arc<dyn PinStore>,
    pub backoff: Backoff,
    /// Told the cameras of the printers that run (#66); none when POP has
    /// no go2rtc, and then no printer reads as having a camera.
    pub cameras: Option<watch::Sender<crate::cameras::Wanted>>,
}

/// A printer as it runs: its settings, and the access code it was given.
#[derive(PartialEq)]
struct Running {
    config: PrinterConfig,
    access_code: Option<String>,
    /// The login POP keeps for its camera, if any (#150).
    camera_login: Option<crate::secrets::CameraLogin>,
    /// The login POP keeps for its own host, if any, for its address now
    /// (#29, #30).
    printer_login: Option<pop_printers::Login>,
    watched: bool,
}

/// The driver for a printer, if POP has one for how it connects.
fn factory(
    connection: &Connection,
    access_code: Option<&str>,
    login: Option<&pop_printers::Login>,
    pins: &Arc<dyn PinStore>,
) -> Option<Factory> {
    let login = login.cloned();
    match connection.clone() {
        Connection::Moonraker { url } => Some(Arc::new(move || {
            Box::new(Moonraker::new(url.clone(), None)) as Box<dyn Driver>
        })),
        Connection::Bambu { host, serial } => {
            let code = access_code?.to_owned();
            let pins = pins.clone();
            Some(Arc::new(move || {
                Box::new(Bambu::new(
                    host.clone(),
                    serial.clone(),
                    code.clone(),
                    pins.clone(),
                )) as Box<dyn Driver>
            }))
        }
        Connection::OctoPrint { url } => {
            // OctoPrint's login is its API key; a user and password is not.
            let key = match login {
                Some(pop_printers::Login::ApiKey(k)) => Some(k),
                _ => None,
            };
            Some(Arc::new(move || {
                Box::new(OctoPrint::new(url.clone(), key.clone())) as Box<dyn Driver>
            }))
        }
        Connection::PrusaLink { url } => Some(Arc::new(move || {
            Box::new(PrusaLink::new(url.clone(), login.clone())) as Box<dyn Driver>
        })),
        Connection::UsbSerial { .. } => None,
    }
}

/// Start running the tower's printers. The receiver has them as every screen
/// is sent them.
pub fn spawn(r: Runner) -> (watch::Receiver<Vec<TowerPrinter>>, JoinHandle<()>) {
    let (tx, rx) = watch::channel(Vec::new());
    (rx, tokio::spawn(run(r, tx)))
}

async fn run(mut r: Runner, tx: watch::Sender<Vec<TowerPrinter>>) {
    let mut fleet = Fleet::new(r.backoff);
    let mut updates = fleet.updates();
    let mut running: BTreeMap<String, Running> = BTreeMap::new();
    r.view.mark_changed();
    loop {
        tokio::select! {
            changed = r.view.changed() => {
                if changed.is_err() {
                    return;
                }
                reconcile(&r, &mut fleet, &mut running).await;
            }
            () = r.decided.notified() => reconcile(&r, &mut fleet, &mut running).await,
            u = updates.recv() => match u {
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
        }
        publish(&r.events, &tx, &fleet, &running, r.cameras.is_some());
        if let Some(tx) = &r.cameras {
            cameras(tx, &running);
        }
    }
}

/// Make the fleet what it should be.
async fn reconcile(r: &Runner, fleet: &mut Fleet, running: &mut BTreeMap<String, Running>) {
    let view = r.view.borrow().clone();
    let decided = match crate::tower::states(&r.db).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("reading which printers the tower runs: {e}");
            return;
        }
    };
    let held = Secrets::with_access_codes(&r.db).await.unwrap_or_default();
    let has_code = |id: &str| held.iter().any(|h| h == id);
    let mut wanted = BTreeMap::new();
    for a in &view.printers {
        if decided.get(&a.asset.id) != Some(&TowerState::Added) {
            continue;
        }
        match printer_from(a, &r.fields, &has_code) {
            Ok(config) => {
                let access_code = match config.connection {
                    Connection::Bambu { .. } => {
                        match r.secrets.access_code(&r.db, &a.asset.id).await {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::error!("{}: its access code: {e}", config.name);
                                continue;
                            }
                        }
                    }
                    _ => None,
                };
                // A camera at an address may have a login POP keeps; without
                // it the camera is still tried, and says it refused.
                let camera_login = match &config.camera {
                    Some(pop_integrations::printers::Camera::Address { url }) => {
                        let login = r
                            .secrets
                            .camera_login(&r.db, &a.asset.id)
                            .await
                            .unwrap_or_else(|e| {
                                tracing::error!("{}: its camera's login: {e}", config.name);
                                None
                            });
                        if login
                            .as_ref()
                            .is_some_and(|l| l.origin != crate::cameras::origin(url))
                        {
                            tracing::warn!(
                                "{}: its camera's address changed in BAM!, so POP does not give it the login entered for the old one: enter it again on the desk",
                                config.name
                            );
                        }
                        login
                    }
                    _ => None,
                };
                // The printer's own login, only for the address it was
                // entered for: one changed in BAM! is not given it.
                let printer_login = match crate::printers::login_host(&config.connection) {
                    Some(url) => match r.secrets.printer_login(&r.db, &a.asset.id).await {
                        Ok(Some(l)) if l.origin == crate::cameras::origin(url) => Some(l.login),
                        Ok(Some(_)) => {
                            tracing::warn!(
                                "{}: its address changed in BAM!, so POP does not sign in to it with the login entered for the old one: enter it again on the desk",
                                config.name
                            );
                            None
                        }
                        Ok(None) => None,
                        Err(e) => {
                            tracing::error!("{}: its login: {e}", config.name);
                            None
                        }
                    },
                    None => None,
                };
                wanted.insert(
                    a.asset.id.clone(),
                    (config, access_code, camera_login, printer_login),
                );
            }
            Err(problems) => tracing::warn!(
                "{} is added to the tower but cannot run until it is right in BAM!: {}",
                a.asset.name,
                problems
                    .iter()
                    .map(|p| format!("{}: {}", p.field, p.problem))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        }
    }
    running.retain(|id, _| {
        let keep = wanted.contains_key(id);
        if !keep {
            fleet.remove(id);
        }
        keep
    });
    for (id, (config, access_code, camera_login, printer_login)) in wanted {
        let f = factory(
            &config.connection,
            access_code.as_deref(),
            printer_login.as_ref(),
            &r.pins,
        );
        let next = Running {
            config,
            access_code,
            camera_login,
            printer_login,
            watched: f.is_some(),
        };
        if running.get(&id) == Some(&next) {
            continue;
        }
        fleet.remove(&id);
        if let Some(factory) = f {
            fleet.add(Printer {
                id: id.clone(),
                factory,
            });
        }
        running.insert(id, next);
    }
}

/// Tell the cameras' keeper the running printers' cameras, if they changed.
fn cameras(tx: &watch::Sender<crate::cameras::Wanted>, running: &BTreeMap<String, Running>) {
    let wanted: crate::cameras::Wanted = running
        .iter()
        .filter_map(|(id, r)| {
            crate::cameras::source_of(&r.config, r.access_code.as_deref(), r.camera_login.as_ref())
                .map(|source| {
                    (
                        id.clone(),
                        crate::cameras::Camera {
                            name: r.config.name.clone(),
                            source,
                        },
                    )
                })
        })
        .collect();
    tx.send_if_modified(|w| {
        let changed = *w != wanted;
        *w = wanted;
        changed
    });
}

/// Send every screen the tower's printers, if they changed.
fn publish(
    events: &Events,
    tx: &watch::Sender<Vec<TowerPrinter>>,
    fleet: &Fleet,
    running: &BTreeMap<String, Running>,
    cameras: bool,
) {
    let mut printers: Vec<TowerPrinter> = running
        .iter()
        .map(|(id, r)| TowerPrinter {
            id: id.clone(),
            name: r.config.name.clone(),
            state: fleet.state(id).unwrap_or_else(PrinterState::offline),
            watched: r.watched,
            materials: r.config.materials.clone(),
            training: r.config.training.clone(),
            camera: cameras
                && crate::cameras::source_of(
                    &r.config,
                    r.access_code.as_deref(),
                    r.camera_login.as_ref(),
                )
                .is_some(),
        })
        .collect();
    printers.sort_by_key(|a| a.name.to_lowercase());
    if *tx.borrow() == printers {
        return;
    }
    tx.send_replace(printers.clone());
    // After the receiver has them, so a snapshot taken at this event's id
    // includes them.
    events.publish(ServerEvent::Printers(printers));
}
