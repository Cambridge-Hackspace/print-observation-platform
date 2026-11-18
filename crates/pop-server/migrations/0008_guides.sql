-- Guides (#71): steps and yes/no flowcharts, stored in POP. The steps are one
-- JSON document per guide, checked by pop_server::guides before it is kept.
CREATE TABLE guides (
    id         INTEGER PRIMARY KEY,
    slug       TEXT NOT NULL UNIQUE,
    title      TEXT NOT NULL,
    status     TEXT NOT NULL CHECK (status IN ('draft', 'published')),
    -- {summary, start, steps}: see pop_server::api::GuideBody.
    body       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- A renamed guide's old slugs, so links to them keep working.
CREATE TABLE guide_slugs (
    slug     TEXT PRIMARY KEY NOT NULL,
    guide_id INTEGER NOT NULL REFERENCES guides (id) ON DELETE CASCADE
) STRICT;

-- A guide's images, each a file under the data directory's files/ (so a
-- backup takes them with the print files).
CREATE TABLE guide_images (
    id           TEXT PRIMARY KEY NOT NULL,
    guide_id     INTEGER NOT NULL REFERENCES guides (id) ON DELETE CASCADE,
    content_type TEXT NOT NULL,
    bytes        INTEGER NOT NULL,
    created_at   INTEGER NOT NULL
) STRICT;
