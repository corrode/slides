use std::{fs, path::Path as FilePath};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use serde_json::json;

use crate::{
    bundle::{self, Bundle, BundleError},
    models::{Deck, DeckSummary, legacy_font_id},
    store,
};

use super::{AppState, hash};

const MAX_BUNDLE_UPLOAD_BYTES: usize = 20 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/presentations", get(list))
        .route("/presentations/{slug}", get(get_one).delete(delete))
        .route(
            "/presentations/{slug}/bundle",
            post(upload_bundle).layer(DefaultBodyLimit::max(MAX_BUNDLE_UPLOAD_BYTES)),
        )
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
struct BundleUpload {
    slug: String,
    title: String,
    files: usize,
    bytes: u64,
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

    fn too_large(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "payload_too_large",
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

impl From<BundleError> for ApiError {
    fn from(error: BundleError) -> Self {
        match error {
            BundleError::Invalid(message) => Self::validation(message),
            BundleError::TooLarge(message) => Self::too_large(message),
            BundleError::Internal(error) => Self::internal(error),
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

async fn upload_bundle(
    State(state): State<AppState>,
    _authorization: ApiAuthorization,
    Path(slug): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let slug = normalized_slug(&slug)?;
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if !content_type.is_some_and(|value| value.eq_ignore_ascii_case("application/zip")) {
        return Err(ApiError {
            status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
            code: "unsupported_media_type",
            message: "Bundles must use Content-Type: application/zip.".into(),
        });
    }
    let body = Bytes::from_request(request, &state)
        .await
        .map_err(|error| {
            if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                ApiError::too_large("ZIP upload exceeds 20 MiB.")
            } else {
                ApiError::validation(error.body_text())
            }
        })?;

    // Dropping the request must not cancel a DB commit or remove committed assets.
    // This task owns installation and explicit rollback, even after disconnection.
    let (created, upload) = tokio::spawn(async move {
        let generation = format!("bundle-{:032x}", rand::random::<u128>());
        let embed_dir = state.embed_dir.clone();
        let extract_generation = generation.clone();
        let (bundle, destination) = tokio::task::spawn_blocking(move || {
            install_bundle_files(&embed_dir, &extract_generation, &body)
        })
        .await
        .map_err(ApiError::internal)??;

        let created = match store::install_bundle_draft(
            &state.pool,
            &slug,
            &bundle.title,
            &bundle.source,
            &generation,
        )
        .await
        {
            Ok(created) => created,
            Err(error) => {
                let _ = tokio::task::spawn_blocking(move || remove_directory(&destination)).await;
                return Err(ApiError::internal(error));
            }
        };
        Ok::<_, ApiError>((
            created,
            BundleUpload {
                slug,
                title: bundle.title,
                files: bundle.files,
                bytes: bundle.bytes,
            },
        ))
    })
    .await
    .map_err(ApiError::internal)??;

    let location = HeaderValue::from_str(&format!("/api/v1/presentations/{}", upload.slug))
        .map_err(ApiError::internal)?;
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        [(header::LOCATION, location)],
        Json(upload),
    )
        .into_response())
}

fn install_bundle_files(
    embed_root: &FilePath,
    generation: &str,
    bytes: &[u8],
) -> Result<(Bundle, std::path::PathBuf), ApiError> {
    fs::create_dir_all(embed_root).map_err(ApiError::internal)?;
    let embed_root = fs::canonicalize(embed_root).map_err(ApiError::internal)?;
    let parent = embed_root.parent().ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!("embed root must have a parent directory"))
    })?;
    let temporary = parent.join(format!(".{generation}.upload"));
    let destination = embed_root.join(generation);
    fs::create_dir(&temporary).map_err(ApiError::internal)?;
    let result = (|| {
        let bundle = bundle::extract(bytes, &temporary, generation).map_err(ApiError::from)?;
        // Generations are immutable; never replace an existing directory.
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(ApiError::internal(anyhow::anyhow!(
                "bundle generation already exists"
            )));
        }
        fs::rename(&temporary, &destination).map_err(ApiError::internal)?;
        Ok((bundle, destination))
    })();
    if result.is_err() {
        remove_directory(&temporary);
    }
    result
}

