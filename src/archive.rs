use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read, Write},
    path::{Component, Path},
};

use anyhow::{Context, Result};
use askama::Template;
use serde::Serialize;
use sqlx::SqlitePool;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::{
    markdown::{DeckDocument, Interaction},
    models::{DeckVersion, LiveSession, Theme},
    web::render,
};

#[derive(Template)]
#[template(path = "archive.html")]
struct ArchiveTemplate {
    title: String,
    theme_style: String,
    slides: String,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredResponse {
    slide_index: i64,
    participant_hash: String,
    kind: String,
    value: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredReaction {
    slide_index: i64,
    participant_hash: String,
    kind: String,
    created_at: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredRaisedHand {
    participant_hash: String,
    raised_at: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredQuestion {
    id: i64,
    participant_hash: String,
    body: String,
    created_at: i64,
    answered: bool,
    answered_at: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredQuestionUpvote {
    question_id: i64,
    participant_hash: String,
    created_at: i64,
}

#[derive(Debug, Serialize)]
struct AudienceInput {
    format_version: u8,
    presentation: String,
    session_code: String,
    started_at: i64,
    ended_at: i64,
    limitations: Vec<String>,
    responses: Vec<ExportResponse>,
    reactions: Vec<ExportReaction>,
    questions: Vec<ExportQuestion>,
    question_upvotes: Vec<ExportQuestionUpvote>,
    raised_hands_at_end: Vec<ExportRaisedHand>,
}

#[derive(Debug, Serialize)]
struct ExportResponse {
    slide: i64,
    participant: String,
    kind: String,
    value: String,
    display_value: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Serialize)]
struct ExportReaction {
    slide: i64,
    participant: String,
    kind: String,
    created_at: i64,
}

#[derive(Debug, Serialize)]
struct ExportRaisedHand {
    participant: String,
    raised_at: i64,
}

#[derive(Debug, Serialize)]
struct ExportQuestion {
    id: i64,
    participant: String,
    body: String,
    created_at: i64,
    answered: bool,
    answered_at: Option<i64>,
}

#[derive(Debug, Serialize)]
struct ExportQuestionUpvote {
    question_id: i64,
    participant: String,
    created_at: i64,
}

pub async fn build(
    pool: &SqlitePool,
    session: &LiveSession,
    version: &DeckVersion,
    document: &DeckDocument,
    started_at: i64,
    ended_at: i64,
) -> Result<Vec<u8>> {
    let slides = render::archived_slides(pool, session, document).await?;
    let responses = sqlx::query_as::<_, StoredResponse>(
        r#"SELECT slide_index, participant_hash, kind, value, created_at, updated_at
           FROM responses WHERE session_id = ? ORDER BY slide_index, created_at, id"#,
    )
    .bind(session.id)
    .fetch_all(pool)
    .await?;
    let reactions = sqlx::query_as::<_, StoredReaction>(
        r#"SELECT slide_index, participant_hash, kind, created_at
           FROM reactions WHERE session_id = ? ORDER BY slide_index, created_at, id"#,
    )
    .bind(session.id)
    .fetch_all(pool)
    .await?;
    let raised_hands = sqlx::query_as::<_, StoredRaisedHand>(
        r#"SELECT participant_hash, raised_at FROM raised_hands
           WHERE session_id = ? ORDER BY raised_at, participant_hash"#,
    )
    .bind(session.id)
    .fetch_all(pool)
    .await?;
    let questions = sqlx::query_as::<_, StoredQuestion>(
        r#"SELECT id, participant_hash, body, created_at, answered, answered_at
           FROM questions WHERE session_id = ? AND hidden = 0 ORDER BY created_at, id"#,
    )
    .bind(session.id)
    .fetch_all(pool)
    .await?;
    let question_upvotes = sqlx::query_as::<_, StoredQuestionUpvote>(
        r#"SELECT v.question_id, v.participant_hash, v.created_at
           FROM question_votes v JOIN questions q ON q.id = v.question_id
           WHERE q.session_id = ? AND q.hidden = 0
           ORDER BY v.question_id, v.created_at, v.participant_hash"#,
    )
    .bind(session.id)
    .fetch_all(pool)
    .await?;

    let participant_ids = participant_ids(
        &responses,
        &reactions,
        &raised_hands,
        &questions,
        &question_upvotes,
    );
    let data = AudienceInput {
        format_version: 1,
        presentation: version.title.clone(),
        session_code: session.code.clone(),
        started_at,
        ended_at,
        limitations: vec![
            "Responses contain the final stored answer from each participant, not overwritten answers."
                .into(),
            "Raised hands contain only the hands still raised when the session ended, not hand history."
                .into(),
            "Presenter-dismissed questions and their upvotes are excluded from the shared archive."
                .into(),
        ],
        responses: responses
            .into_iter()
            .map(|row| {
                let display_value = response_display_value(
                    document,
                    row.slide_index,
                    &row.kind,
                    &row.value,
                );
                ExportResponse {
                    slide: row.slide_index + 1,
                    participant: participant_ids[&row.participant_hash].clone(),
                    kind: row.kind,
                    value: row.value,
                    display_value,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                }
            })
            .collect(),
        reactions: reactions
            .into_iter()
            .map(|row| ExportReaction {
                slide: row.slide_index + 1,
                participant: participant_ids[&row.participant_hash].clone(),
                kind: row.kind,
                created_at: row.created_at,
            })
            .collect(),
        questions: questions
            .into_iter()
            .map(|row| ExportQuestion {
                id: row.id,
                participant: participant_ids[&row.participant_hash].clone(),
                body: row.body,
                created_at: row.created_at,
                answered: row.answered,
                answered_at: row.answered_at,
            })
            .collect(),
        question_upvotes: question_upvotes
            .into_iter()
            .map(|row| ExportQuestionUpvote {
                question_id: row.question_id,
                participant: participant_ids[&row.participant_hash].clone(),
                created_at: row.created_at,
            })
            .collect(),
        raised_hands_at_end: raised_hands
            .into_iter()
            .map(|row| ExportRaisedHand {
                participant: participant_ids[&row.participant_hash].clone(),
                raised_at: row.raised_at,
            })
            .collect(),
    };
    let audience_json = serde_json::to_vec_pretty(&data)?;
    let page = ArchiveTemplate {
        title: version.title.clone(),
        theme_style: Theme::from(version).style(),
        slides,
    }
    .render()?;

    tokio::task::spawn_blocking(move || package(page, audience_json, Path::new("assets")))
        .await
        .context("archive packaging task failed")?
}

fn response_display_value(
    document: &DeckDocument,
    slide_index: i64,
    kind: &str,
    value: &str,
) -> String {
    let interaction = usize::try_from(slide_index)
        .ok()
        .and_then(|index| document.slides.get(index))
        .and_then(|slide| slide.interaction.as_ref());
    match (kind, interaction) {
        ("poll", Some(Interaction::Poll { options, .. })) => value
            .parse::<usize>()
            .ok()
            .and_then(|index| options.get(index))
            .cloned()
            .unwrap_or_else(|| value.to_owned()),
        ("quiz", Some(Interaction::Quiz { options, .. })) => value
            .parse::<usize>()
            .ok()
            .and_then(|index| options.get(index))
            .map(|option| option.label.clone())
            .unwrap_or_else(|| value.to_owned()),
        ("ordering", Some(Interaction::Ordering { options, .. })) => value
            .split(',')
            .map(str::parse::<usize>)
            .collect::<Result<Vec<_>, _>>()
            .ok()
            .filter(|indices| indices.len() == options.len())
            .and_then(|indices| {
                indices
                    .into_iter()
                    .map(|index| options.get(index).cloned())
                    .collect::<Option<Vec<_>>>()
            })
            .map(|labels| labels.join(" > "))
            .unwrap_or_else(|| value.to_owned()),
        _ => value.to_owned(),
    }
}

fn participant_ids(
    responses: &[StoredResponse],
    reactions: &[StoredReaction],
    raised_hands: &[StoredRaisedHand],
    questions: &[StoredQuestion],
    question_upvotes: &[StoredQuestionUpvote],
) -> BTreeMap<String, String> {
    let hashes = responses
        .iter()
        .map(|row| &row.participant_hash)
        .chain(reactions.iter().map(|row| &row.participant_hash))
        .chain(raised_hands.iter().map(|row| &row.participant_hash))
        .chain(questions.iter().map(|row| &row.participant_hash))
        .chain(question_upvotes.iter().map(|row| &row.participant_hash))
        .collect::<BTreeSet<_>>();
    hashes
        .into_iter()
        .enumerate()
        .map(|(index, hash)| (hash.clone(), format!("participant-{:03}", index + 1)))
        .collect()
}

fn package(mut page: String, audience_json: Vec<u8>, asset_root: &Path) -> Result<Vec<u8>> {
    let mut packaged_assets = BTreeMap::new();
    let mut missing_assets = Vec::new();
    for (url, path) in local_assets(&page) {
        match std::fs::read(asset_root.join(&path)) {
            Ok(contents) => {
                page = page.replace(
                    &format!("src=\"/assets/{url}\""),
                    &format!("src=\"assets/{url}\""),
                );
                packaged_assets.entry(path).or_insert(contents);
            }
            Err(error) => {
                tracing::warn!(asset = %url, ?error, "could not include local slide asset in archive");
                missing_assets.push(url);
            }
        }
    }

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    write_entry(&mut writer, "index.html", page.as_bytes(), options)?;
    write_entry(&mut writer, "audience-input.json", &audience_json, options)?;
    let stylesheet = std::fs::read(asset_root.join("app.css"))
        .context("could not read assets/app.css for the session archive")?;
    write_entry(&mut writer, "assets/app.css", &stylesheet, options)?;
    for (path, contents) in packaged_assets {
        write_entry(&mut writer, &format!("assets/{path}"), &contents, options)?;
    }
    if !missing_assets.is_empty() {
        let manifest = format!(
            "The following local slide assets could not be included:\n{}\n",
            missing_assets.join("\n")
        );
        write_entry(
            &mut writer,
            "missing-assets.txt",
            manifest.as_bytes(),
            options,
        )?;
    }
    Ok(writer.finish()?.into_inner())
}

fn write_entry(
    writer: &mut ZipWriter<Cursor<Vec<u8>>>,
    name: &str,
    contents: &[u8],
    options: SimpleFileOptions,
) -> Result<()> {
    writer.start_file(name, options)?;
    writer.write_all(contents)?;
    Ok(())
}

fn local_assets(page: &str) -> BTreeMap<String, String> {
    let marker = "src=\"/assets/";
    let mut assets = BTreeMap::new();
    let mut remaining = page;
    while let Some(start) = remaining.find(marker) {
        remaining = &remaining[start + marker.len()..];
        let Some(end) = remaining.find('"') else {
            break;
        };
        let candidate = &remaining[..end];
        let path_end = candidate.find(['?', '#']).unwrap_or(candidate.len());
        let path = &candidate[..path_end];
        if safe_relative_path(path) {
            assets.insert(candidate.to_owned(), path.to_owned());
        }
        remaining = &remaining[end + 1..];
    }
    assets
}

fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

pub fn read_entry(archive: &[u8], name: &str) -> Result<Option<Vec<u8>>> {
    if !safe_relative_path(name) {
        return Ok(None);
    }
    let mut archive = ZipArchive::new(Cursor::new(archive))?;
    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut contents = Vec::new();
    entry.read_to_end(&mut contents)?;
    Ok(Some(contents))
}

#[cfg(test)]
mod tests {
    use super::{local_assets, package, read_entry, response_display_value};
    use crate::markdown::parse_deck;

    #[test]
    fn resolves_stored_option_indexes_for_readers() {
        let document = parse_deck(":::poll\n- Alpha\n- Beta\n:::").unwrap();

        assert_eq!(response_display_value(&document, 0, "poll", "1"), "Beta");
        assert_eq!(
            response_display_value(&document, 0, "wordcloud", "ownership"),
            "ownership"
        );
    }

    #[test]
    fn packages_local_assets_and_rejects_traversal() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("app.css"), "body {}").unwrap();
        std::fs::write(directory.path().join("cat.webp"), b"cat").unwrap();
        let page = r#"<img src="/assets/cat.webp?v=2"><img src="/assets/missing.png"><img src="/assets/../secret">"#.into();

        let archive = package(page, b"{}".to_vec(), directory.path()).unwrap();
        let index =
            String::from_utf8(read_entry(&archive, "index.html").unwrap().unwrap()).unwrap();

        assert!(index.contains("src=\"assets/cat.webp?v=2\""));
        assert!(index.contains("src=\"/assets/missing.png\""));
        assert!(index.contains("src=\"/assets/../secret\""));
        assert_eq!(
            read_entry(&archive, "assets/cat.webp").unwrap().unwrap(),
            b"cat"
        );
        assert!(read_entry(&archive, "../secret").unwrap().is_none());
        assert!(
            read_entry(&archive, "missing-assets.txt")
                .unwrap()
                .is_some()
        );
        assert_eq!(local_assets(&index).len(), 1);
    }
}
