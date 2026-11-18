#!/bin/sh
# Checks tools/sign-release.sh (#148) with the TEST keys in
# crates/pop-release/tests/vectors: it signs, and it refuses each mistake it
# is there to catch. CI runs it in SIGN_IMAGE (OpenSSL 3.5); so can any
# machine with OpenSSL 3.5 or later.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
V="$HERE/../crates/pop-release/tests/vectors"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
fail() {
  echo "sign-release-check: $*" >&2
  exit 1
}

mkdir "$T/release" "$T/pubs" "$T/others" "$T/half"
cp "$V/SHA256SUMS" "$T/release/"
cp "$V/test-ml-dsa-65.pub" "$T/pubs/ml-dsa-65.pub"
cp "$V/test-ed25519.pub" "$T/pubs/ed25519.pub"
cp "$V/other-ml-dsa-65.pub" "$T/others/ml-dsa-65.pub"
cp "$V/test-ed25519.pub" "$T/others/ed25519.pub"
cp "$V/test-ml-dsa-65.pub" "$T/half/ml-dsa-65.pub"

# Signs, and both signatures check with OpenSSL.
"$HERE/sign-release.sh" "$T/release" "$V/test-ml-dsa-65.key" "$V/test-ed25519.key" "$T/pubs" ||
  fail "did not sign with the right keys"
[ -s "$T/release/SHA256SUMS.ml-dsa-65.sig" ] && [ -s "$T/release/SHA256SUMS.ed25519.sig" ] ||
  fail "wrote no signatures"

# Each mistake is refused, saying which (the message is the evidence).
refused() { # what it should say, then the command
  want=$1
  shift
  if out=$("$@" 2>&1); then
    fail "signed though $want"
  fi
  case $out in
    *"$want"*) echo "sign-release-check: refused: $want" ;;
    *) fail "refused, but not because $want: $out" ;;
  esac
}
refused "are not a pair" \
  "$HERE/sign-release.sh" "$T/release" "$V/test-ml-dsa-65.key" "$V/test-ed25519.key" "$T/others"
refused "is not an ML-DSA-65 key" \
  "$HERE/sign-release.sh" "$T/release" "$V/test-ed25519.key" "$V/test-ml-dsa-65.key" "$T/pubs"
refused "no ed25519 public key" \
  "$HERE/sign-release.sh" "$T/release" "$V/test-ml-dsa-65.key" "$V/test-ed25519.key" "$T/half"
: >"$T/release/SHA256SUMS"
refused "is empty" \
  "$HERE/sign-release.sh" "$T/release" "$V/test-ml-dsa-65.key" "$V/test-ed25519.key" "$T/pubs"
echo "sign-release-check: ok"
