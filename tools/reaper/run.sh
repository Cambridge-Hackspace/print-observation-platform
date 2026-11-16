#!/bin/sh
# The reaper battery: tools/check.sh with reaper's caches. reaper runs this on
# the guest itself (exec = "host" in .reaper.toml), because the battery starts
# containers of its own. Playwright's traces and screenshots are copied into
# REAPER_OUT, pass or fail, so a failure's evidence comes back rather than
# dying with the guest.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

POP_CARGO_HOME="$REAPER_CACHE_CARGO"
POP_TARGET="$REAPER_CACHE_TARGET"
POP_NPM_CACHE="$REAPER_CACHE_NPM"
POP_NODE_MODULES="$REAPER_CACHE_NODE_MODULES"
export POP_CARGO_HOME POP_TARGET POP_NPM_CACHE POP_NODE_MODULES

status=0
"$ROOT/tools/check.sh" || status=$?

if [ -d "$ROOT/web/test-results" ]; then
  mkdir -p "$REAPER_OUT/playwright"
  cp -R "$ROOT/web/test-results/." "$REAPER_OUT/playwright/"
fi
exit "$status"
