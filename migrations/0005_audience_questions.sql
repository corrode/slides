CREATE TABLE questions (
    id INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    participant_hash TEXT NOT NULL,
    body TEXT NOT NULL CHECK (length(trim(body)) > 0),
    created_at INTEGER NOT NULL,
    answered INTEGER NOT NULL DEFAULT 0 CHECK (answered IN (0, 1)),
    answered_at INTEGER,
    hidden INTEGER NOT NULL DEFAULT 0 CHECK (hidden IN (0, 1)),
    hidden_at INTEGER,
    CHECK (
        (answered = 0 AND answered_at IS NULL)
        OR (answered = 1 AND answered_at IS NOT NULL)
    ),
    CHECK (
        (hidden = 0 AND hidden_at IS NULL)
        OR (hidden = 1 AND hidden_at IS NOT NULL)
    )
) STRICT;

CREATE INDEX questions_for_session
    ON questions(session_id, hidden, answered, created_at);
CREATE INDEX questions_for_participant
    ON questions(session_id, participant_hash);

CREATE TABLE question_votes (
    question_id INTEGER NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    participant_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (question_id, participant_hash)
) WITHOUT ROWID, STRICT;
