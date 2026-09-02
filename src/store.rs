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
    DEFAULT_THEME_ACCENT, DEFAULT_THEME_BACKGROUND, DEFAULT_THEME_TEXT, Deck, DeckSummary,
    DeckVersion, LiveSession,
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

pub async fn create_deck(pool: &SqlitePool, slug: &str, title: &str) -> Result<Deck> {
    let now = now_millis();
    let source = format!(
        "# {title}\n\nYour presentation starts here.\n\n---\n\n# Ask the audience\n\n:::poll question=\"Which option do you prefer?\"\n- The first option\n- The second option\n:::"
    );
    let id = sqlx::query(
        r#"INSERT INTO decks
           (slug, title, draft_source, theme_background, theme_text, theme_accent, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(slug)
    .bind(title)
    .bind(source)
    .bind(DEFAULT_THEME_BACKGROUND)
    .bind(DEFAULT_THEME_TEXT)
    .bind(DEFAULT_THEME_ACCENT)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?
    .last_insert_rowid();
    get_deck(pool, id).await
}

pub async fn get_deck(pool: &SqlitePool, id: i64) -> Result<Deck> {
    Ok(sqlx::query_as::<_, Deck>(
        r#"SELECT id, slug, title, draft_source, theme_font, theme_background,
                  theme_text, theme_accent
           FROM decks WHERE id = ?"#,
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

pub async fn deck_by_slug(pool: &SqlitePool, slug: &str) -> Result<Option<Deck>> {
    Ok(sqlx::query_as::<_, Deck>(
        r#"SELECT id, slug, title, draft_source, theme_font, theme_background,
                  theme_text, theme_accent
           FROM decks WHERE slug = ?"#,
    )
    .bind(slug)
    .fetch_optional(pool)
    .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn save_deck(
    pool: &SqlitePool,
    id: i64,
    title: &str,
    source: &str,
    font: &str,
    background: &str,
    text: &str,
    accent: &str,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE decks
           SET title = ?, draft_source = ?, theme_font = ?, theme_background = ?,
               theme_text = ?, theme_accent = ?, updated_at = ?
           WHERE id = ?"#,
    )
    .bind(title)
    .bind(source)
    .bind(font)
    .bind(background)
    .bind(text)
    .bind(accent)
    .bind(now_millis())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn save_and_publish_deck(
    pool: &SqlitePool,
    deck_id: i64,
    title: &str,
    draft_source: &str,
    published_source: &str,
    font: &str,
    background: &str,
    text: &str,
    accent: &str,
) -> Result<i64> {
    let now = now_millis();
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"UPDATE decks
           SET title = ?, draft_source = ?, theme_font = ?, theme_background = ?,
               theme_text = ?, theme_accent = ?, updated_at = ?
           WHERE id = ?"#,
    )
    .bind(title)
    .bind(draft_source)
    .bind(font)
    .bind(background)
    .bind(text)
    .bind(accent)
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
           (deck_id, version_number, title, source, theme_font, theme_background,
            theme_text, theme_accent, show_join_code, published_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?)"#,
    )
    .bind(deck_id)
    .bind(version_number)
    .bind(title)
    .bind(published_source)
    .bind(font)
    .bind(background)
    .bind(text)
    .bind(accent)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .last_insert_rowid();
    tx.commit().await?;
    Ok(id)
}

pub async fn latest_version(pool: &SqlitePool, deck_id: i64) -> Result<Option<DeckVersion>> {
    Ok(sqlx::query_as::<_, DeckVersion>(
        r#"SELECT id, deck_id, title, source, theme_font, theme_background,
                  theme_text, theme_accent
           FROM deck_versions WHERE deck_id = ? ORDER BY version_number DESC LIMIT 1"#,
    )
    .bind(deck_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn get_version(pool: &SqlitePool, id: i64) -> Result<DeckVersion> {
    Ok(sqlx::query_as::<_, DeckVersion>(
        r#"SELECT id, deck_id, title, source, theme_font, theme_background,
                  theme_text, theme_accent
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

pub async fn end_session(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("UPDATE sessions SET ended_at = ? WHERE id = ? AND ended_at IS NULL")
        .bind(now_millis())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
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
    use crate::models::Theme;

    #[tokio::test]
    async fn publishes_and_runs_a_session() {
        let directory = tempfile::tempdir().unwrap();
        let database_url = format!("sqlite://{}", directory.path().join("slides.db").display());
        let pool = connect(&database_url).await.unwrap();
        healthcheck(&pool).await.unwrap();

        let deck = create_deck(&pool, "rust-errors", "Rust Errors")
            .await
            .unwrap();
        assert_eq!(deck.theme_background, DEFAULT_THEME_BACKGROUND);
        assert_eq!(deck.theme_text, DEFAULT_THEME_TEXT);
        assert_eq!(deck.theme_accent, DEFAULT_THEME_ACCENT);
        let theme_style = Theme::from(&deck).style();
        assert!(theme_style.contains("--surface:#181825"));
        assert!(theme_style.contains("--highlight:#f9e2af"));
        assert!(!theme_style.contains("gradient"));

        let published_source = "# Published snapshot";
        let version_id = save_and_publish_deck(
            &pool,
            deck.id,
            &deck.title,
            &deck.draft_source,
            published_source,
            &deck.theme_font,
            &deck.theme_background,
            &deck.theme_text,
            &deck.theme_accent,
        )
        .await
        .unwrap();
        assert_eq!(
            get_deck(&pool, deck.id).await.unwrap().draft_source,
            deck.draft_source
        );
        assert_eq!(
            latest_version(&pool, deck.id)
                .await
                .unwrap()
                .unwrap()
                .source,
            published_source
        );
        let session = start_session(&pool, deck.id, version_id).await.unwrap();

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

        end_session(&pool, session.id).await.unwrap();
        assert!(
            active_session_for_deck(&pool, deck.id)
                .await
                .unwrap()
                .is_none()
        );
    }
}
