-- Which guide fixes each error the catalog knows (#73), by the catalog
-- entry's stable key. A guide that is deleted takes its links with it.
CREATE TABLE error_guides (
    key      TEXT PRIMARY KEY NOT NULL,
    guide_id INTEGER NOT NULL REFERENCES guides (id) ON DELETE CASCADE
) STRICT;
