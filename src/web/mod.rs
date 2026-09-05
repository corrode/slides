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
        .layer(middleware::from_fn(sandbox_html_asset));

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
        .route("/admin/decks", post(admin::create_deck))
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

const MAX_IFRAME_HTML_BYTES: usize = 4 * 1024 * 1024;

async fn sandbox_html_asset(request: Request<Body>, next: Next) -> Response {
    let inject_navigation = request.method() == Method::GET;
    let mut response = next.run(request).await;
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
        let database_url = format!("sqlite://{}", directory.path().join("slides.db").display());
        let state = AppState {
            pool: store::connect(&database_url).await.unwrap(),
            hub: Arc::new(LiveHub::default()),
            admin_password_hash: hash("password"),
            admin_cookie: hash("cookie"),
            secure_cookies: false,
            embed_dir,
        };

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
