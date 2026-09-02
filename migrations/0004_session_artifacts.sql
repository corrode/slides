CREATE TABLE session_artifacts (
    session_id INTEGER PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    share_token TEXT NOT NULL COLLATE BINARY UNIQUE CHECK (length(share_token) = 64),
    format_version INTEGER NOT NULL DEFAULT 1 CHECK (format_version = 1),
    archive BLOB NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;
