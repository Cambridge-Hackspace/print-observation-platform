#!/bin/sh
# Tests the packaging (#9) as an installer would meet it: the container image
# builds and serves, as a user that is not root; and, with `systemd`, the
# service units install, start, keep POP's data to POP, and run go2rtc with
# its password asked of every request.
#
#   tools/package-test.sh image        build the image, run it, ask /healthz
#   tools/package-test.sh systemd      install the units on this machine (root)
#
# Run by ci.yml's package job, on runners of both architectures; the systemd
# half needs a machine it may install services on, which CI's runners are.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
. "$ROOT/tools/images.sh"
IMAGE="${POP_TEST_IMAGE:-pop:package-test}"

fail() {
  echo "package-test: $*" >&2
  exit 1
}

# Wait for `url` to answer 200, or fail saying what it last said.
healthy() {
  url="$1"
  for _ in $(seq 1 100); do
    code=$(curl -s -o /tmp/pop-health.json -w '%{http_code}' "$url" || true)
    [ "$code" = 200 ] && return 0
    sleep 0.2
  done
  fail "$url never answered 200 (last: $code $(cat /tmp/pop-health.json 2>/dev/null))"
}

image() {
  "$CONTAINER" build -f "$ROOT/packaging/Containerfile" \
    --build-arg RUST_IMAGE="$RUST_IMAGE" --build-arg NODE_IMAGE="$NODE_IMAGE" \
    -t "$IMAGE" "$ROOT"
  "$CONTAINER" rm -f pop-package-test >/dev/null 2>&1 || true
  "$CONTAINER" run -d --name pop-package-test -p 127.0.0.1:18080:8080 "$IMAGE" >/dev/null
  trap '"$CONTAINER" logs pop-package-test >&2 || true; "$CONTAINER" rm -f pop-package-test >/dev/null 2>&1 || true' EXIT
  healthy http://127.0.0.1:18080/healthz
  # The web app is compiled in: the binary serves it, with nothing beside it.
  curl -sf http://127.0.0.1:18080/ | grep -q '<div id="app"' || fail "the web app is not served"
  uid=$("$CONTAINER" exec pop-package-test id -u)
  [ "$uid" != 0 ] || fail "POP runs as root in the image"
  mode=$("$CONTAINER" exec pop-package-test stat -c %a /var/lib/pop)
  [ "$mode" = 700 ] || fail "/var/lib/pop is $mode, not 700"
  echo "package-test: the image builds, serves, and runs as uid $uid"
}

systemd() {
  [ "$(id -u)" = 0 ] || fail "the systemd test installs services: run it as root"
  command -v systemctl >/dev/null || fail "no systemd here"
  # The binary, from the image the release ships it from.
  id=$("$CONTAINER" create "$IMAGE")
  "$CONTAINER" cp "$id:/usr/local/bin/pop" /usr/local/bin/pop
  "$CONTAINER" rm "$id" >/dev/null
  id -u pop >/dev/null 2>&1 || useradd --system --home-dir /var/lib/pop --shell /usr/sbin/nologin pop
  install -d -m 0755 /etc/pop
  printf '[server]\nlisten = "127.0.0.1:18081"\n' >/etc/pop/pop.toml
  install -m 0644 "$ROOT/packaging/systemd/pop.service" /etc/systemd/system/pop.service
  systemd-analyze verify /etc/systemd/system/pop.service
  systemctl daemon-reload
  systemctl start pop
  trap 'journalctl -u pop -u go2rtc --no-pager -n 80 >&2 || true' EXIT
  healthy http://127.0.0.1:18081/healthz
  # What POP writes, only POP can read (UMask=0077; threat model, gap 9).
  [ "$(stat -c %a /var/lib/pop)" = 700 ] || fail "/var/lib/pop is $(stat -c %a /var/lib/pop), not 700"
  db=$(find /var/lib/pop -name 'pop.db' | head -1)
  [ -n "$db" ] || fail "no database in /var/lib/pop"
  [ "$(stat -c %a "$db")" = 600 ] || fail "$db is $(stat -c %a "$db"), not 600"
  [ "$(stat -c %U "$db")" = pop ] || fail "$db is $(stat -c %U "$db")'s, not pop's"
  echo "package-test: pop.service runs, and keeps its data to itself"

  # go2rtc, as its own service, with its password asked of every request.
  version=v1.9.15
  case "$(uname -m)" in
    x86_64) asset=go2rtc_linux_amd64 ;;
    aarch64) asset=go2rtc_linux_arm64 ;;
    *) fail "no go2rtc build named for $(uname -m)" ;;
  esac
  curl -sfL -o /usr/local/bin/go2rtc "https://github.com/AlexxIT/go2rtc/releases/download/$version/$asset"
  want=$(curl -sf "https://api.github.com/repos/AlexxIT/go2rtc/releases/tags/$version" |
    python3 -c "import json,sys; print([a['digest'] for a in json.load(sys.stdin)['assets'] if a['name']=='$asset'][0].split(':')[1])")
  got=$(sha256sum /usr/local/bin/go2rtc | cut -d' ' -f1)
  [ "$got" = "$want" ] || fail "go2rtc $version's $asset is not the one released ($got, not $want)"
  chmod 0755 /usr/local/bin/go2rtc
  id -u go2rtc >/dev/null 2>&1 || useradd --system --home-dir /var/lib/go2rtc --shell /usr/sbin/nologin go2rtc
  install -d -o go2rtc -g go2rtc -m 0700 /var/lib/go2rtc
  sed 's/CHANGE-ME/package-test-password/' "$ROOT/packaging/go2rtc.yaml" >/var/lib/go2rtc/go2rtc.yaml
  chown go2rtc:go2rtc /var/lib/go2rtc/go2rtc.yaml
  chmod 0600 /var/lib/go2rtc/go2rtc.yaml
  install -m 0644 "$ROOT/packaging/systemd/go2rtc.service" /etc/systemd/system/go2rtc.service
  systemd-analyze verify /etc/systemd/system/go2rtc.service
  systemctl daemon-reload
  systemctl start go2rtc
  for _ in $(seq 1 50); do
    curl -s -o /dev/null -u pop:package-test-password http://127.0.0.1:1984/api && break
    sleep 0.2
  done
  code=$(curl -s -o /dev/null -w '%{http_code}' -u pop:package-test-password http://127.0.0.1:1984/api)
  [ "$code" = 200 ] || fail "go2rtc does not answer with its password ($code)"
  code=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:1984/api)
  [ "$code" = 401 ] || fail "go2rtc answers its own machine without its password ($code): local_auth"
  # go2rtc can write its configuration, as POP's streams need (#66).
  sudo -u go2rtc test -w /var/lib/go2rtc/go2rtc.yaml || fail "go2rtc cannot write its configuration"
  echo "package-test: go2rtc.service runs, asks for its password, and can keep POP's streams"
}

case "${1:-}" in
  image) image ;;
  systemd) systemd ;;
  *)
    echo "usage: $0 image|systemd" >&2
    exit 2
    ;;
esac
