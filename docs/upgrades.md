# Automatic upgrades

Optional (#148). Off unless turned on. When on, the tower takes each new
release by itself, in hours you choose, never mid-print, and puts the old
one back if the new one does not come up healthy.

It is its own small service, `pop-updater`, beside POP, so a POP that will
not start can still be put back. A systemd timer runs it every quarter of
an hour.

## What one run does

1. **Nothing**, unless `enabled = true` and the time is within `hours`
   (this machine's time zone).
2. **Asks GitHub for the newest release**: releases only, unless
   `prereleases = true`. A release that is not newer than what runs is
   never taken: the updater does not downgrade.
3. **Waits while any printer POP watches is busy with a print**: printing,
   paused, or stopped mid-print for a person. It asks POP, on this machine
   only (`/api/updater/quiet`).
4. **Downloads** `SHA256SUMS`, its two signatures and this machine's binary,
   and **refuses the release** unless both signatures check against the
   release keys and the binary is the one `SHA256SUMS` names
   (docs/releases.md). A refused release is not tried again; the next one is.
5. **Puts the new binary in**, keeping the old one as `pop.previous` beside
   it, and restarts POP. POP backs its database up before it changes it
   (docs/deployment.md, "Upgrading").
6. **Waits up to `health_wait_secs`** for the new POP to say its new
   version, with its database working and every `/healthz` check that passed
   before the upgrade still passing. (A check that failed before, BAM! being
   down say, is not the new release's doing.)
7. If it does not: **rolls back**. It puts the old binary back, and if the
   new one changed the database, the database as it was before (from that
   backup). The new release's data directory is kept beside it, as
   `/var/lib/pop.failed-<version>-<time>`, for whoever looks into it. A
   release that failed is not tried again.

Every check, download, refusal, upgrade and rollback goes in POP's audit log,
as done by "the updater". Admins see the running version and the last
result on `GET /api/admin/updater`.

## Turning it on

As root, from a release (`VERSION` and `ARCH` as in docs/deployment.md), with
the binary checked against the release's signed `SHA256SUMS`
(docs/releases.md):

```sh
curl -fLO "https://github.com/Cambridge-Hackspace/print-observation-platform/releases/download/$VERSION/pop-updater-$VERSION-linux-$ARCH"
sha256sum --check --ignore-missing SHA256SUMS
sudo install -m 0755 "pop-updater-$VERSION-linux-$ARCH" /usr/local/bin/pop-updater
sudo install -d -m 0755 /usr/local/share/pop/release-keys
sudo install -m 0644 packaging/release-keys/*.pub /usr/local/share/pop/release-keys/
sudo install -m 0644 packaging/updater.toml /etc/pop/updater.toml   # enabled = true; check hours
sudo install -m 0644 packaging/systemd/pop-updater.service packaging/systemd/pop-updater.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now pop-updater.timer
```

Check it: `sudo systemctl start pop-updater.service`, then
`journalctl -u pop-updater -n 20`. Outside its hours it says so and does
nothing.

To turn it off: `enabled = false` in `/etc/pop/updater.toml`, or
`sudo systemctl disable --now pop-updater.timer`.

The updater does not upgrade itself. A new `pop-updater` is installed by
hand, as above, when a release's notes say it changed.

## Settings

`/etc/pop/updater.toml` (packaging/updater.toml):

| Setting | Default | What it does |
|---|---|---|
| `enabled` | `false` | Nothing happens unless this is `true`. |
| `prereleases` | `false` | Take pre-releases too. |
| `hours` | `"03:00-05:00"` | When an upgrade may go in, in this machine's time zone. May run past midnight. |
| `repo` | `Cambridge-Hackspace/print-observation-platform` | Where releases come from. |
| `api` | `https://api.github.com` | GitHub's API. |
| `keys` | `/usr/local/share/pop/release-keys` | The release public keys. |
| `binary` | `/usr/local/bin/pop` | POP's binary. |
| `pop` | `http://127.0.0.1:8080` | POP, as this machine reaches it. |
| `service` | `pop.service` | POP's systemd unit. |
| `data_dir` | `/var/lib/pop` | POP's data directory. |
| `user` | `pop` | The account POP runs as. |
| `pop_config` | `/etc/pop/pop.toml` | POP's configuration, for `pop restore`. |
| `health_wait_secs` | `120` | How long a new release has to come up healthy. |

## When it rolled back

The audit log says why, in the updater's words: "v0.3.0 failed its health
check (its database check fails): back to v0.2.0". The failed release's data
is in `/var/lib/pop.failed-<version>-<time>`, and `journalctl -u pop` has
what it said. The updater waits for the release after it.

If the old release did not come back either, the run fails
(`systemctl status pop-updater`), the audit log says so, and a person is
needed: docs/deployment.md, "Upgrading", is the way back by hand.

## What is tested, and what is not

`crates/pop-updater/tests/updater.rs` runs the updater against a fake
release feed, a fake POP and a fake machine: a good release is applied, a
tampered one refused, a failing one rolled back (with its database, by POP's
own backup and restore), nothing applied mid-print or outside its hours,
nothing older ever, pre-releases only when asked, a restart that did not
happen is not taken for an upgrade, and a rollback that does not come back
is said. CI's systemd check (`tools/package-test.sh systemd`) runs the real
units on a real machine: off, it does nothing; on, it checks a feed and
records it where POP reads it. Applying a real release on a real machine
is not tested before the first release exists.
