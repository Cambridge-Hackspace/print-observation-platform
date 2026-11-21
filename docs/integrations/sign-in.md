# Signing in through a provider

Members can sign in to POP with an account they already have elsewhere,
through OpenID Connect (#14): CS:S first (#16), or any provider that speaks
it. Their POP username and password keep working beside it. The kiosk does
not offer it, since a provider's own page needs a keyboard; there, members
tap their card (#19, docs/integrations/card-sign-in.md).

## How it works

1. **The member chooses "Sign in with CS:S"** on the phone app or the admin
   desk, and their browser goes to the provider.
2. **They sign in there.** The provider sends them back to POP with a code.
3. **POP checks everything before it trusts the code.**
   - The code must come back to the same browser that left.
   - POP exchanges it using PKCE.
   - The ID token must be signed by one of the provider's published keys.
   - The token must be for POP, from that issuer, unexpired, and carry the
     nonce POP sent.
4. **POP signs the member in.** The first time, it makes them a member account.

POP links an account to the provider's `sub`, its unchanging id for the
person. It never links by email or username. A member of the provider who
calls themselves `ada` gets an account of their own (`ada-2`), not access to
POP's existing `ada`.

## Setting up CS:S

In CS:S, under **Admin → Apps**, register POP (CS:S's own guide to this is
"CS:S as an OpenID Connect provider", in the CS:S repository):

- **Redirect URI:** `{POP's public address}/api/sign-in/css/callback`, as
  `https://pop.example.org/api/sign-in/css/callback`. It must match exactly.
- **Scopes:** `openid`, `profile`, and `roles` if CS:S roles should make POP
  admins (below).
- **A secret:** POP runs on a server, so give it one.
- **"The space's own app":** on, so members are not asked to agree first.

Then tell POP, in `/etc/pop/pop.toml`:

```toml
[server]
public_url = "https://pop.example.org"     # needed: the provider sends members back here

[[sign_in.providers]]
id = "css"
label = "CS:S"
issuer = "https://css.example.org"
client_id = "<the id CS:S gave POP>"
scopes = ["profile", "roles"]
```

and the secret in `/etc/pop/pop.env`, never in `pop.toml`:

```sh
POP_SIGN_IN_CSS_CLIENT_SECRET=<the secret CS:S showed once>
```

Check it with `sudo popctl config check`, restart POP, and the sign-in screens
offer **Sign in with CS:S**. Reach POP at its public address for this, since
that is where the provider sends members back. If you run nginx in front,
docs/remote/nginx.conf already forwards the routes.

Another provider works the same way. Give it its own `id`, which then
appears in its redirect URI and its secret's variable name
(`POP_SIGN_IN_<ID>_CLIENT_SECRET`).

## Admins from a provider's roles

POP's own roles decide who is an admin, on the desk's People page. A
provider can also make admins for you (cal, 2026-11-19):

```toml
roles_claim = "roles"            # the ID token claim listing roles or groups
admin_roles = ["print-admin"]    # holding any of these makes a POP admin
```

At each sign-in:

- **A member who holds one of `admin_roles`** becomes an admin, recorded as
  that provider's doing.
- **An admin it made who holds none of them any more** becomes a member
  again. It takes back only what it gave:
  - an admin made on the desk is never demoted for lacking the role;
  - an admin it made whose role you set on the desk afterwards is the desk's
    from then on;
  - it never removes the last admin.
- Each change is in the audit log, as "the CS:S sign-in made Grace an admin".

The change happens at sign-in, not before. Someone whose role is taken away
in CS:S stays an admin in POP until they next sign in through CS:S, or
until you change them on the desk.

## When it does not work

The sign-in screen says, in a word, what went wrong:

- **failed:** the code or token did not check out.
- **cancelled:** the member said no at the provider.
- **expired:** they took more than ten minutes.
- **deactivated:** their POP account is switched off.
- **unreachable:** POP could not reach the provider.

`journalctl -u pop` has the detail, such as "its ID token: …" or "its
discovery document: …".
