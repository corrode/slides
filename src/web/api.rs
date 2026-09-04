use std::{
    fs,
    io::{Cursor, Read},
    path::{Component, Path as FilePath, PathBuf},
};

use anyhow::anyhow;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequestParts, Path, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use zip::ZipArchive;

use crate::{
    markdown::parse_deck,
    models::{
        CODE_FONT_IDS, DEFAULT_CODE_FONT, DEFAULT_HEADLINE_FONT, DEFAULT_TEXT_FONT,
        DEFAULT_THEME_ACCENT, DEFAULT_THEME_BACKGROUND, DEFAULT_THEME_TEXT, Deck, DeckSummary,
        HEADLINE_FONT_IDS, TEXT_FONT_IDS, Theme, legacy_font_id, valid_code_font,
        valid_headline_font, valid_text_font,
    },
    store,
};

use super::{AppState, hash};

const MAX_API_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_EMBED_UPLOAD_BYTES: usize = 20 * 1024 * 1024;
const MAX_EMBED_BUNDLE_FILES: usize = 512;
const MAX_EMBED_BUNDLE_BYTES: u64 = 100 * 1024 * 1024;
const MAX_EMBED_HTML_BYTES: u64 = 4 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/presentations", get(list).post(create))
        .route(
            "/presentations/{slug}",
            get(get_one).patch(update).delete(delete),
        )
        .layer(DefaultBodyLimit::max(MAX_API_BODY_BYTES))
        .route(
            "/embeds/{bundle}",
            put(upload_embed).layer(DefaultBodyLimit::max(MAX_EMBED_UPLOAD_BYTES)),
        )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePresentation {
    title: String,
    #[serde(default)]
    slug: Option<String>,
    source: String,
    #[serde(default)]
    theme: ThemeInput,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ThemeInput {
    font: Option<String>,
    headline_font: Option<String>,
    text_font: Option<String>,
    code_font: Option<String>,
    background: Option<String>,
    text: Option<String>,
    accent: Option<String>,
}

impl ThemeInput {
    fn is_empty(&self) -> bool {
        self.font.is_none()
            && self.headline_font.is_none()
            && self.text_font.is_none()
            && self.code_font.is_none()
            && self.background.is_none()
            && self.text.is_none()
            && self.accent.is_none()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdatePresentation {
    title: Option<String>,
    source: Option<String>,
    #[serde(default)]
    theme: Option<ThemeInput>,
}

impl UpdatePresentation {
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.source.is_none()
            && self.theme.as_ref().is_none_or(ThemeInput::is_empty)
    }
}

#[derive(Debug, Serialize)]
struct PresentationList {
    presentations: Vec<PresentationSummary>,
}

#[derive(Debug, Serialize)]
struct PresentationSummary {
    slug: String,
    title: String,
    published_versions: i64,
    active_session_code: Option<String>,
}

impl From<DeckSummary> for PresentationSummary {
    fn from(deck: DeckSummary) -> Self {
        Self {
            slug: deck.slug,
            title: deck.title,
            published_versions: deck.published_versions,
            active_session_code: deck.active_code,
        }
    }
}

#[derive(Debug, Serialize)]
struct Presentation {
    slug: String,
    title: String,
    source: String,
    theme: PresentationTheme,
}

impl From<&Deck> for Presentation {
    fn from(deck: &Deck) -> Self {
        Self {
            slug: deck.slug.clone(),
            title: deck.title.clone(),
            source: deck.draft_source.clone(),
            theme: PresentationTheme::from(deck),
        }
    }
}

#[derive(Debug, Serialize)]
struct EmbedUpload {
    bundle: String,
    files: usize,
    bytes: u64,
    url_prefix: String,
}

#[derive(Debug, Serialize)]
struct PresentationTheme {
    font: String,
    headline_font: String,
    text_font: String,
    code_font: String,
    background: String,
    text: String,
    accent: String,
}

impl From<&Deck> for PresentationTheme {
    fn from(deck: &Deck) -> Self {
        Self {
            font: legacy_font_id(&deck.theme_headline_font).into(),
            headline_font: deck.theme_headline_font.clone(),
            text_font: deck.theme_text_font.clone(),
            code_font: deck.theme_code_font.clone(),
            background: deck.theme_background.clone(),
            text: deck.theme_text.clone(),
            accent: deck.theme_accent.clone(),
        }
    }
}

struct ApiAuthorization;

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "Provide a valid API token in the Authorization header.".into(),
        }
    }

    fn validation(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "validation_error",
            message: message.into(),
        }
    }

    fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: "Presentation not found.".into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: message.into(),
        }
    }

    fn invalid_json(error: JsonRejection) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_json",
            message: error.body_text(),
        }
    }

    fn internal(error: impl Into<anyhow::Error>) -> Self {
        let error = error.into();
        tracing::error!(?error, "API request failed");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "Something went wrong while processing the request.".into(),
        }
    }
}

