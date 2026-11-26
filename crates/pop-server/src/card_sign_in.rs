//! A card tap at a kiosk's reader signs the member in there (#18), where POP
//! is a CS:S device (pop_integrations::css_device).
//!
//! 1. The tap goes from the reader to CS:S's edge. On a yes, the edge leases
//!    POP, bound as `power` on that resource, and on a resource set to
//!    identify its member the lease says who, sealed with POP's command key.
//!    POP finds the enrolled kiosk whose card reader that resource is
//!    (kiosks::with_card_reader). A tap at a reader no kiosk has is no
//!    sign-in anywhere.
//! 2. The tap waits for that kiosk, in memory, for [`TAP_FOR`]. A newer tap
//!    there replaces it.
//! 3. The kiosk asks for it ([`waiting`], `GET /api/kiosk/tap`, which waits
//!    up to [`LONG_POLL`] for one) with its own enrolled credential (the
//!    routes' need, `Need::Kiosk`, crate::access), and is told only whom to
//!    welcome. It is never on the live stream, which is public.
//! 4. The kiosk takes it ([`take`], `POST /api/kiosk/tap`): the account the
//!    member's CS:S id names is signed in as a browser sign-in's would be, a
//!    kiosk session, and made on their first tap as on a first sign-in.
//! 5. Or the kiosk sets it aside ([`dismiss`], `DELETE /api/kiosk/tap`).
//!
//! **What POP believes, and why.** The member is CS:S's word, sealed: the
//! edge seals each lease with the key CS:S gave POP alone, so a lease forged
//! on the broker signs nobody in (crate::css_device). POP believes the edge
//! as far as CS:S does. A card sign-in carries no roles, so it never makes or
//! unmakes an admin: only a browser sign-in, whose ID token names them, does.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use pop_integrations::css_device::{LocalMember, Tap};
use serde::Serialize;
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use ts_rs::TS;

use crate::{AppState, access::KioskId, api::ApiError, sign_in::NoAccount};

/// How long the kiosk's ask waits for a tap before answering that there is none.
pub const LONG_POLL: Duration = Duration::from_secs(25);
/// How long a tap waits for its kiosk to take it: long enough for a screen
/// that was asleep, short enough that the next member is not signed in as
/// the last one.
pub const TAP_FOR: Duration = Duration::from_secs(60);

/// What the kiosk shows for its tap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TapView {
    /// A member's sign-in, ready: welcome them by `name`, and take it.
    SignIn {
        /// Their first name, as CS:S holds their name.
        name: String,
    },
}

/// `GET /api/kiosk/tap`'s answer: the kiosk's tap, or none yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct KioskTap {
    pub tap: Option<TapView>,
}

/// The first word of a member's name, which is how a kiosk greets them:
/// CS:S keeps one full name and no given name apart.
pub fn first_name(full: &str) -> String {
    full.split_whitespace().next().unwrap_or("").to_owned()
}

struct Waiting {
    member: LocalMember,
    at: Instant,
}

impl Waiting {
    fn fresh(&self) -> bool {
        self.at.elapsed() < TAP_FOR
    }

    fn view(&self) -> TapView {
        TapView::SignIn {
            name: first_name(&self.member.name),
        }
    }
}

/// The taps waiting for each kiosk, by its id.
#[derive(Clone)]
pub struct CardSignIns {
    /// The `[[sign_in.providers]]` whose `sub` a member's CS:S id is.
    provider: String,
    waiting: Arc<Mutex<HashMap<i64, Waiting>>>,
    /// Bumped at every tap, for the kiosks waiting to hear of one.
    changed: watch::Sender<u64>,
    /// How long a kiosk's ask waits: [`LONG_POLL`], but for tests.
    long_poll: Duration,
}

impl CardSignIns {
    pub fn new(provider: &str) -> Self {
        Self {
            provider: provider.to_owned(),
            waiting: Arc::new(Mutex::new(HashMap::new())),
            changed: watch::channel(0).0,
            long_poll: LONG_POLL,
        }
    }

    /// The same, with a kiosk's ask waiting `d` instead (for tests).
    pub fn with_long_poll(mut self, d: Duration) -> Self {
        self.long_poll = d;
        self
    }

    /// A tap the edge leased POP for (step 1), kept for the kiosk whose
    /// reader it was.
    pub async fn heard(&self, db: &sqlx::SqlitePool, tap: Tap) {
        let kiosk = match crate::kiosks::with_card_reader(db, &tap.resource).await {
            Ok(Some(k)) => k,
            Ok(None) => {
                tracing::debug!("card taps: {} is no kiosk's reader", tap.resource);
                return;
            }
            Err(e) => {
                tracing::error!("card taps: finding the kiosk: {e}");
                return;
            }
        };
        self.waiting.lock().unwrap().insert(
            kiosk,
            Waiting {
                member: tap.member,
                at: Instant::now(),
            },
        );
        self.changed.send_modify(|n| *n += 1);
    }

