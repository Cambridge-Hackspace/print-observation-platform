//! Checking a POP release before anything runs it (#148).
//!
//! A release is its binaries, `SHA256SUMS` naming each with its SHA-256, and
//! two signatures of `SHA256SUMS`: one ML-DSA-65 (FIPS 204), one Ed25519,
//! each made by OpenSSL in the release workflow with a key only that
//! workflow holds (docs/releases.md). A release is good only when both
//! signatures check: ML-DSA keeps it unforgeable against a quantum
//! computer, and Ed25519 means a fault in one young implementation alone
//! cannot pass a forged release.
//!
//! The file names in `SHA256SUMS` carry the version, `pop-v0.2.0-linux-
//! x86_64`, so the signature covers it: an old release, still signed,
//! cannot be offered as a new one.

use std::{collections::BTreeMap, fmt};

use ml_dsa::{MlDsa65, VerifyingKey};
use pkcs8::DecodePublicKey;
use sha2::{Digest, Sha256};

/// What is wrong with a release, in words an admin can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A release key would not read.
    Key(String),
    /// A signature does not check, naming which.
    Signature(&'static str),
    /// SHA256SUMS is not what a release's is.
    Sums(String),
    /// The release is not the version it was offered as.
    Version(String),
    /// A file is not the one SHA256SUMS names.
    Digest(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(e) => write!(f, "a release key will not read: {e}"),
            Self::Signature(which) => write!(f, "the release's {which} signature does not check"),
            Self::Sums(e) => write!(f, "the release's SHA256SUMS is wrong: {e}"),
            Self::Version(e) => write!(f, "{e}"),
            Self::Digest(name) => write!(f, "{name} is not the file the release signed"),
        }
    }
}

impl std::error::Error for Error {}

/// The two public keys a release is checked with.
pub struct Keys {
    ml_dsa: VerifyingKey<MlDsa65>,
    ed25519: ed25519_dalek::VerifyingKey,
}

impl Keys {
    /// From the PEM public keys OpenSSL writes (`openssl pkey -pubout`).
    pub fn from_pem(ml_dsa: &str, ed25519: &str) -> Result<Self, Error> {
        Ok(Self {
            ml_dsa: VerifyingKey::<MlDsa65>::from_public_key_pem(ml_dsa)
                .map_err(|e| Error::Key(format!("ML-DSA-65: {e}")))?,
            ed25519: ed25519_dalek::VerifyingKey::from_public_key_pem(ed25519)
                .map_err(|e| Error::Key(format!("Ed25519: {e}")))?,
        })
    }
}

/// A version, `v1.2.3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// `v1.2.3`, or `1.2.3`; nothing else.
    pub fn parse(s: &str) -> Option<Self> {
        let mut n = s.strip_prefix('v').unwrap_or(s).split('.');
        let mut next = || {
            n.next()
                .filter(|p| !p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|p| p.parse().ok())
        };
        let v = Self(next()?, next()?, next()?);
        n.next().is_none().then_some(v)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}.{}", self.0, self.1, self.2)
    }
}

/// A release whose signatures checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// Each binary, by architecture as `uname -m` says it, with its file name
    /// and SHA-256.
    pub binaries: BTreeMap<String, (String, [u8; 32])>,
}

impl Release {
    /// Whether `bytes` are the binary for `arch` this release signed.
    pub fn check_binary(&self, arch: &str, bytes: &[u8]) -> Result<(), Error> {
        let (name, want) = self
            .binaries
            .get(arch)
            .ok_or_else(|| Error::Sums(format!("it has no binary for {arch}")))?;
        let got: [u8; 32] = Sha256::digest(bytes).into();
        if &got == want {
            Ok(())
        } else {
            Err(Error::Digest(name.clone()))
        }
    }
}

/// Check the release offered as `tag`: both signatures of `sums`, then that
/// `sums` names that version's binaries and nothing else.
pub fn check(
    keys: &Keys,
    tag: &str,
    sums: &[u8],
    ml_dsa_sig: &[u8],
    ed25519_sig: &[u8],
) -> Result<Release, Error> {
    // Signatures first: nothing below reads what no key vouched for.
    let sig = ml_dsa::Signature::<MlDsa65>::try_from(ml_dsa_sig)
        .map_err(|_| Error::Signature("ML-DSA-65"))?;
    // OpenSSL's pure ML-DSA, with an empty context.
    if !keys.ml_dsa.verify_with_context(sums, &[], &sig) {
        return Err(Error::Signature("ML-DSA-65"));
    }
    let sig = ed25519_dalek::Signature::from_slice(ed25519_sig)
        .map_err(|_| Error::Signature("Ed25519"))?;
    keys.ed25519
        .verify_strict(sums, &sig)
        .map_err(|_| Error::Signature("Ed25519"))?;

    let version = Version::parse(tag)
        .filter(|v| v.to_string() == tag)
        .ok_or_else(|| Error::Version(format!("{tag:?} is not a release version like v1.2.3")))?;
    let text = std::str::from_utf8(sums).map_err(|_| Error::Sums("not text".into()))?;
    let prefix = format!("pop-{version}-linux-");
    let mut binaries = BTreeMap::new();
    for line in text.lines() {
        let (hex, name) = line
            .split_once("  ")
            .ok_or_else(|| Error::Sums(format!("{line:?} is not \"<sha256>  <file>\"")))?;
        let digest =
            decode_hex(hex).ok_or_else(|| Error::Sums(format!("{hex:?} is not a SHA-256")))?;
        let arch = name.strip_prefix(&prefix).ok_or_else(|| {
            Error::Version(format!("this release is not {version}: it signed {name}"))
        })?;
        if arch.is_empty() || !arch.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(Error::Sums(format!("{name} names no architecture")));
        }
        if binaries
            .insert(arch.to_owned(), (name.to_owned(), digest))
            .is_some()
        {
            return Err(Error::Sums(format!("{arch} is named twice")));
        }
    }
    Ok(Release { version, binaries })
}

fn decode_hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    s.bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        .then_some(out)
}
