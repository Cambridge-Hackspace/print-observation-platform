#!/bin/sh
# Regenerates the web app's TypeScript types (web/src/lib/generated) from the
# server's Rust types (crates/pop-server/src/api.rs). Run it after changing
# one; crates/pop-server/tests/types.rs fails until you do.
set -eu
cd "$(dirname "$0")/.."
POP_WRITE_TYPES=1 cargo test -q -p pop-server --test types
