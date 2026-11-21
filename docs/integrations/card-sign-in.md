# Signing in with a card

Where the space runs CS:S, a member can sign in at the tower by tapping their
card on the tower's reader (#19). It is a CS:S sign-in like the phone's
(docs/integrations/sign-in.md), with no keyboard: the same account, linked
by the same CS:S id, and an admin or not by the same roles. CS:S can also ask
for the code from the member's authenticator app first, for everyone or for
admins only, which the kiosk takes on a pad.

## How it works

1. **The member taps their card on the tower's reader.** The reader is a CS:S
   device, and the tap goes to CS:S, as at any tool or door.
2. **CS:S answers on the space's broker**, naming the reader's resource. For
   a resource set to sign in to POP, its answer carries a one-time code, or a
   ticket that wants the member's authenticator code, or why not.
3. **POP hears the answer** and finds the kiosk whose card reader that is.
   POP only listens on the broker: it never asks and never publishes.
4. **The kiosk shows it.** POP keeps the code or ticket itself; the kiosk is
   told only what to show. For a code, the kiosk signs in at once. For a
   ticket, it shows a pad for the six digits, and a wrong code may be tried
   again, three times in all, as CS:S allows.
5. **POP redeems it with CS:S**, with POP's own client secret, and believes
   the ID token as it believes a phone sign-in's: signed by CS:S's published
   keys, for POP, from CS:S, unexpired. The member is signed in to a kiosk
   session, which ends when the screen is left alone (#12).

A refusal ("Unknown card", or "set up an authenticator app first") shows in
CS:S's words, and goes by itself after a few seconds.

## Setting up CS:S

POP must already sign members in through CS:S (docs/integrations/sign-in.md),
with a secret, as "the space's own app": CS:S gives card sign-ins to no other
kind of app.

In CS:S's **Resource wiring**, for the tower's reader's resource:

- **Identify the member on a scan:** on.
- **A tap signs in to:** POP.
- **Also ask for an authenticator code from:** nobody, admins, or everyone.
  Asked and not set up, a member is refused in words and told where to set
  one up.

The tower's reader is bound to that resource like any reader, and to an edge
on the space's broker.

## Setting up POP

In `/etc/pop/pop.toml`:

```toml
[card_sign_in]
provider = "css"                      # the [[sign_in.providers]] id
broker = "mqtt://192.168.1.20:1883"   # the space's broker, where the edge answers taps
username = "pop"                      # POP's login there, if it asks for one
```

and the broker's password, if it asks for one, in `/etc/pop/pop.env`:

```sh
POP_CARD_SIGN_IN_PASSWORD=<POP's broker password>
```

`mqtts://` connects with TLS, trusting what this machine trusts, as POP's
other connections do. A broker whose certificate the space's own authority
signed is trusted once that authority is installed: on Debian, copy it to
`/usr/local/share/ca-certificates/` and run `sudo update-ca-certificates`.

Check it with `sudo popctl config check` and restart POP. `/healthz` then has
a `card_reader` check, which fails while POP cannot hear the broker.

Then, on the admin desk, under **Kiosks**, give the tower's kiosk its **card
reader**: the resource its reader is, as the reader names it (its CS:S id, or
its external id, as `tower-1`). A tap at a reader no kiosk has signs nobody
in anywhere, and a reader belongs to one kiosk at a time.

## What it trusts

The broker. POP believes a reply about which reader a tap was at, so
whoever can publish on the space's broker can show a made-up refusal at a
kiosk, or send a real member's reply to another kiosk within its minute.
They cannot make a sign-in: a code is redeemed only with POP's secret, and
one CS:S never issued is refused. Give the broker logins, let only the edge
publish scan replies, and use `mqtts://` where the broker offers it
(docs/threat-model.md, gap 23).

## When it does not work

- **Nothing happens at a tap.** Check `/healthz` for `card_reader`, then that
  the kiosk's card reader on the desk is exactly what the reader names, then
  that the resource in CS:S is set to sign in to POP.
- **"CS:S could not sign you in just now."** The edge allowed the tap but
  could not reach CS:S for a sign-in. Sign in another way, and look at the
  edge.
- **"POP could not finish signing you in with your card."** POP's own setup:
  its log says why, usually the client secret, or CS:S unreachable.
