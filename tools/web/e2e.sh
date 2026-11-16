#!/bin/sh
# The end-to-end battery (web/e2e): builds the app, builds a release `pop`
# with it embedded, starts that on a fresh database, and drives it with
# Playwright in the pinned Playwright image.
#
#   tools/web/e2e.sh            every spec
#   tools/web/e2e.sh smoke      the specs whose names match
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tools/images.sh"

"$ROOT/tools/web/build.sh"

mkdir -p "$ROOT/web/.e2e"
"$CONTAINER" run --rm \
  -v "$ROOT:/src" -w /src \
  -v "$CARGO_CACHE:/cargo-home" -e CARGO_HOME=/cargo-home \
  -v "$TARGET_CACHE:/target" -e CARGO_TARGET_DIR=/target \
  "$RUST_IMAGE" \
  sh -c 'set -e
    cargo build --locked --release -p pop-server
    cp /target/release/pop web/.e2e/pop'

# The script below expands its variables inside the container.
# shellcheck disable=SC2016
"$CONTAINER" run --rm \
  -v "$ROOT:/src" -w /src/web \
  -v "$NODE_MODULES:/src/web/node_modules" \
  -v "$NPM_CACHE:/root/.npm" \
  -e PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
  "$PLAYWRIGHT_IMAGE" \
  sh -c '
    set -e
    npm ci --no-audit --no-fund >/dev/null
    state=$(mktemp -d)
    POP_DATA_DIR="$state" POP_LISTEN=127.0.0.1:8080 .e2e/pop >"$state/server.log" 2>&1 &
    up=0
    for _ in $(seq 1 50); do
      if wget -q -O /dev/null http://127.0.0.1:8080/healthz 2>/dev/null; then up=1; break; fi
      sleep 0.2
    done
    if [ "$up" != 1 ]; then echo "pop did not come up:"; cat "$state/server.log"; exit 1; fi
    status=0
    POP_URL=http://127.0.0.1:8080 npx playwright test "$@" || status=$?
    if [ "$status" -ne 0 ]; then echo "--- server log ---"; tail -50 "$state/server.log"; fi
    exit "$status"
  ' sh "$@"
