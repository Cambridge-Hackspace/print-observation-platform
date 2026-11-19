#!/bin/sh
# Tests the packaging (#9) as an installer would meet it: the container image
# builds and serves, as a user that is not root; and, with `systemd`, the
# service units install, start, keep POP's data to POP, and run go2rtc with
# its password asked of every request.
#
#   tools/package-test.sh image        build the image, run it, ask /healthz
#   tools/package-test.sh systemd      install the units on this machine (root)
#   tools/package-test.sh kiosk        the kiosk's packages and unit, in Debian
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
  "$CONTAINER" cp "$id:/usr/local/bin/pop-updater" /usr/local/bin/pop-updater
  "$CONTAINER" rm "$id" >/dev/null
  id -u pop >/dev/null 2>&1 || useradd --system --home-dir /var/lib/pop --shell /usr/sbin/nologin pop
  install -d -m 0755 /etc/pop
  printf '[server]\nlisten = "127.0.0.1:18081"\n' >/etc/pop/pop.toml
  install -m 0644 "$ROOT/packaging/systemd/pop.service" /etc/systemd/system/pop.service
  systemd-analyze verify /etc/systemd/system/pop.service
  systemctl daemon-reload
  systemctl start pop
  trap 'journalctl -u pop -u go2rtc -u pop-backup --no-pager -n 80 >&2 || true' EXIT
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
  # The SHA-256 of each, as go2rtc's release publishes it: pinned here, so
  # the check needs no lookup (an unauthenticated one is rate limited).
  case "$(uname -m)" in
    x86_64)
      asset=go2rtc_linux_amd64
      want=20b46c6910734cb05ecefb234066f36868ff19b42ba1418a7a32e13a3e035712
      ;;
    aarch64)
      asset=go2rtc_linux_arm64
      want=af416f22b21a24ab83b8d78642190b4932ae695b4e386853f11d1b0319781763
      ;;
    *) fail "no go2rtc build named for $(uname -m)" ;;
  esac
  curl -sfL -o /usr/local/bin/go2rtc "https://github.com/AlexxIT/go2rtc/releases/download/$version/$asset"
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

  # The updater (#148): its units are accepted, and off, a run does nothing.
  install -m 0644 "$ROOT/packaging/systemd/pop-updater.service" /etc/systemd/system/
  install -m 0644 "$ROOT/packaging/systemd/pop-updater.timer" /etc/systemd/system/
  systemd-analyze verify /etc/systemd/system/pop-updater.service /etc/systemd/system/pop-updater.timer
  systemctl daemon-reload
  systemctl start pop-updater.service || fail "pop-updater, off, failed"
  journalctl -u pop-updater --no-pager | grep -q "automatic upgrades are off" ||
    fail "pop-updater, off, did not say so"
  [ ! -e /var/lib/pop/updater/events.jsonl ] || fail "pop-updater, off, recorded something"
  # On, at any hour, against a feed with no release: from inside its
  # sandbox it reads the running POP's version through the real binary,
  # asks the feed, and writes its record where POP reads it. (Applying a
  # release is crates/pop-updater's tests', against fakes.)
  feed=$(mktemp -d)
  (cd "$feed" && python3 -m http.server 18099 --bind 127.0.0.1 >/dev/null 2>&1) &
  feed_pid=$!
  cat >/etc/pop/updater.toml <<EOF
