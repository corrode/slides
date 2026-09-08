CREATE TABLE presentation_bundles (
    generation TEXT PRIMARY KEY NOT NULL,
    deck_id INTEGER NOT NULL REFERENCES decks(id) ON DELETE CASCADE
) STRICT;

CREATE TABLE bundle_drafts (
    deck_id INTEGER PRIMARY KEY REFERENCES decks(id) ON DELETE CASCADE,
    generation TEXT NOT NULL REFERENCES presentation_bundles(generation)
) STRICT;
