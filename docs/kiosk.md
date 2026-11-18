# The kiosk box

The tower's own screen (#10): a small Linux box with a touch screen, showing
POP's kiosk page full screen from boot. It is Chromium under
[cage](https://github.com/cage-kiosk/cage), a Wayland compositor that runs one
application and nothing else (ADR 0001, "Kiosk runtime"). The same box can
run POP itself (docs/deployment.md).

## Setting it up

On Debian 12 (bookworm) or later, with POP installed as docs/deployment.md
says, from a checkout of POP:

```sh
sudo packaging/kiosk/setup.sh                       # POP on this box
sudo packaging/kiosk/setup.sh http://pop.local:8080/kiosk   # or elsewhere
sudo reboot
```

It installs cage and Chromium, makes a `kiosk` user that can do nothing but
run the screen (no password, no shell), and starts the screen on boot on the
first virtual terminal (`pop-kiosk@tty1.service`). Running it again puts the
files back as they should be.

What it installs: `/etc/systemd/system/pop-kiosk@.service`
(packaging/kiosk/pop-kiosk@.service), `/etc/pam.d/pop-kiosk`
(packaging/kiosk/pam-pop-kiosk), and `/etc/pop/kiosk.env`, which names the
address the screen opens.

## Making it the kiosk (once)

POP counts the space's network as the space only while an enrolled kiosk
checks in from it (#12). Until this is done, nobody signed out at the space
sees names or cameras, and no member there can control a printer.

1. Plug a keyboard into the kiosk box, and on its screen open POP's admin desk
   (`/admin`). Sign in as an admin.
2. Under **Kiosks**, name it ("The tower") and press **Make this screen a
   kiosk**. That browser, and only it, now holds the kiosk's credential: a
   cookie no page can read, kept by POP only as a hash.
3. Go back to `/kiosk`, sign out of the desk, and unplug the keyboard.

From then on the kiosk checks in every half minute; the desk's **Kiosks**
list shows when and from where it last did, and **Revoke** ends its word at
once. Check it: from a phone on the space's Wi-Fi, signed out, a printer's
camera plays; off the Wi-Fi, it asks you to sign in.

A kiosk vouches for its own network only, and for two minutes after its last
check-in: switch it off, and within two minutes the space's network is like
anywhere else.

## What the screen does

- **Starts on boot** and lands on the tower's home screen.
- **Comes back if the browser crashes or quits:** cage exits when its one
  application does, and the unit starts both again, two seconds later.
- **Never blanks:** cage blanks nothing itself; it only tells other programs
  the screen is idle, and the setup installs none that would act on it.
- **No cursor:** cage shows none while no mouse is plugged in. Plug one in to
  have one.
- **Touch, no keyboard:** taps are clicks. Chromium is in kiosk mode, so
  there is no address bar, no pinch-zoom and no swipe-back to leave the page.

A printer with a camera has a camera button on its card: it opens the camera
full screen, through POP, and the tower comes back after five minutes or a
tap on Back (#58). The space's own network may watch without signing in, so
the kiosk needs no session for it.

Signing in at the kiosk needs a keyboard until kiosk sessions start by a card
tap (#12): plug one in to sign in as an admin there.

## Looking after it

- See what it is doing: `journalctl -u pop-kiosk@tty1 -f`.
- Restart the screen: `sudo systemctl restart pop-kiosk@tty1`.
- Another address: edit `/etc/pop/kiosk.env`, then restart it.
- A console instead: switch to another terminal (Ctrl+Alt+F2) and sign in as
  an administrator there.

Debian 12 installs cage 0.1.4. What this page says cage does (no blanking,
no cursor without a mouse, `-d`) was read in cage's source at 0.1.4 and at
0.3.1, and holds for both.

## What is not tested

CI installs the packages in the Debian the box runs, checks that the
programs the unit starts are where it says, and has systemd check the unit
(`tools/package-test.sh kiosk`). That the screen comes up on a real panel,
that touch lands where it is tapped, that it never blanks over a day, and
that it comes back after the browser is killed (`sudo pkill chromium`), are
for the tower's own box: check each once when it is set up.
