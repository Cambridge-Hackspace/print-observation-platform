#!/bin/sh
# Every suite, in order of cost: the Rust workspace, the web app's checks and
# unit tests, then the end-to-end battery against a real binary. This is the
# reaper battery (.reaper.toml) and CI (.github/workflows/ci.yml) alike, so
# the two cannot drift apart. Needs only podman (or docker, with
# CONTAINER=docker); docs/testing.md says what each suite catches.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

step() { printf '\n=== %s\n' "$*"; }

step "Rust: fmt, clippy, tests"
"$ROOT/tools/rust.sh"

step "web: svelte-check and Vitest"
"$ROOT/tools/web/build.sh" check

step "end to end: Playwright against a release binary"
"$ROOT/tools/web/e2e.sh"

step "all passed"
