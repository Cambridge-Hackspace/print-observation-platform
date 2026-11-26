# Signing in with a card

Where the space runs CS:S, a member can sign in at the tower by tapping their
card on the tower's reader (#18). The tower greets them by name ("Welcome,
Alex!") and signs them in to their own account: the same account a CS:S
sign-in on their phone reaches (docs/integrations/sign-in.md), linked by
the same CS:S id.

## How it works

POP is a CS:S device, like the ToolGuard on a printer or the plug on a fan.
CS:S's edge treats it as one.

1. **The member taps their card on the tower's reader.** The reader is a CS:S
   device, and the tap goes to the edge, as at any tool or door.
2. **On a yes, the edge leases POP.** POP is bound to the reader's resource
   as `power`, the role the edge leases. On a resource set to identify its
   member, the lease says who tapped: their CS:S id and name.
3. **POP checks the lease is the edge's own.** The edge seals every lease it
   sends POP with the key CS:S gave POP when it registered, and no one else
   holds that key. POP acts on no lease that is unsigned, sealed with another
   key, or a copy of one it has already had.
4. **The kiosk whose card reader that resource is welcomes the member and
   signs them in**, to a kiosk session, which ends when the screen is left
   alone (#12). Their first tap makes their account in POP, as a first
   sign-in in a browser does.

A refusal ("Unknown card", "Training required") shows on the reader itself.
The edge leases nobody on a no, so POP never hears of one.

A card sign-in never makes or takes away an admin. A lease carries no roles,
so only a sign-in in a browser, whose ID token names them, does that.

## Setting up CS:S

You need CS:S 0.6.1 or later. An older edge does not seal its leases, so
POP believes none of them.

1. POP must already sign members in through CS:S
   (docs/integrations/sign-in.md), so that a tap's CS:S id is the `sub` of
   an account POP knows.
2. Mint a device invite in CS:S, and register POP with it (below).
3. In CS:S's **Resource wiring**, for the tower's reader's resource:
   - **Identify the member on a scan:** on. Without it, a lease names nobody
     and POP's log says to switch this on.
   - Bind **POP** to it as **power**. The tower's reader stays bound as
     **reader**, and the resource stays bound to the space's edge.

## Setting up POP

POP keeps its CS:S identity sealed with its other secrets, so it needs a
secrets key (`POP_SECRETS_KEY`, docs/deployment.md). Register it once,
with the invite:

```sh
sudo popctl css-device register --server https://css.example.org --invite <code>
```

This prints the device id to bind in CS:S. `popctl css-device status` says
whether POP is registered, and `popctl css-device forget` forgets the
identity, so POP can be registered again. Delete the old device in CS:S
then too.

Then, in `/etc/pop/pop.toml`:

```toml
[card_sign_in]
provider = "css"                      # the [[sign_in.providers]] id
broker = "mqtt://192.168.1.20:1883"   # the space's broker, where the edge leases its devices
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

Check it with `sudo popctl config check` and restart POP. POP refuses to
start with card sign-in on and no CS:S identity, and says how to register.
`/healthz` then has a `card_reader` check, which fails while POP cannot hear
the broker.

Then, on the admin desk, under **Kiosks**, give the tower's kiosk its **card
reader**: the CS:S id of the reader's resource, the UUID CS:S shows for it.
A lease always names its resource by that id, never by an external id. A
tap at a reader no kiosk has signs nobody in anywhere, and a reader belongs
to one kiosk at a time.

## What it trusts

The edge, as far as CS:S does. A lease POP acts on was sealed by whoever
holds POP's command key: CS:S, and the edge it gave POP's key to. Whoever
can only publish on the space's broker can sign nobody in. What they can
still do is stop POP hearing the edge, which signs nobody in either
(docs/threat-model.md, gap 23).

## When it does not work

- **Nothing happens at a tap.** Check, in this order:
  - `/healthz` for `card_reader`.
  - POP's log: "names no member" means the resource isn't set to identify
    its member, and "not believed" means a lease came that was not sealed
    for POP. That usually means an edge older than CS:S 0.6.1, or a POP
    registered again whose old device is still bound.
  - That the kiosk's card reader on the desk is the resource's CS:S id.
  - That POP is bound to that resource as power.
- **"This account is deactivated in POP."** An admin deactivated the
  member's account in POP. Reactivate it on the desk.
