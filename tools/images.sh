# shellcheck shell=sh
# The pinned images every script uses, by digest alone (reaper refuses a tag
# beside a digest; .reaper.toml lists the same digests so reaper pulls them
# before a run). Sourced, not run.

# rust:1.97-bookworm, the toolchain CS:S's battery already uses.
RUST_IMAGE="docker.io/library/rust@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97"
# node:22-alpine, as BAM! uses.
NODE_IMAGE="docker.io/library/node@sha256:2c752226d477b4a886378baa95b9af252be59301b725fdb0b7e15208131505a8"
# playwright:v1.64.0-noble, matching @playwright/test in web/package.json.
PLAYWRIGHT_IMAGE="mcr.microsoft.com/playwright@sha256:bc72a8df40831ded821e3cbc79c318c21b44d6e51dff9fd89ace7a8249c5b462"

CONTAINER="${CONTAINER:-podman}"
# Caches: named volumes by default, or directories (reaper points them at its
# own caches, off the guest's small root disk).
CARGO_CACHE="${POP_CARGO_HOME:-pop-cargo-home}"
TARGET_CACHE="${POP_TARGET:-pop-target}"
NPM_CACHE="${POP_NPM_CACHE:-pop-npm-cache}"
NODE_MODULES="${POP_NODE_MODULES:-pop-node-modules}"
export CONTAINER MSYS_NO_PATHCONV=1 RUST_IMAGE NODE_IMAGE PLAYWRIGHT_IMAGE \
  CARGO_CACHE TARGET_CACHE NPM_CACHE NODE_MODULES
