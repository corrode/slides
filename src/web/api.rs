use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    markdown::parse_deck,
    models::{
        DEFAULT_THEME_ACCENT, DEFAULT_THEME_BACKGROUND, DEFAULT_THEME_TEXT, Deck, DeckSummary,
    },
    store,
};

use super::{AppState, hash};

const MAX_API_BODY_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_FONT: &str = "system";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/presentations", get(list).post(create))
        .route(
            "/presentations/{slug}",
            get(get_one).patch(update).delete(delete),
        )
        .layer(DefaultBodyLimit::max(MAX_API_BODY_BYTES))
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
    background: Option<String>,
    text: Option<String>,
    accent: Option<String>,
}

impl ThemeInput {
    fn is_empty(&self) -> bool {
        self.font.is_none()
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
            theme: PresentationTheme {
                font: deck.theme_font.clone(),
                background: deck.theme_background.clone(),
                text: deck.theme_text.clone(),
                accent: deck.theme_accent.clone(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct PresentationTheme {
    font: String,
    background: String,
    text: String,
    accent: String,
}

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

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<PresentationList>, ApiError> {
    authorize(&state, &headers).await?;
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
    headers: HeaderMap,
    payload: Result<Json<CreatePresentation>, JsonRejection>,
) -> Result<Response, ApiError> {
    authorize(&state, &headers).await?;
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

    let deck = store::create_deck_with_content(
        &state.pool,
        &slug,
        &title,
        &payload.source,
        &theme.font,
        &theme.background,
        &theme.text,
        &theme.accent,
    )
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
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<Json<Presentation>, ApiError> {
    authorize(&state, &headers).await?;
    let deck = required_deck(&state, &slug).await?;
    Ok(Json(Presentation::from(&deck)))
}

async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    payload: Result<Json<UpdatePresentation>, JsonRejection>,
) -> Result<Json<Presentation>, ApiError> {
    authorize(&state, &headers).await?;
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
    deck.theme_font = theme.font;
    deck.theme_background = theme.background;
    deck.theme_text = theme.text;
    deck.theme_accent = theme.accent;

    store::save_deck(
        &state.pool,
        deck.id,
        &deck.title,
        &deck.draft_source,
        &deck.theme_font,
        &deck.theme_background,
        &deck.theme_text,
        &deck.theme_accent,
    )
    .await
    .map_err(ApiError::internal)?;

    Ok(Json(Presentation::from(&deck)))
}

async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &headers).await?;
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

fn merged_theme(
    existing: Option<&Deck>,
    update: ThemeInput,
) -> Result<PresentationTheme, ApiError> {
    let theme = PresentationTheme {
        font: update.font.unwrap_or_else(|| {
            existing
                .map(|deck| deck.theme_font.clone())
                .unwrap_or_else(|| DEFAULT_FONT.into())
        }),
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
    if !matches!(theme.font.as_str(), "system" | "serif" | "mono") {
        return Err(ApiError::validation(
            "Theme font must be system, serif, or mono.",
        ));
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

fn valid_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::{Value, json};
    use sqlx::SqlitePool;
    use tower::ServiceExt;

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
                    "source": "# Updated\n\n```mermaid\nflowchart TD\n    A --> B\n```"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let updated = json_body(response).await;
        assert_eq!(updated["title"], "Updated API deck");
        assert!(updated["source"].as_str().unwrap().contains("mermaid"));

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
            &deck.theme_font,
            &deck.theme_background,
            &deck.theme_text,
            &deck.theme_accent,
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
