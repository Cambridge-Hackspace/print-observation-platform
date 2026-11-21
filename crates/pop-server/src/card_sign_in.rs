//! A card tap at a kiosk's reader signs the member in there (#19), where
//! CS:S signs members in by card (Cambridge-Hackspace/cooperative-systems-spaces#241).
//!
//! 1. The tap goes from the reader to CS:S. Its answer, on the space's
//!    broker, names the resource the reader is; POP hears it
//!    (pop_integrations::card_taps) and finds the enrolled kiosk whose card
//!    reader that is (kiosks::with_card_reader). A tap at a reader no kiosk
//!    has is no sign-in anywhere.
//! 2. The tap waits for that kiosk, in memory, briefly: CS:S's code is good
//!    for a minute, its ticket for two. A newer tap there replaces it.
//! 3. The kiosk asks for it ([`waiting`], `GET /api/kiosk/tap`, which waits
//!    up to [`LONG_POLL`] for one) with its own enrolled credential (the
//!    routes' need, `Need::Kiosk`, crate::access), and
//!    never sees the code or the ticket: only what to show. It is never on
//!    the live stream, which is public.
//! 4. The kiosk takes it ([`take`], `POST /api/kiosk/tap`, with the TOTP
//!    code when CS:S asked for one): POP redeems it with CS:S with its own
//!    secret (sign_in::SignIns::redeem_card), and the account the ID token
//!    names is signed in as a browser sign-in's would be, a kiosk session.
//!    A wrong TOTP code may be tried again, as CS:S allows, three times.
//! 5. Or the kiosk sets it aside ([`dismiss`], `DELETE /api/kiosk/tap`).

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
use pop_integrations::card_taps::{LocalSignIn, Outcome, Tap};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use ts_rs::TS;

use crate::{
    AppState,
    access::KioskId,
    api::ApiError,
    sign_in::{CardFailed, CardGrant, NoAccount},
};

/// How long the kiosk's ask waits for a tap before answering that there is none.
pub const LONG_POLL: Duration = Duration::from_secs(25);
/// How long a code waits: CS:S's minute, less the time to redeem it.
const CODE_FOR: Duration = Duration::from_secs(50);
/// How long a ticket waits for its TOTP code: CS:S's two minutes, less the same.
const TICKET_FOR: Duration = Duration::from_secs(110);
/// How long a refusal stays up for the member to read.
const REFUSAL_FOR: Duration = Duration::from_secs(15);
/// A ticket's tries, as CS:S counts them.
const TRIES: u8 = 3;

/// What the kiosk shows for its tap, never with the code or ticket in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TapView {
    /// A sign-in, ready: take it at once.
    SignIn,
    /// CS:S asks this member for their authenticator code first.
    SecondFactor {
        /// What went wrong with the last code typed, for a try again.
        #[ts(optional)]
        #[serde(skip_serializing_if = "Option::is_none")]
        problem: Option<String>,
    },
    /// No sign-in for this tap, and why, for the member to read.
    Refused { reason: String },
}

/// `GET /api/kiosk/tap`'s answer: the kiosk's tap, or none yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct KioskTap {
    pub tap: Option<TapView>,
}

/// `POST /api/kiosk/tap`'s body.
#[derive(Debug, Clone, Default, Deserialize, TS)]
pub struct TakeTap {
    /// The member's TOTP code, when CS:S asked for one.
    #[serde(default)]
    #[ts(optional)]
    pub totp: Option<String>,
}

struct Waiting {
    outcome: Outcome,
    at: Instant,
    tries: u8,
    /// What the last wrong code was told.
    problem: Option<String>,
}

impl Waiting {
    fn fresh(&self) -> bool {
        let for_ = match &self.outcome {
            Outcome::SignIn(LocalSignIn::Code { .. }) => CODE_FOR,
            Outcome::SignIn(LocalSignIn::Totp { .. }) => TICKET_FOR,
            _ => REFUSAL_FOR,
        };
        self.at.elapsed() < for_
    }

    fn view(&self) -> TapView {
        match &self.outcome {
            Outcome::SignIn(LocalSignIn::Code { .. }) => TapView::SignIn,
            Outcome::SignIn(LocalSignIn::Totp { .. }) => TapView::SecondFactor {
                problem: self.problem.clone(),
            },
            Outcome::SignIn(LocalSignIn::Refused { reason }) | Outcome::NotSignedIn(reason) => {
                TapView::Refused {
                    reason: reason.clone(),
                }
            }
        }
    }
}

/// The taps waiting for each kiosk, by its id.
#[derive(Clone)]
pub struct CardSignIns {
    /// The `[[sign_in.providers]]` taps are redeemed with.
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

    /// A tap CS:S answered (step 1), kept for the kiosk whose reader it was.
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
                outcome: tap.outcome,
                at: Instant::now(),
                tries: 0,
                problem: None,
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

/// Whether POP hears the space's broker, on `/healthz` (#19).
pub struct BrokerHealth(pub pop_integrations::card_taps::Hearing);

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
    body: Result<Json<TakeTap>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some(cards) = state.card_sign_ins.as_ref() else {
        return no_cards();
    };
    // No body, or none POP can read, carries no TOTP code: a ticket then
    // says so in words below, and a code needs none.
    let totp = body
        .ok()
        .and_then(|Json(b)| b.totp)
        .map(|t| t.trim().to_owned());
    // Taken out while it is redeemed, so two screens' worth of taps cannot
    // spend one code twice; a ticket's wrong code puts it back.
    let w = {
        let mut all = cards.waiting.lock().unwrap();
        all.retain(|_, w| w.fresh());
        all.remove(&kiosk)
    };
    let Some(mut w) = w else {
        return error(
            StatusCode::NOT_FOUND,
            "there is no card tap to sign in with: tap again",
        );
    };
    let grant = match (&w.outcome, totp) {
        (Outcome::SignIn(LocalSignIn::Code { code }), _) => CardGrant::Code(code.clone()),
        (Outcome::SignIn(LocalSignIn::Totp { ticket }), Some(t))
            if t.len() == 6 && t.bytes().all(|b| b.is_ascii_digit()) =>
        {
            CardGrant::Ticket {
                ticket: ticket.clone(),
                totp: t,
            }
        }
        (Outcome::SignIn(LocalSignIn::Totp { .. }), _) => {
            cards.waiting.lock().unwrap().insert(kiosk, w);
            return error(
                StatusCode::BAD_REQUEST,
                "totp: the six digits your authenticator app shows",
            );
        }
        _ => return error(StatusCode::CONFLICT, "this tap signs nobody in"),
    };
    let Some(sign_ins) = state.sign_ins.as_ref() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "POP signs in with no provider",
        );
    };
    let ticket = matches!(grant, CardGrant::Ticket { .. });
    let signed = match sign_ins.redeem_card(&cards.provider, grant).await {
        Ok(s) => s,
        Err(CardFailed::Refused(why)) => {
            w.tries += 1;
            if ticket && w.tries < TRIES && w.fresh() {
                w.problem = Some(why.clone());
                cards.waiting.lock().unwrap().insert(kiosk, w);
                cards.changed.send_modify(|n| *n += 1);
            }
            return error(StatusCode::BAD_REQUEST, why);
        }
        Err(CardFailed::Broken(why)) => {
            tracing::warn!("card sign-in: {why}");
            return error(
                StatusCode::BAD_GATEWAY,
                "POP could not finish signing you in with your card: sign in another way",
            );
        }
    };
    let Some(p) = sign_ins.provider_of(&cards.provider) else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "POP signs in with no provider",
        );
    };
    let user = match crate::sign_in::account(&state, p, &signed).await {
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
