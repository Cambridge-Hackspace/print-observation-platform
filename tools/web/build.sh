#!/bin/sh
# Builds the web app into web/dist, where the server's release build embeds
# it; or, with `check`, type-checks it and runs its unit tests instead. Runs in
# the pinned Node image.
#
#   tools/web/build.sh          build the app
#   tools/web/build.sh check    svelte-check and Vitest
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tools/images.sh"

case "${1:-build}" in
  build) STEPS="npm run build" ;;
  check) STEPS="npm run check && npm test" ;;
  *)
    echo "usage: $0 [build|check]" >&2
    exit 2
    ;;
esac

# node_modules lives in a volume: it holds Linux binaries for this image,
# which must not land in the checkout.
"$CONTAINER" run --rm \
  -v "$ROOT:/src" -w /src/web \
  -v "$NODE_MODULES:/src/web/node_modules" \
  -v "$NPM_CACHE:/root/.npm" \
  -e PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
  "$NODE_IMAGE" \
  sh -c "npm ci --no-audit --no-fund && $STEPS"
