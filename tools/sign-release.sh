#!/bin/sh
# Signs a release's SHA256SUMS with both release keys (#148,
# docs/releases.md), then checks each signature against the public key
# committed in packaging/release-keys, so a release is never published
# with signatures POP will not accept.
#
#   tools/sign-release.sh <dir holding SHA256SUMS> <ML-DSA-65 key> <Ed25519 key> [public key dir]
#
# Needs OpenSSL 3.5 or later (ML-DSA): CI runs it in SIGN_IMAGE
# (tools/images.sh). Writes SHA256SUMS.ml-dsa-65.sig and
# SHA256SUMS.ed25519.sig beside SHA256SUMS.
set -eu
fail() {
  echo "sign-release: $*" >&2
  exit 1
}
[ $# -ge 3 ] || fail "usage: $0 <dir> <ML-DSA-65 key> <Ed25519 key> [public key dir]"
dir=$1
pubs=${4:-$(dirname "$0")/../packaging/release-keys}
[ -f "$dir/SHA256SUMS" ] || fail "no SHA256SUMS in $dir"
[ -s "$dir/SHA256SUMS" ] || fail "SHA256SUMS in $dir is empty"

sign() { # name, OpenSSL's name for the algorithm, private key
  [ -f "$3" ] || fail "no $1 key at $3"
  [ -f "$pubs/$1.pub" ] || fail "no $1 public key at $pubs/$1.pub: commit it first (docs/releases.md)"
  # The key is the kind it says: two secrets swapped fail here, by name.
  openssl pkey -in "$3" -noout -text 2>/dev/null | head -1 | grep -q "$2" ||
    fail "the $1 key is not an $2 key"
  openssl pkeyutl -sign -rawin -inkey "$3" -in "$dir/SHA256SUMS" -out "$dir/SHA256SUMS.$1.sig" ||
    fail "OpenSSL could not sign with the $1 key (OpenSSL 3.5 or later is needed)"
  openssl pkeyutl -verify -rawin -pubin -inkey "$pubs/$1.pub" -in "$dir/SHA256SUMS" \
    -sigfile "$dir/SHA256SUMS.$1.sig" >/dev/null 2>&1 ||
    fail "the $1 signature does not check against $pubs/$1.pub: the secret and the committed public key are not a pair"
}
sign ml-dsa-65 ML-DSA-65 "$2"
sign ed25519 ED25519 "$3"
echo "sign-release: SHA256SUMS signed with ML-DSA-65 and Ed25519, and both check"
