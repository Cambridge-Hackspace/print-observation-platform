//! How POP signs in to a printer that asks (#29, #30): kept sealed in POP,
//! entered on the admin desk, never in BAM!.

use std::fmt;

/// A printer's login, as its host asks for it.
#[derive(Clone, PartialEq, Eq)]
pub enum Login {
    /// HTTP Digest: PrusaLink on the printer itself (MK4, XL, MINI+), with
    /// the user and password the printer shows in its network settings.
    Digest { username: String, password: String },
    /// An API key in `X-Api-Key`: OctoPrint, and PrusaLink on a Raspberry Pi.
    ApiKey(String),
}

impl fmt::Debug for Login {
    // Never the secret itself, in a log or a panic.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Digest { username, .. } => write!(f, "Digest({username}, ****)"),
            Self::ApiKey(_) => f.write_str("ApiKey(****)"),
        }
    }
}