impl FromRequestParts<AppState> for ApiAuthorization {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        authorize(state, &parts.headers).await?;
        Ok(Self)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(json!({
                "error": {
                    "code": self.code,
                    "message": self.message,
                }
            })),
        )
            .into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

async fn upload_embed(
    State(state): State<AppState>,
    _authorization: ApiAuthorization,
    Path(bundle): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<EmbedUpload>), ApiError> {
    let bundle = normalized_bundle_name(&bundle)?;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if !matches!(
        content_type,
        Some("application/zip" | "application/x-zip-compressed")
    ) {
        return Err(ApiError::validation(
            "Embed bundles must use Content-Type: application/zip.",
        ));
    }
    if body.is_empty() {
        return Err(ApiError::validation("The embed ZIP cannot be empty."));
    }

    let embed_dir = state.embed_dir.clone();
    let install_bundle = bundle.clone();
    let stats = tokio::task::spawn_blocking(move || {
        install_embed_bundle(&embed_dir, &install_bundle, body.as_ref())
    })
    .await
    .map_err(|error| ApiError::internal(anyhow!(error)))?
    .map_err(|error| match error {
        EmbedInstallError::Invalid(message) => ApiError::validation(message),
        EmbedInstallError::Internal(error) => ApiError::internal(error),
    })?;

    Ok((
        StatusCode::CREATED,
        Json(EmbedUpload {
            url_prefix: format!("/assets/embeds/{bundle}/"),
            bundle,
            files: stats.files,
            bytes: stats.bytes,
        }),
    ))
}

async fn list(
    State(state): State<AppState>,
    _authorization: ApiAuthorization,
) -> Result<Json<PresentationList>, ApiError> {
    let presentations = store::list_decks(&state.pool)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(PresentationSummary::from)
        .collect();
    Ok(Json(PresentationList { presentations }))
}

async fn create(
    State(state): State<AppState>,
    _authorization: ApiAuthorization,
    payload: Result<Json<CreatePresentation>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(payload) = payload.map_err(ApiError::invalid_json)?;
    let title = normalized_title(&payload.title)?;
    validate_source(&payload.source)?;
    let slug = match payload
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|slug| !slug.is_empty())
    {
        Some(slug) => normalized_slug(slug)?,
        None => available_slug(&state, &title).await?,
    };
    let theme = merged_theme(None, payload.theme)?;

    let deck = store::create_deck_with_content(&state.pool, &slug, &title, &payload.source, &theme)
        .await
        .map_err(|error| {
            if error.to_string().contains("UNIQUE constraint failed") {
                ApiError::conflict("That presentation slug is already in use.")
            } else {
                ApiError::internal(error)
            }
        })?;

    let location = format!("/api/v1/presentations/{}", deck.slug);
    let mut response = (StatusCode::CREATED, Json(Presentation::from(&deck))).into_response();
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&location).expect("API presentation path is a valid header value"),
    );
    Ok(response)
}

