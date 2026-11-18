# Feedback as GitHub issues

Whoever runs the print stations can point at something on any screen, draw on
it, say what is wrong, and have it arrive as an issue on POP's repository
(#96). POP files it through a GitHub App; nothing goes through a third party.

**The repository is public, so every report is.** The issue, its
screenshot and its context can be read by anyone. The screen redacts
members' details before the screenshot is taken, and the person filing sees
the image, and can black out anything else, before sending.

## Setting it up

1. Create a GitHub App (Settings, Developer settings, GitHub Apps) with
   **Repository permissions: Issues: Read and write, Contents: Read and
   write**, and nothing else. No webhook is needed.
2. Install it on the repository only, and note the installation's id (the
   number at the end of the installation's settings page).
3. Generate a private key, and put the file where only POP's user can read
   it.
4. Create the labels `feedback`, `bug`, `enhancement` and `question` on the
   repository, if it lacks them.
5. Tell POP:

   ```toml
   [feedback]
   app_id = "<the App's id>"
   installation_id = "<the installation's id>"
   private_key_file = "/etc/pop/github-app.pem"
   repo = "Cambridge-Hackspace/print-observation-platform"
   roles = ["admin"]
   per_hour = 10
   ```

   The key never leaves the server, and never reaches a browser.

Without `app_id` there is no feedback button anywhere, and the endpoint
answers 404.

## Who can file

Anyone whose role is in `roles` holds `feedback.file`: admins only, unless
said otherwise. Everyone else sees no button, and the endpoint answers 403.
One person may file `per_hour` reports an hour.

## What POP does with a report

1. Keeps it, and answers at once. The screen shows it sent only once the
   issue exists.
2. Puts the screenshot on the `feedback-attachments` branch, made from the
   default branch the first time, as `feedback/<report>.png`. GitHub has no
   supported API for the uploads a pasted image uses.
3. Files the issue: the title as written, the description, the screenshot,
   "Reported by {name} from the {kiosk|phone|desk}", and the context in a
   collapsed block. Labels: `feedback`, and `bug`, `enhancement` or
   `question`.
4. Records it in the audit log, with the issue's address.

If GitHub or the network is down, the report stays kept and is tried again,
from 30 seconds apart up to 30 minutes, until it goes. A report retried after
its picture went up uses the picture already there. If GitHub refuses the
App (a wrong id, installation or key, or a missing permission), the report is
not tried again; the log says why.

## What is taken out

- **The screenshot** must be a PNG, at most 4 MB, and loses every chunk
  that does not draw it: text, times, EXIF.
- **The context** (route, viewport, recent console errors and failed
  requests) is scrubbed again on the server, whatever the screen did: any
  value under a key naming a token, cookie, password, secret, session,
  authorization, API key or access code; GitHub tokens, JWTs, session
  cookies and long key-like strings wherever they appear; whatever follows
  `Bearer` or `Basic`; and every query string.
- **The printers' states** are added by POP itself, with nobody named in
  them.

What is not proven: that GitHub itself takes the writes as its
documentation says. POP's fake GitHub follows the documentation, and the
reads were checked against GitHub's own answers (`crates/pop-integrations/src/github.rs`).
