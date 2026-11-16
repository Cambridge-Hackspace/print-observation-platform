# BAM!

POP reads its printers from BAM! (ADR 0001): a category of BAM! assets, and
the fields that say how to reach each printer. It never writes to BAM!, and
keeps no inventory of its own. This was built against BAM! v0.20.0.

## Giving POP a token

POP needs an API token that can read the printer category's assets and
events, and nothing more. In BAM!, a token holds roles, so:

1. **Make a role** that holds `assets.read` and `events.read` (Settings →
   Roles), named for POP.
2. **Narrow it to the printers** (recommended). BAM! limits what a role
   reaches with *scopes*, which are labels on categories: put a scope (say
   `printers`) on the printer category, which its subcategories inherit, and
   give the token the POP role in that scope only. Without this, the token can
   read every asset in BAM!.
3. **Make an API token** with that role, and give its secret to POP. BAM!
   shows the secret once.

A token that is wrong, expired or revoked shows on POP's health check as
"BAM! refused POP's token"; one whose role lacks a permission says which.

## The printer category

POP reads the assets **directly in** the category you name. Assets in its
subcategories are not printers to POP, and neither are archived ones, which
POP retires (#23). Which of the category's fields give each printer's
connection, address, camera, model, materials and training is set in POP
(#21).

## Which fields say what

The `[bam.fields]` section of POP's configuration names, by key, the field
of the printer category that gives each printer's settings
(`pop.example.toml` has the defaults):

| Setting | Default key | What POP needs in it |
| --- | --- | --- |
| connection | `connection` | `moonraker`, `octoprint`, `prusalink`, `bambu` or `usb_serial` |
| address | `address` | Moonraker: its host, or a ws:// or http:// address. OctoPrint and PrusaLink: an http:// address. Bambu: its IP address or name alone. USB: the device, such as /dev/ttyACM0 |
| camera | `camera` | an rtsp, rtsps, http or https address (optional) |
| model | `model` | the model, as people say it (optional) |
| materials | `materials` | the materials it prints, comma-separated (optional) |
| training | `training` | the training level it needs (optional) |
| serial | `serial` | a Bambu printer's serial number |

A printer whose fields are missing or wrong is not run: POP's admin list
(`GET /api/admin/printers`) shows it with each problem, naming the field, so
it can be put right in BAM!.

## Secrets stay in POP

A Bambu printer's LAN access code never goes in BAM!. An admin enters it in
POP, at the space (it changes a printer's settings), and POP keeps it sealed
with ChaCha20-Poly1305 under the key in `POP_SECRETS_KEY` (`openssl rand
-base64 32`). Without a key, POP refuses to keep it, rather than keep it in
plain text. Changing the key means entering the codes again.

## What POP was tested against

`crates/pop-integrations/tests/recordings` holds a real BAM! v0.20.0's
answers to everything POP asks, recorded with `tools/bam/record.py`, and POP's
tests run against a fake that serves them. A new BAM! version is recorded the
same way.