async fn get_one(
    State(state): State<AppState>,
    _authorization: ApiAuthorization,
    Path(slug): Path<String>,
) -> Result<Json<Presentation>, ApiError> {
    let deck = required_deck(&state, &slug).await?;
    Ok(Json(Presentation::from(&deck)))
}

async fn update(
    State(state): State<AppState>,
    _authorization: ApiAuthorization,
    Path(slug): Path<String>,
    payload: Result<Json<UpdatePresentation>, JsonRejection>,
) -> Result<Json<Presentation>, ApiError> {
    let Json(payload) = payload.map_err(ApiError::invalid_json)?;
    if payload.is_empty() {
        return Err(ApiError::validation(
            "Provide at least one field to update.",
        ));
    }

    let mut deck = required_deck(&state, &slug).await?;
    if let Some(title) = payload.title {
        deck.title = normalized_title(&title)?;
    }
    if let Some(source) = payload.source {
        validate_source(&source)?;
        deck.draft_source = source;
    }
    let theme = merged_theme(Some(&deck), payload.theme.unwrap_or_default())?;
    deck.theme_headline_font = theme.headline_font.clone();
    deck.theme_text_font = theme.text_font.clone();
    deck.theme_code_font = theme.code_font.clone();
    deck.theme_background = theme.background.clone();
    deck.theme_text = theme.text.clone();
    deck.theme_accent = theme.accent.clone();

    store::save_deck(
        &state.pool,
        deck.id,
        &deck.title,
        &deck.draft_source,
        &theme,
    )
    .await
    .map_err(ApiError::internal)?;

    Ok(Json(Presentation::from(&deck)))
}

async fn delete(
    State(state): State<AppState>,
    _authorization: ApiAuthorization,
    Path(slug): Path<String>,
) -> Result<StatusCode, ApiError> {
    let deck = required_deck(&state, &slug).await?;
    if !store::delete_deck(&state.pool, deck.id)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::conflict(
            "End the live session before deleting this presentation.",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(ApiError::unauthorized());
    };
    let mut parts = value.split_whitespace();
    let scheme = parts.next().unwrap_or_default();
    let token = parts.next().unwrap_or_default();
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() || parts.next().is_some() {
        return Err(ApiError::unauthorized());
    }
    let matches = store::api_token_matches(&state.pool, &hash(token))
        .await
        .map_err(ApiError::internal)?;
    if matches {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

async fn required_deck(state: &AppState, slug: &str) -> Result<Deck, ApiError> {
    store::deck_by_slug(&state.pool, slug)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)
}

fn normalized_title(title: &str) -> Result<String, ApiError> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 120 {
        return Err(ApiError::validation(
            "Titles must contain between 1 and 120 characters.",
        ));
    }
    Ok(title.into())
}

fn normalized_slug(slug: &str) -> Result<String, ApiError> {
    const RESERVED: &[&str] = &[
        "admin", "api", "assets", "healthz", "join", "present", "sessions",
    ];
    let slug = slug.to_ascii_lowercase();
    let valid = (1..=48).contains(&slug.len())
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !RESERVED.contains(&slug.as_str());
    if !valid {
        return Err(ApiError::validation(
            "Slugs must be 1–48 lowercase letters, numbers, or hyphens.",
        ));
    }
    Ok(slug)
}

fn slugify_title(title: &str) -> Option<String> {
    let mut slug = String::new();
    let mut pending_separator = false;
    for character in title.chars() {
        if character.is_ascii_alphanumeric() {
            if pending_separator && !slug.is_empty() {
                if slug.len() + 2 > 48 {
                    break;
                }
                slug.push('-');
            }
            if slug.len() == 48 {
                break;
            }
            slug.push(character.to_ascii_lowercase());
            pending_separator = false;
        } else if !slug.is_empty() {
            pending_separator = true;
        }
    }
    (!slug.is_empty()).then_some(slug)
}

