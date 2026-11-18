# Watching from outside the space

Members can watch from anywhere: the printers, a print's progress and its
camera, the guides, and saying that something looks wrong. Controlling a
printer is for members at the space. An admin signed in from anywhere holds
every class, settings and printer control included, and each such sign-in is
told to the group chat (#12; the threat model, "Classes, local and online").
This guide puts POP on the internet without changing that (#93).

POP decides what is local by where a request began, and a kiosk's word: the
request came from one of `[access] local_networks` (behind a proxy it trusts,
by the address the proxy gives), AND an enrolled kiosk has checked in from that
same network in the last two minutes. So two things matter: that the proxy
says where a request began truthfully, and that it forwards nothing that is
for POP's own machine.

## The proxy

`docs/remote/nginx.conf` is a working nginx server block. It:

- **forwards only what POP serves from anywhere**: the web app, health and
  status, the live stream, signing in, guides, watching, feedback, the admin
  desk, and a kiosk's check-in (which vouches for nothing from outside the
  space's networks). Every other API path, the updater's among them, is
  refused by nginx before POP sees it;
- **sets `X-Forwarded-For` to the client's own address**, and clears
  `Forwarded`, so a client cannot claim to be inside the space by sending
  one;
- holds the live stream and the camera open, unbuffered, and upgrades the
  camera's WebSocket.

`crates/pop-server/tests/remote_access.rs` holds that file to POP's route
table: every route POP serves from anywhere is forwarded, no route for POP's
own machine is, no API path POP does not have is, and the forwarding header is
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

**Name the proxy in `trusted_proxies`.** Without it, everything nginx forwards
arrives from 127.0.0.1 with a forwarding header from a peer POP does not
believe, so POP treats it as outside the space, kiosk or no kiosk. Named, POP
judges each request by the client address nginx gives.

## Tunnels

A tunnel (Cloudflare Tunnel, Tailscale Funnel, ngrok and the like) delivers
requests to POP from POP's own machine. That no longer makes them local
(#12), but **point the tunnel at nginx, never at POP**, so nginx sets the
header and refuses what is not POP's to serve from anywhere.

## Checking it

From outside the space (a phone off the space's Wi-Fi):

1. `https://pop.example.org/` shows the printers, live.
2. `curl -i https://pop.example.org/api/updater/quiet` answers 404, from
   nginx: it is for POP's own machine.
3. `curl -i https://pop.example.org/api/admin/notify` answers 401, from POP:
   forwarded, and for admins only.
4. `curl -i -H 'X-Forwarded-For: 192.168.1.20' https://pop.example.org/api/printers/anything/camera/mjpeg`
   answers 401: what a client says about itself is not believed, so it is
   outside the space and signed out.
5. Signed in as an admin from outside, the desk works in full, and the group
   chat is told that an admin signed in from outside the space.
