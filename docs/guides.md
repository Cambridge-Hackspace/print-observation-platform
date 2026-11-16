# Guides

Guides live in POP (ADR 0001): steps and yes/no flowcharts, with images, in one
format that the kiosk, the phone and the public pages all read.

## The format

```json
{
  "slug": "stringing-on-petg",
  "title": "Stringing on PETG",
  "status": "draft",
  "summary": "Fine hairs between the parts of a print.",
  "start": "hairs",
  "steps": [
    {"id": "hairs", "title": "Are there fine hairs between parts?",
     "then": {"kind": "question", "yes": "dry-box", "no": "blobs"}},
    {"id": "blobs", "title": "Those are blobs, not strings: see the blobs guide.",
     "then": {"kind": "done"}},
    {"id": "dry-box", "title": "Has this spool been out of the dry box for more than a day?",
     "then": {"kind": "question", "yes": "dry-it", "no": "cooler"}},
    {"id": "dry-it", "title": "Dry it for 4 hours at 65 °C.", "image": "<an image id>",
     "then": {"kind": "next", "step": "test-tower"}},
    {"id": "cooler", "title": "Print with the nozzle 5 °C cooler.",
     "then": {"kind": "next", "step": "test-tower"}},
    {"id": "test-tower", "title": "Print the test tower.", "then": {"kind": "done"}}
  ]
}
```

Each step has an id, a title (what to do, or the question), optional `body`
text and an optional image, and says where it leads: `done` (the end, so the
step says what to do), `next`, or a yes/no `question`.

POP keeps a guide only if:

- every `next`, `yes` and `no` leads to a step that exists, and `start` is one;
- every step can be reached from the start;
- from every step, a `done` step can be reached. A loop back ("still
  stringing? start again") is fine; a loop with no way out is not;
- step ids are unique, and every step has a title;
- each image is one of this guide's own.

A guide that breaks any of these is refused with every problem, each naming its
step.

## Slugs

The slug is the guide's address, for links: lowercase words joined by hyphens.
Renaming a guide keeps its old slug, which leads to the guide as it is now, and
no other guide can take it.

## Draft or published

Only published guides are shown to members and to the public (#94).

## Images

PNG, JPEG or WebP, up to 5 MB, sent as base64 in JSON like every other request
that changes something. They are kept under the data directory's
`files/guide-images/`, so `pop backup` takes them with the print files.

## The admin API

| | |
|---|---|
| `GET /api/admin/guides` | every guide, drafts too |
| `POST /api/admin/guides` | a new guide |
| `GET /api/admin/guides/{slug}` | one guide |
| `PUT /api/admin/guides/{slug}` | replace it, perhaps renaming or publishing it |
| `POST /api/admin/guides/{slug}/images` | `{content_type, data_base64}`: an image, by id |
| `GET /api/admin/guides/{slug}/images/{id}` | an image |

Admins only, from anywhere: nothing here reaches a printer. Writing, editing,
renaming, publishing and adding an image are in the audit log.

## For everyone

Published guides can be read by anyone, without signing in, from anywhere, so
CS:S, Discord messages and anyone else can link to them:

| | |
|---|---|
| `GET /api/public/guides` | the published guides, by title |
| `GET /api/public/guides/{slug}` | one published guide |
| `GET /api/public/guides/{slug}/images/{id}` | a published guide's image |

A draft is not there at all: its slug, its old slugs and its images all answer
`404`. An old slug of a published guide redirects (`308`) to its slug now.
Every answer carries an `ETag`, and a request with `If-None-Match` gets `304`
while nothing has changed, so CS:S or a proxy can ask often and cheaply.

These routes only read. The route table holds every route a stranger can reach
to changing nothing but their own sign-in (#7), and a test tries each other
method on them and finds the guide unchanged.

Two settings, in `[guides]`:

- `license`: shown on each guide (`"CC BY-SA 4.0"`); empty shows none.
- `indexable`: whether search engines may index the guides. Off by default:
  every public answer says `X-Robots-Tag: noindex`.

The public pages at `/guides` and `/guides/{slug}` come with the guide viewers
(#56, #72), which render the same walk-through.

## Errors link to the right guide

Each error the catalog knows (#35) has a stable key, such as
`klipper-must-home`, and a printer's error carries the key of the entry that
explained it. An admin links a guide to a key:

| | |
|---|---|
| `GET /api/admin/error-guides` | every error the catalog knows, its source, and its guide |
| `PUT /api/admin/error-guides/{key}` | `{"guide": "<slug>"}` to link one, `{"guide": null}` to unlink |

`GET /api/public/error-guides` gives each key's guide, `{"klipper-must-home":
"homing"}`, for "Show me how" on the needs-a-hand screen. Only published guides
are in it: a link to a draft is never shown. A link follows its guide through a
rename.

On that screen (#55), the guide's first steps are numbered, from its start for
as long as each leads straight to the next (up to five). A guide that opens
with a yes/no question, or goes on past that, shows what it can and offers
"The rest of the guide". So write a guide's opening as plain steps where you
can: they are what someone at the kiosk reads first. The list always ends at
the printer, which POP never controls: "Press Resume on the printer's screen"
when the printer is holding the print, or "Clear the error on the printer's
screen" when the print has stopped.
