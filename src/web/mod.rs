mod admin;
mod api;
mod playground;
pub(crate) mod render;
mod session;
mod settings;
mod shared;

use std::sync::Arc;

use askama::Template;
use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderName, HeaderValue, StatusCode},
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
}

pub fn router(state: AppState) -> Router {
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
        .nest_service("/assets", ServeDir::new("assets"))
        .route("/{slug}", get(session::named_shortlink))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' http: https: data:; connect-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
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
