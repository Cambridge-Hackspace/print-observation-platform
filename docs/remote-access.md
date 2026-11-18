# Watching from outside the space

Members can watch prints from anywhere: the printers, a print's progress and
its camera, the guides, and saying that something looks wrong. Controlling a
printer, adding one, entering an access code or changing where notices go can
only be done at the space (#7). This guide puts POP on the internet without
changing that (#93).

POP decides what is local by where a request comes from: the networks in
`[access] local_networks`, and, behind a proxy it trusts, the address the
proxy says the request came from. So two things matter: that the proxy says
so truthfully, and that it forwards nothing POP keeps to the space.

## The proxy

`docs/remote/nginx.conf` is a working nginx server block. It:

- **forwards only what POP allows from anywhere**: the web app, health and
  status, the live stream, signing in, guides, watching, feedback, and the
  admin desk's accounts, guides and audit log. Every other API path is
  refused by nginx before POP sees it, and POP refuses the local-only ones
  again on its own;
- **sets `X-Forwarded-For` to the client's own address**, and clears
  `Forwarded`, so a client cannot claim to be inside the space by sending
  one;
- holds the live stream and the camera open, unbuffered, and upgrades the
  camera's WebSocket.

`crates/pop-server/tests/remote_access.rs` holds that file to POP's route
table: every route POP allows from anywhere is forwarded, no route kept to
the space is, no API path POP does not have is, and the forwarding header is
set rather than added to. A new route fails it until the file is updated, so
the example cannot drift from POP. The test reads the file as nginx chooses
locations; nginx itself is not run in POP's tests.

## POP's side

In `pop.toml`:

```toml
[server]
# Where phones reach POP from outside: the kiosk's QR codes use it.
public_url = "https://pop.example.org"

[access]
# The space's own network: what counts as being at the space.
local_networks = ["192.168.1.0/24"]
# The proxy, by the address POP sees it at: 127.0.0.1/32 when nginx runs on
# the same box. Only these may say where a request came from.
trusted_proxies = ["127.0.0.1/32"]
```

**Name the proxy in `trusted_proxies`.** A request from POP's own machine is
local, and without a trusted proxy everything nginx forwards would arrive
from 127.0.0.1, looking as if it came from inside the space. Named, POP
judges each request by the client address nginx gives.

## Tunnels

A tunnel (Cloudflare Tunnel, Tailscale Funnel, ngrok and the like) that
delivers requests to POP from POP's own machine with no forwarding header
makes every request local (threat model, gap 2). **Point the tunnel at nginx,
never at POP**, so nginx sets the header and refuses the local-only routes.

## Checking it

From outside the space (a phone off the space's Wi-Fi):

1. `https://pop.example.org/` shows the printers, live.
2. `curl -i -X POST https://pop.example.org/api/admin/printers/anything/add`
   answers 404, from nginx.
3. `curl -i https://pop.example.org/api/admin/notify` answers 404, from nginx.
4. `curl -i -H 'X-Forwarded-For: 192.168.1.20' -X POST https://pop.example.org/api/admin/printers/anything/add`
   still answers 404: what a client says about itself is not believed.
5. Signed in as an admin from outside, the desk shows accounts and guides,
   and refuses to add a printer.
