# Recorded BAM! answers

`v0.20.0/recordings.json` holds a real BAM!'s answers to everything POP's BAM!
client asks (#20), recorded by `tools/bam/record.py` on 2026-11-16 from BAM!
v0.20.0's own release binary (`bam-freebsd-amd64`, SHA-256
`3127f95ad6a4d97ca07b09fecb4ffb1b9bd41eccf740b91d928303b7d6c1831e`) on a fresh
database. The fake BAM! (`src/fakes/bam.rs`) serves them, and POP's tests run
against it.

The recording seeds what a space would have: printer fields (connection,
address, camera, model, materials, training, serial number), a Printers
category with a Resin printers subcategory and an unrelated Filament one, four
printers (one with no connection set), one archived, and an edit and an
archive afterwards so the event history has both. POP's token is a role with
only `assets.read` and `events.read`; its secret is never written down.

What it shows about BAM!, and what POP relies on:

- The printer list holds only the Printers category itself: Form 3, in the
  subcategory, is not in it. Archived printers are not in it either; the
  `archived=true` read holds only them.
- The event history filtered to Printers *does* include the subcategory's
  (Form 3's creation), so an event naming a category does not make its asset
  a printer.
- A bad token, no token, a missing asset and a plain request for the live
  WebSocket each have BAM!'s own refusal.
- The live WebSocket (`live`), opened as POP's sync opens it: BAM!'s hello,
  then an edit and a restore made while it was open, frame by frame. The fake
  makes its frames from these, and from the event history's, so every frame
  it sends is a shape BAM! sent.

The fake keeps the printer category's assets as state, seeded from the
recording, so a test can change a printer and see POP follow. Each request is
recorded once: the fake refuses a recording that answers one request twice.

## Recording again

For a new BAM! version, on a fresh, empty BAM! listening on 127.0.0.1:18090:

```sh
bam init --config c.toml       # then set [server] host and port
bam migrate --config c.toml
bam admin create --config c.toml --email ada@example.org --password '...'
bam admin mfa require --config c.toml --role admin --off
bam serve --config c.toml &
BAM_VERSION=v0.21.0 python3 tools/bam/record.py http://127.0.0.1:18090 ada@example.org '...'
```

Then point the tests at the new version's recording and see what changed.
