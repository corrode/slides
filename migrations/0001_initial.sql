PRAGMA foreign_keys = ON;

CREATE TABLE decks (
    id INTEGER PRIMARY KEY,
    slug TEXT NOT NULL COLLATE NOCASE UNIQUE,
    title TEXT NOT NULL,
    draft_source TEXT NOT NULL,
    theme_font TEXT NOT NULL DEFAULT 'system',
    theme_background TEXT NOT NULL DEFAULT '#10141b',
    theme_text TEXT NOT NULL DEFAULT '#f4f1e8',
    theme_accent TEXT NOT NULL DEFAULT '#e0ad52',
    show_join_code INTEGER NOT NULL DEFAULT 1 CHECK (show_join_code IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE deck_versions (
    id INTEGER PRIMARY KEY,
    deck_id INTEGER NOT NULL REFERENCES decks(id) ON DELETE CASCADE,
    version_number INTEGER NOT NULL,
    title TEXT NOT NULL,
    source TEXT NOT NULL,
    theme_font TEXT NOT NULL,
    theme_background TEXT NOT NULL,
    theme_text TEXT NOT NULL,
    theme_accent TEXT NOT NULL,
    show_join_code INTEGER NOT NULL CHECK (show_join_code IN (0, 1)),
    published_at INTEGER NOT NULL,
    UNIQUE (deck_id, version_number)
) STRICT;

CREATE TABLE sessions (
    id INTEGER PRIMARY KEY,
    deck_id INTEGER NOT NULL REFERENCES decks(id) ON DELETE CASCADE,
    deck_version_id INTEGER NOT NULL REFERENCES deck_versions(id),
    code TEXT NOT NULL UNIQUE CHECK (
        length(code) = 6 AND code GLOB '[0-9][0-9][0-9][0-9][0-9][0-9]'
    ),
    current_slide INTEGER NOT NULL DEFAULT 0 CHECK (current_slide >= 0),
    locked INTEGER NOT NULL DEFAULT 1 CHECK (locked IN (0, 1)),
    interaction_open INTEGER NOT NULL DEFAULT 1 CHECK (interaction_open IN (0, 1)),
    results_revealed INTEGER NOT NULL DEFAULT 0 CHECK (results_revealed IN (0, 1)),
    revision INTEGER NOT NULL DEFAULT 0,
    started_at INTEGER NOT NULL,
    ended_at INTEGER
) STRICT;

CREATE UNIQUE INDEX one_active_session_per_deck
    ON sessions(deck_id) WHERE ended_at IS NULL;
CREATE UNIQUE INDEX active_session_code
    ON sessions(code) WHERE ended_at IS NULL;

CREATE TABLE responses (
    id INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    slide_index INTEGER NOT NULL CHECK (slide_index >= 0),
    participant_hash TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('poll', 'wordcloud', 'quiz')),
    value TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (session_id, slide_index, participant_hash, value)
) STRICT;

CREATE INDEX responses_for_slide
    ON responses(session_id, slide_index);
CREATE INDEX responses_for_participant
    ON responses(session_id, participant_hash);

CREATE TABLE reactions (
    id INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    slide_index INTEGER NOT NULL CHECK (slide_index >= 0),
    participant_hash TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('heart', 'thumbs-up', 'applause', 'laugh', 'question')),
    created_at INTEGER NOT NULL
) STRICT;

CREATE INDEX reactions_for_slide
    ON reactions(session_id, slide_index, kind);
CREATE INDEX recent_reactions
    ON reactions(session_id, participant_hash, created_at);