async fn available_slug(state: &AppState, title: &str) -> Result<String, ApiError> {
    let base = slugify_title(title).ok_or_else(|| {
        ApiError::validation("Provide a slug for titles without ASCII letters or numbers.")
    })?;
    if normalized_slug(&base).is_ok()
        && store::deck_by_slug(&state.pool, &base)
            .await
            .map_err(ApiError::internal)?
            .is_none()
    {
        return Ok(base);
    }

    for number in 2..=9_999 {
        let suffix = format!("-{number}");
        let base_length = (48 - suffix.len()).min(base.len());
        let root = base[..base_length].trim_end_matches('-');
        let candidate = format!("{root}{suffix}");
        if store::deck_by_slug(&state.pool, &candidate)
            .await
            .map_err(ApiError::internal)?
            .is_none()
        {
            return Ok(candidate);
        }
    }
    Err(ApiError::conflict(
        "Could not derive an unused slug. Provide one explicitly.",
    ))
}

fn validate_source(source: &str) -> Result<(), ApiError> {
    if source.trim().is_empty() {
        return Err(ApiError::validation(
            "The presentation source cannot be empty.",
        ));
    }
    parse_deck(source).map_err(|error| ApiError::validation(error.to_string()))?;
    Ok(())
}

fn merged_theme(existing: Option<&Deck>, update: ThemeInput) -> Result<Theme, ApiError> {
    let legacy_fonts = update.font.as_deref().map(legacy_font_pair).transpose()?;
    let theme = Theme {
        headline_font: update
            .headline_font
            .or_else(|| legacy_fonts.map(|fonts| fonts.0.into()))
            .or_else(|| existing.map(|deck| deck.theme_headline_font.clone()))
            .unwrap_or_else(|| DEFAULT_HEADLINE_FONT.into()),
        text_font: update
            .text_font
            .or_else(|| legacy_fonts.map(|fonts| fonts.1.into()))
            .or_else(|| existing.map(|deck| deck.theme_text_font.clone()))
            .unwrap_or_else(|| DEFAULT_TEXT_FONT.into()),
        code_font: update
            .code_font
            .or_else(|| existing.map(|deck| deck.theme_code_font.clone()))
            .unwrap_or_else(|| DEFAULT_CODE_FONT.into()),
        background: update.background.unwrap_or_else(|| {
            existing
                .map(|deck| deck.theme_background.clone())
                .unwrap_or_else(|| DEFAULT_THEME_BACKGROUND.into())
        }),
        text: update.text.unwrap_or_else(|| {
            existing
                .map(|deck| deck.theme_text.clone())
                .unwrap_or_else(|| DEFAULT_THEME_TEXT.into())
        }),
        accent: update.accent.unwrap_or_else(|| {
            existing
                .map(|deck| deck.theme_accent.clone())
                .unwrap_or_else(|| DEFAULT_THEME_ACCENT.into())
        }),
    };
    if !valid_headline_font(&theme.headline_font) {
        return Err(ApiError::validation(format!(
            "Theme headline_font must be one of: {}.",
            HEADLINE_FONT_IDS.join(", ")
        )));
    }
    if !valid_text_font(&theme.text_font) {
        return Err(ApiError::validation(format!(
            "Theme text_font must be one of: {}.",
            TEXT_FONT_IDS.join(", ")
        )));
    }
    if !valid_code_font(&theme.code_font) {
        return Err(ApiError::validation(format!(
            "Theme code_font must be one of: {}.",
            CODE_FONT_IDS.join(", ")
        )));
    }
    if [&theme.background, &theme.text, &theme.accent]
        .into_iter()
        .any(|color| !valid_color(color))
    {
        return Err(ApiError::validation(
            "Theme colors must use #RRGGBB format.",
        ));
    }
    Ok(theme)
}

