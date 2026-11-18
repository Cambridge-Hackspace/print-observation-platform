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
    cp /target/release/pop web/.e2e/pop
    # Built on its own: in one build its fakes features would unify into
    # pop-server, and the pop under test would not be the one that ships.
    cargo build --locked --release -p pop-fakes
    cp /target/release/pop-fakes web/.e2e/pop-fakes'

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
    # The binary runs from an empty directory, a copy with nothing beside
    # it: whatever web app it serves is the one compiled into it (#4).
    state=$(mktemp -d)
    cp .e2e/pop "$state/pop"
    # An admin, for the tests that need content an admin writes (a published
    # guide, #72). Made the way a deployment makes its first one (README).
    export POP_E2E_ADMIN_PASSWORD="an e2e admin password"
    (cd "$state" && printf "%s\n" "$POP_E2E_ADMIN_PASSWORD" | POP_STORAGE_DATA_DIR="$state/data" ./pop user create-admin e2e --name "E2E Admin")
    (cd "$state" && POP_STORAGE_DATA_DIR="$state/data" POP_SERVER_LISTEN=127.0.0.1:8080 ./pop >"$state/server.log" 2>&1) &
    up=0
    for _ in $(seq 1 50); do
      if wget -q -O /dev/null http://127.0.0.1:8080/healthz 2>/dev/null; then up=1; break; fi
      sleep 0.2
    done
    if [ "$up" != 1 ]; then echo "pop did not come up:"; cat "$state/server.log"; exit 1; fi

    # A second POP, with printers, for the kiosk (#59): pointed at a fake
    # BAM! and fake Moonraker printers (crates/pop-fakes). The first keeps no
    # BAM!, as a new POP has.
    fleet=$(mktemp -d)
    cp .e2e/pop "$fleet/pop"
    cp .e2e/pop-fakes "$fleet/pop-fakes"
    (cd "$fleet" && POP_FAKES_LISTEN=127.0.0.1:8099 POP_FAKES_PRINTERS=11 POP_FAKES_GITHUB_KEY="$fleet/github-app.pem" ./pop-fakes >"$fleet/fakes.out" 2>"$fleet/fakes.log") &
    ready=""
    for _ in $(seq 1 50); do
      ready=$(grep -m1 "\"ready\":true" "$fleet/fakes.out" 2>/dev/null || true)
      if [ -n "$ready" ]; then break; fi
      sleep 0.2
    done
    if [ -z "$ready" ]; then echo "the fakes did not come up:"; cat "$fleet/fakes.log"; exit 1; fi
    field() { printf "%s" "$ready" | sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p"; }
    (cd "$fleet" && printf "%s\n" "$POP_E2E_ADMIN_PASSWORD" | POP_STORAGE_DATA_DIR="$fleet/data" ./pop user create-admin e2e --name "E2E Admin")
    (cd "$fleet" && POP_STORAGE_DATA_DIR="$fleet/data" POP_SERVER_LISTEN=127.0.0.1:8081 \
      POP_BAM_URL="$(field bam)" POP_BAM_TOKEN="$(field token)" POP_BAM_CATEGORY="$(field category)" \
      POP_FEEDBACK_APP_ID=12345 POP_FEEDBACK_INSTALLATION_ID=678 POP_FEEDBACK_REPO=acme/pop \
      POP_FEEDBACK_API="$(field github)" POP_FEEDBACK_PRIVATE_KEY_FILE="$fleet/github-app.pem" \
      ./pop >"$fleet/server.log" 2>&1) &
    up=0
    for _ in $(seq 1 50); do
      if wget -q -O /dev/null http://127.0.0.1:8081/healthz 2>/dev/null; then up=1; break; fi
      sleep 0.2
    done
    if [ "$up" != 1 ]; then echo "the pop with printers did not come up:"; cat "$fleet/server.log"; exit 1; fi

    status=0
    POP_URL=http://127.0.0.1:8080 POP_FLEET_URL=http://127.0.0.1:8081 POP_FAKES_URL=http://127.0.0.1:8099 \
      npx playwright test "$@" || status=$?
    if [ "$status" -ne 0 ]; then
      echo "--- server log ---"; tail -50 "$state/server.log"
      echo "--- server with printers log ---"; tail -50 "$fleet/server.log"
      echo "--- fakes log ---"; tail -20 "$fleet/fakes.log"
    fi
    exit "$status"
  ' sh "$@"
