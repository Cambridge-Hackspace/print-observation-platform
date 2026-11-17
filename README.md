# POP: Print Observation Platform

POP runs the 3D print tower at the Hackspace. Members send sliced prints from
home, tap a card at the tower to start them, and watch them from anywhere.
Printer admins look after the fleet and the filament.

How and why it is built the way it is: [ADR 0001](docs/adr/0001-architecture-and-stack.md).
What it lets in, from where, and where its secrets live: [the threat model](docs/threat-model.md).

## Layout

```
crates/pop-server        axum: API, live updates, slicer uploads, the web app
crates/pop-printers      the driver interface, and a driver per kind of printer
crates/pop-gcode         slicer metadata from G-code and 3MF
crates/pop-integrations  BAM!, CS:S, Throne, notifications, sign-in providers
web/                     Svelte 5: kiosk, phone and admin screens
tools/                   build and test scripts, all in pinned containers
docs/                    decisions, and how POP is tested
```

## Running it

With Rust 1.97 or later and Node 22:

```sh
cd web && npm install && npm run build && cd ..   # the app, into web/dist
cargo run -p pop-server                            # http://127.0.0.1:8080
```

The server embeds `web/dist` when it is compiled, so build the app first.
For the app with hot reload, run the server, then `npm run dev` in `web/`.

Configuration is one TOML file, `pop.toml` beside the binary or named with
`--config`. [`pop.example.toml`](pop.example.toml) documents every key with its
default, and `pop config check` validates a file without starting the server.
Any key can be overridden from the environment as `POP_<SECTION>_<KEY>`:
`POP_SERVER_LISTEN` overrides `server.listen`.

## Accounts

Create the first admin from the command line; they create everyone else.

```sh
pop user create-admin ada --name "Ada Lovelace"   # asks for the password
```

In a script, give the password on standard input instead. Members and admins
sign in on the web. On the admin desk (`/admin`), under **People**, an admin
adds people, renames them, makes them admins or members, records the training
they hold here (named as a printer's BAM! training field names it), and resets
passwords, which signs that person out everywhere. Deactivating someone signs
them out and stops them signing in, and keeps their history; reactivating
undoes it. POP always keeps at least one active admin.

## Backing up

`pop backup <file>` writes the database and the stored print files to one
archive, and is safe while the server is running. `pop restore <file>` puts one
back into the configured data directory, which must not already hold a
database. A restore checks every file against the archive's manifest and the
database's integrity before anything is put in place.

## Testing

`tools/check.sh` runs every suite in pinned containers; [docs/testing.md](docs/testing.md)
says what each one catches. After changing a type in
`crates/pop-server/src/api.rs`, run `tools/gen-types.sh` to regenerate the web
app's copy.

## License

Blue Oak Model License 1.0.0 for the code, with specifications under the Open
Web Foundation Agreement 1.0 and documentation and diagrams under Creative
Commons Attribution 4.0 International; see [LICENSE](LICENSE).