fn legacy_font_pair(font: &str) -> Result<(&'static str, &'static str), ApiError> {
    match font {
        "system" => Ok(("inter", "inter")),
        "serif" => Ok(("georgia", "georgia")),
        "mono" => Ok(("system-mono", "system-mono")),
        _ => Err(ApiError::validation(
            "Legacy theme font must be system, serif, or mono. Use headline_font, text_font, and code_font for new themes.",
        )),
    }
}

fn valid_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug)]
struct EmbedBundleStats {
    files: usize,
    bytes: u64,
}

#[derive(Debug)]
enum EmbedInstallError {
    Invalid(String),
    Internal(anyhow::Error),
}

fn normalized_bundle_name(bundle: &str) -> Result<String, ApiError> {
    let bundle = bundle.trim();
    let valid = !bundle.is_empty()
        && bundle.len() <= 64
        && bundle.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && byte == b'-')
        });
    if !valid {
        return Err(ApiError::validation(
            "Embed bundle names must use 1-64 lowercase letters, numbers, or hyphens, and must start with a letter or number.",
        ));
    }
    Ok(bundle.into())
}

fn install_embed_bundle(
    embed_root: &FilePath,
    bundle: &str,
    bytes: &[u8],
) -> Result<EmbedBundleStats, EmbedInstallError> {
    fs::create_dir_all(embed_root).map_err(embed_internal)?;
    let upload_id = rand::random::<u64>();
    let temporary = embed_root.join(format!(".{bundle}-{upload_id:016x}.upload"));
    let backup = embed_root.join(format!(".{bundle}-{upload_id:016x}.backup"));
    let destination = embed_root.join(bundle);
    fs::create_dir(&temporary).map_err(embed_internal)?;

    let extraction = extract_embed_archive(bytes, &temporary);
    let stats = match extraction {
        Ok(stats) => stats,
        Err(error) => {
            let _ = fs::remove_dir_all(&temporary);
            return Err(error);
        }
    };

    if destination.exists() {
        fs::rename(&destination, &backup).map_err(|error| {
            let _ = fs::remove_dir_all(&temporary);
            embed_internal(error)
        })?;
    }
    if let Err(error) = fs::rename(&temporary, &destination) {
        let restore_result = backup
            .exists()
            .then(|| fs::rename(&backup, &destination))
            .transpose();
        let _ = fs::remove_dir_all(&temporary);
        return match restore_result {
            Ok(_) => Err(embed_internal(error)),
            Err(restore_error) => Err(embed_internal(anyhow!(
                "could not activate embed bundle: {error}; restoring the previous bundle also failed: {restore_error}"
            ))),
        };
    }
    if backup.exists()
        && let Err(error) = fs::remove_dir_all(&backup)
    {
        tracing::warn!(?error, path = %backup.display(), "could not remove replaced embed bundle");
    }
    Ok(stats)
}

fn extract_embed_archive(
    bytes: &[u8],
    destination: &FilePath,
) -> Result<EmbedBundleStats, EmbedInstallError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| {
        EmbedInstallError::Invalid("The request body is not a valid ZIP archive.".into())
    })?;
    if archive.len() > MAX_EMBED_BUNDLE_FILES {
        return Err(EmbedInstallError::Invalid(format!(
            "Embed ZIPs cannot contain more than {MAX_EMBED_BUNDLE_FILES} entries."
        )));
    }
    let mut stats = EmbedBundleStats { files: 0, bytes: 0 };
    let mut contains_html = false;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|_| {
            EmbedInstallError::Invalid("The embed ZIP contains an unreadable entry.".into())
        })?;
        let path = safe_embed_entry_path(entry.name())?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(EmbedInstallError::Invalid(
                "Embed ZIPs cannot contain symbolic links.".into(),
            ));
        }

        let output_path = destination.join(&path);
        if entry.is_dir() {
            fs::create_dir_all(&output_path).map_err(embed_internal)?;
            continue;
        }

        stats.files += 1;
        let is_html = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")
            });
        contains_html |= is_html;
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(embed_internal)?;
        }
        let mut output = fs::File::create(&output_path).map_err(embed_internal)?;
        let remaining = MAX_EMBED_BUNDLE_BYTES.saturating_sub(stats.bytes);
        let copied = std::io::copy(&mut entry.by_ref().take(remaining + 1), &mut output)
            .map_err(embed_internal)?;
        if copied > remaining {
            return Err(EmbedInstallError::Invalid(
                "Uncompressed embed bundles cannot exceed 100 MiB.".into(),
            ));
        }
        if is_html && copied > MAX_EMBED_HTML_BYTES {
            return Err(EmbedInstallError::Invalid(
                "Individual embed HTML files cannot exceed 4 MiB.".into(),
            ));
        }
        stats.bytes += copied;
    }

    if stats.files == 0 {
        return Err(EmbedInstallError::Invalid(
            "The embed ZIP must contain at least one file.".into(),
        ));
    }
    if !contains_html {
        return Err(EmbedInstallError::Invalid(
            "The embed ZIP must contain an HTML file.".into(),
        ));
    }
    Ok(stats)
}