    /// Hear taps from `taps` until it closes.
    pub fn spawn(self, db: sqlx::SqlitePool, mut taps: mpsc::Receiver<Tap>) -> JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(t) = taps.recv().await {
                self.heard(&db, t).await;
            }
        })
    }

    fn view(&self, kiosk: i64) -> Option<TapView> {
        let mut all = self.waiting.lock().unwrap();
        all.retain(|_, w| w.fresh());
        all.get(&kiosk).map(Waiting::view)
    }
}

/// Whether POP hears the space's broker, on `/healthz` (#18, #19).
pub struct BrokerHealth(pub pop_integrations::css_device::Hearing);

impl crate::health::HealthCheck for BrokerHealth {
    fn name(&self) -> &'static str {
        "card_reader"
    }

    fn check(&self) -> crate::health::CheckFuture<'_> {
        let now = self.0.borrow().clone();
        Box::pin(async move {
            match now {
                None => Ok(()),
                Some(why) => {
                    tracing::error!("health: card taps: {why}");
                    Err("POP cannot hear card taps: the space's broker does not answer".to_owned())
                }
            }
        })
    }
}

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ApiError { error: msg.into() })).into_response()
}

/// The refusal when POP has no card sign-in.
fn no_cards() -> Response {
    error(StatusCode::NOT_FOUND, "POP has no card sign-in")
}

/// `GET /api/kiosk/tap` (step 3): this kiosk's tap, waiting up to
/// [`LONG_POLL`] for one.
pub async fn waiting(
    State(state): State<AppState>,
    Extension(KioskId(kiosk)): Extension<KioskId>,
) -> Response {
    let Some(cards) = state.card_sign_ins.as_ref() else {
        return no_cards();
    };
    let mut changed = cards.changed.subscribe();
    let deadline = tokio::time::Instant::now() + cards.long_poll;
    loop {
        if let Some(tap) = cards.view(kiosk) {
            return Json(KioskTap { tap: Some(tap) }).into_response();
        }
        match tokio::time::timeout_at(deadline, changed.changed()).await {
            Ok(Ok(())) => continue,
            _ => return Json(KioskTap { tap: None }).into_response(),
        }
    }
}

/// `DELETE /api/kiosk/tap` (step 5): set this kiosk's tap aside.
pub async fn dismiss(
    State(state): State<AppState>,
    Extension(KioskId(kiosk)): Extension<KioskId>,
) -> Response {
    let Some(cards) = state.card_sign_ins.as_ref() else {
        return no_cards();
    };
    cards.waiting.lock().unwrap().remove(&kiosk);
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/kiosk/tap` (step 4): sign in with this kiosk's tap.
pub async fn take(
    State(state): State<AppState>,
    Extension(KioskId(kiosk)): Extension<KioskId>,
    connect: Option<Extension<ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
) -> Response {
    let Some(cards) = state.card_sign_ins.as_ref() else {
        return no_cards();
    };
    // Taken out as it is used, so two screens' worth of taps cannot sign in
    // with one tap twice.
    let w = {
        let mut all = cards.waiting.lock().unwrap();
        all.retain(|_, w| w.fresh());
        all.remove(&kiosk)
    };
    let Some(w) = w else {
        return error(
            StatusCode::NOT_FOUND,
            "there is no card tap to sign in with: tap again",
        );
    };
    let Some(p) = state
        .sign_ins
        .as_ref()
        .and_then(|s| s.provider_of(&cards.provider))
    else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "POP signs in with no provider",
        );
    };
    let user = match crate::sign_in::member_account(&state, p, &w.member.id, &w.member.name).await {
        Ok(u) => u,
        Err(NoAccount::Deactivated) => {
            return error(StatusCode::FORBIDDEN, "this account is deactivated in POP");
        }
        Err(NoAccount::Failed) => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "sign-in is not working right now",
            );
        }
    };
    let via = format!("a {} card", p.label);
    let peer = connect.map(|Extension(c)| c.0.ip());
    let token = match crate::auth::begin(&state, peer, &headers, &user, Some(&via)).await {
        Ok(t) => t,
        Err(r) => return *r,
    };
    let mut res = Json(user).into_response();
    res.headers_mut().insert(
        header::SET_COOKIE,
        crate::auth::session_cookie(&token, crate::users::SESSION_SECS),
    );
    res
}
