use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
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
    has_mermaid: bool,
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
    embed_root: &Path,
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
    let theme = Theme::from(version);
    let font_assets = selected_font_assets(&theme);
    let iframe_assets = document
        .slides
        .iter()
        .flat_map(|slide| slide.iframe_assets.iter().cloned())
        .collect::<BTreeSet<_>>();
    let page = ArchiveTemplate {
        title: version.title.clone(),
        theme_style: theme.style(),
        slides,
        has_mermaid: document
            .slides
            .iter()
            .any(|slide| slide.html.contains("data-mermaid-diagram")),
    }
    .render()?;

    let embed_root = embed_root.to_owned();
    tokio::task::spawn_blocking(move || {
        package(
            page,
            audience_json,
            Path::new("assets"),
            Some(&embed_root),
            &font_assets,
            &iframe_assets,
        )
    })
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

fn package(
    mut page: String,
    audience_json: Vec<u8>,
    asset_root: &Path,
    uploaded_embed_root: Option<&Path>,
    font_assets: &BTreeSet<&'static str>,
    iframe_assets: &BTreeSet<String>,
) -> Result<Vec<u8>> {
    let mut packaged_assets = BTreeMap::new();
    package_iframe_bundles(
        asset_root,
        uploaded_embed_root,
        iframe_assets,
        &mut packaged_assets,
    )?;
    let mut missing_assets = Vec::new();
    for (url, path) in local_assets(&page) {
        if packaged_assets.contains_key(&path) {
            page = page.replace(
                &format!("src=\"/assets/{url}\""),
                &format!("src=\"assets/{url}\""),
            );
            continue;
        }
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
    if packaged_assets.contains_key("vendor/mermaid/mermaid.min.js") {
        let license = std::fs::read(asset_root.join("vendor/mermaid/LICENSE"))
            .context("could not read the Mermaid license for the session archive")?;
        packaged_assets.insert("vendor/mermaid/LICENSE".into(), license);
    }
    for path in font_assets {
        let contents = std::fs::read(asset_root.join(path))
            .with_context(|| format!("could not read {path} for the session archive"))?;
        packaged_assets.insert((*path).into(), contents);
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

const MAX_IFRAME_BUNDLE_FILES: usize = 512;
const MAX_IFRAME_BUNDLE_BYTES: u64 = 100 * 1024 * 1024;

fn package_iframe_bundles(
    asset_root: &Path,
    uploaded_embed_root: Option<&Path>,
    iframe_assets: &BTreeSet<String>,
    packaged_assets: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    if iframe_assets.is_empty() {
        return Ok(());
    }

    let static_embed_root = asset_root.join("embeds");
    let mut bundle_roots = BTreeSet::new();
    for asset in iframe_assets {
        let components = Path::new(asset)
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => value.to_str(),
                _ => None,
            })
            .collect::<Vec<_>>();
        if components.len() < 3 || components[0] != "embeds" {
            bail!("invalid iframe asset path {asset:?}");
        }
        bundle_roots.insert(PathBuf::from(components[0]).join(components[1]));
    }

    let mut file_count = 0;
    let mut byte_count = 0;
    for bundle_root in bundle_roots {
        let bundle_name = bundle_root
            .file_name()
            .context("iframe bundle path has no directory name")?;
        let (requested_source, allowed_root) = uploaded_embed_root
            .map(|root| (root.join(bundle_name), root))
            .filter(|(source, _)| source.exists())
            .unwrap_or_else(|| {
                (
                    static_embed_root.join(bundle_name),
                    static_embed_root.as_path(),
                )
            });
        if std::fs::symlink_metadata(&requested_source)?
            .file_type()
            .is_symlink()
        {
            bail!(
                "iframe bundle roots cannot be symlinks: {}",
                requested_source.display()
            );
        }
        let canonical_root = std::fs::canonicalize(allowed_root)
            .context("could not resolve the iframe embed directory")?;
        let source = std::fs::canonicalize(&requested_source).with_context(|| {
            format!(
                "could not resolve iframe bundle {:?}",
                bundle_root.to_string_lossy()
            )
        })?;
        if !source.starts_with(&canonical_root) {
            bail!("iframe bundle escapes the embed directory");
        }
        collect_iframe_bundle(
            &source,
            &bundle_root,
            packaged_assets,
            &mut file_count,
            &mut byte_count,
        )?;
    }
    Ok(())
}

fn collect_iframe_bundle(
    source: &Path,
    archive_path: &Path,
    packaged_assets: &mut BTreeMap<String, Vec<u8>>,
    file_count: &mut usize,
    byte_count: &mut u64,
) -> Result<()> {
    for entry in std::fs::read_dir(source).with_context(|| {
        format!(
            "could not read iframe bundle directory {}",
            source.display()
        )
    })? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            bail!(
                "iframe bundles cannot contain symlinks: {}",
                entry.path().display()
            );
        }
        let child_archive_path = archive_path.join(entry.file_name());
        if file_type.is_dir() {
            collect_iframe_bundle(
                &entry.path(),
                &child_archive_path,
                packaged_assets,
                file_count,
                byte_count,
            )?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        let key = archive_path_string(&child_archive_path)?;
        let metadata = entry.metadata()?;
        let next_file_count = *file_count + 1;
        let next_byte_count = (*byte_count)
            .checked_add(metadata.len())
            .context("iframe bundle size overflowed")?;
        if next_file_count > MAX_IFRAME_BUNDLE_FILES {
            bail!("iframe bundles contain more than {MAX_IFRAME_BUNDLE_FILES} files");
        }
        if next_byte_count > MAX_IFRAME_BUNDLE_BYTES {
            bail!("iframe bundles exceed 100 MiB");
        }

        let mut contents = std::fs::read(entry.path())?;
        if matches!(
            child_archive_path
                .extension()
                .and_then(|value| value.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("html" | "htm")
        ) {
            contents = secure_archived_html(contents)?;
        }
        *file_count = next_file_count;
        *byte_count = next_byte_count;
        packaged_assets.insert(key, contents);
    }
    Ok(())
}

fn archive_path_string(path: &Path) -> Result<String> {
    path.components()
        .map(|component| match component {
            Component::Normal(value) => {
                let value = value
                    .to_str()
                    .context("iframe bundle paths must be valid UTF-8")?;
                if value.contains(['\\', ':']) || value.chars().any(char::is_control) {
                    bail!("iframe bundle paths contain an unsafe filename");
                }
                Ok(value.to_owned())
            }
            _ => bail!("iframe bundle paths must be relative"),
        })
        .collect::<Result<Vec<_>>>()
        .map(|components| components.join("/"))
}

const ARCHIVE_IFRAME_CSP: &str = "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; media-src 'self'; connect-src 'none'; worker-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";

fn secure_archived_html(contents: Vec<u8>) -> Result<Vec<u8>> {
    let mut html = String::from_utf8(contents).context("iframe HTML must be valid UTF-8")?;
    let lowercase = html.to_ascii_lowercase();
    let insertion = format!(
        "<head><meta http-equiv=\"Content-Security-Policy\" content=\"{ARCHIVE_IFRAME_CSP}\"></head>"
    );
    let insertion_at = leading_doctype_end(&lowercase)?.unwrap_or_default();
    html.insert_str(insertion_at, &insertion);
    Ok(crate::web::add_iframe_navigation_bridge(html).into_bytes())
}

fn leading_doctype_end(html: &str) -> Result<Option<usize>> {
    let mut offset = 0;
    loop {
        let remaining = &html[offset..];
        offset += remaining.len() - remaining.trim_start().len();
        let remaining = &html[offset..];

        if offset == 0 && remaining.starts_with('\u{feff}') {
            offset += '\u{feff}'.len_utf8();
            continue;
        }
        if remaining.starts_with("<!--") {
            let Some(comment_end) = remaining.find("-->") else {
                return Ok(None);
            };
            offset += comment_end + "-->".len();
            continue;
        }
        if !remaining.starts_with("<!doctype") {
            return Ok(None);
        }
        return remaining
            .find('>')
            .map(|doctype_end| Some(offset + doctype_end + 1))
            .context("iframe HTML has an unterminated doctype");
    }
}

fn selected_font_assets(theme: &Theme) -> BTreeSet<&'static str> {
    let mut assets = BTreeSet::from(["fonts/README.md"]);
    for font in [&theme.headline_font, &theme.text_font, &theme.code_font] {
        match font.as_str() {
            "inter" => assets.extend([
                "fonts/inter-variable.woff2",
                "fonts/inter-variable-italic.woff2",
                "fonts/LICENSE-Inter.txt",
            ]),
            "bebas-neue" => assets.extend([
                "fonts/bebas-neue-regular.woff2",
                "fonts/LICENSE-Bebas-Neue.txt",
            ]),
            "happy" => assets.extend(["fonts/happy-headline.woff2", "fonts/happy-regular.woff2"]),
            "merriweather" => assets.extend([
                "fonts/merriweather-regular.woff2",
                "fonts/merriweather-bold.woff2",
                "fonts/merriweather-italic.woff2",
                "fonts/LICENSE-Merriweather.txt",
            ]),
            "jetbrains-mono" => assets.extend([
                "fonts/jetbrains-mono-regular.woff2",
                "fonts/LICENSE-JetBrains-Mono.txt",
            ]),
            _ => {}
        }
    }
    assets
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
    use std::{collections::BTreeSet, path::Path};

    use super::{
        archive_path_string, local_assets, package, read_entry, response_display_value,
        secure_archived_html, selected_font_assets,
    };
    use crate::{markdown::parse_deck, models::Theme};

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
    fn selects_only_fonts_used_by_the_theme() {
        let theme = Theme {
            headline_font: "happy".into(),
            text_font: "merriweather".into(),
            code_font: "system-mono".into(),
            ..Theme::default()
        };

        let assets = selected_font_assets(&theme);

        assert!(assets.contains("fonts/happy-headline.woff2"));
        assert!(assets.contains("fonts/happy-regular.woff2"));
        assert!(assets.contains("fonts/merriweather-italic.woff2"));
        assert!(assets.contains("fonts/LICENSE-Merriweather.txt"));
        assert!(!assets.contains("fonts/inter-variable.woff2"));
        assert!(!assets.contains("fonts/jetbrains-mono-regular.woff2"));
    }

    #[test]
    fn packages_local_assets_and_rejects_traversal() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("app.css"), "body {}").unwrap();
        std::fs::write(directory.path().join("app.js"), "renderDiagrams()").unwrap();
        std::fs::create_dir_all(directory.path().join("vendor/mermaid")).unwrap();
        std::fs::write(
            directory.path().join("vendor/mermaid/mermaid.min.js"),
            "window.mermaid = {}",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("vendor/mermaid/LICENSE"),
            "MIT License",
        )
        .unwrap();
        std::fs::write(directory.path().join("cat.webp"), b"cat").unwrap();
        std::fs::create_dir_all(directory.path().join("fonts/licenses")).unwrap();
        std::fs::write(
            directory.path().join("fonts/example-regular.woff2"),
            b"font data",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("fonts/licenses/LICENSE-example.txt"),
            b"font license",
        )
        .unwrap();
        let page = r#"<script src="/assets/vendor/mermaid/mermaid.min.js"></script><script src="/assets/app.js"></script><img src="/assets/cat.webp?v=2"><img src="/assets/missing.png"><img src="/assets/../secret">"#.into();
        let font_assets = BTreeSet::from([
            "fonts/example-regular.woff2",
            "fonts/licenses/LICENSE-example.txt",
        ]);

        let archive = package(
            page,
            b"{}".to_vec(),
            directory.path(),
            None,
            &font_assets,
            &BTreeSet::new(),
        )
        .unwrap();
        let index =
            String::from_utf8(read_entry(&archive, "index.html").unwrap().unwrap()).unwrap();

        assert!(index.contains("src=\"assets/cat.webp?v=2\""));
        assert!(index.contains("src=\"/assets/missing.png\""));
        assert!(index.contains("src=\"/assets/../secret\""));
        assert_eq!(
            read_entry(&archive, "assets/cat.webp").unwrap().unwrap(),
            b"cat"
        );
        assert_eq!(
            read_entry(&archive, "assets/app.js").unwrap().unwrap(),
            b"renderDiagrams()"
        );
        assert_eq!(
            read_entry(&archive, "assets/vendor/mermaid/mermaid.min.js")
                .unwrap()
                .unwrap(),
            b"window.mermaid = {}"
        );
        assert_eq!(
            read_entry(&archive, "assets/vendor/mermaid/LICENSE")
                .unwrap()
                .unwrap(),
            b"MIT License"
        );
        assert_eq!(
            read_entry(&archive, "assets/fonts/example-regular.woff2")
                .unwrap()
                .unwrap(),
            b"font data"
        );
        assert_eq!(
            read_entry(&archive, "assets/fonts/licenses/LICENSE-example.txt")
                .unwrap()
                .unwrap(),
            b"font license"
        );
        assert!(read_entry(&archive, "../secret").unwrap().is_none());
        assert!(
            read_entry(&archive, "missing-assets.txt")
                .unwrap()
                .is_some()
        );
        assert_eq!(local_assets(&index).len(), 1);
    }

    #[test]
    fn packages_complete_local_iframe_bundles() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("app.css"), "body {}").unwrap();
        let bundle = directory.path().join("embeds/demo");
        std::fs::create_dir_all(bundle.join("styles")).unwrap();
        std::fs::create_dir_all(bundle.join("scripts")).unwrap();
        std::fs::create_dir_all(bundle.join("images")).unwrap();
        std::fs::write(
            bundle.join("index.html"),
            r#"<link rel="stylesheet" href="styles/app.css"><script src="scripts/app.js"></script><img src="images/logo.png">"#,
        )
        .unwrap();
        std::fs::write(bundle.join("styles/app.css"), "body { color: hotpink; }").unwrap();
        std::fs::write(
            bundle.join("scripts/app.js"),
            "document.body.dataset.ready = 'yes';",
        )
        .unwrap();
        std::fs::write(bundle.join("images/logo.png"), b"png").unwrap();
        let page = r#"<iframe src="/assets/embeds/demo/index.html"></iframe>"#.into();

        let archive = package(
            page,
            b"{}".to_vec(),
            directory.path(),
            None,
            &BTreeSet::new(),
            &BTreeSet::from(["embeds/demo/index.html".to_owned()]),
        )
        .unwrap();
        let index =
            String::from_utf8(read_entry(&archive, "index.html").unwrap().unwrap()).unwrap();

        assert!(index.contains("src=\"assets/embeds/demo/index.html\""));
        let iframe_html = String::from_utf8(
            read_entry(&archive, "assets/embeds/demo/index.html")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(iframe_html.contains("http-equiv=\"Content-Security-Policy\""));
        assert!(iframe_html.contains("connect-src 'none'"));
        assert!(iframe_html.contains("data-slides-navigation-bridge"));
        assert!(
            read_entry(&archive, "assets/embeds/demo/styles/app.css")
                .unwrap()
                .is_some()
        );
        assert!(
            read_entry(&archive, "assets/embeds/demo/scripts/app.js")
                .unwrap()
                .is_some()
        );
        assert_eq!(
            read_entry(&archive, "assets/embeds/demo/images/logo.png")
                .unwrap()
                .unwrap(),
            b"png"
        );
    }

    #[test]
    fn prefers_uploaded_iframe_bundles_when_archiving() {
        let directory = tempfile::tempdir().unwrap();
        let assets = directory.path().join("assets");
        let uploaded = directory.path().join("uploaded");
        std::fs::create_dir_all(assets.join("embeds/demo")).unwrap();
        std::fs::create_dir_all(uploaded.join("demo")).unwrap();
        std::fs::write(assets.join("app.css"), "body {}").unwrap();
        std::fs::write(assets.join("embeds/demo/index.html"), "Static").unwrap();
        std::fs::write(uploaded.join("demo/index.html"), "Uploaded").unwrap();

        let archive = package(
            r#"<iframe src="/assets/embeds/demo/index.html"></iframe>"#.into(),
            b"{}".to_vec(),
            &assets,
            Some(&uploaded),
            &BTreeSet::new(),
            &BTreeSet::from(["embeds/demo/index.html".to_owned()]),
        )
        .unwrap();
        let iframe_html = String::from_utf8(
            read_entry(&archive, "assets/embeds/demo/index.html")
                .unwrap()
                .unwrap(),
        )
        .unwrap();

        assert!(iframe_html.contains("Uploaded"));
        assert!(!iframe_html.contains("Static"));
    }

    #[test]
    fn preserves_iframe_doctypes_after_leading_comments() {
        let html = b"\xef\xbb\xbf<!-- License -->\n<!doctype html><html><body>Demo</body></html>";

        let secured = String::from_utf8(secure_archived_html(html.to_vec()).unwrap()).unwrap();

        assert!(secured.starts_with("\u{feff}<!-- License -->\n<!doctype html><head>"));
        assert!(secured.contains("http-equiv=\"Content-Security-Policy\""));
    }

    #[test]
    fn rejects_unsafe_iframe_archive_filenames() {
        assert!(archive_path_string(Path::new("embeds/demo/..\\payload.txt")).is_err());
        assert!(archive_path_string(Path::new("embeds/demo/C:payload.txt")).is_err());
    }

    #[test]
    fn rejects_oversized_iframe_entrypoints_before_reading_them() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("app.css"), "body {}").unwrap();
        let bundle = directory.path().join("embeds/demo");
        std::fs::create_dir_all(&bundle).unwrap();
        let html = std::fs::File::create(bundle.join("index.html")).unwrap();
        html.set_len(super::MAX_IFRAME_BUNDLE_BYTES + 1).unwrap();

        let error = package(
            r#"<iframe src="/assets/embeds/demo/index.html"></iframe>"#.into(),
            b"{}".to_vec(),
            directory.path(),
            None,
            &BTreeSet::new(),
            &BTreeSet::from(["embeds/demo/index.html".to_owned()]),
        )
        .unwrap_err();

        assert!(error.to_string().contains("exceed 100 MiB"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_in_iframe_bundles() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("app.css"), "body {}").unwrap();
        let bundle = directory.path().join("embeds/demo");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("index.html"), "<!doctype html>").unwrap();
        std::fs::write(directory.path().join("secret.txt"), "secret").unwrap();
        symlink(
            directory.path().join("secret.txt"),
            bundle.join("secret.txt"),
        )
        .unwrap();

        let error = package(
            r#"<iframe src="/assets/embeds/demo/index.html"></iframe>"#.into(),
            b"{}".to_vec(),
            directory.path(),
            None,
            &BTreeSet::new(),
            &BTreeSet::from(["embeds/demo/index.html".to_owned()]),
        )
        .unwrap_err();

        assert!(error.to_string().contains("cannot contain symlinks"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_iframe_bundle_roots() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("app.css"), "body {}").unwrap();
        let embeds = directory.path().join("embeds");
        let real_bundle = embeds.join("real");
        std::fs::create_dir_all(&real_bundle).unwrap();
        std::fs::write(real_bundle.join("index.html"), "<!doctype html>").unwrap();
        symlink(&real_bundle, embeds.join("demo")).unwrap();

        let error = package(
            r#"<iframe src="/assets/embeds/demo/index.html"></iframe>"#.into(),
            b"{}".to_vec(),
            directory.path(),
            None,
            &BTreeSet::new(),
            &BTreeSet::from(["embeds/demo/index.html".to_owned()]),
        )
        .unwrap_err();

        assert!(error.to_string().contains("roots cannot be symlinks"));
    }
}