fn safe_embed_entry_path(name: &str) -> Result<PathBuf, EmbedInstallError> {
    if name.is_empty()
        || name.contains(['\\', ':'])
        || name.chars().any(char::is_control)
        || name.len() > 1024
    {
        return Err(EmbedInstallError::Invalid(
            "The embed ZIP contains an unsafe filename.".into(),
        ));
    }
    let path = FilePath::new(name);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(EmbedInstallError::Invalid(
            "The embed ZIP contains an unsafe path.".into(),
        ));
    }
    Ok(path.to_owned())
}

fn embed_internal(error: impl Into<anyhow::Error>) -> EmbedInstallError {
    EmbedInstallError::Internal(error.into())
}

#[cfg(test)]
mod tests {
    use std::{io::Write, sync::Arc};

    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::{Value, json};
    use sqlx::SqlitePool;
    use tower::ServiceExt;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    use crate::live::LiveHub;

    use super::*;

    async fn test_app() -> (tempfile::TempDir, SqlitePool, Router) {
        let directory = tempfile::tempdir().unwrap();
        let database_url = format!("sqlite://{}", directory.path().join("slides.db").display());
        let pool = store::connect(&database_url).await.unwrap();
        store::replace_api_token(&pool, &hash("slides_test_token"), "slides_test")
            .await
            .unwrap();
        let state = AppState {
            pool: pool.clone(),
            hub: Arc::new(LiveHub::default()),
            admin_password_hash: hash("password"),
            admin_cookie: hash("cookie"),
            secure_cookies: false,
            embed_dir: directory.path().join("embeds"),
        };
        (directory, pool, router().with_state(state))
    }

