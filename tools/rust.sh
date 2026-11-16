#!/bin/sh
# The Rust workspace's checks, in the pinned toolchain: formatting, clippy
# with warnings as errors, and every test. Needs only podman (or docker, with
# CONTAINER=docker).
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
. "$ROOT/tools/images.sh"

# The toolchain image carries neither rustfmt nor clippy.
"$CONTAINER" run --rm \
  -v "$ROOT:/src" -w /src \
  -v "$CARGO_CACHE:/cargo-home" -e CARGO_HOME=/cargo-home \
  -v "$TARGET_CACHE:/target" -e CARGO_TARGET_DIR=/target \
  "$RUST_IMAGE" \
  sh -c 'set -e
    rustup component add rustfmt clippy >/dev/null
    echo "--- cargo fmt --check"
    cargo fmt --all --check
    echo "--- cargo clippy"
    cargo clippy --locked --workspace --all-targets -- -D warnings
    echo "--- cargo test"
    cargo test --locked --workspace'