enabled = true
hours = "00:00-23:59"
api = "http://127.0.0.1:18099"
repo = "o/r"
pop = "http://127.0.0.1:18081"
EOF
  sleep 1
  systemctl start pop-updater.service || fail "pop-updater, on, failed"
  kill "$feed_pid"
  grep -q '"kind":"checked".*No release yet: v' /var/lib/pop/updater/events.jsonl ||
    fail "pop-updater did not record its check: $(cat /var/lib/pop/updater/events.jsonl 2>&1)"
  sudo -u pop test -r /var/lib/pop/updater/events.jsonl || fail "POP cannot read the updater's record"
  echo "package-test: pop-updater's units are accepted; off it does nothing, on it checks and records"

  # popctl (#183): POP's own commands, run as the service runs POP. A
  # setting only in pop.env, which is root's alone, must reach the command.
  install -m 0755 "$ROOT/packaging/popctl" /usr/local/sbin/popctl
  cp /etc/pop/pop.toml /etc/pop/pop.toml.served
  printf '[server]\nlisten = "127.0.0.1:18081"\n[bam]\nurl = "https://bam.invalid"\ncategory = "c"\n' >/etc/pop/pop.toml
  install -m 0600 /dev/null /etc/pop/pop.env
  echo 'POP_BAM_TOKEN=package-test-token' >/etc/pop/pop.env
  # The control: without pop.env, as the guide's old commands ran it.
  if sudo -u pop POP_STORAGE_DATA_DIR=/var/lib/pop /usr/local/bin/pop --config /etc/pop/pop.toml config check >/dev/null 2>&1; then
    fail "the configuration is valid without pop.env: this test proves nothing"
  fi
  popctl config check | grep -q "The configuration is valid" ||
    fail "popctl did not give the command pop.env"
  ! popctl config nonsense >/dev/null 2>&1 || fail "popctl hid a failing command's exit"
  cp /etc/pop/pop.toml.served /etc/pop/pop.toml
  printf 'package-test-password\n' | popctl user create-admin ci-admin --name "CI admin" ||
    fail "popctl user create-admin failed"
  code=$(curl -s -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' \
    -d '{"username":"ci-admin","password":"package-test-password"}' http://127.0.0.1:18081/api/session)
  [ "$code" = 200 ] || fail "the admin popctl made cannot sign in ($code)"
  echo "package-test: popctl gives POP's commands pop.env, and its first admin signs in"

  # The nightly backup (#183): an archive only POP can read, and those over
  # two weeks old deleted, newer ones kept.
  install -m 0644 "$ROOT/packaging/systemd/pop-backup.service" /etc/systemd/system/
  install -m 0644 "$ROOT/packaging/systemd/pop-backup.timer" /etc/systemd/system/
  systemd-analyze verify /etc/systemd/system/pop-backup.service /etc/systemd/system/pop-backup.timer
  systemctl daemon-reload
  # As on a machine that has never run it: no folder for the backups. The
  # unit must make it itself; the first install found one made by a step
  # of the unit comes too late (#188).
  b=/var/lib/pop-backups
  rm -rf "$b"
  systemctl start pop-backup.service || fail "pop-backup failed on a machine with no $b"
  [ "$(stat -c %a "$b")" = 700 ] || fail "$b is $(stat -c %a "$b"), not 700"
  [ "$(stat -c %U "$b")" = pop ] || fail "$b is $(stat -c %U "$b")'s, not pop's"
  made=$(find "$b" -name 'pop-2*.tar.gz' | head -1)
  [ -n "$made" ] || fail "pop-backup made no archive: $(ls -l "$b")"
  [ -s "$made" ] || fail "$made is empty"
  [ "$(stat -c %a "$made")" = 600 ] || fail "$made is $(stat -c %a "$made"), not 600"
  [ "$(stat -c %U "$made")" = pop ] || fail "$made is $(stat -c %U "$made")'s, not pop's"
  # A second run, as the next night's: those over two weeks old go.
  install -o pop -g pop -m 0600 /dev/null "$b/pop-old.tar.gz"
  touch -d '20 days ago' "$b/pop-old.tar.gz"
  install -o pop -g pop -m 0600 /dev/null "$b/pop-recent.tar.gz"
  touch -d '3 days ago' "$b/pop-recent.tar.gz"
  sleep 1
  systemctl start pop-backup.service || fail "pop-backup's second run failed"
  [ "$(find "$b" -name 'pop-2*.tar.gz' | wc -l)" = 2 ] || fail "the second run made no second archive: $(ls -l "$b")"
  [ ! -e "$b/pop-old.tar.gz" ] || fail "a backup 20 days old was kept"
  [ -e "$b/pop-recent.tar.gz" ] || fail "a backup 3 days old was deleted"
  echo "package-test: pop-backup writes an archive only POP can read, and keeps two weeks"
}

# The kiosk's packages and unit (#10), in the Debian the box runs: what the
# unit starts is there, by the path it names, and systemd accepts the unit.
# What needs the tower's own screen and touch panel is not tested here
# (docs/kiosk.md, "What is not tested").
kiosk() {
  debian="docker.io/library/debian@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3"
  # The script runs in the container, so its $ are the container's.
  # shellcheck disable=SC2016
  "$CONTAINER" run --rm -v "$ROOT:/src:ro" "$debian" sh -c '
    set -eu
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends cage chromium systemd >/dev/null
    for b in $(sed -n "s/^ExecStart=\([^ ]*\) .* -- \([^ ]*\) .*/\1 \2/p" /src/packaging/kiosk/pop-kiosk@.service); do
      [ -x "$b" ] || { echo "package-test: the unit starts $b, which the packages do not install" >&2; exit 1; }
    done
    cage -v
    chromium --version
    useradd --create-home --shell /usr/sbin/nologin kiosk
    install -m 0644 /src/packaging/kiosk/pop-kiosk@.service /etc/systemd/system/pop-kiosk@.service
    install -m 0644 /src/packaging/kiosk/pam-pop-kiosk /etc/pam.d/pop-kiosk
    systemd-analyze verify /etc/systemd/system/pop-kiosk@tty1.service
  '
  echo "package-test: the kiosk's packages install, and its unit is one systemd accepts"
}

case "${1:-}" in
  image) image ;;
  systemd) systemd ;;
  kiosk) kiosk ;;
  *)
    echo "usage: $0 image|systemd|kiosk" >&2
    exit 2
    ;;
esac
