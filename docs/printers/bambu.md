# Bambu printers

POP watches Bambu printers over the network, in LAN mode (#31). Sending
prints to them comes in Phase 2 (#32).

## What the printer needs

- **LAN-only mode, with Developer Mode switched on.** Current Bambu firmware
  lets other software talk to a printer only this way. In LAN-only mode the
  printer does not reach Bambu's cloud, so Bambu Handy and cloud printing
  stop working for it.
- **Its LAN access code and serial number**, both on the printer's own
  screen under the network settings. POP signs in to the printer with the
  access code; it changes when the printer's network settings are reset.
- **A fixed address on the space's network**, by a DHCP reservation, so POP
  finds it after a restart.

## The printer's certificate

A Bambu printer presents a certificate it signed itself, which no authority
vouches for. POP trusts the certificate it sees the first time it connects
to a printer, and from then on only that one. It keeps that certificate's
fingerprint in its database, so a restart does not trust a new one afresh.
If the printer later presents
a different one (the printer was replaced, its firmware reset it, or
something on the network is pretending to be it), POP refuses to connect
and the printer shows the reason. An admin has to confirm it is the same
printer before POP trusts the new certificate.

## The camera

Bambu printers have two kinds of camera stream, by model (OpenBambuAPI's
`video.md`):

| Models | Stream | How POP shows it |
| ------ | ------ | ---------------- |
| X1 series, P2S | RTSPS on port 322, `rtsps://<printer>:322/streaming/live/1` | through go2rtc (docs/integrations/go2rtc.md) |
| H2S, H2D | the same, once **LAN Only Liveview** is turned on at the printer; it is off by default | through go2rtc |
| A1 and P1 series | JPEG pictures over TLS on port 6000 | decoded by POP |

Both sign in as `bblp` with the printer's LAN access code, which POP keeps
sealed (#21).

The port-6000 stream is a 16-byte header giving each picture's size, then
the JPEG. POP reads it with a decoder held to never panic, never take a
picture over 4 MB, and never keep a picture that is not a whole JPEG,
whatever the printer or the network sends: it is fuzzed with random,
truncated and corrupted streams on every test run
(`crates/pop-printers/tests/bambu_camera.rs`; the run prints its seed, and
`POP_FUZZ_SEED` replays it). The camera's certificate is pinned like the
printer's, under its own key, since the sources do not say whether the two
ports present the same certificate.

In BAM!, set a Bambu printer's `camera` field to `built-in`, and POP picks
the kind by its `model` (`X1C`, `P1S`, `A1 mini` and so on, as above; a model
POP does not know is a problem on the printer, naming the field). Its RTSPS
address on its own host, `rtsps://<printer>:322/streaming/live/1`, means the
same as `built-in`. Either way POP signs in with the access code it keeps:
never put the code in BAM!.

- **RTSPS** is played by go2rtc, but not straight from the printer: go2rtc
  checks no certificate for a camera addressed by IP (go2rtc 1.9.15,
  `pkg/tcp/dial.go`), as Bambu printers are, so it would give the access
  code to anything posing as the printer. POP relays the camera instead: it
  makes the TLS connection, with the pinned certificate, and go2rtc dials
  POP on the loopback address. A certificate that is not the pin stops the
  relay, and the admin's camera list says why.
- **JPEG pictures** are read by POP itself, one connection per printer
  however many watch, and only while someone does (or while POP looks at it,
  every half-minute). They are watched as MJPEG; go2rtc is not involved.

What is not proven yet: that a real X1 takes RTSP through the relay (the
fake answers any request), and that a real A1 or P1 sends what the sources
describe.

## What it has been tested against

The protocol is not documented by Bambu. The driver follows the community's
documentation of it, [OpenBambuAPI](https://github.com/Doridian/OpenBambuAPI).

| Model | Firmware | Tested |
| ----- | -------- | ------ |
| none yet | | |

The driver has so far been tested only against POP's own fake printer
(`crates/pop-printers/src/fakes/bambu.rs`), which follows OpenBambuAPI as of
November 2026. `TESTED_FIRMWARE` in `crates/pop-printers/src/bambu.rs` and
this table gain a row once a real printer running that firmware has been
watched through a print. A Bambu firmware update can change the protocol.
