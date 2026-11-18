# Test vectors: TEST KEYS ONLY

Made with OpenSSL 3.5 the way the release workflow signs (docs/releases.md),
so the checks in ../check.rs are of OpenSSL's signatures against the Rust
verifiers: two implementations, not one agreeing with itself.

The `test-*` and `other-*` keys sign nothing but these files. They are not
POP's release keys, which live only in the repository's secrets.

    openssl genpkey -algorithm ML-DSA-65 -out test-ml-dsa-65.key
    openssl pkey -in test-ml-dsa-65.key -pubout -out test-ml-dsa-65.pub

and the same with `ED25519` (and `other-*`, which sign nothing). The
signatures of `SHA256SUMS` are the release workflow's own script's:

    tools/sign-release.sh crates/pop-release/tests/vectors \
      crates/pop-release/tests/vectors/test-ml-dsa-65.key \
      crates/pop-release/tests/vectors/test-ed25519.key <dir holding the test-*.pub as ml-dsa-65.pub and ed25519.pub>

`SHA256SUMS-twice` (a line named twice) is signed with plain
`openssl pkeyutl -sign -rawin`, since the script refuses nothing about
its content.