fn remove_directory(path: &FilePath) {
    if let Err(error) = fs::remove_dir_all(path) {
        tracing::warn!(
            ?error,
            ?path,
            "Could not clean up failed bundle installation"
        );
    }
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

async fn get_one(
    State(state): State<AppState>,
    _authorization: ApiAuthorization,
    Path(slug): Path<String>,
) -> Result<Json<Presentation>, ApiError> {
    let deck = required_deck(&state, &slug).await?;
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

fn normalized_slug(slug: &str) -> Result<String, ApiError> {
    const RESERVED: &[&str] = &[
        "admin", "api", "assets", "healthz", "join", "present", "sessions", "shared",
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
            "Slugs must be 1–48 lowercase letters, numbers, or hyphens, and not reserved.",
        ));
    }
    Ok(slug)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Cursor, Write},
        sync::Arc,
    };

    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::Value;
    use sqlx::SqlitePool;
    use tower::ServiceExt;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    use crate::{live::LiveHub, models::Theme};

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
        (directory, pool, super::super::router(state))
    }

    fn request(method: &str, path: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(format!("/api/v1{path}"))
            .header(header::AUTHORIZATION, "Bearer slides_test_token")
            .body(Body::empty())
            .unwrap()
    }

    fn upload(slug: &str, bytes: Vec<u8>) -> Request<Body> {
        let mut request = request("POST", &format!("/presentations/{slug}/bundle"));
        request.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/zip"),
        );
        *request.body_mut() = Body::from(bytes);
        request
    }

    async fn json_body(response: Response) -> Value {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn bundle_zip(source: &str, html: &str) -> Vec<u8> {
        bundle_zip_files(&[
            ("slides.md", source.as_bytes()),
            ("demo.html", html.as_bytes()),
        ])
    }

    fn bundle_zip_files(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (path, contents) in files {
            writer.start_file(*path, options).unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn admin_form(slug: &str, action: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(format!("/admin/decks/{slug}/{action}"))
            .header(header::COOKIE, format!("slides_admin={}", hash("cookie")))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body.to_owned()))
            .unwrap()
    }

    async fn publish_stored_draft(pool: &SqlitePool, deck: &Deck) -> i64 {
        store::save_and_publish_deck(
            pool,
            deck.id,
            &deck.title,
            &deck.draft_source,
            &deck.draft_source,
            &Theme::from(deck),
        )
        .await
        .unwrap()
    }

    fn deck_form(title: &str, source: &str, theme: &Theme) -> String {
        [
            ("title", title),
            ("source", source),
            ("headline_font", &theme.headline_font),
            ("text_font", &theme.text_font),
            ("code_font", &theme.code_font),
            ("background", &theme.background),
            ("text", &theme.text),
            ("accent", &theme.accent),
        ]
        .into_iter()
        .map(|(name, value)| {
            let encoded: String = value.bytes().map(|byte| format!("%{byte:02X}")).collect();
            format!("{name}={encoded}")
        })
        .collect::<Vec<_>>()
        .join("&")
    }

    fn admin_get(path: &str) -> Request<Body> {
        Request::builder()
            .uri(path)
            .header(header::COOKIE, format!("slides_admin={}", hash("cookie")))
            .body(Body::empty())
            .unwrap()
    }

    async fn html_body(response: Response) -> String {
        String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    fn generations(directory: &FilePath) -> Vec<std::path::PathBuf> {
        let mut paths: Vec<_> = fs::read_dir(directory.join("embeds"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        paths.sort();
        paths
    }

    fn assert_no_temporary_directories(directory: &FilePath) {
        assert!(fs::read_dir(directory).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".upload")
        }));
    }

    #[tokio::test]
    async fn authenticates_before_reading_upload_body() {
        let (_directory, _pool, app) = test_app().await;
        for token in [None, Some("Bearer wrong"), Some("Basic slides_test_token")] {
            let mut req = upload("demo", vec![0; MAX_BUNDLE_UPLOAD_BYTES + 1]);
            req.headers_mut().remove(header::AUTHORIZATION);
            if let Some(token) = token {
                req.headers_mut()
                    .insert(header::AUTHORIZATION, HeaderValue::from_static(token));
            }
            let response = app.clone().oneshot(req).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
        }
        for (method, path) in [
            ("GET", "/presentations"),
            ("GET", "/presentations/demo"),
            ("DELETE", "/presentations/demo"),
        ] {
            let mut req = request(method, path);
            req.headers_mut().remove(header::AUTHORIZATION);
            assert_eq!(
                app.clone().oneshot(req).await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
        }
    }

    #[tokio::test]
    async fn creates_replaces_reads_lists_and_deletes_bundles() {
        let (directory, pool, app) = test_app().await;
        let source = "# First\n\n:::iframe\nsrc=\"demo.html\"\ntitle=\"Demo\"\n:::\n";
        let response = app
            .clone()
            .oneshot(upload("demo", bundle_zip(source, "first")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/api/v1/presentations/demo"
        );
        assert_eq!(
            json_body(response).await,
            json!({
                "slug": "demo", "title": "First", "files": 2, "bytes": source.len() + 5,
            })
        );
        let old_generation = generations(directory.path()).pop().unwrap();
        let generation = old_generation.file_name().unwrap().to_str().unwrap();
        assert!(generation.starts_with("bundle-"));
        assert_eq!(generation.len(), 39);
        assert!(generation[7..].bytes().all(|b| b.is_ascii_hexdigit()));
        let deck = store::deck_by_slug(&pool, "demo").await.unwrap().unwrap();
        assert!(
            deck.draft_source
                .contains(&format!("/assets/embeds/{generation}/demo.html"))
        );

        let response = app
            .clone()
            .oneshot(upload("demo", bundle_zip("# Second", "second")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/api/v1/presentations/demo"
        );
        assert_eq!(json_body(response).await["title"], "Second");
        assert_eq!(generations(directory.path()).len(), 2);
        assert_eq!(
            fs::read_to_string(old_generation.join("demo.html")).unwrap(),
            "first"
        );
        assert_no_temporary_directories(directory.path());

        let response = app
            .clone()
            .oneshot(request("GET", "/presentations/demo"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["title"], "Second");
        assert_eq!(body["source"], "# Second");
        assert_eq!(
            body["theme"],
            serde_json::to_value(PresentationTheme::from(&deck)).unwrap()
        );
        let response = app
            .clone()
            .oneshot(request("GET", "/presentations"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await["presentations"][0]["slug"],
            "demo"
        );
        assert_eq!(
            app.clone()
                .oneshot(request("DELETE", "/presentations/demo"))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            app.oneshot(request("GET", "/presentations/demo"))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn replacing_bundle_preserves_published_source_and_generation_assets() {
        let (directory, pool, app) = test_app().await;
        let source_a =
            "# A\n\n![Image](image.svg)\n\n:::iframe\nsrc=\"demo.html\"\ntitle=\"Demo\"\n:::\n";
        let image_a = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><text>A</text></svg>";
        let html_a = b"<!doctype html><html><body>A</body></html>";
        let files_a: &[(&str, &[u8])] = &[
            ("slides.md", source_a.as_bytes()),
            ("image.svg", image_a),
            ("demo.html", html_a),
        ];
        assert_eq!(
            app.clone()
                .oneshot(upload("demo", bundle_zip_files(files_a)))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        let deck_a = store::deck_by_slug(&pool, "demo").await.unwrap().unwrap();
        let generation_a = generations(directory.path()).pop().unwrap();
        let name_a = generation_a.file_name().unwrap().to_str().unwrap();
        for asset in ["image.svg", "demo.html"] {
            assert!(
                deck_a
                    .draft_source
                    .contains(&format!("/assets/embeds/{name_a}/{asset}"))
            );
        }
        let version_a = publish_stored_draft(&pool, &deck_a).await;

        let source_b = source_a.replacen("# A", "# B", 1);
        let image_b = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><text>B</text></svg>";
        let html_b = b"<!doctype html><html><body>B</body></html>";
        assert_eq!(
            app.clone()
                .oneshot(upload(
                    "demo",
                    bundle_zip_files(&[
                        ("slides.md", source_b.as_bytes()),
                        ("image.svg", image_b),
                        ("demo.html", html_b),
                    ])
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let deck_b = store::deck_by_slug(&pool, "demo").await.unwrap().unwrap();
        assert_eq!(deck_b.id, deck_a.id);
        assert_eq!(deck_b.title, "B");
        let paths = generations(directory.path());
        assert_eq!(paths.len(), 2);
        let generation_b = paths.iter().find(|path| **path != generation_a).unwrap();
        let name_b = generation_b.file_name().unwrap().to_str().unwrap();
        for asset in ["image.svg", "demo.html"] {
            assert!(
                deck_b
                    .draft_source
                    .contains(&format!("/assets/embeds/{name_b}/{asset}"))
            );
        }
        assert!(!deck_b.draft_source.contains(name_a));
        let published_a = store::get_version(&pool, version_a).await.unwrap();
        assert_eq!(published_a.title, "A");
        assert_eq!(published_a.source, deck_a.draft_source);
        assert!(!published_a.source.contains(name_b));
        for (path, contents) in files_a {
            assert_eq!(fs::read(generation_a.join(path)).unwrap(), *contents);
        }
        assert_eq!(fs::read(generation_b.join("image.svg")).unwrap(), image_b);
        assert_eq!(fs::read(generation_b.join("demo.html")).unwrap(), html_b);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/assets/embeds/{name_a}/image.svg"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .as_ref(),
            image_a
        );
        assert_no_temporary_directories(directory.path());
    }

    #[tokio::test]
    async fn bundle_browser_edits_publish_submitted_source_and_preserve_live_snapshot() {
        let (directory, pool, app) = test_app().await;
        let source = "# Imported\n\n![Image](image.svg)\n\n```rust code/main.rs\n```\n\n:::iframe\nsrc=\"demo.html\"\ntitle=\"Demo\"\n:::\n";
        let image = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><text>Original</text></svg>";
        let demo = b"<!doctype html><html><body>Original demo</body></html>";
        assert_eq!(
            app.clone()
                .oneshot(upload(
                    "demo",
                    bundle_zip_files(&[
                        ("slides.md", source.as_bytes()),
                        ("image.svg", image),
                        ("demo.html", demo),
                        (
                            "code/main.rs",
                            b"fn main() { println!(\"original & code\"); }"
                        ),
                    ])
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        let imported = store::deck_by_slug(&pool, "demo").await.unwrap().unwrap();
        let paths = generations(directory.path());
        let generation = paths[0].file_name().unwrap().to_str().unwrap();
        let response = app
            .clone()
            .oneshot(admin_get("/admin/decks/demo/edit"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let editor = html_body(response).await;
        // Inspect the editable control, not a matching string in the rendered preview.
        let textarea = editor.split_once("<textarea ").unwrap().1;
        let (attributes, contents) = textarea.split_once('>').unwrap();
        assert!(attributes.contains("name=\"source\""));
        assert!(!attributes.contains("readonly"));
        assert!(!attributes.contains("disabled"));
        let editable =
            html_escape::decode_html_entities(contents.split_once("</textarea>").unwrap().0);
        assert_eq!(editable, imported.draft_source);
        assert!(editable.contains("```rust\nfn main()"));
        assert!(!editable.contains("code/main.rs"));
        for asset in ["image.svg", "demo.html"] {
            assert!(editable.contains(&format!("/assets/embeds/{generation}/{asset}")));
        }
        let theme = Theme {
            headline_font: "georgia".into(),
            text_font: "merriweather".into(),
            code_font: "system-mono".into(),
            background: "#102030".into(),
            text: "#eeeeee".into(),
            accent: "#abcdef".into(),
        };
        let saved_source = editable
            .replace("# Imported", "# Browser saved")
            .replace("original & code", "edited & code");
        let response = app
            .clone()
            .oneshot(admin_form(
                "demo",
                "save",
                &deck_form("Browser title", &saved_source, &theme),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(html_body(response).await.contains("Draft saved."));
        let saved = store::deck_by_slug(&pool, "demo").await.unwrap().unwrap();
        assert_eq!(saved.title, "Browser title");
        assert_eq!(saved.draft_source, saved_source);
        assert_eq!(Theme::from(&saved).style(), theme.style());
        assert_eq!(generations(directory.path()), paths);

        let printed_source = saved_source.replace("# Browser saved", "# Print submitted");
        let response = app
            .clone()
            .oneshot(admin_form(
                "demo",
                "print",
                &deck_form("Print title", &printed_source, &theme),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let printed = html_body(response).await;
        assert!(printed.contains("Print title"));
        assert!(printed.contains("Print submitted"));
        assert!(!printed.contains("Browser saved"));
        assert!(printed.contains("#102030"));
        assert_eq!(
            store::deck_by_slug(&pool, "demo")
                .await
                .unwrap()
                .unwrap()
                .draft_source,
            saved_source
        );

        let published_source = saved_source.replace("# Browser saved", "# Publish submitted");
        let response = app
            .clone()
            .oneshot(admin_form(
                "demo",
                "publish",
                &deck_form("Published title", &published_source, &theme),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/admin/decks/demo/edit?published=1"
        );
        let published_id: i64 =
            sqlx::query_scalar("SELECT id FROM deck_versions WHERE deck_id = ?")
                .bind(imported.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let published = store::get_version(&pool, published_id).await.unwrap();
        assert_eq!(published.title, "Published title");
        assert_eq!(published.source, published_source);
        assert_eq!(Theme::from(&published).style(), theme.style());
        assert_eq!(
            store::deck_by_slug(&pool, "demo")
                .await
                .unwrap()
                .unwrap()
                .draft_source,
            published_source
        );

        let live_source = published_source.replace("# Publish submitted", "# Live submitted");
        let response = app
            .clone()
            .oneshot(admin_form(
                "demo",
                "sessions",
                &deck_form("Live title", &live_source, &theme),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let presenter_url = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        let active = store::active_session(&pool).await.unwrap().unwrap();
        let session = store::get_session(&pool, active.id).await.unwrap();
        let live_id = session.deck_version_id;
        assert_ne!(live_id, published_id);
        assert_eq!(presenter_url, format!("/present/{}", session.code));

        for (action, expected) in [("save", StatusCode::OK), ("publish", StatusCode::SEE_OTHER)] {
            let response = app
                .clone()
                .oneshot(admin_form(
                    "demo",
                    action,
                    &deck_form("Later title", "# Later draft", &Theme::default()),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{action}");
            if action == "save" {
                assert!(
                    html_body(response)
                        .await
                        .contains("Changes apply to the next session.")
                );
            }
            let live = store::get_version(&pool, live_id).await.unwrap();
            assert_eq!(live.source, live_source);
            assert_eq!(live.title, "Live title");
            assert_eq!(Theme::from(&live).style(), theme.style());
        }
        assert_eq!(
            app.clone()
                .oneshot(upload(
                    "demo",
                    bundle_zip("# Reuploaded", "replacement demo")
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            store::deck_by_slug(&pool, "demo")
                .await
                .unwrap()
                .unwrap()
                .draft_source,
            "# Reuploaded"
        );
        assert_eq!(generations(directory.path()).len(), 2);
        let live_session = store::get_session(&pool, active.id).await.unwrap();
        assert_eq!(live_session.deck_version_id, live_id);
        let live = store::get_version(&pool, live_id).await.unwrap();
        assert_eq!(live.source, live_source);
        assert_eq!(live.title, "Live title");
        assert_eq!(Theme::from(&live).style(), theme.style());
        assert_eq!(
            store::get_version(&pool, published_id)
                .await
                .unwrap()
                .source,
            published_source
        );
        let response = app
            .clone()
            .oneshot(admin_get(&presenter_url))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let presenter = html_body(response).await;
        assert!(presenter.contains("Live submitted"));
        assert!(!presenter.contains("Reuploaded"));
        assert!(!presenter.contains("Later draft"));
        for (asset, original) in [
            ("image.svg", image.as_slice()),
            ("demo.html", demo.as_slice()),
        ] {
            assert_eq!(fs::read(paths[0].join(asset)).unwrap(), original);
            let response = app
                .clone()
                .oneshot(admin_get(&format!("/assets/embeds/{generation}/{asset}")))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = html_body(response).await;
            if asset == "image.svg" {
                assert_eq!(body.as_bytes(), original);
            } else {
                // HTML assets receive the navigation bridge when served.
                assert!(body.contains("Original demo"));
                assert!(!body.contains("replacement demo"));
                assert!(body.contains("data-slides-navigation-bridge"));
            }
        }
        assert_no_temporary_directories(directory.path());
    }

    #[tokio::test]
    async fn bundle_browser_validates_forms_and_keeps_invalid_markdown_as_draft_only() {
        let (directory, pool, app) = test_app().await;
        assert_eq!(
            app.clone()
                .oneshot(upload("demo", bundle_zip("# Original", "original")))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        let before = store::deck_by_slug(&pool, "demo").await.unwrap().unwrap();
        let paths = generations(directory.path());
        let theme = Theme::from(&before);
        for action in ["save", "print", "publish", "sessions"] {
            for (body, expected) in [
                (String::new(), StatusCode::UNPROCESSABLE_ENTITY),
                (deck_form("", "# Changed", &theme), StatusCode::BAD_REQUEST),
                (
                    deck_form("Changed", "# Changed", &theme)
                        .replace("headline_font=", "headline_font=unsupported"),
                    StatusCode::BAD_REQUEST,
                ),
                (
                    deck_form("Changed", "# Changed", &theme)
                        .replace("background=", "background=invalid"),
                    StatusCode::BAD_REQUEST,
                ),
            ] {
                let response = app
                    .clone()
                    .oneshot(admin_form("demo", action, &body))
                    .await
                    .unwrap();
                assert_eq!(response.status(), expected, "{action}: {body}");
                let after = store::deck_by_slug(&pool, "demo").await.unwrap().unwrap();
                assert_eq!(
                    serde_json::to_value(Presentation::from(&after)).unwrap(),
                    serde_json::to_value(Presentation::from(&before)).unwrap()
                );
            }
        }
        let invalid = "# Invalid draft\n\n:::notes\nMissing closing delimiter";
        let response = app
            .clone()
            .oneshot(admin_form(
                "demo",
                "save",
                &deck_form("Invalid draft", invalid, &theme),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(html_body(response).await.contains("Preview unavailable"));
        for action in ["print", "publish", "sessions"] {
            for source in [invalid, ""] {
                let response = app
                    .clone()
                    .oneshot(admin_form(
                        "demo",
                        action,
                        &deck_form("Rejected title", source, &theme),
                    ))
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{action}");
            }
        }
        let draft = store::deck_by_slug(&pool, "demo").await.unwrap().unwrap();
        assert_eq!(draft.title, "Invalid draft");
        assert_eq!(draft.draft_source, invalid);
        assert!(store::active_session(&pool).await.unwrap().is_none());
        let versions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM deck_versions WHERE deck_id = ?")
                .bind(before.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(versions, 0);
        assert_eq!(generations(directory.path()), paths);
        assert_eq!(
            fs::read_to_string(paths[0].join("demo.html")).unwrap(),
            "original"
        );
    }

    #[tokio::test]
    async fn converting_legacy_deck_to_bundle_preserves_published_version() {
        let (_directory, pool, app) = test_app().await;
        let theme = Theme::default();
        let legacy_source = "# Legacy draft";
        let published_source = "# Legacy published\n\nSnapshot before conversion.";
        let legacy =
            store::create_deck_with_content(&pool, "demo", "Legacy", legacy_source, &theme)
                .await
                .unwrap();
        let version_id = store::save_and_publish_deck(
            &pool,
            legacy.id,
            "Legacy",
            legacy_source,
            published_source,
            &theme,
        )
        .await
        .unwrap();

        assert_eq!(
            app.clone()
                .oneshot(upload("demo", bundle_zip("# Bundle", "bundle")))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let converted = store::deck_by_slug(&pool, "demo").await.unwrap().unwrap();
        assert_eq!(converted.id, legacy.id);
        assert_eq!(converted.title, "Bundle");
        assert_eq!(converted.draft_source, "# Bundle");

        assert_eq!(
            app.oneshot(admin_form(
                "demo",
                "publish",
                &deck_form(
                    &converted.title,
                    &converted.draft_source,
                    &Theme::from(&converted)
                )
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::SEE_OTHER
        );
        let versions: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM deck_versions WHERE deck_id = ? ORDER BY version_number",
        )
        .bind(legacy.id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0], version_id);
        let old = store::get_version(&pool, version_id).await.unwrap();
        assert_eq!(old.title, "Legacy");
        assert_eq!(old.source, published_source);
        assert_eq!(old.theme_background, theme.background);
        let new = store::get_version(&pool, versions[1]).await.unwrap();
        assert_eq!(new.title, "Bundle");
        assert_eq!(new.source, converted.draft_source);
    }

    #[tokio::test]
    async fn failed_replacements_preserve_draft_and_assets() {
        let (directory, pool, app) = test_app().await;
        assert_eq!(
            app.clone()
                .oneshot(upload("demo", bundle_zip("# Original", "original")))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        let before = generations(directory.path());
        // Missing iframe target fails after files have been extracted.
        for bytes in [
            b"not a ZIP".to_vec(),
            bundle_zip(
                "# Invalid\n\n:::iframe\nsrc=\"missing.html\"\ntitle=\"Missing\"\n:::\n",
                "bad",
            ),
        ] {
            assert_eq!(
                app.clone()
                    .oneshot(upload("demo", bytes))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
            assert_eq!(generations(directory.path()), before);
            assert_no_temporary_directories(directory.path());
        }
        sqlx::query("CREATE TRIGGER reject_bundle_update BEFORE UPDATE ON decks BEGIN SELECT RAISE(ABORT, 'test failure'); END")
            .execute(&pool).await.unwrap();
        assert_eq!(
            app.oneshot(upload("demo", bundle_zip("# Replacement", "replacement")))
                .await
                .unwrap()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        let deck = store::deck_by_slug(&pool, "demo").await.unwrap().unwrap();
        assert_eq!(deck.title, "Original");
        assert_eq!(deck.draft_source, "# Original");
        assert_eq!(generations(directory.path()), before);
        assert_eq!(
            fs::read_to_string(before[0].join("demo.html")).unwrap(),
            "original"
        );
        assert_no_temporary_directories(directory.path());
    }

    #[tokio::test]
    async fn rejects_content_types_sizes_and_reserved_slugs() {
        let (directory, _pool, app) = test_app().await;
        for content_type in [
            None,
            Some("application/json"),
            Some("application/x-zip-compressed"),
        ] {
            let mut req = upload("demo", Vec::new());
            req.headers_mut().remove(header::CONTENT_TYPE);
            if let Some(value) = content_type {
                req.headers_mut()
                    .insert(header::CONTENT_TYPE, HeaderValue::from_static(value));
            }
            assert_eq!(
                app.clone().oneshot(req).await.unwrap().status(),
                StatusCode::UNSUPPORTED_MEDIA_TYPE
            );
        }
        assert_eq!(
            app.clone()
                .oneshot(upload("demo", vec![0; MAX_BUNDLE_UPLOAD_BYTES + 1]))
                .await
                .unwrap()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let oversized_html = "x".repeat(4 * 1024 * 1024 + 1);
        assert_eq!(
            app.clone()
                .oneshot(upload("demo", bundle_zip("# Too large", &oversized_html)))
                .await
                .unwrap()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert!(generations(directory.path()).is_empty());
        for slug in ["shared", "SHARED", "admin", "bad_slug", "-bad"] {
            assert_eq!(
                app.clone()
                    .oneshot(upload(slug, bundle_zip("# Valid", "ok")))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
        assert_eq!(
            app.oneshot(upload("demo", Vec::new()))
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_no_temporary_directories(directory.path());
    }

    #[tokio::test]
    async fn old_mutation_endpoints_are_removed() {
        let (_directory, _pool, app) = test_app().await;
        for (method, path, expected) in [
            ("POST", "/presentations", StatusCode::METHOD_NOT_ALLOWED),
            (
                "PATCH",
                "/presentations/demo",
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            ("PUT", "/embeds/demo", StatusCode::NOT_FOUND),
        ] {
            assert_eq!(
                app.clone()
                    .oneshot(request(method, path))
                    .await
                    .unwrap()
                    .status(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn rejects_deleting_a_presentation_with_an_active_session() {
        let (_directory, pool, app) = test_app().await;
        assert_eq!(
            app.clone()
                .oneshot(upload("live-api-deck", bundle_zip("# Live", "live")))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        let deck = store::deck_by_slug(&pool, "live-api-deck")
            .await
            .unwrap()
            .unwrap();
        let version_id = publish_stored_draft(&pool, &deck).await;
        store::start_session(&pool, deck.id, version_id)
            .await
            .unwrap();
        let response = app
            .oneshot(request("DELETE", "/presentations/live-api-deck"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(json_body(response).await["error"]["code"], "conflict");
    }
}
