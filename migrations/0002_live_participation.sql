ALTER TABLE sessions
ADD COLUMN follow_revision INTEGER NOT NULL DEFAULT 0;

ALTER TABLE responses RENAME TO responses_before_ordering;

CREATE TABLE responses (
    id INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    slide_index INTEGER NOT NULL CHECK (slide_index >= 0),
    participant_hash TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('poll', 'wordcloud', 'quiz', 'ordering')),
    value TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (session_id, slide_index, participant_hash, value)
) STRICT;

INSERT INTO responses
    (id, session_id, slide_index, participant_hash, kind, value, created_at, updated_at)
SELECT id, session_id, slide_index, participant_hash, kind, value, created_at, updated_at
FROM responses_before_ordering;

DROP TABLE responses_before_ordering;

CREATE INDEX responses_for_slide
    ON responses(session_id, slide_index);
CREATE INDEX responses_for_participant
    ON responses(session_id, participant_hash);

ALTER TABLE reactions RENAME TO reactions_before_live_feed;

CREATE TABLE reactions (
    id INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    slide_index INTEGER NOT NULL CHECK (slide_index >= 0),
    participant_hash TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('applause', 'lightbulb', 'question')),
    created_at INTEGER NOT NULL
) STRICT;

INSERT INTO reactions
    (id, session_id, slide_index, participant_hash, kind, created_at)
SELECT id, session_id, slide_index, participant_hash, kind, created_at
FROM reactions_before_live_feed
WHERE kind IN ('applause', 'question');

DROP TABLE reactions_before_live_feed;

CREATE INDEX reactions_for_slide
    ON reactions(session_id, slide_index, kind);
CREATE INDEX recent_reactions
    ON reactions(session_id, participant_hash, created_at);

CREATE TABLE raised_hands (
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    participant_hash TEXT NOT NULL,
    raised_at INTEGER NOT NULL,
    PRIMARY KEY (session_id, participant_hash)
) WITHOUT ROWID, STRICT;
