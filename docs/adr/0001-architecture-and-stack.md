# ADR 0001: Architecture and technology stack

- **Status:** Accepted
- **Date:** 2026-11-15
- **Origin:** CS:S issue #99, "Epic: scope out print tower"

## Context

POP (Print Observation Platform) runs the 3D print tower at the Hackspace.
Members send sliced prints from home, tap a card at the tower to start them, and
watch them from anywhere. Printer admins look after the fleet and the filament.
The approved screens are round 6 of the design canvas.

Several decisions were settled before we chose a stack, and they constrain it.

1. **POP is a separate program.** It integrates with Cooperative Systems:
   Spaces (CS:S), but it does not depend on it. A space without CS:S can run
   POP on its own.
2. **Sign-in has interchangeable providers.** Local users come first, then
   OAuth 2.0 / OpenID Connect. CS:S becomes one of the OIDC providers
   once it can act as one; today it cannot. SAML was planned too and was
   dropped on 2026-11-28 (#15): it would have put libxml2 and xmlsec1 in every
   build, and a SAML-only identity provider can reach POP through an
   OIDC bridge such as Keycloak or SATOSA.
3. **BAM! is required.** Printers and filament live in Boring Asset Manager.
   POP reads printers from a configured BAM! category and its fields, and reads
   and writes filament stock through BAM!. POP never keeps its own inventory,
   because that would duplicate BAM!.
4. **POP is not a slicer.** The tower is a printer target for open-source
   slicers such as OrcaSlicer, PrusaSlicer and SuperSlicer. POP reads the
   metadata that the slicer writes into a file. It never slices anything.
5. **Printers are treated as dumb.** POP owns the queue, the checks, the record
   of who printed what, and the timing. A printer receives a job only at the
   moment POP dispatches it.
6. **Monitoring works from anywhere. Control is local.** Anyone with access can
   watch a print from outside the space. Nothing outside the space can start,
   pause, stop or resume a printer.
7. **Every behavioral check is configurable.** This covers training, material
   match, enough filament, bed cleared and waiting in line. Each check is off,
   warn or block. Each has a tower-wide default and an optional per-printer
   override.
8. **POP does not handle money.** Communal filament requests use Throne. The
   kiosk and phone show a QR code that links to the wishlist. POP listens to
   Throne's webhook to follow contributions and to mark a request as funded
   (see "Funding: Throne webhooks" below). If the webhook proves unreliable, an
   admin can mark requests by hand.
9. **There are two roles: member and admin.**

The quality bar comes from the design work. The UI must be snappy and visually
clean on modest kiosk hardware. The hardest engineering is forced
interoperability with printers that were never designed to be driven this way.

## Decision

### Backend: Rust

The backend is written in Rust and runs on tokio, with axum as the HTTP server.
It ships as a single static binary that also serves the web front end.

POP's riskiest code works at the byte level.

- **USB serial streaming to dumb printers.** POP sends G-code line by line,
  with line numbers, checksums, resend handling and `ok` flow control. It must
  never stall partway through a print.
- **Bambu's LAN protocols.** These are MQTT over TLS against a self-signed
  certificate, which POP pins, plus implicit FTPS for uploads and a binary
  camera frame stream.
- **G-code and 3MF metadata parsing.**

Rust's type system makes these protocol state machines hard to get wrong. It
has no garbage-collection pauses during a stream. It builds as one binary for
ARM boxes.

### Printers: one driver interface

All printer access goes through one Rust trait. Each driver declares what it
can do: upload, start, status, camera and errors. Each connected printer runs as
its own task, and a supervisor restarts the task if it fails. That isolates one
misbehaving printer from the rest of the fleet.

Drivers are built in this order:

1. Moonraker (Klipper): HTTP, plus its JSON-RPC WebSocket for status.
2. OctoPrint and PrusaLink.
3. Bambu in LAN mode, with full control (see below).
4. Direct USB serial for Marlin printers. POP is the print host, which makes
   any printer with a USB port a dumb printer.

### The Bambu LAN driver

Bambu printers are in scope with full control, not watch-only. Bambu does not
document the LAN protocol, so the driver follows the community's documentation
of it (for example the OpenBambuAPI project) and is tested against a fake that
speaks it.

- **Status and commands** go over MQTT with TLS on port 8883. POP signs in with
  the printer's LAN access code and uses the printer's serial number in the
  topic names. The printer presents a self-signed certificate, which POP pins
  per printer rather than trusting blindly.
- **Uploads** use implicit FTPS on port 990. Bambu printers take a sliced
  `.gcode.3mf` rather than plain G-code, so POP's upload endpoint accepts both.
  POP reads metadata from the G-code inside the 3MF.
- **Starting a print** is an MQTT command that names the uploaded file.
- **Cameras:** models that serve RTSPS go through go2rtc. Models that send a
  framed JPEG stream over TLS on port 6000 are decoded by the driver itself.
- **Deployment requirement:** current Bambu firmware allows third-party control
  only in LAN-only mode with Developer Mode switched on. POP's printer setup
  says so, and the driver reports clearly when a printer refuses commands. A
  Bambu firmware update can change this behavior, so the driver records the
  firmware versions it was tested against.

Each driver is tested against a fake printer that speaks the real protocol.
Each fake is self-tested by confirming that it catches a deliberately broken
driver.

### Slicer target: OctoPrint-compatible upload

POP implements the OctoPrint upload endpoints, `GET /api/version` and
`POST /api/files/local`. Each member authenticates with a personal API key, and
that key is how POP knows who sent a file. This endpoint shape is the common
standard: Moonraker accepts slicer uploads through its own OctoPrint
compatibility layer, and OrcaSlicer, PrusaSlicer, SuperSlicer and Cura (through
a plugin) all speak it.

A request to "upload and print" is treated as "upload". The print still waits
for its owner to tap in at the tower. Members add the tower as a single physical
printer in their slicer, and choose the actual printer at the kiosk. The printer
model in the G-code decides which printers the kiosk offers.

### Front end: Svelte 5 on Vite, without SvelteKit

The kiosk, phone and admin screens are a single-page Svelte 5 application,
built with Vite and served by the Rust binary.

Svelte compiles components into direct DOM updates, with no virtual DOM and a
very small runtime. That suits a kiosk that is constantly animating progress
bars and auto-scrolling on modest hardware, and it keeps the phone screens
light. Scoped styles, transitions and tweened values are built in. Those cover
the motion the design asks for (progress easing forward, cards reordering, idle
auto-scroll) without adding libraries. SvelteKit is not used, because server
rendering adds nothing when one local binary serves the application.

The design is implemented as hand-built components on CSS custom properties,
ported from the round 6 design kit. No component library is used, because one
would fight the established look.

### Shared types: generated from Rust

The TypeScript types the front end uses are generated from the backend's Rust
types, with `specta` or `ts-rs`. A printer state, a job or a check result is
defined once, in Rust, so the front end cannot drift from it.

### Live updates: Server-Sent Events

Server state reaches screens over Server-Sent Events. Updates only flow from
server to screen, while actions are ordinary HTTP requests. EventSource
reconnects automatically and resumes from the last event it received, so a
kiosk or phone that loses its connection catches up instead of rebuilding its
state.

### Cameras: go2rtc alongside POP

Camera streams go through [go2rtc](https://github.com/AlexxIT/go2rtc), which
runs as a separate process next to POP. It already handles RTSP/RTSPS, MJPEG and
WebRTC. POP proxies the streams and checks who is allowed to watch. Writing our
own video handling would be the wrong use of effort.

### Storage: SQLite through sqlx

POP's data is small: users, jobs, events and settings. SQLite needs no database
server, and backing it up is a file copy. sqlx is async-native to match tokio,
checks plain SQL against the schema at compile time, and runs migrations from
plain SQL files. Uploaded print files live on disk next to the database.

### Kiosk runtime: Chromium in `cage`

The tower runs Chromium in kiosk mode under `cage`, a single-application Wayland
compositor, on a small Linux box. The same box runs POP. We considered WPE
WebKit, which is built for embedded kiosks and uses less memory. We chose
Chromium because the kiosk shows camera streams, and Chromium's WebRTC is the
reference implementation.

### Local control, enforced by the server

*Amended 2026-11-18 (cal, on #12).* There are three classes of permission,
each including the one before: **visibility** (see everything, names
included), **printer** (control printers) and **admin** (configure the
system). Locally, at the kiosk or on the space's network, a stranger has
visibility, a member printer and an admin admin; online, a stranger has none,
a member visibility and an admin admin. A request is local only when it began
on a configured space network AND an enrolled kiosk has checked in from that
same network lately: the kiosk confirms from its own side. Admins online hold
every class, and each such sign-in is told to the group chat.

The rule as first written, kept for the record: routes that dispatch a print,
confirm a cleared bed, or change anything about a printer accept requests only
from a kiosk session on the local network. Routes reachable from outside the
space, such as watching a print, following progress, reporting a problem or
uploading a file, are read-only with respect to printers.

Either way, who may do what is a property of the server, not of which buttons
the UI happens to hide.

### Integrations: interchangeable adapters

Each outside system sits behind an adapter that is chosen in configuration.

| Concern       | Adapters                                                                                    |
| ------------- | ------------------------------------------------------------------------------------------- |
| Sign-in       | Local users; OIDC; CS:S (as an OIDC provider, once CS:S supports it)                        |
| Card reader   | USB reader on the kiosk box; ToolGuard over MQTT when CS:S is present, with the member named in the scan reply (CS:S #228) |
| Training      | Local flags on each user; identity-provider groups or claims; CS:S                          |
| Group notices | A Discord webhook, directly, as BAM! does, whether or not CS:S is present |
| Personal messages | CS:S's notification relay when present; email over SMTP when POP runs on its own; web push from POP's phone app either way |
| Inventory     | BAM!, required: a REST client plus a webhook receiver                                       |
| Funding       | Throne: a signed-webhook receiver, with manual marking as the fallback                      |

Personal messages (your print is done, needs a hand, it's your turn) go to one
member. When CS:S is present, POP hands them to CS:S's notification relay
(CS:S issue #226), addressed only by the member's identifier. CS:S delivers
each one however that member has chosen in CS:S, so POP never knows or stores
where a member's notifications go. Web push from POP's own phone app is offered
alongside it, for members who want alerts on their phone.

Sign-in, card reader and training are separate settings. A space could, for
example, sign in through Google while keeping its card list in POP.

### CS:S's wire crate is reused

When POP talks to CS:S's local broker, for example to a ToolGuard card reader,
it uses CS:S's `css_wire` crate rather than re-implementing the messages.
`css_wire` is published by git tag for exactly this kind of reuse (the
firmware depends on `css_wire-v0.1.0`), and it pulls in nothing but serde.
POP pins a tag the same way. The tag is a source dependency, not a running
CS:S, so POP still builds and runs without a CS:S installation.

### Funding: Throne webhooks

Throne's webhook documentation is treated as authoritative until we observe
otherwise
(<https://help.throne.com/en/articles/15935990-how-do-i-set-up-webhook-integration>).

- **Verification.** Every request is signed with Ed25519 against Throne's
  published public key. POP verifies the hex signature in `X-Signature-Ed25519`
  over `timestamp + "." + raw body`, using the timestamp from
  `X-Signature-Timestamp`. It verifies the raw body before parsing it, rejects
  a missing or non-numeric timestamp or a signature that isn't 64 bytes, and
  rejects timestamps outside a short window to stop replays.
- **Idempotency.** `event_id` is stored. A repeated delivery returns 2xx and
  changes nothing. POP answers within Throne's 10-second limit by recording the
  event first and acting on it afterwards.
- **Events.**
  - `contribution_purchased` adds `amount` to a request's progress.
  - `gift_crowdfunded` marks a request fully funded.
  - `gift_purchased` marks a request funded outright.
  - Amounts are integers in the currency's minor unit, alongside an ISO 4217
    `currency`.
- **Matching.** The payload names the item (`item_name`) but carries no item
  ID. An admin therefore links each POP request to the exact Throne item name.
  An event that matches no request is kept and shown to admins rather than
  dropped.
- **No money passes through POP.** The kiosk only shows a QR code that links to
  the wishlist.

### Guides live in POP, and are public

Guides (steps and yes/no flowcharts) are stored and edited in POP. Published
guides can be read without signing in, at stable URLs and through a public
JSON API, so CS:S and anyone else can link to them. POP does not read guides
from CS:S. Those public routes are read-only, like every other route reachable
from outside the space.

### Repository layout

```
crates/pop-server        axum: API, SSE, OctoPrint-compatible upload, static files
crates/pop-printers      the driver trait and each driver
crates/pop-gcode         slicer metadata from G-code and 3MF
crates/pop-integrations  BAM!, CS:S, Throne, notifications, sign-in providers
web/                     Svelte 5: kiosk, phone and admin screens
docs/adr/                architecture decision records
```

## Alternatives considered

- **Elixir on the BEAM.** This was the strongest alternative. A fleet of
  unreliable devices, each a supervised process, with live updates fanned out to
  every screen, is the problem the BEAM was built for, and Phoenix PubSub would
  give us the fan-out for free. We chose Rust anyway, because POP's hardest and
  least forgiving work is byte-level: serial streaming, Bambu's protocols and
  binary parsing. That is where Rust is safest. One task per printer under a
  supervisor recovers most of what the BEAM offers.
- **Go.** It would be a fine choice. go2rtc is written in Go and could be
  embedded rather than run alongside. We rejected Go because its type system is
  weaker at modeling protocol state machines, which is exactly the code we
  expect to struggle with.
- **Python.** It has the largest collection of existing printer libraries, but
  it is weaker for long-running concurrent device connections, and packaging it
  for a kiosk box is harder.
- **Vue 3, as in CS:S.** We rejected it on technical grounds: Svelte's compiled
  output and built-in motion fit the kiosk better. Familiarity was not a factor
  in this decision.
- **Solid.** Its performance is comparable to Svelte's. Svelte won on built-in
  scoped styling and transitions, which carry most of this design.
- **A native kiosk, like CS:S's Bevy kiosk.** We rejected it because POP has
  three screens (kiosk, phone and admin desk) that share components. A native
  kiosk would mean building the UI twice.
- **WebSockets for live state.** The live updates only flow one way, and SSE
  adds automatic reconnect and resume.
- **Postgres.** It is unnecessary at POP's size. sqlx could move to Postgres if
  a deployment ever needs it.
- **diesel.** It is a strong choice. sqlx won by a small margin for being async
  on tokio and for using plain SQL migrations.

## Consequences

- POP depends on CS:S's `css_wire` crate by git tag, so a change to the broker
  vocabulary reaches POP only when POP moves to a new tag.
- "Sign in with CS:S" depends on CS:S becoming an OpenID Connect provider,
  which is CS:S issue #225. Until then, POP uses local users or another
  identity provider.
- Bambu control depends on each printer being in LAN-only mode with Developer
  Mode on. It is exposed to Bambu firmware changes, because the protocol is
  undocumented.
- Throne requests are matched by item name, because the webhook carries no item
  ID. Renaming an item on Throne breaks the link until an admin re-links it.
- Deployment is one binary plus go2rtc on one box at the tower. Remote watching
  needs that box to be reachable from outside, for example through a reverse
  proxy or a tunnel that exposes only the read-only routes.
- Two languages are needed to contribute, Rust and TypeScript (Svelte).
  Generating the shared types from Rust keeps the boundary between them small.
