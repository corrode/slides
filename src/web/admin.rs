use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;

use crate::{
    error::{AppError, AppResult},
    markdown::{parse_deck, resolve_code_references},
    models::{Deck, DeckSummary, EndedSessionSummary, Theme},
    store,
    web::{AppState, is_admin, require_admin, template},
};

use super::render;

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    decks: Vec<DeckSummary>,
    ended_sessions: Vec<EndedSessionSummary>,
}

#[derive(Template)]
#[template(path = "editor.html")]
struct EditorTemplate {
    deck: Deck,
    active_code: Option<String>,
    initial_notice: String,
    initial_preview: String,
}

#[derive(Template)]
#[template(path = "print.html")]
struct PrintTemplate {
    title: String,
    theme_style: String,
    slides: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    password: String,
}

#[derive(Debug, Deserialize)]
pub struct NewDeckForm {
    title: String,
    slug: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeckForm {
    title: String,
    source: String,
    font: String,
    background: String,
    text: String,
    accent: String,
}

pub async fn login_page(State(state): State<AppState>, jar: CookieJar) -> AppResult<Response> {
    if is_admin(&jar, &state) {
        return Ok(Redirect::to("/admin").into_response());
    }
    template(LoginTemplate { error: None })
}

pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> AppResult<Response> {
    if !super::secrets_equal(&super::hash(&form.password), &state.admin_password_hash) {
        return template(LoginTemplate {
            error: Some("That password is not correct.".into()),
        });
    }

    let cookie = Cookie::build(("slides_admin", state.admin_cookie.clone()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(state.secure_cookies)
        .build();
    Ok((jar.add(cookie), Redirect::to("/admin")).into_response())
}

pub async fn dashboard(State(state): State<AppState>, jar: CookieJar) -> AppResult<Response> {
    if !is_admin(&jar, &state) {
        return Ok(Redirect::to("/admin/login").into_response());
    }
    template(DashboardTemplate {
        decks: store::list_decks(&state.pool).await?,
        ended_sessions: store::list_ended_sessions(&state.pool).await?,
    })
}

pub async fn delete_deck(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(slug): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let deck = required_deck(&state, &slug).await?;
    if !store::delete_deck(&state.pool, deck.id).await? {
        return Err(AppError::bad_request(
            "End the live session before deleting this presentation.",
        ));
    }
    Ok(Redirect::to("/admin").into_response())
}

pub async fn delete_ended_session(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let session = store::session_by_code(&state.pool, &code)
        .await?
        .ok_or_else(|| AppError::not_found("Session not found."))?;
    if session.ended_at.is_none() {
        return Err(AppError::bad_request(
            "End the live session before deleting it.",
        ));
    }
    if !store::delete_ended_session(&state.pool, session.id).await? {
        return Err(AppError::not_found("Session not found."));
    }
    Ok(Redirect::to("/admin").into_response())
}

pub async fn create_deck(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<NewDeckForm>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let title = form.title.trim();
    validate_title(title)?;
    let slug = match form
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|slug| !slug.is_empty())
    {
        Some(slug) => {
            let slug = slug.to_ascii_lowercase();
            validate_slug(&slug)?;
            slug
        }
        None => {
            let base = slugify_title(title).ok_or_else(|| {
                AppError::bad_request(
                    "Enter a shortlink for titles without ASCII letters or numbers.",
                )
            })?;
            available_slug(&state, &base).await?
        }
    };
    let deck = store::create_deck(&state.pool, &slug, title)
        .await
        .map_err(|error| {
            if error.to_string().contains("UNIQUE constraint failed") {
                AppError::bad_request("That shortlink is already in use.")
            } else {
                error.into()
            }
        })?;
    let location = format!("/admin/decks/{}/edit", deck.slug);
    if headers.contains_key("hx-request") {
        let mut response = StatusCode::NO_CONTENT.into_response();
        response.headers_mut().insert(
            "hx-redirect",
            HeaderValue::from_str(&location).expect("deck edit path is a valid header value"),
        );
        Ok(response)
    } else {
        Ok(Redirect::to(&location).into_response())
    }
}

pub async fn editor(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(slug): Path<String>,
) -> AppResult<Response> {
    if !is_admin(&jar, &state) {
        return Ok(Redirect::to("/admin/login").into_response());
    }
    let deck = required_deck(&state, &slug).await?;
    let active_code = store::active_session_for_deck(&state.pool, deck.id)
        .await?
        .map(|session| session.code);
    let (initial_preview, initial_notice) = match parse_deck(&deck.draft_source) {
        Ok(document) => (
            render::preview(&document, &Theme::from(&deck)),
            "<span>Changes save automatically.</span>".into(),
        ),
        Err(error) => (
            "<div class=\"empty-state\">Preview unavailable until the Markdown is valid.</div>"
                .into(),
            format!(
                "<div class=\"notice error\">Draft saved, but preview unavailable: {}</div>",
                html_escape::encode_text(&error.to_string())
            ),
        ),
    };
    template(EditorTemplate {
        deck,
        active_code,
        initial_notice,
        initial_preview,
    })
}

pub async fn save(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(slug): Path<String>,
    Form(form): Form<DeckForm>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let deck = required_deck(&state, &slug).await?;
    validate_draft_form(&form)?;
    store::save_deck(
        &state.pool,
        deck.id,
        form.title.trim(),
        &form.source,
        &form.font,
        &form.background,
        &form.text,
        &form.accent,
    )
    .await?;
    let active = store::active_session_for_deck(&state.pool, deck.id)
        .await?
        .is_some();
    let saved = if active {
        "Draft saved. Changes apply to the next session."
    } else {
        "Draft saved."
    };
    let response = match parse_deck(&form.source) {
        Ok(document) => {
            let preview = render::preview(&document, &theme_from_form(&form));
            format!(
                "<span>{saved}</span><div id=\"preview\" hx-swap-oob=\"innerHTML\">{preview}</div>"
            )
        }
        Err(error) => format!(
            "<span class=\"error-text\">{saved} Preview unavailable: {}</span>",
            html_escape::encode_text(&error.to_string())
        ),
    };
    Ok(Html(response).into_response())
}

pub async fn print_deck(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(slug): Path<String>,
    Form(form): Form<DeckForm>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    required_deck(&state, &slug).await?;
    validate_deck_form(&form)?;
    let document =
        parse_deck(&form.source).map_err(|error| AppError::bad_request(error.to_string()))?;
    let theme = theme_from_form(&form);
    template(PrintTemplate {
        title: form.title.trim().into(),
        theme_style: theme.style(),
        slides: render::printable(&document),
    })
}

pub async fn publish(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Form(form): Form<DeckForm>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let deck = required_deck(&state, &slug).await?;
    validate_deck_form(&form)?;
    let published_source = resolve_code_references(&form.source)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    parse_deck(&published_source).map_err(|error| AppError::bad_request(error.to_string()))?;
    store::save_and_publish_deck(
        &state.pool,
        deck.id,
        form.title.trim(),
        &form.source,
        &published_source,
        &form.font,
        &form.background,
        &form.text,
        &form.accent,
    )
    .await?;
    if headers.contains_key("hx-request") {
        let mut response = StatusCode::NO_CONTENT.into_response();
        response
            .headers_mut()
            .insert("hx-refresh", HeaderValue::from_static("true"));
        Ok(response)
    } else {
        Ok(Redirect::to(&format!("/admin/decks/{slug}/edit")).into_response())
    }
}

pub async fn start_session(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Form(form): Form<DeckForm>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let deck = required_deck(&state, &slug).await?;
    if let Some(session) = store::active_session_for_deck(&state.pool, deck.id).await? {
        return Ok(Redirect::to(&format!("/present/{}", session.code)).into_response());
    }
    validate_deck_form(&form)?;
    let published_source = resolve_code_references(&form.source)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    parse_deck(&published_source).map_err(|error| AppError::bad_request(error.to_string()))?;
    let version_id = store::save_and_publish_deck(
        &state.pool,
        deck.id,
        form.title.trim(),
        &form.source,
        &published_source,
        &form.font,
        &form.background,
        &form.text,
        &form.accent,
    )
    .await?;
    let session = store::start_session(&state.pool, deck.id, version_id).await?;
    let location = format!("/present/{}", session.code);
    if headers.contains_key("hx-request") {
        let mut response = StatusCode::NO_CONTENT.into_response();
        response.headers_mut().insert(
            "hx-redirect",
            HeaderValue::from_str(&location).expect("presenter path is a valid header value"),
        );
        Ok(response)
    } else {
        Ok(Redirect::to(&location).into_response())
    }
}

fn validate_deck_form(form: &DeckForm) -> AppResult<()> {
    validate_draft_form(form)?;
    if form.source.trim().is_empty() {
        return Err(AppError::bad_request("The deck cannot be empty."));
    }
    Ok(())
}

fn validate_draft_form(form: &DeckForm) -> AppResult<()> {
    validate_title(form.title.trim())?;
    if !matches!(form.font.as_str(), "system" | "serif" | "mono") {
        return Err(AppError::bad_request("Unsupported font choice."));
    }
    for color in [&form.background, &form.text, &form.accent] {
        if !valid_color(color) {
            return Err(AppError::bad_request(
                "Theme colors must use #RRGGBB format.",
            ));
        }
    }
    Ok(())
}

fn theme_from_form(form: &DeckForm) -> Theme {
    Theme {
        font: form.font.clone(),
        background: form.background.clone(),
        text: form.text.clone(),
        accent: form.accent.clone(),
    }
}

fn valid_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_title(title: &str) -> AppResult<()> {
    if title.is_empty() || title.chars().count() > 120 {
        return Err(AppError::bad_request(
            "Titles must contain between 1 and 120 characters.",
        ));
    }
    Ok(())
}

fn validate_slug(slug: &str) -> AppResult<()> {
    const RESERVED: &[&str] = &[
        "admin", "api", "assets", "healthz", "join", "present", "sessions",
    ];
    let valid = (1..=48).contains(&slug.len())
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !RESERVED.contains(&slug);
    if !valid {
        return Err(AppError::bad_request(
            "Shortlinks must be 1–48 lowercase letters, numbers, or hyphens.",
        ));
    }
    Ok(())
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

async fn available_slug(state: &AppState, base: &str) -> AppResult<String> {
    if validate_slug(base).is_ok() && store::deck_by_slug(&state.pool, base).await?.is_none() {
        return Ok(base.to_owned());
    }

    for number in 2..=9_999 {
        let suffix = format!("-{number}");
        let base_length = (48 - suffix.len()).min(base.len());
        let root = base[..base_length].trim_end_matches('-');
        let candidate = format!("{root}{suffix}");
        if store::deck_by_slug(&state.pool, &candidate)
            .await?
            .is_none()
        {
            return Ok(candidate);
        }
    }

    Err(AppError::bad_request(
        "Could not derive an unused shortlink. Enter one explicitly.",
    ))
}

async fn required_deck(state: &AppState, slug: &str) -> AppResult<Deck> {
    store::deck_by_slug(&state.pool, slug)
        .await?
        .ok_or_else(|| AppError::not_found("Presentation not found."))
}

#[cfg(test)]
mod tests {
    use super::{slugify_title, validate_slug};

    #[test]
    fn derives_clean_shortlinks_from_titles() {
        assert_eq!(
            slugify_title("  Rust   + Axum & SQLite! ").as_deref(),
            Some("rust-axum-sqlite")
        );
        assert_eq!(slugify_title("2026").as_deref(), Some("2026"));
        assert_eq!(slugify_title("---"), None);
    }

    #[test]
    fn route_names_are_not_valid_shortlinks() {
        assert!(validate_slug("healthz").is_err());
        assert!(validate_slug("admin").is_err());
        assert!(validate_slug("api").is_err());
        assert!(validate_slug("my-healthz-talk").is_ok());
    }

    #[test]
    fn derived_shortlinks_do_not_exceed_the_limit() {
        let slug = slugify_title(
            "A very long presentation title with many words that cannot fit into one shortlink",
        )
        .unwrap();
        assert!(slug.len() <= 48);
        assert!(!slug.ends_with('-'));
    }
}
