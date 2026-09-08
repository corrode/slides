mod admin;
mod api;
mod playground;
pub(crate) mod render;
mod session;
mod settings;
mod shared;

use std::{path::PathBuf, sync::Arc};

use askama::Template;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::CookieJar;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer, trace::TraceLayer};

use crate::{
    error::{AppError, AppResult},
    live::LiveHub,
    store,
};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub hub: Arc<LiveHub>,
    pub admin_password_hash: String,
    pub admin_cookie: String,
    pub secure_cookies: bool,
    pub embed_dir: PathBuf,
}

pub fn router(state: AppState) -> Router {
    let assets = Router::new()
        .nest_service(
            "/embeds",
            ServeDir::new(state.embed_dir.clone()).fallback(ServeDir::new("assets/embeds")),
        )
        .fallback_service(ServeDir::new("assets"))
        .layer(middleware::from_fn(sandbox_html_asset))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            guard_bundle_assets,
        ));

    Router::new()
        .route("/", get(session::landing))
        .route("/healthz", get(healthz))
        .nest("/api/v1", api::router())
        .route(
            "/api/playground/run",
            post(playground::run).layer(DefaultBodyLimit::max(70 * 1024)),
        )
        .route("/join", post(session::join_code))
        .route("/join/{code}", get(session::audience))
        .route("/admin/login", get(admin::login_page).post(admin::login))
        .route("/admin", get(admin::dashboard))
        .route("/admin/settings", get(settings::page))
        .route(
            "/admin/settings/api-token",
            post(settings::rotate_token),
        )
        .route(
            "/admin/settings/api-token/revoke",
            post(settings::revoke_token),
        )
        .route("/admin/decks/{slug}/delete", post(admin::delete_deck))
        .route("/admin/decks/{slug}/edit", get(admin::editor))
        .route("/admin/decks/{slug}/save", post(admin::save))
        .route("/admin/decks/{slug}/print", post(admin::print_deck))
        .route("/admin/decks/{slug}/publish", post(admin::publish))
        .route("/admin/decks/{slug}/sessions", post(admin::start_session))
        .route("/present/{code}", get(session::presenter))
        .route("/sessions/{code}/events", get(session::events))
        .route("/sessions/{code}/first", post(session::first))
        .route("/sessions/{code}/previous", post(session::previous))
        .route("/sessions/{code}/next", post(session::next))
        .route("/sessions/{code}/attention", post(session::focus_audience))
        .route("/sessions/{code}/hand", post(session::toggle_hand))
        .route("/sessions/{code}/hands/reset", post(session::reset_hands))
        .route("/sessions/{code}/lock", post(session::toggle_lock))
        .route(
            "/sessions/{code}/interaction",
            post(session::interaction_state),
        )
        .route("/sessions/{code}/answer", post(session::answer))
        .route("/sessions/{code}/questions", post(session::ask_question))
        .route(
            "/sessions/{code}/questions/{question_id}/vote",
            post(session::toggle_question_upvote),
        )
        .route(
            "/sessions/{code}/questions/{question_id}/moderate",
            post(session::moderate_question),
        )
        .route("/sessions/{code}/react/{kind}", post(session::react))
        .route("/sessions/{code}/end", post(session::end))
        .route("/admin/sessions/{code}/ended", get(session::ended))
        .route(
            "/admin/sessions/{code}/artifact",
            post(session::create_artifact),
        )
        .route(
            "/admin/sessions/{code}/delete",
            post(admin::delete_ended_session),
        )
        .route("/shared/{token}", get(shared::redirect_to_archive))
        .route("/shared/{token}/", get(shared::archive_page))
        .route("/shared/{token}/download", get(shared::download))
        .route("/shared/{token}/{*path}", get(shared::archive_file))
        .nest("/assets", assets)
        .route("/{slug}", get(session::named_shortlink))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' http: https: data:; font-src 'self'; media-src 'self'; connect-src 'self'; frame-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
            ),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

