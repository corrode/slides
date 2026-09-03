CREATE TABLE api_token (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    token_hash TEXT NOT NULL CHECK (
        length(token_hash) = 64
        AND token_hash NOT GLOB '*[^0-9a-f]*'
    ),
    prefix TEXT NOT NULL CHECK (length(trim(prefix)) > 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
) STRICT;