    fn request(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        builder = builder.header(header::AUTHORIZATION, "Bearer slides_test_token");
        if body.is_some() {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        builder
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap()
    }

    async fn json_body(response: Response) -> Value {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn embed_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (path, contents) in entries {
            writer.start_file(*path, options).unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[tokio::test]
    async fn uploads_an_iframe_bundle() {
        let (directory, _pool, app) = test_app().await;
        let bundle = embed_zip(&[
            (
                "index.html",
                b"<!doctype html><script src=\"app.js\"></script>",
            ),
            ("app.js", b"document.body.textContent = 'Ready';"),
        ]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/embeds/demo")
                    .header(header::AUTHORIZATION, "Bearer slides_test_token")
                    .header(header::CONTENT_TYPE, "application/zip")
                    .body(Body::from(bundle))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = json_body(response).await;
        assert_eq!(body["url_prefix"], "/assets/embeds/demo/");
        assert_eq!(
            fs::read_to_string(directory.path().join("embeds/demo/index.html")).unwrap(),
            "<!doctype html><script src=\"app.js\"></script>"
        );
    }

    #[test]
    fn rejects_unsafe_embed_paths() {
        assert!(safe_embed_entry_path("../secret.html").is_err());
        assert!(safe_embed_entry_path("folder\\secret.html").is_err());
        assert!(safe_embed_entry_path("/absolute.html").is_err());
    }

    #[tokio::test]
    async fn requires_a_bearer_token() {
        let (_directory, _pool, app) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/presentations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer"
        );
        assert_eq!(json_body(response).await["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn creates_reads_updates_lists_and_deletes_presentations() {
        let (_directory, _pool, app) = test_app().await;
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                "/presentations",
                Some(json!({
                    "title": "API deck",
                    "slug": "api-deck",
                    "source": "# Created through the API",
                    "theme": { "font": "mono", "accent": "#89b4fa" }
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/api/v1/presentations/api-deck"
        );
        let created = json_body(response).await;
        assert_eq!(created["slug"], "api-deck");
        assert_eq!(created["theme"]["font"], "mono");
        assert_eq!(created["theme"]["headline_font"], "system-mono");
        assert_eq!(created["theme"]["text_font"], "system-mono");
        assert_eq!(created["theme"]["code_font"], DEFAULT_CODE_FONT);
        assert_eq!(created["theme"]["background"], DEFAULT_THEME_BACKGROUND);

        let response = app
            .clone()
            .oneshot(request("GET", "/presentations", None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await["presentations"][0]["slug"],
            "api-deck"
        );

        let response = app
            .clone()
            .oneshot(request(
                "PATCH",
                "/presentations/api-deck",
                Some(json!({
                    "title": "Updated API deck",
                    "source": "# Updated\n\n```mermaid\nflowchart TD\n    A --> B\n```",
                    "theme": {
                        "headline_font": "bebas-neue",
                        "text_font": "inter",
                        "code_font": "system-mono"
                    }
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let updated = json_body(response).await;
        assert_eq!(updated["title"], "Updated API deck");
        assert!(updated["source"].as_str().unwrap().contains("mermaid"));
        assert_eq!(updated["theme"]["font"], "system");
        assert_eq!(updated["theme"]["headline_font"], "bebas-neue");
        assert_eq!(updated["theme"]["text_font"], "inter");
        assert_eq!(updated["theme"]["code_font"], "system-mono");

        let response = app
            .clone()
            .oneshot(request(
                "PATCH",
                "/presentations/api-deck",
                Some(json!({ "theme": updated["theme"].clone() })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(request("GET", "/presentations/api-deck", None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json_body(response).await["title"], "Updated API deck");

        let response = app
            .oneshot(request("DELETE", "/presentations/api-deck", None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn rejects_deleting_a_presentation_with_an_active_session() {
        let (_directory, pool, app) = test_app().await;
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                "/presentations",
                Some(json!({
                    "title": "Live API deck",
                    "slug": "live-api-deck",
                    "source": "# Live",
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let deck = store::deck_by_slug(&pool, "live-api-deck")
            .await
            .unwrap()
            .unwrap();
        let version_id = store::save_and_publish_deck(
            &pool,
            deck.id,
            &deck.title,
            &deck.draft_source,
            &deck.draft_source,
            &Theme::from(&deck),
        )
        .await
        .unwrap();
        store::start_session(&pool, deck.id, version_id)
            .await
            .unwrap();

        let response = app
            .oneshot(request("DELETE", "/presentations/live-api-deck", None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(json_body(response).await["error"]["code"], "conflict");
    }

    #[tokio::test]
    async fn rejects_invalid_markdown_and_unknown_fields() {
        let (_directory, _pool, app) = test_app().await;
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                "/presentations",
                Some(json!({
                    "title": "Broken",
                    "source": "---",
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let response = app
            .oneshot(request(
                "POST",
                "/presentations",
                Some(json!({
                    "title": "Unknown field",
                    "source": "# Valid",
                    "publish": true,
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json_body(response).await["error"]["code"], "invalid_json");
    }
}
