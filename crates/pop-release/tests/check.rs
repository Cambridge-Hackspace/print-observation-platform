//! A release is good only when both signatures check, and it is the
//! version it was offered as (#148). The vectors are OpenSSL's signatures
//! (tests/vectors/README.md), checked by the Rust verifiers.

use pop_release::{Error, Keys, Version, check};

const SUMS: &[u8] = include_bytes!("vectors/SHA256SUMS");
const ML_DSA_SIG: &[u8] = include_bytes!("vectors/SHA256SUMS.ml-dsa-65.sig");
const ED25519_SIG: &[u8] = include_bytes!("vectors/SHA256SUMS.ed25519.sig");
const ML_DSA: &str = include_str!("vectors/test-ml-dsa-65.pub");
const ED25519: &str = include_str!("vectors/test-ed25519.pub");
const OTHER_ML_DSA: &str = include_str!("vectors/other-ml-dsa-65.pub");
const OTHER_ED25519: &str = include_str!("vectors/other-ed25519.pub");
const TAG: &str = "v9.8.7";

fn keys() -> Keys {
    Keys::from_pem(ML_DSA, ED25519).unwrap()
}

#[test]
fn a_release_both_signatures_vouch_for_checks_and_says_its_binaries() {
    let r = check(&keys(), TAG, SUMS, ML_DSA_SIG, ED25519_SIG).unwrap();
    assert_eq!(r.version, Version(9, 8, 7));
    let key = |t: &str, a: &str| (t.to_owned(), a.to_owned());
    assert_eq!(
        r.binaries.keys().cloned().collect::<Vec<_>>(),
        [
            key("pop", "aarch64"),
            key("pop", "x86_64"),
            key("pop-updater", "aarch64"),
            key("pop-updater", "x86_64"),
        ]
    );
    assert_eq!(
        r.binaries[&key("pop", "x86_64")].0,
        "pop-v9.8.7-linux-x86_64"
    );
    for (tool, arch) in r.binaries.keys() {
        let bytes = std::fs::read(format!(
            "{}/tests/vectors/{tool}-v9.8.7-linux-{arch}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        r.check_binary(tool, arch, &bytes).unwrap();
    }
    // Another architecture's binary, another program's, or a changed one,
    // is not it.
    assert_eq!(
        r.check_binary(
            "pop",
            "x86_64",
            include_bytes!("vectors/pop-v9.8.7-linux-aarch64")
        ),
        Err(Error::Digest("pop-v9.8.7-linux-x86_64".into()))
    );
    assert_eq!(
        r.check_binary(
            "pop",
            "x86_64",
            include_bytes!("vectors/pop-updater-v9.8.7-linux-x86_64")
        ),
        Err(Error::Digest("pop-v9.8.7-linux-x86_64".into()))
    );
    assert!(matches!(
        r.check_binary("pop", "riscv64", b"x86"),
        Err(Error::Sums(_))
    ));
    assert!(matches!(
        r.check_binary("go2rtc", "x86_64", b"x86"),
        Err(Error::Sums(_))
    ));
}

/// Each signature is needed: either one alone, with the other wrong, fails.
#[test]
fn either_signature_wrong_fails_the_release() {
    let mut bad_ml = ML_DSA_SIG.to_vec();
    bad_ml[100] ^= 1;
    assert_eq!(
        check(&keys(), TAG, SUMS, &bad_ml, ED25519_SIG),
        Err(Error::Signature("ML-DSA-65"))
    );
    let mut bad_ed = ED25519_SIG.to_vec();
    bad_ed[10] ^= 1;
    assert_eq!(
        check(&keys(), TAG, SUMS, ML_DSA_SIG, &bad_ed),
        Err(Error::Signature("Ed25519"))
    );
    // Missing, or each in the other's place.
    assert!(check(&keys(), TAG, SUMS, &[], ED25519_SIG).is_err());
    assert!(check(&keys(), TAG, SUMS, ML_DSA_SIG, &[]).is_err());
    assert!(check(&keys(), TAG, SUMS, ED25519_SIG, ML_DSA_SIG).is_err());
}

#[test]
fn a_release_signed_by_other_keys_fails() {
    let ml = Keys::from_pem(OTHER_ML_DSA, ED25519).unwrap();
    assert_eq!(
        check(&ml, TAG, SUMS, ML_DSA_SIG, ED25519_SIG),
        Err(Error::Signature("ML-DSA-65"))
    );
    let ed = Keys::from_pem(ML_DSA, OTHER_ED25519).unwrap();
    assert_eq!(
        check(&ed, TAG, SUMS, ML_DSA_SIG, ED25519_SIG),
        Err(Error::Signature("Ed25519"))
    );
}

#[test]
fn a_changed_sums_file_fails_whatever_is_changed() {
    let text = std::str::from_utf8(SUMS).unwrap();
    for changed in [
        text.replacen("ddf7", "ddf8", 1),
        text.replacen("x86_64", "x86_65", 1),
        format!("{text}{}  pop-v9.8.7-linux-riscv64\n", "0".repeat(64)),
        text.lines().next().unwrap().to_owned() + "\n",
    ] {
        assert_eq!(
            check(&keys(), TAG, changed.as_bytes(), ML_DSA_SIG, ED25519_SIG),
            Err(Error::Signature("ML-DSA-65")),
            "{changed}"
        );
    }
}

/// Signed as v9.8.7, it is not any other version: an old release cannot be
/// offered as a new one.
#[test]
fn a_release_is_only_the_version_it_signed() {
    for tag in ["v9.8.8", "v10.0.0", "v9.8.6"] {
        assert!(
            matches!(
                check(&keys(), tag, SUMS, ML_DSA_SIG, ED25519_SIG),
                Err(Error::Version(_))
            ),
            "{tag}"
        );
    }
    for tag in ["9.8.7", "v9.8", "v9.8.7.1", "v9.8.07", "latest", ""] {
        assert!(
            matches!(
                check(&keys(), tag, SUMS, ML_DSA_SIG, ED25519_SIG),
                Err(Error::Version(_))
            ),
            "{tag:?}"
        );
    }
}

#[test]
fn versions_order_as_numbers_and_only_parse_when_whole() {
    let v = |s| Version::parse(s).unwrap();
    assert!(v("v0.10.0") > v("v0.9.9"));
    assert!(v("v1.0.0") > v("v0.99.99"));
    assert!(v("v0.1.1") > v("v0.1.0"));
    assert_eq!(v("0.2.0"), Version(0, 2, 0));
    assert_eq!(Version(0, 2, 0).to_string(), "v0.2.0");
    for bad in [
        "v1.2",
        "v1.2.3.4",
        "v1..3",
        "v1.2.x",
        "v-1.2.3",
        "v1.2.3-rc.1",
        "v1234567890.0.0",
    ] {
        assert_eq!(Version::parse(bad), None, "{bad}");
    }
}

#[test]
fn a_key_that_is_not_one_is_said() {
    assert!(
        matches!(Keys::from_pem(ED25519, ED25519), Err(Error::Key(e)) if e.starts_with("ML-DSA-65"))
    );
    assert!(
        matches!(Keys::from_pem(ML_DSA, ML_DSA), Err(Error::Key(e)) if e.starts_with("Ed25519"))
    );
    assert!(matches!(Keys::from_pem("", ED25519), Err(Error::Key(_))));
}

/// Signed, and still not a release: the signature vouches for who made it,
/// not that it is right.
#[test]
fn a_signed_sums_file_that_is_not_a_releases_fails() {
    let twice = check(
        &keys(),
        TAG,
        include_bytes!("vectors/SHA256SUMS-twice"),
        include_bytes!("vectors/SHA256SUMS-twice.ml-dsa-65.sig"),
        include_bytes!("vectors/SHA256SUMS-twice.ed25519.sig"),
    );
    assert_eq!(
        twice,
        Err(Error::Sums("pop for aarch64 is named twice".into()))
    );
    let stranger = check(
        &keys(),
        TAG,
        include_bytes!("vectors/SHA256SUMS-stranger"),
        include_bytes!("vectors/SHA256SUMS-stranger.ml-dsa-65.sig"),
        include_bytes!("vectors/SHA256SUMS-stranger.ed25519.sig"),
    );
    assert_eq!(
        stranger,
        Err(Error::Sums(
            "stranger-v9.8.7-linux-x86_64 is not one of POP's programs".into()
        ))
    );
    // An empty SHA256SUMS is not here: OpenSSL will not sign nothing, and a
    // release naming no binaries has none for any machine (check_binary).
}
