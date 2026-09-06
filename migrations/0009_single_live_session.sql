UPDATE sessions
SET ended_at = MAX(
    started_at,
    CAST(strftime('%s', 'now') AS INTEGER) * 1000
)
WHERE ended_at IS NULL
  AND id <> (
      SELECT id
      FROM sessions
      WHERE ended_at IS NULL
      ORDER BY started_at DESC, id DESC
      LIMIT 1
  );

DROP INDEX one_active_session_per_deck;
DROP INDEX active_session_code;

CREATE UNIQUE INDEX one_live_session
    ON sessions ((1)) WHERE ended_at IS NULL;
