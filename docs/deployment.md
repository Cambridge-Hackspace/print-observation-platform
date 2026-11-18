# Deploying POP

POP is one binary with the web app inside it (#4), a SQLite database, and,
for cameras, go2rtc beside it as its own service. It runs on the small Linux
box at the tower that also drives the kiosk screen (#10), or on any Linux
machine on the space's network. This guide takes it from nothing to running,
in order; each step says how to check it worked.

What it needs:

- **Linux** on x86-64 or ARM64, with systemd for the service units, or any
  machine that runs containers.
- **BAM!**, where the space's printers are kept: POP reads them from it
  (docs/integrations/bam.md).
- Optional: **go2rtc** for cameras, a **Discord** webhook for the group chat,
  a **GitHub App** for feedback, and a **proxy** for watching from outside
  the space. CS:S is not used yet; signing in with it comes in Phase 2 (#16).

## 1. The binary, or the image

From a release (`.github/workflows/release.yml` makes one for each `v*` tag):

```sh
VERSION=v0.1.0          # the release you are installing
ARCH=$(uname -m)        # x86_64 or aarch64
curl -fLO "https://github.com/Cambridge-Hackspace/print-observation-platform/releases/download/$VERSION/pop-$VERSION-linux-$ARCH"
curl -fLO "https://github.com/Cambridge-Hackspace/print-observation-platform/releases/download/$VERSION/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
# And check SHA256SUMS's two signatures: docs/releases.md, "Checking a release by hand".
sudo install -m 0755 "pop-$VERSION-linux-$ARCH" /usr/local/bin/pop
pop --version
```

Or the container image, `ghcr.io/cambridge-hackspace/print-observation-platform:<version>`,
which keeps its data in the volume `/var/lib/pop` and listens on port 8080
(packaging/Containerfile). Everything below applies to it too, with the
settings given as `POP_*` environment variables instead of a file.

To build it yourself instead: README.md, "Running it".

## 2. A user, and where POP keeps its data

```sh
sudo useradd --system --home-dir /var/lib/pop --shell /usr/sbin/nologin pop
sudo install -d -m 0755 /etc/pop
```

systemd makes `/var/lib/pop` itself, readable only by POP.

## 3. The configuration

Copy `pop.example.toml` to `/etc/pop/pop.toml`: it documents every key with
its default. The ones a deployment sets:

```toml
[server]
listen = "127.0.0.1:8080"          # or 0.0.0.0:8080 with no proxy in front
public_url = "https://pop.example.org"   # what phones open; the kiosk's QR codes use it

[access]
local_networks = ["192.168.1.0/24"]      # the space's own network
trusted_proxies = []                     # a proxy in front: docs/remote-access.md

[bam]
url = "https://bam.example.org"
category = "<the printer category's id>"
# token: in pop.env, below

[go2rtc]                                 # for cameras; leave out for none
url = "http://127.0.0.1:1984"
username = "pop"
# password: in pop.env, below
```

The secrets go in `/etc/pop/pop.env`, readable only by root (systemd reads it
before POP drops to its own user), never in `pop.toml`:

```sh
sudo install -m 0600 /dev/null /etc/pop/pop.env
sudo tee /etc/pop/pop.env >/dev/null <<'EOF'
POP_BAM_TOKEN=<POP's BAM! token>
POP_SECRETS_KEY=<openssl rand -base64 32>
POP_GO2RTC_PASSWORD=<go2rtc's api password>
EOF
```

`POP_SECRETS_KEY` seals the secrets POP keeps, such as a Bambu printer's
access code and the Discord webhook. Keep a copy somewhere safe: a backup
cannot be opened without it, and it is not in the backup.

Check it: `sudo -u pop env $(sudo cat /etc/pop/pop.env) pop --config /etc/pop/pop.toml config check`
says "The configuration is valid", or names what is wrong.

## 4. The service

```sh
sudo install -m 0644 packaging/systemd/pop.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now pop
curl -s http://127.0.0.1:8080/healthz
```

`/healthz` answers 200 with every check passing, or 503 naming the one that
fails: the database, BAM!, go2rtc. `journalctl -u pop` has the detail.

## 5. The first admin

```sh
sudo -u pop env POP_STORAGE_DATA_DIR=/var/lib/pop pop --config /etc/pop/pop.toml \
  user create-admin <username> --name "<their name>"
```

It asks for the password. Then sign in at `/admin`; the admin adds everyone
else there, under **People** (README.md, "Accounts").

## 6. Printers

In BAM!, give POP a token whose role has only `assets.read` and
`events.read`, and set each printer's fields (docs/integrations/bam.md). On
the admin desk, from inside the space, each printer in the category is
offered under **New in BAM!**: add the ones the tower runs. A Bambu printer
also needs its LAN access code, entered on the desk: **Enter its access code**
on its banner, or **Enter** in its **Kept in POP** column (docs/printers/bambu.md).

Check it: the tower (`/kiosk`) shows each added printer, live.

## 7. Cameras (optional)

go2rtc runs as its own service (docs/integrations/go2rtc.md):

```sh
sudo useradd --system --home-dir /var/lib/go2rtc --shell /usr/sbin/nologin go2rtc
sudo install -m 0755 go2rtc_linux_amd64 /usr/local/bin/go2rtc     # 1.9.15 or later
sudo install -d -o go2rtc -g go2rtc -m 0700 /var/lib/go2rtc
sudo install -o go2rtc -g go2rtc -m 0600 packaging/go2rtc.yaml /var/lib/go2rtc/go2rtc.yaml
sudoedit /var/lib/go2rtc/go2rtc.yaml        # set its password: the same as POP_GO2RTC_PASSWORD
sudo install -m 0644 packaging/systemd/go2rtc.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now go2rtc
sudo systemctl restart pop
```

Give each printer's `camera` field in BAM! its camera's address, without
its password, or `built-in` for a Bambu printer's own. A camera that asks
for a login gets it on the admin desk: **Kept in POP**, then **Enter** beside
"Camera login". Check it: the admin desk's camera list
(`/api/admin/cameras`) says `live` for each, and `/healthz`'s `go2rtc` check
passes. It fails, by name, if go2rtc does not ask for its password (#149).

## 8. The group chat (optional)

Make a Discord webhook for the channel, and give it to POP on the admin
desk, from inside the space (docs/integrations/discord.md). Check it with
the desk's **Send a test message**.

## 9. Feedback (optional)

A GitHub App on the repository files feedback from any screen as issues
(docs/integrations/github-feedback.md). Put its key where only POP can read
it, and name it in `[feedback]`.

## 10. Watching from outside (optional)

Put nginx in front, as docs/remote-access.md shows: it forwards only what
POP allows from anywhere, and says truthfully where each request came from.

## 11. Backups

`pop backup <file>` is safe while POP runs. A nightly one, kept for two
weeks:

```sh
sudo install -d -o pop -g pop -m 0700 /var/backups/pop
sudo tee /etc/cron.d/pop-backup >/dev/null <<'EOF'
15 3 * * * pop POP_STORAGE_DATA_DIR=/var/lib/pop /usr/local/bin/pop --config /etc/pop/pop.toml backup /var/backups/pop/pop-$(date +\%F).tar.gz && find /var/backups/pop -name 'pop-*.tar.gz' -mtime +14 -delete
EOF
```

Copy them off the box. To restore onto a fresh data directory:
`pop restore <file>`, then start POP; it needs the same `POP_SECRETS_KEY`.

## 12. Upgrading

Replace the binary and restart; POP does the rest (#147):

```sh
sudo systemctl stop pop
sudo install -m 0755 pop-<new>-linux-$(uname -m) /usr/local/bin/pop    # checked against SHA256SUMS
sudo systemctl start pop
journalctl -u pop -n 20        # "upgrading the database from schema N to M; backed up first to ..."
curl -s http://127.0.0.1:8080/healthz
```

When the new POP knows database changes the old one did not, it backs the
database up before making them, into `/var/lib/pop/backups/`
(`before-schema-<new>-from-<old>-<time>.tar.gz`), keeps the newest three,
and says where in its log. Then it brings the database up to date. An
upgrade with no database changes takes no backup.

To go back to the old POP:

```sh
sudo systemctl stop pop
sudo install -m 0755 pop-<old>-linux-$(uname -m) /usr/local/bin/pop
sudo mv /var/lib/pop /var/lib/pop.upgraded                  # kept, not deleted
sudo install -d -o pop -g pop -m 0700 /var/lib/pop
sudo -u pop env POP_STORAGE_DATA_DIR=/var/lib/pop pop --config /etc/pop/pop.toml \
  restore /var/lib/pop.upgraded/backups/before-schema-<new>-from-<old>-<time>.tar.gz
sudo systemctl start pop
```

The old POP refuses a database the new one has changed, rather than opening
a schema it does not know: it says "this database was last used by a newer
POP", and points here. Restoring the backup taken before the upgrade is the
way back. Anything done after the upgrade is in `/var/lib/pop.upgraded`, not
in the restored database.

## The kiosk

The tower's own screen opens `/kiosk` full screen, from boot: docs/kiosk.md
sets the box up to do it (`packaging/kiosk/setup.sh`).
