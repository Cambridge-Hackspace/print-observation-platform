# Releases

A release is made by pushing a tag like `v0.2.0` on `main`
(`.github/workflows/release.yml`). It holds:

- `pop-<version>-linux-x86_64` and `pop-<version>-linux-aarch64`, POP;
- `pop-updater-<version>-linux-x86_64` and `-aarch64`, its updater
  (docs/upgrades.md);
- `SHA256SUMS`, naming each binary with its SHA-256;
- `SHA256SUMS.ml-dsa-65.sig` and `SHA256SUMS.ed25519.sig`, two signatures of
  `SHA256SUMS` (#148);
- the container image `ghcr.io/cambridge-hackspace/print-observation-platform:<version>`.

## The signatures

`SHA256SUMS` is signed twice, and a release is good only when **both**
signatures check:

- **ML-DSA-65** (FIPS 204), a post-quantum signature: a release stays
  unforgeable to someone with a quantum computer.
- **Ed25519**, so a fault in one implementation alone cannot pass a forged
  release. ML-DSA's Rust implementation is young; Ed25519's is not.

What it costs: 3.3 KB of signature per release, and milliseconds to check.

The file names in `SHA256SUMS` carry the version, so the signatures cover
it: an older release, still signed, cannot be passed off as a newer one.

The workflow signs with OpenSSL 3.5 in a pinned Debian image
(`tools/sign-release.sh`), and checks both signatures against the public
keys committed in `packaging/release-keys` before it publishes anything.
POP checks them with the Rust `ml-dsa` and `ed25519-dalek` crates
(`crates/pop-release`): two implementations, and the tests check one's
signatures with the other.

## Checking a release by hand

With OpenSSL 3.5 or later (`openssl version`), from a checkout of POP:

```sh
for k in ml-dsa-65 ed25519; do
  openssl pkeyutl -verify -rawin -pubin -inkey packaging/release-keys/$k.pub \
    -in SHA256SUMS -sigfile SHA256SUMS.$k.sig
done
sha256sum --check --ignore-missing SHA256SUMS
```

Each `pkeyutl` line must say `Signature Verified Successfully`.

## Making the release keys (once)

Someone with admin rights on the repository does this once, on a machine
they trust, with OpenSSL 3.5 or later. Debian 13 has it; so does a
container: `docker run --rm -it -v "$PWD:/k" -w /k debian:trixie-slim sh -c
'apt-get update && apt-get install -y openssl && sh'`.

1. Make the two keys, readable only by you:

   ```sh
   umask 077
   openssl genpkey -algorithm ML-DSA-65 -out pop-release-ml-dsa-65.key
   openssl genpkey -algorithm ED25519 -out pop-release-ed25519.key
   openssl pkey -in pop-release-ml-dsa-65.key -pubout -out ml-dsa-65.pub
   openssl pkey -in pop-release-ed25519.key -pubout -out ed25519.pub
   ```

2. Give the private halves to the release workflow: the repository's
   **Settings → Secrets and variables → Actions → New repository secret**,
   twice:
   - `POP_RELEASE_ML_DSA_KEY`: the whole of `pop-release-ml-dsa-65.key`,
     `-----BEGIN PRIVATE KEY-----` to `-----END PRIVATE KEY-----`;
   - `POP_RELEASE_ED25519_KEY`: the whole of `pop-release-ed25519.key`.

3. Commit the public halves, `ml-dsa-65.pub` and `ed25519.pub`, to
   `packaging/release-keys/`. They are not secret.

4. Keep the private halves offline (an encrypted USB stick, a password
   manager), or delete them: the secrets are what signs. Deleting them
   means a lost secret is replaced with new keys (below), not restored.

Never paste a private half into an issue, a chat or a commit.

## Replacing the keys

If a private half may have leaked, or is lost: make new keys as above,
replace both secrets, and commit the new public halves. Releases signed
before stay checkable with the old public halves from git history, and
POP installs that check releases take the new public halves with the next
release they install by hand.