// Inspect the same decoded path ServeDir sees, without banning ordinary encoded
// filenames in legacy assets. Bundle URLs must use their canonical spelling.
async fn guard_bundle_assets(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let Some(decoded) = decode_asset_path(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if decoded.contains(['\\', '\0']) || decoded.split('/').any(|part| part == "." || part == "..")
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mut components = decoded.split('/').filter(|part| !part.is_empty());
    if components
        .next()
        .is_some_and(|part| part.eq_ignore_ascii_case("embeds"))
        && let Some(generation) = components
            .next()
            .filter(|part| part.to_ascii_lowercase().starts_with("bundle-"))
    {
        if !path.starts_with("/embeds/") || path.contains('%') || path.contains("//") {
            return StatusCode::NOT_FOUND.into_response();
        }
        let asset = components.collect::<Vec<_>>().join("/");
        if asset.eq_ignore_ascii_case("slides.md") {
            return StatusCode::NOT_FOUND.into_response();
        }
        match store::bundle_exists(&state.pool, generation).await {
            Ok(true) => {}
            Ok(false) => return StatusCode::NOT_FOUND.into_response(),
            Err(error) => {
                tracing::error!(?error, "could not check bundle visibility");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }

        // Never let a missing uploaded generation/file use the static fallback.
        // Check directory indexes only after establishing registry visibility.
        let mut file = state.embed_dir.join(generation).join(asset);
        if tokio::fs::metadata(&file)
            .await
            .is_ok_and(|metadata| metadata.is_dir())
        {
            file.push("index.html");
        }
        if !tokio::fs::metadata(file)
            .await
            .is_ok_and(|metadata| metadata.is_file())
        {
            return StatusCode::NOT_FOUND.into_response();
        }
    }
    next.run(request).await
}

fn decode_asset_path(path: &str) -> Option<String> {
    let mut bytes = path.bytes();
    let mut decoded = Vec::with_capacity(path.len());
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = char::from(bytes.next()?).to_digit(16)?;
            let low = char::from(bytes.next()?).to_digit(16)?;
            decoded.push((high * 16 + low) as u8);
        } else {
            decoded.push(byte);
        }
    }
    String::from_utf8(decoded).ok()
}

const MAX_IFRAME_HTML_BYTES: usize = 4 * 1024 * 1024;

async fn sandbox_html_asset(request: Request<Body>, next: Next) -> Response {
    let inject_navigation = request.method() == Method::GET;
    let mut response = next.run(request).await;
    let is_svg = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("image/svg+xml"));
    if is_svg {
        response.headers_mut().extend(iframe_asset_headers());
        return response;
    }
    let is_html = response.status().is_success()
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"));
    if !is_html {
        return response;
    }

    response.headers_mut().extend(iframe_asset_headers());
    let content_length = response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if !inject_navigation
        || response.status() != StatusCode::OK
        || content_length.is_none_or(|length| length > MAX_IFRAME_HTML_BYTES)
    {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_IFRAME_HTML_BYTES).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let body = match String::from_utf8(bytes.to_vec()) {
        Ok(html) => Body::from(add_iframe_navigation_bridge(html)),
        Err(error) => Body::from(error.into_bytes()),
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, body)
}

pub(crate) fn add_iframe_navigation_bridge(mut html: String) -> String {
    const BRIDGE: &str = r#"<script data-slides-navigation-bridge>
(() => {
  if (window.parent === window) return;
  const blocksShortcuts = (target) =>
    target instanceof HTMLElement &&
    (target.matches("input, textarea, select, button, a") || target.isContentEditable);
  document.addEventListener("keydown", (event) => {
    if (
      event.defaultPrevented ||
      event.repeat ||
      event.metaKey ||
      event.ctrlKey ||
      event.altKey ||
      blocksShortcuts(event.target)
    ) return;
    const action = {
      ArrowLeft: "previous",
      PageUp: "previous",
      ArrowRight: "next",
      PageDown: "next",
      Home: "current",
    }[event.key];
    if (!action) return;
    event.preventDefault();
    window.parent.postMessage({ type: "slides:navigate", action }, "*");
  });
})();
</script>"#;

    let lowercase = html.to_ascii_lowercase();
    if let Some(index) = lowercase
        .rfind("</body>")
        .or_else(|| lowercase.rfind("</html>"))
    {
        html.insert_str(index, BRIDGE);
    } else {
        html.push_str(BRIDGE);
    }
    html
}

pub(crate) fn iframe_asset_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; media-src 'self'; connect-src 'none'; worker-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'; sandbox allow-scripts",
        ),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    headers
}

