# Training

Training decides which printers a member may use (#17). It comes from one
of two places, set by `[training] source` in `/etc/pop/pop.toml`:

- **`local`** (the default): admins tick each account's training on POP's
  desk, and a printer needs the training level its BAM! `training` field
  names.
- **`css`**: CS:S says. A printer needs the CS:S tool its BAM! `css_tool`
  field names, and POP asks CS:S each time it checks.

A printer whose field is empty needs no training. An admin needs training
as a member does.

POP checks at each use, and keeps no answer, so a member who loses training
is refused at their next check, though they stay signed in. A member asks
for themselves with `GET /api/printers/{bam_id}/training`, which answers
`{"may_use": false, "reason": "..."}`, with the reason in words for them.

## Training on POP's desk

On the admin desk, under **People**, tick the training each member holds.
The levels are whatever the space calls them, as `FDM basics`. A printer's
BAM! `training` field names the one it needs, matched whatever its case.

## Training from CS:S

POP asks CS:S with its own token, not the member's. POP keeps no member's
CS:S tokens: CS:S answers POP about any member, by the CS:S id POP linked
their account to when they first signed in with CS:S.

### Setting up CS:S

It needs CS:S 0.5.1 or later. POP must already sign members in through CS:S
with a secret (docs/integrations/sign-in.md). In CS:S, under **Apps**,
give POP's app **Read any member's training**: the `training:members` scope.
Give it to POP alone, since an app holding it can read every member's
training.

### Setting up POP

In `/etc/pop/pop.toml`:

```toml
[training]
source = "css"
provider = "css"    # the [[sign_in.providers]] id
```

Then, in BAM!, give each printer that needs training its CS:S tool in its
`css_tool` field (docs/integrations/bam.md): the tool's external id as CS:S
shows it, as `prusa-mk4`, or its id.

Check it with `sudo popctl config check` and restart POP.

### What a member is told

- **"Sign in with CS:S once, so POP knows which member you are."** Their
  POP account isn't linked to CS:S: it was made on the desk and has never
  signed in with CS:S.
- **"CS:S says you aren't trained on Prusa MK4 yet."** CS:S says they may
  not use the printer's tool.
- **"CS:S has no active member for your account."** CS:S doesn't know them,
  or they are deactivated there.
- **"This printer's training isn't set up in CS:S yet."** The printer's
  `css_tool` names a tool CS:S doesn't have. POP's log names it.
- **"POP couldn't check your training with CS:S just now."** CS:S didn't
  answer, or refused POP's own token. The member is refused: training is
  never assumed. POP's log says why, usually the client secret, or the
  scope missing in CS:S.
