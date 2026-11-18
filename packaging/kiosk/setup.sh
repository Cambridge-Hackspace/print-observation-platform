#!/bin/sh
# Sets up the tower's own box as POP's kiosk (#10): Chromium full screen
# under cage, on boot, on tty1, as a user that can do nothing else. Debian 12
# (bookworm) or later, run as root, from a checkout of POP:
#
#   sudo packaging/kiosk/setup.sh [POP's kiosk address]
#
# POP itself is installed as docs/deployment.md says, on this box or another;
# the address defaults to this box's own, http://127.0.0.1:8080/kiosk.
# Running it again puts the files back as they should be.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
URL="${1:-http://127.0.0.1:8080/kiosk}"

[ "$(id -u)" = 0 ] || {
  echo "setup.sh: run it as root" >&2
  exit 1
}

apt-get update
apt-get install -y --no-install-recommends cage chromium dbus fonts-dejavu-core

# A user that only runs the screen: no password, no shell, no sudo. It is in
# the groups that may use the screen and input devices.
if ! id -u kiosk >/dev/null 2>&1; then
  useradd --create-home --shell /usr/sbin/nologin kiosk
fi
usermod -aG video,render,input kiosk

install -d -m 0755 /etc/pop
printf 'POP_KIOSK_URL=%s\n' "$URL" >/etc/pop/kiosk.env
chmod 0644 /etc/pop/kiosk.env

install -m 0644 "$HERE/pam-pop-kiosk" /etc/pam.d/pop-kiosk
install -m 0644 "$HERE/pop-kiosk@.service" /etc/systemd/system/pop-kiosk@.service
systemctl daemon-reload
systemctl enable pop-kiosk@tty1.service
systemctl set-default graphical.target

echo "setup.sh: the kiosk starts on tty1 at the next boot, showing $URL"
echo "setup.sh: to start it now: systemctl start pop-kiosk@tty1.service"