async fn healthz(State(state): State<AppState>) -> StatusCode {
    match store::healthcheck(&state.pool).await {
        Ok(()) => StatusCode::OK,
        Err(error) => {
            tracing::warn!(?error, "health check failed");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

pub fn random_token() -> String {
    format!(
        "{:016x}{:016x}{:016x}{:016x}",
        rand::random::<u64>(),
        rand::random::<u64>(),
        rand::random::<u64>(),
        rand::random::<u64>(),
    )
}

pub fn hash(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn secrets_equal(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0, |difference, (left, right)| difference | (left ^ right))
            == 0
}

pub fn is_admin(jar: &CookieJar, state: &AppState) -> bool {
    jar.get("slides_admin")
        .is_some_and(|cookie| secrets_equal(cookie.value(), &state.admin_cookie))
}

pub fn participant_hash(jar: &CookieJar) -> Option<String> {
    jar.get("slides_participant")
        .map(|cookie| hash(cookie.value()))
}

pub fn require_admin(jar: &CookieJar, state: &AppState) -> AppResult<()> {
    if is_admin(jar, state) {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "Presenter authentication is required.",
        ))
    }
}

pub fn template<T: Template>(value: T) -> AppResult<Response> {
    Ok(Html(value.render()?).into_response())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;

    use crate::{live::LiveHub, store};

    use super::{AppState, add_iframe_navigation_bridge, hash, iframe_asset_headers, router};

    async fn test_state(directory: &tempfile::TempDir) -> AppState {
        let database_url = format!("sqlite://{}", directory.path().join("slides.db").display());
        AppState {
            pool: store::connect(&database_url).await.unwrap(),
            hub: Arc::new(LiveHub::default()),
            admin_password_hash: hash("password"),
            admin_cookie: hash("cookie"),
            secure_cookies: false,
            embed_dir: directory.path().join("embeds"),
        }
    }

    async fn asset_request(
        app: &axum::Router,
        method: &str,
        path: &str,
    ) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn register_bundle(state: &AppState, generation: &str) {
        store::install_bundle_draft(
            &state.pool,
            "security-test",
            "Security test",
            "# Test",
            generation,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn bundle_assets_require_registration_before_files_indexes_or_redirects() {
        let directory = tempfile::tempdir().unwrap();
        let state = test_state(&directory).await;
        let generation = "bundle-security-test";
        let bundle = state.embed_dir.join(generation);
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(
            bundle.join("index.html"),
            "<html><body>Registered bundle</body></html>",
        )
        .unwrap();
        std::fs::write(bundle.join("slides.md"), "SECRET SOURCE").unwrap();
        std::fs::write(
            bundle.join("diagram.svg"),
            "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
        )
        .unwrap();
        let app = router(state.clone());

        for method in ["GET", "HEAD"] {
            for suffix in ["", "/", "/index.html", "/diagram.svg", "/slides.md"] {
                let path = format!("/assets/embeds/{generation}{suffix}");
                assert_eq!(
                    asset_request(&app, method, &path).await.status(),
                    StatusCode::NOT_FOUND,
                    "{method} {path}"
                );
            }
        }

        register_bundle(&state, generation).await;
        for suffix in ["/", "/index.html"] {
            let response =
                asset_request(&app, "GET", &format!("/assets/embeds/{generation}{suffix}")).await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let html = String::from_utf8(body.to_vec()).unwrap();
            assert!(html.contains("Registered bundle"));
            assert!(html.contains("data-slides-navigation-bridge"));
        }
        assert_eq!(
            asset_request(&app, "GET", &format!("/assets/embeds/{generation}"))
                .await
                .status(),
            StatusCode::TEMPORARY_REDIRECT
        );
        for method in ["GET", "HEAD"] {
            let response = asset_request(
                &app,
                method,
                &format!("/assets/embeds/{generation}/diagram.svg"),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
            for (name, value) in iframe_asset_headers() {
                assert_eq!(response.headers()[name.unwrap()], value);
            }
            assert_eq!(
                response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                "nosniff"
            );
            assert_eq!(
                asset_request(
                    &app,
                    method,
                    &format!("/assets/embeds/{generation}/slides.md")
                )
                .await
                .status(),
                StatusCode::NOT_FOUND
            );
        }
    }

    #[tokio::test]
    async fn bundle_aliases_cannot_expose_unregistered_assets_or_root_source() {
        let directory = tempfile::tempdir().unwrap();
        let state = test_state(&directory).await;
        for generation in ["bundle-hidden", "bundle-visible"] {
            let bundle = state.embed_dir.join(generation);
            std::fs::create_dir_all(&bundle).unwrap();
            std::fs::write(bundle.join("index.html"), "SECRET INDEX").unwrap();
            std::fs::write(bundle.join("slides.md"), "SECRET SOURCE").unwrap();
        }
        register_bundle(&state, "bundle-visible").await;
        let app = router(state);
        for path in [
            "/assets/embeds/Bundle-hidden/index.html",
            "/assets/embeds/Bundle-visible/slides.md",
            "/assets/embeds/bundle-visible/SLIDES.MD",
            "/assets/embeds/%62undle-hidden/index.html",
            "/assets/embeds/bundle%2dhidden/",
            "/assets/embeds/bundle-hidden%2findex.html",
            "/assets/%65mbeds/bundle-hidden/index.html",
            "/assets/embeds//bundle-hidden/index.html",
            "/assets/embeds/./bundle-hidden/index.html",
            "/assets/embeds/bundle-visible/../bundle-hidden/index.html",
            "/assets/embeds/bundle-visible/%2e%2e/bundle-hidden/index.html",
            "/assets/embeds/bundle-visible/%2E%2E%2Fbundle-hidden/index.html",
            "/assets/embeds/bundle-visible/%252e%252e/bundle-hidden/index.html",
            "/assets/embeds/bundle-visible/%73lides.md",
            "/assets/embeds/bundle-visible/./slides.md",
            "/assets/embeds/bundle-visible//slides.md",
            "/assets/embeds/bundle-visible/slides.md/",
            "/assets/embeds/bundle-visible/%2e/slides.md",
            "/assets/embeds/bundle-visible/%5cslides.md",
        ] {
            for method in ["GET", "HEAD"] {
                let response = asset_request(&app, method, path).await;
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
                let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
                assert!(!String::from_utf8_lossy(&body).contains("SECRET"), "{path}");
            }
        }
    }

    #[tokio::test]
    async fn registered_bundle_never_falls_back_to_static_assets() {
        let directory = tempfile::tempdir().unwrap();
        let state = test_state(&directory).await;
        // A unique, automatically cleaned-up fixture in the real static fallback.
        let fallback = tempfile::Builder::new()
            .prefix("bundle-security-")
            .tempdir_in("assets/embeds")
            .unwrap();
        let generation = fallback.path().file_name().unwrap().to_str().unwrap();
        std::fs::write(fallback.path().join("index.html"), "STATIC FALLBACK").unwrap();
        std::fs::write(fallback.path().join("missing.svg"), "<svg/>").unwrap();
        register_bundle(&state, generation).await;
        let app = router(state.clone());
        for create_generation in [false, true] {
            if create_generation {
                std::fs::create_dir_all(state.embed_dir.join(generation)).unwrap();
            }
            for suffix in ["", "/", "/index.html", "/missing.svg"] {
                for method in ["GET", "HEAD"] {
                    let path = format!("/assets/embeds/{generation}{suffix}");
                    assert_eq!(
                        asset_request(&app, method, &path).await.status(),
                        StatusCode::NOT_FOUND,
                        "{method} {path}, local directory: {create_generation}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn legacy_assets_keep_percent_encoded_filenames_and_static_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let state = test_state(&directory).await;
        std::fs::create_dir_all(state.embed_dir.join("legacy")).unwrap();
        std::fs::write(state.embed_dir.join("legacy/my image.txt"), "legacy asset").unwrap();
        let app = router(state);
        let response = asset_request(&app, "GET", "/assets/embeds/legacy/my%20image.txt").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "legacy asset"
        );
        assert_eq!(
            asset_request(&app, "GET", "/assets/embeds/README.md")
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            asset_request(&app, "GET", "/assets/embeds/README%2emd")
                .await
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn serves_uploaded_iframe_html_with_sandbox_and_navigation_bridge() {
        let directory = tempfile::tempdir().unwrap();
        let embed_dir = directory.path().join("embeds");
        std::fs::create_dir_all(embed_dir.join("demo")).unwrap();
        std::fs::write(
            embed_dir.join("demo/index.html"),
            "<!doctype html><html><body>Uploaded</body></html>",
        )
        .unwrap();
        let state = test_state(&directory).await;

        let app = router(state);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/assets/embeds/demo/index.html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("sandbox allow-scripts")
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Uploaded"));
        assert!(html.contains("data-slides-navigation-bridge"));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/assets/embeds/demo/index.html")
                    .header(header::RANGE, "bytes=0-9")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(
            !String::from_utf8(body.to_vec())
                .unwrap()
                .contains("data-slides-navigation-bridge")
        );
    }

    #[test]
    fn adds_navigation_bridge_before_the_document_end() {
        let html = add_iframe_navigation_bridge(
            "<!doctype html><html><body><p>Demo</p></body></html>".into(),
        );

        assert!(html.contains("data-slides-navigation-bridge"));
        assert!(
            html.find("data-slides-navigation-bridge").unwrap() < html.find("</body>").unwrap()
        );
        assert!(html.contains("window.parent.postMessage"));
    }

    #[test]
    fn iframe_assets_are_sandboxed_for_direct_and_embedded_views() {
        let headers = iframe_asset_headers();
        let policy = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();

        assert!(policy.contains("frame-ancestors 'self'"));
        assert!(policy.contains("sandbox allow-scripts"));
        assert!(policy.contains("connect-src 'none'"));
        assert!(!policy.contains("allow-same-origin"));
        assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
    }
}
