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
