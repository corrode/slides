use std::{
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, bail};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

use crate::models::{
    ApiTokenSummary, Deck, DeckSummary, DeckVersion, EndedSessionSummary, LiveSession, Theme,
    legacy_font_id,
};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValueCount {
    pub value: String,
    pub count: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WordCloudResponse {
    pub value: String,
    pub participant_hash: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReactionCount {
    pub kind: String,
    pub count: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SessionArtifact {
    pub share_token: String,
    pub archive: Vec<u8>,
    pub code: String,
    pub title: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct QuestionRow {
    pub(crate) id: i64,
    pub(crate) body: String,
    pub(crate) answered: bool,
    pub(crate) vote_count: i64,
    pub(crate) participant_upvoted: bool,
}

pub async fn connect(database_url: &str) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}

pub async fn healthcheck(pool: &SqlitePool) -> Result<()> {
    sqlx::query("SELECT 1").execute(pool).await?;
    Ok(())
}

pub async fn api_token(pool: &SqlitePool) -> Result<Option<ApiTokenSummary>> {
    Ok(sqlx::query_as::<_, ApiTokenSummary>(
        "SELECT prefix, created_at FROM api_token WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?)
}

pub async fn replace_api_token(
    pool: &SqlitePool,
    token_hash: &str,
    prefix: &str,
) -> Result<ApiTokenSummary> {
    Ok(sqlx::query_as::<_, ApiTokenSummary>(
        r#"INSERT INTO api_token (id, token_hash, prefix, created_at)
           VALUES (1, ?, ?, ?)
           ON CONFLICT(id) DO UPDATE SET
               token_hash = excluded.token_hash,
               prefix = excluded.prefix,
               created_at = excluded.created_at
           RETURNING prefix, created_at"#,
    )
    .bind(token_hash)
    .bind(prefix)
    .bind(now_millis())
    .fetch_one(pool)
    .await?)
}

pub async fn api_token_matches(pool: &SqlitePool, token_hash: &str) -> Result<bool> {
    let matches: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM api_token WHERE id = 1 AND token_hash = ?)",
    )
    .bind(token_hash)
    .fetch_one(pool)
    .await?;
    Ok(matches != 0)
}

pub async fn revoke_api_token(pool: &SqlitePool) -> Result<bool> {
    let deleted = sqlx::query("DELETE FROM api_token WHERE id = 1")
        .execute(pool)
        .await?;
    Ok(deleted.rows_affected() > 0)
}

pub async fn list_decks(pool: &SqlitePool) -> Result<Vec<DeckSummary>> {
    Ok(sqlx::query_as::<_, DeckSummary>(
        r#"
        SELECT d.slug, d.title,
               (SELECT COUNT(*) FROM deck_versions v WHERE v.deck_id = d.id) AS published_versions,
               (SELECT code FROM sessions s WHERE s.deck_id = d.id AND s.ended_at IS NULL) AS active_code
        FROM decks d
        ORDER BY d.updated_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?)
}

pub async fn list_ended_sessions(pool: &SqlitePool) -> Result<Vec<EndedSessionSummary>> {
    Ok(sqlx::query_as::<_, EndedSessionSummary>(
        r#"SELECT v.title, s.code, a.share_token
           FROM sessions s
           JOIN deck_versions v ON v.id = s.deck_version_id
           LEFT JOIN session_artifacts a ON a.session_id = s.id
           WHERE s.ended_at IS NOT NULL
           ORDER BY s.ended_at DESC"#,
    )
    .fetch_all(pool)
    .await?)
}

pub async fn create_deck(pool: &SqlitePool, slug: &str, title: &str) -> Result<Deck> {
    let source = format!(
        "# {title}\n\nYour presentation starts here.\n\n---\n\n# Ask the audience\n\n:::poll question=\"Which option do you prefer?\"\n- The first option\n- The second option\n:::"
    );
    create_deck_with_content(pool, slug, title, &source, &Theme::default()).await
}

pub async fn create_deck_with_content(
    pool: &SqlitePool,
    slug: &str,
    title: &str,
    source: &str,
    theme: &Theme,
) -> Result<Deck> {
    let now = now_millis();
    Ok(sqlx::query_as::<_, Deck>(
        r#"INSERT INTO decks
           (slug, title, draft_source, theme_font, theme_headline_font, theme_text_font,
            theme_code_font, theme_background, theme_text, theme_accent, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           RETURNING id, slug, title, draft_source, theme_headline_font, theme_text_font,
                     theme_code_font, theme_background, theme_text, theme_accent"#,
    )
    .bind(slug)
    .bind(title)
    .bind(source)
    .bind(legacy_font_id(&theme.headline_font))
    .bind(&theme.headline_font)
    .bind(&theme.text_font)
    .bind(&theme.code_font)
    .bind(&theme.background)
    .bind(&theme.text)
    .bind(&theme.accent)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await?)
}

#[cfg(test)]
async fn get_deck(pool: &SqlitePool, id: i64) -> Result<Deck> {
    Ok(sqlx::query_as::<_, Deck>(
        r#"SELECT id, slug, title, draft_source, theme_headline_font, theme_text_font,
                  theme_code_font, theme_background, theme_text, theme_accent
           FROM decks WHERE id = ?"#,
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

pub async fn deck_by_slug(pool: &SqlitePool, slug: &str) -> Result<Option<Deck>> {
    Ok(sqlx::query_as::<_, Deck>(
        r#"SELECT id, slug, title, draft_source, theme_headline_font, theme_text_font,
                  theme_code_font, theme_background, theme_text, theme_accent
           FROM decks WHERE slug = ?"#,
    )
    .bind(slug)
    .fetch_optional(pool)
    .await?)
}

pub async fn save_deck(
    pool: &SqlitePool,
    id: i64,
    title: &str,
    source: &str,
    theme: &Theme,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE decks
           SET title = ?, draft_source = ?, theme_font = ?, theme_headline_font = ?,
               theme_text_font = ?, theme_code_font = ?, theme_background = ?,
               theme_text = ?, theme_accent = ?, updated_at = ?
           WHERE id = ?"#,
    )
    .bind(title)
    .bind(source)
    .bind(legacy_font_id(&theme.headline_font))
    .bind(&theme.headline_font)
    .bind(&theme.text_font)
    .bind(&theme.code_font)
    .bind(&theme.background)
    .bind(&theme.text)
    .bind(&theme.accent)
    .bind(now_millis())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn save_and_publish_deck(
    pool: &SqlitePool,
    deck_id: i64,
    title: &str,
    draft_source: &str,
    published_source: &str,
    theme: &Theme,
) -> Result<i64> {
    let now = now_millis();
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"UPDATE decks
           SET title = ?, draft_source = ?, theme_font = ?, theme_headline_font = ?,
               theme_text_font = ?, theme_code_font = ?, theme_background = ?,
               theme_text = ?, theme_accent = ?, updated_at = ?
           WHERE id = ?"#,
    )
    .bind(title)
    .bind(draft_source)
    .bind(legacy_font_id(&theme.headline_font))
    .bind(&theme.headline_font)
    .bind(&theme.text_font)
    .bind(&theme.code_font)
    .bind(&theme.background)
    .bind(&theme.text)
    .bind(&theme.accent)
    .bind(now)
    .bind(deck_id)
    .execute(&mut *tx)
    .await?;

    let version_number: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version_number), 0) + 1 FROM deck_versions WHERE deck_id = ?",
    )
    .bind(deck_id)
    .fetch_one(&mut *tx)
    .await?;
    let id = sqlx::query(
        r#"INSERT INTO deck_versions
           (deck_id, version_number, title, source, theme_font, theme_headline_font,
            theme_text_font, theme_code_font, theme_background, theme_text, theme_accent,
            show_join_code, published_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?)"#,
    )
    .bind(deck_id)
    .bind(version_number)
    .bind(title)
    .bind(published_source)
    .bind(legacy_font_id(&theme.headline_font))
    .bind(&theme.headline_font)
    .bind(&theme.text_font)
    .bind(&theme.code_font)
    .bind(&theme.background)
    .bind(&theme.text)
    .bind(&theme.accent)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .last_insert_rowid();
    tx.commit().await?;
    Ok(id)
}

pub async fn get_version(pool: &SqlitePool, id: i64) -> Result<DeckVersion> {
    Ok(sqlx::query_as::<_, DeckVersion>(
        r#"SELECT title, source, theme_headline_font, theme_text_font, theme_code_font,
                  theme_background, theme_text, theme_accent
           FROM deck_versions WHERE id = ?"#,
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

pub async fn start_session(
    pool: &SqlitePool,
    deck_id: i64,
    version_id: i64,
) -> Result<LiveSession> {
    if let Some(session) = active_session_for_deck(pool, deck_id).await? {
        return Ok(session);
    }

    for _ in 0..20 {
        let code = format!("{:06}", 100_000 + rand::random::<u32>() % 900_000);
        let result = sqlx::query(
            r#"INSERT INTO sessions (deck_id, deck_version_id, code, started_at)
               VALUES (?, ?, ?, ?)"#,
        )
        .bind(deck_id)
        .bind(version_id)
        .bind(&code)
        .bind(now_millis())
        .execute(pool)
        .await;

        match result {
            Ok(result) => return get_session(pool, result.last_insert_rowid()).await,
            Err(error)
                if error
                    .as_database_error()
                    .is_some_and(|e| e.is_unique_violation()) =>
            {
                let message = error
                    .as_database_error()
                    .map(|error| error.message())
                    .unwrap_or_default();
                if message.contains("sessions.deck_id") {
                    return active_session_for_deck(pool, deck_id)
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("active session was created concurrently"));
                }
                if message.contains("sessions.code") {
                    continue;
                }
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("could not allocate a session code")
}

pub async fn active_session_for_deck(
    pool: &SqlitePool,
    deck_id: i64,
) -> Result<Option<LiveSession>> {
    Ok(sqlx::query_as::<_, LiveSession>(
        r#"SELECT id, deck_version_id, code, current_slide, locked,
                  interaction_open, results_revealed, follow_revision, ended_at
           FROM sessions WHERE deck_id = ? AND ended_at IS NULL"#,
    )
    .bind(deck_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn active_session_for_slug(pool: &SqlitePool, slug: &str) -> Result<Option<LiveSession>> {
    Ok(sqlx::query_as::<_, LiveSession>(
        r#"SELECT s.id, s.deck_version_id, s.code, s.current_slide, s.locked,
                  s.interaction_open, s.results_revealed, s.follow_revision, s.ended_at
           FROM sessions s JOIN decks d ON d.id = s.deck_id
           WHERE d.slug = ? AND s.ended_at IS NULL"#,
    )
    .bind(slug)
    .fetch_optional(pool)
    .await?)
}

pub async fn session_by_code(pool: &SqlitePool, code: &str) -> Result<Option<LiveSession>> {
    Ok(sqlx::query_as::<_, LiveSession>(
        r#"SELECT id, deck_version_id, code, current_slide, locked,
                  interaction_open, results_revealed, follow_revision, ended_at
           FROM sessions WHERE code = ?"#,
    )
    .bind(code)
    .fetch_optional(pool)
    .await?)
}

pub async fn get_session(pool: &SqlitePool, id: i64) -> Result<LiveSession> {
    Ok(sqlx::query_as::<_, LiveSession>(
        r#"SELECT id, deck_version_id, code, current_slide, locked,
                  interaction_open, results_revealed, follow_revision, ended_at
           FROM sessions WHERE id = ?"#,
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

pub async fn deck_slug_for_session(pool: &SqlitePool, session_id: i64) -> Result<String> {
    Ok(sqlx::query_scalar(
        r#"SELECT d.slug
           FROM decks d
           JOIN sessions s ON s.deck_id = d.id
           WHERE s.id = ?"#,
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?)
}

pub async fn delete_deck(pool: &SqlitePool, deck_id: i64) -> Result<bool> {
    let deleted = sqlx::query(
        r#"DELETE FROM decks
           WHERE id = ?
             AND NOT EXISTS (
                 SELECT 1 FROM sessions
                 WHERE deck_id = ? AND ended_at IS NULL
             )"#,
    )
    .bind(deck_id)
    .bind(deck_id)
    .execute(pool)
    .await?;
    Ok(deleted.rows_affected() > 0)
}

pub async fn delete_ended_session(pool: &SqlitePool, session_id: i64) -> Result<bool> {
    let deleted = sqlx::query("DELETE FROM sessions WHERE id = ? AND ended_at IS NOT NULL")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(deleted.rows_affected() > 0)
}

pub async fn move_to_slide(pool: &SqlitePool, id: i64, slide: usize) -> Result<()> {
    sqlx::query(
        r#"UPDATE sessions SET current_slide = ?, interaction_open = 1,
                  results_revealed = 0, follow_revision = follow_revision + 1
           WHERE id = ? AND ended_at IS NULL"#,
    )
    .bind(slide as i64)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn focus_audience(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query(
        "UPDATE sessions SET follow_revision = follow_revision + 1 WHERE id = ? AND ended_at IS NULL",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_lock(pool: &SqlitePool, id: i64, locked: bool) -> Result<()> {
    sqlx::query("UPDATE sessions SET locked = ? WHERE id = ? AND ended_at IS NULL")
        .bind(locked)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_interaction_state(
    pool: &SqlitePool,
    id: i64,
    open: bool,
    revealed: bool,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE sessions SET interaction_open = ?, results_revealed = ?
           WHERE id = ? AND ended_at IS NULL"#,
    )
    .bind(open)
    .bind(revealed)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn session_started_at(pool: &SqlitePool, id: i64) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT started_at FROM sessions WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

pub(crate) async fn create_question(
    pool: &SqlitePool,
    session_id: i64,
    participant_hash: &str,
    body: &str,
) -> Result<QuestionRow> {
    Ok(sqlx::query_as::<_, QuestionRow>(
        r#"INSERT INTO questions (session_id, participant_hash, body, created_at)
           VALUES (?, ?, ?, ?)
           RETURNING id, body, answered, 0 AS vote_count, 0 AS participant_upvoted"#,
    )
    .bind(session_id)
    .bind(participant_hash)
    .bind(body)
    .bind(now_millis())
    .fetch_one(pool)
    .await?)
}

pub(crate) async fn participant_question_count(
    pool: &SqlitePool,
    session_id: i64,
    participant_hash: &str,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM questions WHERE session_id = ? AND participant_hash = ?",
    )
    .bind(session_id)
    .bind(participant_hash)
    .fetch_one(pool)
    .await?)
}

pub(crate) async fn list_visible_questions(
    pool: &SqlitePool,
    session_id: i64,
    participant_hash: &str,
) -> Result<Vec<QuestionRow>> {
    Ok(sqlx::query_as::<_, QuestionRow>(
        r#"SELECT q.id, q.body, q.answered, COUNT(v.question_id) AS vote_count,
                  EXISTS (
                      SELECT 1 FROM question_votes participant_vote
                      WHERE participant_vote.question_id = q.id
                        AND participant_vote.participant_hash = ?
                  ) AS participant_upvoted
           FROM questions q
           LEFT JOIN question_votes v ON v.question_id = q.id
           WHERE q.session_id = ? AND q.hidden = 0
           GROUP BY q.id
           ORDER BY q.answered, vote_count DESC, q.created_at, q.id"#,
    )
    .bind(participant_hash)
    .bind(session_id)
    .fetch_all(pool)
    .await?)
}

pub(crate) async fn toggle_question_upvote(
    pool: &SqlitePool,
    session_id: i64,
    question_id: i64,
    participant_hash: &str,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let removed = sqlx::query(
        r#"DELETE FROM question_votes
           WHERE question_id = ? AND participant_hash = ?
             AND EXISTS (
                 SELECT 1 FROM questions q
                 WHERE q.id = question_votes.question_id AND q.session_id = ?
             )"#,
    )
    .bind(question_id)
    .bind(participant_hash)
    .bind(session_id)
    .execute(&mut *tx)
    .await?;
    if removed.rows_affected() > 0 {
        tx.commit().await?;
        return Ok(false);
    }

    let inserted = sqlx::query(
        r#"INSERT INTO question_votes (question_id, participant_hash, created_at)
           SELECT q.id, ?, ? FROM questions q
           WHERE q.id = ? AND q.session_id = ?"#,
    )
    .bind(participant_hash)
    .bind(now_millis())
    .bind(question_id)
    .bind(session_id)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() == 0 {
        bail!("question does not belong to session");
    }
    tx.commit().await?;
    Ok(true)
}

pub(crate) async fn set_question_answered(
    pool: &SqlitePool,
    session_id: i64,
    question_id: i64,
    answered: bool,
) -> Result<bool> {
    let updated = sqlx::query(
        r#"UPDATE questions
           SET answered = ?,
               answered_at = CASE WHEN ? THEN COALESCE(answered_at, ?) ELSE NULL END
           WHERE id = ? AND session_id = ?"#,
    )
    .bind(answered)
    .bind(answered)
    .bind(now_millis())
    .bind(question_id)
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() > 0)
}

pub(crate) async fn dismiss_question(
    pool: &SqlitePool,
    session_id: i64,
    question_id: i64,
) -> Result<bool> {
    let updated = sqlx::query(
        r#"UPDATE questions SET hidden = 1, hidden_at = COALESCE(hidden_at, ?)
           WHERE id = ? AND session_id = ?"#,
    )
    .bind(now_millis())
    .bind(question_id)
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() > 0)
}

pub async fn artifact_for_session(
    pool: &SqlitePool,
    session_id: i64,
) -> Result<Option<SessionArtifact>> {
    Ok(sqlx::query_as::<_, SessionArtifact>(
        r#"SELECT a.share_token, a.archive, s.code, v.title
           FROM session_artifacts a
           JOIN sessions s ON s.id = a.session_id
           JOIN deck_versions v ON v.id = s.deck_version_id
           WHERE a.session_id = ?"#,
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn artifact_by_token(pool: &SqlitePool, token: &str) -> Result<Option<SessionArtifact>> {
    Ok(sqlx::query_as::<_, SessionArtifact>(
        r#"SELECT a.share_token, a.archive, s.code, v.title
           FROM session_artifacts a
           JOIN sessions s ON s.id = a.session_id
           JOIN deck_versions v ON v.id = s.deck_version_id
           WHERE a.share_token = ?"#,
    )
    .bind(token)
    .fetch_optional(pool)
    .await?)
}

pub async fn finish_session_with_artifact(
    pool: &SqlitePool,
    session_id: i64,
    ended_at: i64,
    share_token: &str,
    archive: &[u8],
) -> Result<String> {
    let mut tx = pool.begin().await?;
    if let Some(existing) = sqlx::query_scalar::<_, String>(
        "SELECT share_token FROM session_artifacts WHERE session_id = ?",
    )
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        tx.commit().await?;
        return Ok(existing);
    }

    let updated = sqlx::query("UPDATE sessions SET ended_at = COALESCE(ended_at, ?) WHERE id = ?")
        .bind(ended_at)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
    if updated.rows_affected() == 0 {
        bail!("session does not exist");
    }
    sqlx::query(
        r#"INSERT INTO session_artifacts
           (session_id, share_token, format_version, archive, created_at)
           VALUES (?, ?, 1, ?, ?)"#,
    )
    .bind(session_id)
    .bind(share_token)
    .bind(archive)
    .bind(now_millis())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(share_token.to_owned())
}

pub async fn replace_answer(
    pool: &SqlitePool,
    session_id: i64,
    slide_index: usize,
    participant_hash: &str,
    kind: &str,
    value: &str,
) -> Result<()> {
    let now = now_millis();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM responses WHERE session_id = ? AND slide_index = ? AND participant_hash = ?",
    )
    .bind(session_id)
    .bind(slide_index as i64)
    .bind(participant_hash)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"INSERT INTO responses
           (session_id, slide_index, participant_hash, kind, value, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(session_id)
    .bind(slide_index as i64)
    .bind(participant_hash)
    .bind(kind)
    .bind(value)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn toggle_answer(
    pool: &SqlitePool,
    session_id: i64,
    slide_index: usize,
    participant_hash: &str,
    kind: &str,
    value: &str,
) -> Result<()> {
    let existing: Option<i64> = sqlx::query_scalar(
        r#"SELECT id FROM responses
           WHERE session_id = ? AND slide_index = ? AND participant_hash = ? AND value = ?"#,
    )
    .bind(session_id)
    .bind(slide_index as i64)
    .bind(participant_hash)
    .bind(value)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = existing {
        sqlx::query("DELETE FROM responses WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
    } else {
        let now = now_millis();
        sqlx::query(
            r#"INSERT INTO responses
               (session_id, slide_index, participant_hash, kind, value, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(session_id)
        .bind(slide_index as i64)
        .bind(participant_hash)
        .bind(kind)
        .bind(value)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn selected_values(
    pool: &SqlitePool,
    session_id: i64,
    slide_index: usize,
    participant_hash: &str,
) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT value FROM responses WHERE session_id = ? AND slide_index = ? AND participant_hash = ?",
    )
    .bind(session_id)
    .bind(slide_index as i64)
    .bind(participant_hash)
    .fetch_all(pool)
    .await?)
}

pub async fn value_counts(
    pool: &SqlitePool,
    session_id: i64,
    slide_index: usize,
) -> Result<Vec<ValueCount>> {
    Ok(sqlx::query_as::<_, ValueCount>(
        r#"SELECT value, COUNT(*) AS count FROM responses
           WHERE session_id = ? AND slide_index = ? GROUP BY value ORDER BY count DESC, value"#,
    )
    .bind(session_id)
    .bind(slide_index as i64)
    .fetch_all(pool)
    .await?)
}

pub async fn word_cloud_responses(
    pool: &SqlitePool,
    session_id: i64,
    slide_index: usize,
) -> Result<Vec<WordCloudResponse>> {
    Ok(sqlx::query_as::<_, WordCloudResponse>(
        r#"SELECT value, participant_hash FROM responses
           WHERE session_id = ? AND slide_index = ? AND kind = 'wordcloud'
           ORDER BY created_at, id"#,
    )
    .bind(session_id)
    .bind(slide_index as i64)
    .fetch_all(pool)
    .await?)
}

pub async fn answerer_count(pool: &SqlitePool, session_id: i64, slide_index: usize) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(DISTINCT participant_hash) FROM responses WHERE session_id = ? AND slide_index = ?",
    )
    .bind(session_id)
    .bind(slide_index as i64)
    .fetch_one(pool)
    .await?)
}

pub async fn ordering_values(
    pool: &SqlitePool,
    session_id: i64,
    slide_index: usize,
) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT value FROM responses WHERE session_id = ? AND slide_index = ? AND kind = 'ordering'",
    )
    .bind(session_id)
    .bind(slide_index as i64)
    .fetch_all(pool)
    .await?)
}

pub async fn add_reaction(
    pool: &SqlitePool,
    session_id: i64,
    slide_index: usize,
    participant_hash: &str,
    kind: &str,
) -> Result<bool> {
    let cutoff = now_millis() - 2_000;
    let recent: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reactions WHERE session_id = ? AND participant_hash = ? AND created_at >= ?",
    )
    .bind(session_id)
    .bind(participant_hash)
    .bind(cutoff)
    .fetch_one(pool)
    .await?;
    if recent >= 6 {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO reactions (session_id, slide_index, participant_hash, kind, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(session_id)
    .bind(slide_index as i64)
    .bind(participant_hash)
    .bind(kind)
    .bind(now_millis())
    .execute(pool)
    .await?;
    Ok(true)
}

pub async fn toggle_hand(
    pool: &SqlitePool,
    session_id: i64,
    participant_hash: &str,
) -> Result<bool> {
    let removed =
        sqlx::query("DELETE FROM raised_hands WHERE session_id = ? AND participant_hash = ?")
            .bind(session_id)
            .bind(participant_hash)
            .execute(pool)
            .await?;
    if removed.rows_affected() > 0 {
        return Ok(false);
    }

    sqlx::query(
        "INSERT INTO raised_hands (session_id, participant_hash, raised_at) VALUES (?, ?, ?)",
    )
    .bind(session_id)
    .bind(participant_hash)
    .bind(now_millis())
    .execute(pool)
    .await?;
    Ok(true)
}

pub async fn hand_is_raised(
    pool: &SqlitePool,
    session_id: i64,
    participant_hash: &str,
) -> Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM raised_hands WHERE session_id = ? AND participant_hash = ?",
    )
    .bind(session_id)
    .bind(participant_hash)
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}

pub async fn raised_hand_count(pool: &SqlitePool, session_id: i64) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM raised_hands WHERE session_id = ?")
            .bind(session_id)
            .fetch_one(pool)
            .await?,
    )
}

pub async fn reset_hands(pool: &SqlitePool, session_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM raised_hands WHERE session_id = ?")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn reaction_counts(
    pool: &SqlitePool,
    session_id: i64,
    slide_index: usize,
) -> Result<Vec<ReactionCount>> {
    Ok(sqlx::query_as::<_, ReactionCount>(
        r#"SELECT kind, COUNT(*) AS count FROM reactions
           WHERE session_id = ? AND slide_index = ? GROUP BY kind ORDER BY kind"#,
    )
    .bind(session_id)
    .bind(slide_index as i64)
    .fetch_all(pool)
    .await?)
}

pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        DEFAULT_CODE_FONT, DEFAULT_HEADLINE_FONT, DEFAULT_TEXT_FONT, DEFAULT_THEME_ACCENT,
        DEFAULT_THEME_BACKGROUND, DEFAULT_THEME_TEXT, Theme,
    };

    async fn start_test_session(pool: &SqlitePool, slug: &str) -> LiveSession {
        let deck = create_deck(pool, slug, slug).await.unwrap();
        let version_id = save_and_publish_deck(
            pool,
            deck.id,
            &deck.title,
            &deck.draft_source,
            &deck.draft_source,
            &Theme::from(&deck),
        )
        .await
        .unwrap();
        start_session(pool, deck.id, version_id).await.unwrap()
    }

    #[tokio::test]
    async fn replaces_and_revokes_api_token() {
        let directory = tempfile::tempdir().unwrap();
        let database_url = format!("sqlite://{}", directory.path().join("slides.db").display());
        let pool = connect(&database_url).await.unwrap();
        let old_hash = "a".repeat(64);
        let new_hash = "b".repeat(64);

        let created = replace_api_token(&pool, &old_hash, "slides_old")
            .await
            .unwrap();
        assert_eq!(created.prefix, "slides_old");
        assert!(created.created_at > 0);
        assert!(api_token_matches(&pool, &old_hash).await.unwrap());

        let replaced = replace_api_token(&pool, &new_hash, "slides_new")
            .await
            .unwrap();
        assert_eq!(replaced.prefix, "slides_new");
        assert!(!api_token_matches(&pool, &old_hash).await.unwrap());
        assert!(api_token_matches(&pool, &new_hash).await.unwrap());

        let summary = api_token(&pool).await.unwrap().unwrap();
        assert_eq!(summary.prefix, "slides_new");
        assert_eq!(summary.created_at, replaced.created_at);

        assert!(revoke_api_token(&pool).await.unwrap());
        assert!(!revoke_api_token(&pool).await.unwrap());
        assert!(api_token(&pool).await.unwrap().is_none());
        assert!(!api_token_matches(&pool, &new_hash).await.unwrap());
    }

    #[tokio::test]
    async fn creates_deck_with_full_content() {
        let directory = tempfile::tempdir().unwrap();
        let database_url = format!("sqlite://{}", directory.path().join("slides.db").display());
        let pool = connect(&database_url).await.unwrap();

        let theme = Theme {
            headline_font: "bebas-neue".into(),
            text_font: "merriweather".into(),
            code_font: "system-mono".into(),
            background: "#010203".into(),
            text: "#fefefe".into(),
            accent: "#abcdef".into(),
        };
        let created = create_deck_with_content(
            &pool,
            "custom-deck",
            "Custom Deck",
            "# Custom source\n\nWith full content.",
            &theme,
        )
        .await
        .unwrap();
        let persisted = get_deck(&pool, created.id).await.unwrap();

        assert_eq!(persisted.slug, "custom-deck");
        assert_eq!(persisted.title, "Custom Deck");
        assert_eq!(
            persisted.draft_source,
            "# Custom source\n\nWith full content."
        );
        assert_eq!(persisted.theme_headline_font, "bebas-neue");
        assert_eq!(persisted.theme_text_font, "merriweather");
        assert_eq!(persisted.theme_code_font, "system-mono");
        let legacy_font: String = sqlx::query_scalar("SELECT theme_font FROM decks WHERE id = ?")
            .bind(created.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(legacy_font, "system");
        assert_eq!(persisted.theme_background, "#010203");
        assert_eq!(persisted.theme_text, "#fefefe");
        assert_eq!(persisted.theme_accent, "#abcdef");
    }

    #[tokio::test]
    async fn publishes_and_runs_a_session() {
        let directory = tempfile::tempdir().unwrap();
        let database_url = format!("sqlite://{}", directory.path().join("slides.db").display());
        let pool = connect(&database_url).await.unwrap();
        healthcheck(&pool).await.unwrap();

        let deck = create_deck(&pool, "rust-errors", "Rust Errors")
            .await
            .unwrap();
        assert_eq!(deck.theme_headline_font, DEFAULT_HEADLINE_FONT);
        assert_eq!(deck.theme_text_font, DEFAULT_TEXT_FONT);
        assert_eq!(deck.theme_code_font, DEFAULT_CODE_FONT);
        assert_eq!(deck.theme_background, DEFAULT_THEME_BACKGROUND);
        assert_eq!(deck.theme_text, DEFAULT_THEME_TEXT);
        assert_eq!(deck.theme_accent, DEFAULT_THEME_ACCENT);
        let theme_style = Theme::from(&deck).style();
        assert!(theme_style.contains("--deck-surface:rgb(255 255 255 / 8%)"));
        assert!(theme_style.contains("--deck-accent:#fc218a"));
        assert!(!theme_style.contains("gradient"));

        let published_source = "# Published snapshot";
        let published_theme = Theme {
            headline_font: "bebas-neue".into(),
            text_font: "merriweather".into(),
            code_font: "system-mono".into(),
            ..Theme::from(&deck)
        };
        let version_id = save_and_publish_deck(
            &pool,
            deck.id,
            &deck.title,
            &deck.draft_source,
            published_source,
            &published_theme,
        )
        .await
        .unwrap();
        assert_eq!(
            get_deck(&pool, deck.id).await.unwrap().draft_source,
            deck.draft_source
        );
        let version = get_version(&pool, version_id).await.unwrap();
        assert_eq!(version.source, published_source);
        assert_eq!(version.theme_headline_font, "bebas-neue");
        assert_eq!(version.theme_text_font, "merriweather");
        assert_eq!(version.theme_code_font, "system-mono");
        let legacy_font: String =
            sqlx::query_scalar("SELECT theme_font FROM deck_versions WHERE id = ?")
                .bind(version_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(legacy_font, "system");
        let session = start_session(&pool, deck.id, version_id).await.unwrap();
        assert_eq!(
            deck_slug_for_session(&pool, session.id).await.unwrap(),
            deck.slug
        );

        replace_answer(&pool, session.id, 0, "participant", "poll", "0")
            .await
            .unwrap();
        assert_eq!(answerer_count(&pool, session.id, 0).await.unwrap(), 1);

        replace_answer(&pool, session.id, 2, "participant", "wordcloud", "Hiking")
            .await
            .unwrap();
        let word_cloud = word_cloud_responses(&pool, session.id, 2).await.unwrap();
        assert_eq!(word_cloud.len(), 1);
        assert_eq!(word_cloud[0].value, "Hiking");
        assert_eq!(word_cloud[0].participant_hash, "participant");

        assert!(
            add_reaction(&pool, session.id, 0, "participant", "applause")
                .await
                .unwrap()
        );
        assert_eq!(
            reaction_counts(&pool, session.id, 0).await.unwrap()[0].count,
            1
        );

        replace_answer(&pool, session.id, 1, "participant", "ordering", "2,0,1")
            .await
            .unwrap();
        assert_eq!(
            ordering_values(&pool, session.id, 1).await.unwrap(),
            vec!["2,0,1"]
        );

        assert!(toggle_hand(&pool, session.id, "participant").await.unwrap());
        assert!(
            hand_is_raised(&pool, session.id, "participant")
                .await
                .unwrap()
        );
        assert_eq!(raised_hand_count(&pool, session.id).await.unwrap(), 1);
        assert!(!toggle_hand(&pool, session.id, "participant").await.unwrap());
        assert_eq!(raised_hand_count(&pool, session.id).await.unwrap(), 0);
        assert!(toggle_hand(&pool, session.id, "participant").await.unwrap());
        reset_hands(&pool, session.id).await.unwrap();
        assert_eq!(raised_hand_count(&pool, session.id).await.unwrap(), 0);

        move_to_slide(&pool, session.id, 1).await.unwrap();
        move_to_slide(&pool, session.id, 0).await.unwrap();
        assert_eq!(
            get_session(&pool, session.id)
                .await
                .unwrap()
                .follow_revision,
            2
        );
        focus_audience(&pool, session.id).await.unwrap();
        assert_eq!(
            get_session(&pool, session.id)
                .await
                .unwrap()
                .follow_revision,
            3
        );

        let token = "a".repeat(64);
        finish_session_with_artifact(&pool, session.id, now_millis(), &token, b"archive")
            .await
            .unwrap();
        let artifact = artifact_for_session(&pool, session.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(artifact.share_token, token);
        assert_eq!(artifact.archive, b"archive");
        assert_eq!(
            artifact_by_token(&pool, &token)
                .await
                .unwrap()
                .unwrap()
                .code,
            session.code
        );
        assert!(
            active_session_for_deck(&pool, deck.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn deletes_inactive_decks_and_ended_sessions_with_their_archives() {
        let directory = tempfile::tempdir().unwrap();
        let database_url = format!("sqlite://{}", directory.path().join("slides.db").display());
        let pool = connect(&database_url).await.unwrap();

        let deck_session = start_test_session(&pool, "delete-deck").await;
        let deck = deck_by_slug(&pool, "delete-deck").await.unwrap().unwrap();
        assert!(!delete_deck(&pool, deck.id).await.unwrap());
        let deck_token = "a".repeat(64);
        finish_session_with_artifact(
            &pool,
            deck_session.id,
            now_millis(),
            &deck_token,
            b"deck archive",
        )
        .await
        .unwrap();

        assert!(delete_deck(&pool, deck.id).await.unwrap());
        assert!(deck_by_slug(&pool, "delete-deck").await.unwrap().is_none());
        assert!(
            session_by_code(&pool, &deck_session.code)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            artifact_by_token(&pool, &deck_token)
                .await
                .unwrap()
                .is_none()
        );

        let ended_session = start_test_session(&pool, "delete-session").await;
        let session_token = "b".repeat(64);
        finish_session_with_artifact(
            &pool,
            ended_session.id,
            now_millis(),
            &session_token,
            b"session archive",
        )
        .await
        .unwrap();

        assert!(delete_ended_session(&pool, ended_session.id).await.unwrap());
        assert!(
            session_by_code(&pool, &ended_session.code)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            artifact_by_token(&pool, &session_token)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            deck_by_slug(&pool, "delete-session")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn persists_session_questions_and_upvotes() {
        let directory = tempfile::tempdir().unwrap();
        let database_url = format!("sqlite://{}", directory.path().join("slides.db").display());
        let pool = connect(&database_url).await.unwrap();
        let session = start_test_session(&pool, "questions").await;

        let first = create_question(&pool, session.id, "alice", "How does this scale?")
            .await
            .unwrap();
        let second = create_question(&pool, session.id, "bob", "Can you show an example?")
            .await
            .unwrap();
        let third = create_question(&pool, session.id, "alice", "Will slides be shared?")
            .await
            .unwrap();
        assert_eq!(first.body, "How does this scale?");
        assert!(!first.answered);
        assert_eq!(first.vote_count, 0);
        assert!(!first.participant_upvoted);
        assert_eq!(
            participant_question_count(&pool, session.id, "alice")
                .await
                .unwrap(),
            2
        );

        assert!(
            toggle_question_upvote(&pool, session.id, first.id, "bob")
                .await
                .unwrap()
        );
        let duplicate_vote = sqlx::query(
            "INSERT INTO question_votes (question_id, participant_hash, created_at) VALUES (?, ?, ?)",
        )
        .bind(first.id)
        .bind("bob")
        .bind(now_millis())
        .execute(&pool)
        .await
        .unwrap_err();
        assert!(
            duplicate_vote
                .as_database_error()
                .is_some_and(|error| error.is_unique_violation())
        );
        assert!(
            toggle_question_upvote(&pool, session.id, first.id, "carol")
                .await
                .unwrap()
        );
        assert!(
            toggle_question_upvote(&pool, session.id, second.id, "alice")
                .await
                .unwrap()
        );

        let questions = list_visible_questions(&pool, session.id, "bob")
            .await
            .unwrap();
        assert_eq!(
            questions
                .iter()
                .map(|question| question.id)
                .collect::<Vec<_>>(),
            vec![first.id, second.id, third.id]
        );
        assert_eq!(questions[0].vote_count, 2);
        assert!(questions[0].participant_upvoted);
        assert_eq!(questions[1].vote_count, 1);
        assert!(!questions[1].participant_upvoted);

        assert!(
            set_question_answered(&pool, session.id, first.id, true)
                .await
                .unwrap()
        );
        let questions = list_visible_questions(&pool, session.id, "bob")
            .await
            .unwrap();
        assert_eq!(questions.last().unwrap().id, first.id);
        assert!(questions.last().unwrap().answered);
        let answered_at: Option<i64> =
            sqlx::query_scalar("SELECT answered_at FROM questions WHERE id = ?")
                .bind(first.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(answered_at.is_some());

        assert!(
            set_question_answered(&pool, session.id, first.id, false)
                .await
                .unwrap()
        );
        let questions = list_visible_questions(&pool, session.id, "bob")
            .await
            .unwrap();
        assert_eq!(questions[0].id, first.id);
        assert!(!questions[0].answered);

        let other_session = start_test_session(&pool, "other-questions").await;
        let error = toggle_question_upvote(&pool, other_session.id, first.id, "bob")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("does not belong to session"));
        assert!(
            !set_question_answered(&pool, other_session.id, first.id, true)
                .await
                .unwrap()
        );

        assert!(dismiss_question(&pool, session.id, first.id).await.unwrap());
        let questions = list_visible_questions(&pool, session.id, "bob")
            .await
            .unwrap();
        assert_eq!(
            questions
                .iter()
                .map(|question| question.id)
                .collect::<Vec<_>>(),
            vec![second.id, third.id]
        );
        let hidden_state: (bool, Option<i64>) =
            sqlx::query_as("SELECT hidden, hidden_at FROM questions WHERE id = ?")
                .bind(first.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(hidden_state.0);
        assert!(hidden_state.1.is_some());
        assert_eq!(
            participant_question_count(&pool, session.id, "alice")
                .await
                .unwrap(),
            2
        );
    }
}
