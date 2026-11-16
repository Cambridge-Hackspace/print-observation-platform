# Group notices on Discord

POP posts group notices (a printer needs a hand, it is printing again) to one
Discord channel through a webhook, the way BAM! does. CS:S is not involved;
personal messages to one member go through CS:S instead (#70).

## Setting it up

1. In Discord, open the channel's settings, then **Integrations → Webhooks →
   New Webhook**, name it POP, and **Copy Webhook URL**.
2. Give POP that URL, from inside the space, as an admin:
   `PUT /api/admin/notify/discord` with `{"webhook_url": "<the URL>"}`. (The
   admin desk's settings page will do this, with a **Send a test message**
   button: `POST /api/admin/notify/discord/test`.)

POP needs its secrets key (`POP_SECRETS_KEY`) for this: the URL is a secret,
since anyone holding it can post to the channel. POP keeps it sealed, and
never shows it in full again, only its id and the last four characters of its
token. Saving, removing and testing it are in the audit log, without the URL.

## What POP posts

One plain sentence, naming the printer and the person. Nothing POP posts can
ping anyone: a name such as `@everyone` stays text.

A printer that needs a hand: "Ender 3 needs a hand: the filament ran out at
layer 112. Sam's print is paused." The print is "paused" only when the printer
is holding it to be resumed there (a Bambu printer pausing for an error); when
the printer has given up on it (a Klipper error or shutdown, a failed Bambu
print) it "stopped", so nobody is told a print is safe that is not. When it is
fixed: "Ender 3 is printing again."

The printer's screen at the kiosk shows the needs-a-hand notice as the group
chat saw it, with the time Discord took it, while the printer still needs a
hand. A notice Discord never took (no webhook, or POP gave up) is not shown;
the screen says POP hasn't posted it instead.

## When Discord does not answer

Notices go out one at a time, in order. A failure is tried again after a wait
that doubles from 2 seconds to 5 minutes, ten tries in all. When Discord says
it is rate limiting the webhook, POP waits as long as Discord asks. A webhook
Discord no longer has (it was deleted) is not tried again. Every failure, and
giving up, is in POP's log with the notice's text; a notice is never dropped
without a line saying so.
