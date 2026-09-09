use askama::Template;
use axum::{
    Form,
    body::Body,
    extract::{Path, Query, State, rejection::FormRejection},
    http::{HeaderMap, HeaderValue, Request, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;

use crate::{
    error::{AppError, AppResult},
    markdown::{parse_deck, resolve_code_references},
    models::{
        Deck, DeckSummary, EndedSessionSummary, Theme, valid_code_font, valid_headline_font,
        valid_text_font,
    },
    store,
    web::{AppState, is_admin, require_admin, template},
};

use super::render;

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    error: Option<String>,
    next: String,
    sign_in_required: bool,
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
    live_code: Option<String>,
    other_live: Option<(String, String)>,
    published: bool,
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
    #[serde(flatten)]
    navigation: LoginQuery,
}

#[derive(Debug, Default, Deserialize)]
pub struct LoginQuery {
    next: Option<String>,
    #[serde(default, deserialize_with = "published_flag")]
    sign_in_required: bool,
}

fn safe_login_next(next: Option<&str>) -> &str {
    let Some(next) = next else { return "/admin" };
    if next == "/admin" {
        return next;
    }
    if let Some(slug) = next
        .strip_prefix("/admin/decks/")
        .and_then(|path| path.strip_suffix("/edit"))
        && (1..=48).contains(&slug.len())
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
    {
        return next;
    }
    "/admin"
}

fn sign_in_redirect(destination: &str, htmx: bool) -> Response {
    let destination = safe_login_next(Some(destination)).replace('/', "%2F");
    let location = format!("/admin/login?next={destination}&sign_in_required=1");
    if htmx {
        // HTMX follows ordinary redirects internally; a non-3xx HX-Redirect navigates the page.
        (StatusCode::NO_CONTENT, [("hx-redirect", location)]).into_response()
    } else {
        Redirect::to(&location).into_response()
    }
}

pub(super) async fn guard_deck_auth(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if let Some(path) = request.uri().path().strip_prefix("/admin/decks/")
        && !is_admin(&CookieJar::from_headers(request.headers()), &state)
    {
        let slug = path.split('/').next().unwrap_or_default();
        return sign_in_redirect(
            &format!("/admin/decks/{slug}/edit"),
            request.headers().contains_key("hx-request"),
        );
    }
    next.run(request).await
}

#[derive(Debug, Deserialize)]
pub struct DeckForm {
    title: String,
    source: String,
    headline_font: String,
    text_font: String,
    code_font: String,
    background: String,
    text: String,
    accent: String,
}

pub async fn login_page(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<LoginQuery>,
) -> AppResult<Response> {
    let next = safe_login_next(query.next.as_deref());
    if is_admin(&jar, &state) {
        return Ok(Redirect::to(next).into_response());
    }
    template(LoginTemplate {
        error: None,
        next: next.into(),
        sign_in_required: query.sign_in_required,
    })
}

pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> AppResult<Response> {
    let next = safe_login_next(form.navigation.next.as_deref());
    if !super::secrets_equal(&super::hash(&form.password), &state.admin_password_hash) {
        return template(LoginTemplate {
            error: Some("That password is not correct.".into()),
            next: next.into(),
            sign_in_required: form.navigation.sign_in_required,
        });
    }

    let cookie = Cookie::build(("slides_admin", state.admin_cookie.clone()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(state.secure_cookies)
        .build();
    Ok((jar.add(cookie), Redirect::to(next)).into_response())
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

#[derive(Default, Deserialize)]
pub struct EditorQuery {
    #[serde(default, deserialize_with = "published_flag")]
    published: bool,
}

fn published_flag<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(String::deserialize(deserializer)? == "1")
}

pub async fn editor(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(slug): Path<String>,
    Query(query): Query<EditorQuery>,
) -> AppResult<Response> {
    if !is_admin(&jar, &state) {
        return Ok(sign_in_redirect(
            &format!("/admin/decks/{slug}/edit"),
            false,
        ));
    }
    let deck = required_deck(&state, &slug).await?;
    let (live_code, other_live) = match store::active_session(&state.pool).await? {
        Some(session) if session.deck_id == deck.id => (Some(session.code), None),
        Some(session) => {
            let live = store::get_session(&state.pool, session.id).await?;
            let version = store::get_version(&state.pool, live.deck_version_id).await?;
            (None, Some((session.code, version.title)))
        }
        None => (None, None),
    };

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
        live_code,
        other_live,
        published: query.published,
        initial_notice,
        initial_preview,
    })
}

pub async fn save(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(slug): Path<String>,
    form: Result<Form<DeckForm>, FormRejection>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let deck = required_deck(&state, &slug).await?;

    let Form(form) = match form {
        Ok(form) => form,
        Err(error) => return Ok(error.into_response()),
    };
    validate_draft_form(&form)?;
    let theme = theme_from_form(&form);
    store::save_deck(
        &state.pool,
        deck.id,
        form.title.trim(),
        &form.source,
        &theme,
    )
    .await?;
    let active = store::active_session(&state.pool)
        .await?
        .is_some_and(|session| session.deck_id == deck.id);
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
            "<span class=\"error-text\">{saved} Preview unavailable: {}</span><div id=\"preview\" hx-swap-oob=\"innerHTML\"><div class=\"empty-state preview-error-state\"><div><strong>Preview paused</strong><p>Fix the Markdown error to render the latest draft.</p></div></div></div>",
            html_escape::encode_text(&error.to_string())
        ),
    };
    Ok(Html(response).into_response())
}

pub async fn print_deck(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(slug): Path<String>,
    form: Result<Form<DeckForm>, FormRejection>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    required_deck(&state, &slug).await?;
    let Form(form) = match form {
        Ok(form) => form,
        Err(error) => return Ok(error.into_response()),
    };
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
    form: Result<Form<DeckForm>, FormRejection>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let deck = required_deck(&state, &slug).await?;
    let Form(form) = match form {
        Ok(form) => form,
        Err(error) => return Ok(error.into_response()),
    };
    publish_form(&state, deck.id, &form).await?;
    let location = format!("/admin/decks/{slug}/edit?published=1");
    if headers.contains_key("hx-request") {
        let mut response = StatusCode::NO_CONTENT.into_response();
        response.headers_mut().insert(
            "hx-redirect",
            HeaderValue::from_str(&location)
                .map_err(|_| AppError::bad_request("Invalid presentation URL."))?,
        );
        Ok(response)
    } else {
        Ok(Redirect::to(&location).into_response())
    }
}

pub async fn start_session(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(slug): Path<String>,
    form: Result<Form<DeckForm>, FormRejection>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let deck = required_deck(&state, &slug).await?;
    if let Some(session) = store::active_session(&state.pool).await? {
        return session_start_response(&state, &deck, session.id, &session.code).await;
    }
    let Form(form) = match form {
        Ok(form) => form,
        Err(error) => return Ok(error.into_response()),
    };
    let version_id = publish_form(&state, deck.id, &form).await?;
    let session = store::start_session(&state.pool, deck.id, version_id).await?;
    // The store may return a competing session that started after our initial check.
    session_start_response(&state, &deck, session.id, &session.code).await
}

async fn session_start_response(
    state: &AppState,
    deck: &Deck,
    session_id: i64,
    code: &str,
) -> AppResult<Response> {
    let session_slug = store::deck_slug_for_session(&state.pool, session_id).await?;
    if session_slug == deck.slug {
        return Ok(Redirect::to(&format!("/present/{code}")).into_response());
    }
    let session = store::get_session(&state.pool, session_id).await?;
    let version = store::get_version(&state.pool, session.deck_version_id).await?;
    Ok((StatusCode::CONFLICT, Html(format!(
        "<h1>Another presentation is live</h1><p>End the live session for <strong>{}</strong> before presenting <strong>{}</strong>.</p><p><a href=\"/present/{}\">Open other presentation: {}</a></p><p><a href=\"/admin/decks/{}/edit\">Return to this presentation</a></p>",
        html_escape::encode_text(&version.title),
        html_escape::encode_text(&deck.title),
        html_escape::encode_double_quoted_attribute(code),
        html_escape::encode_text(&version.title),
        html_escape::encode_double_quoted_attribute(&deck.slug),
    ))).into_response())
}

async fn publish_form(state: &AppState, deck_id: i64, form: &DeckForm) -> AppResult<i64> {
    validate_deck_form(form)?;
    let published_source = resolve_code_references(&form.source)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    parse_deck(&published_source).map_err(|error| AppError::bad_request(error.to_string()))?;
    let theme = theme_from_form(form);
    Ok(store::save_and_publish_deck(
        &state.pool,
        deck_id,
        form.title.trim(),
        &form.source,
        &published_source,
        &theme,
    )
    .await?)
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
    if !valid_headline_font(&form.headline_font) {
        return Err(AppError::bad_request("Unsupported headline font choice."));
    }
    if !valid_text_font(&form.text_font) {
        return Err(AppError::bad_request("Unsupported text font choice."));
    }
    if !valid_code_font(&form.code_font) {
        return Err(AppError::bad_request("Unsupported code font choice."));
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
        headline_font: form.headline_font.clone(),
        text_font: form.text_font.clone(),
        code_font: form.code_font.clone(),
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

async fn required_deck(state: &AppState, slug: &str) -> AppResult<Deck> {
    store::deck_by_slug(&state.pool, slug)
        .await?
        .ok_or_else(|| AppError::not_found("Presentation not found."))
}

#[cfg(test)]
mod tests {
    use askama::Template;

    use crate::models::Deck;

    use super::EditorTemplate;

    fn editor_template() -> EditorTemplate {
        EditorTemplate {
            deck: Deck {
                id: 1,
                slug: "demo".into(),
                title: "Demo".into(),
                draft_source: "# Demo".into(),
                theme_headline_font: "inter".into(),
                theme_text_font: "inter".into(),
                theme_code_font: "jetbrains-mono".into(),
                theme_background: "#282934".into(),
                theme_text: "#e1e1e1".into(),
                theme_accent: "#fc218a".into(),
            },
            live_code: None,
            other_live: None,
            published: false,
            initial_notice: "<span>Changes save automatically.</span>".into(),
            initial_preview: "<p>Stored preview</p>".into(),
        }
    }

    #[test]
    fn presentation_start_uses_a_native_form_submission() {
        let template = editor_template();
        let html = template.render().unwrap();
        assert!(html.contains("name=\"source\""));
        assert!(html.contains("name=\"title\""));
        assert!(html.contains("name=\"headline_font\""));
        assert!(html.contains("hx-post=\"/admin/decks/demo/save\""));

        assert!(html.contains(
            "type=\"submit\" formmethod=\"post\" formaction=\"/admin/decks/demo/sessions\""
        ));
        assert!(!html.contains("data-present-url"));
        assert!(!html.contains("hx-post=\"/admin/decks/demo/sessions\""));

        let live_html = EditorTemplate {
            live_code: Some("123456".into()),
            ..template
        }
        .render()
        .unwrap();
        assert!(live_html.contains("href=\"/present/123456\""));
        assert!(!live_html.contains("formaction=\"/admin/decks/demo/sessions\""));
    }

    #[test]
    fn editor_has_editing_controls_and_publish_action() {
        let template = editor_template();
        let html = template.render().unwrap();
        assert!(html.contains("<p>Stored preview</p>"));
        assert!(html.contains(
            "type=\"submit\" formmethod=\"post\" formaction=\"/admin/decks/demo/publish\">Publish"
        ));
        assert!(!html.contains("hx-post=\"/admin/decks/demo/publish\""));
        assert!(html.contains("data-print-url=\"/admin/decks/demo/print\""));
        for editing_control in [
            "name=\"title\"",
            "name=\"source\"",
            "<textarea",
            "<select",
            "type=\"color\"",
            "data-markdown-editor",
            "data-editor-split",
            "/admin/decks/demo/save",
            "codemirror",
            "hx-post=",
        ] {
            assert!(html.contains(editing_control), "missing {editing_control}");
        }
        let live_html = EditorTemplate {
            live_code: Some("123456".into()),
            ..template
        }
        .render()
        .unwrap();
        assert!(live_html.contains("href=\"/present/123456\""));
        assert!(!live_html.contains("action=\"/admin/decks/demo/sessions\""));
        assert!(live_html.contains("formaction=\"/admin/decks/demo/publish\""));
    }

    async fn test_state() -> (tempfile::TempDir, super::AppState) {
        let directory = tempfile::tempdir().unwrap();
        let pool = crate::store::connect(&format!(
            "sqlite://{}",
            directory.path().join("slides.db").display()
        ))
        .await
        .unwrap();
        let state = super::AppState {
            pool,
            hub: std::sync::Arc::new(crate::live::LiveHub::default()),
            admin_password_hash: "unused".into(),
            admin_cookie: "test-cookie".into(),
            secure_cookies: false,
            embed_dir: directory.path().join("embeds"),
        };
        (directory, state)
    }

    fn jar(state: &super::AppState) -> super::CookieJar {
        super::CookieJar::new().add(super::Cookie::new(
            "slides_admin",
            state.admin_cookie.clone(),
        ))
    }

    fn form() -> super::DeckForm {
        let theme = crate::models::Theme::default();
        super::DeckForm {
            title: "Demo".into(),
            source: "# Demo".into(),
            headline_font: theme.headline_font,
            text_font: theme.text_font,
            code_font: theme.code_font,
            background: theme.background,
            text: theme.text,
            accent: theme.accent,
        }
    }

    async fn body(response: super::Response) -> String {
        String::from_utf8(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn editor_and_start_are_scoped_to_the_requested_deck() {
        use super::*;
        let (_directory, state) = test_state().await;
        let deck = store::create_deck(&state.pool, "demo", "Demo")
            .await
            .unwrap();
        store::create_deck(&state.pool, "other", "Other")
            .await
            .unwrap();
        let html = body(
            editor(
                State(state.clone()),
                jar(&state),
                Path("other".into()),
                Query(EditorQuery::default()),
            )
            .await
            .unwrap(),
        )
        .await;
        assert!(html.contains("formaction=\"/admin/decks/other/sessions\""));
        assert!(!html.contains("Open live session"));
        assert!(!html.contains("Another presentation is live"));

        let response = start_session(
            State(state.clone()),
            jar(&state),
            Path("demo".into()),
            Ok(Form(form())),
        )
        .await
        .unwrap();
        let session = store::active_session(&state.pool).await.unwrap().unwrap();
        assert_eq!(
            response.headers()["location"],
            format!("/present/{}", session.code)
        );
        let html = body(
            editor(
                State(state.clone()),
                jar(&state),
                Path("demo".into()),
                Query(EditorQuery::default()),
            )
            .await
            .unwrap(),
        )
        .await;
        assert!(html.contains("Open live session"));
        assert!(!html.contains("Another presentation is live"));
        let response = start_session(
            State(state.clone()),
            jar(&state),
            Path("demo".into()),
            Ok(Form(form())),
        )
        .await
        .unwrap();
        assert_eq!(
            response.headers()["location"],
            format!("/present/{}", session.code)
        );

        let html = body(
            editor(
                State(state.clone()),
                jar(&state),
                Path("other".into()),
                Query(EditorQuery::default()),
            )
            .await
            .unwrap(),
        )
        .await;
        assert!(html.contains("Open other presentation: Demo"));
        assert!(html.contains("disabled>Present"));
        assert!(!html.contains("Open live session"));
        assert!(!html.contains("formaction=\"/admin/decks/other/sessions\""));
        let response = start_session(
            State(state.clone()),
            jar(&state),
            Path("other".into()),
            Ok(Form(form())),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(!response.headers().contains_key("location"));
        assert!(
            body(response)
                .await
                .contains("Open other presentation: Demo")
        );
        assert_eq!(
            store::active_session(&state.pool)
                .await
                .unwrap()
                .unwrap()
                .deck_id,
            deck.id
        );
    }

    #[tokio::test]
    async fn competing_store_result_is_checked_before_redirecting() {
        use super::*;
        let (_directory, state) = test_state().await;
        let deck = store::create_deck(&state.pool, "demo", "Demo")
            .await
            .unwrap();
        let other = store::create_deck(&state.pool, "other", "Other")
            .await
            .unwrap();
        let version = publish_form(&state, deck.id, &form()).await.unwrap();
        store::start_session(&state.pool, deck.id, version)
            .await
            .unwrap();
        let other_version = publish_form(&state, other.id, &form()).await.unwrap();
        // Reproduce the store result when another deck wins between the handler's checks.
        let returned = store::start_session(&state.pool, other.id, other_version)
            .await
            .unwrap();
        let response = session_start_response(&state, &other, returned.id, &returned.code)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(!response.headers().contains_key("location"));
    }

    #[tokio::test]
    async fn publish_acknowledges_native_and_htmx_requests() {
        use super::*;
        let (_directory, state) = test_state().await;
        store::create_deck(&state.pool, "demo", "Demo")
            .await
            .unwrap();
        for htmx in [false, true] {
            let mut headers = HeaderMap::new();
            if htmx {
                headers.insert("hx-request", HeaderValue::from_static("true"));
            }
            let response = publish(
                State(state.clone()),
                jar(&state),
                Path("demo".into()),
                headers,
                Ok(Form(form())),
            )
            .await
            .unwrap();
            assert_eq!(
                response.headers()[if htmx { "hx-redirect" } else { "location" }],
                "/admin/decks/demo/edit?published=1"
            );
            assert!(!response.headers().contains_key("hx-refresh"));
        }
        for (query, expected) in [
            ("", false),
            ("?published=1", true),
            ("?published=0", false),
            ("?published=%3Cscript%3E", false),
        ] {
            let Query(query) = Query::<EditorQuery>::try_from_uri(
                &format!("/admin/decks/demo/edit{query}").parse().unwrap(),
            )
            .unwrap();
            let html = body(
                editor(
                    State(state.clone()),
                    jar(&state),
                    Path("demo".into()),
                    Query(query),
                )
                .await
                .unwrap(),
            )
            .await;
            assert_eq!(html.contains("Published successfully."), expected);
            assert!(html.contains("Changes save automatically."));
        }
    }

    #[test]
    fn notices_preserve_base_feedback_and_escape_other_titles() {
        let html = EditorTemplate {
            other_live: Some(("123456".into(), "<script>Other</script>".into())),
            published: true,
            ..editor_template()
        }
        .render()
        .unwrap();
        assert!(html.contains("Published successfully."));
        assert!(html.contains("Changes save automatically."));
        assert!(html.contains("Open other presentation:"));
        assert!(!html.contains("<script>Other</script>"));
        assert!(html.contains("disabled>Present"));
        assert!(!html.contains("Open live session"));
        assert!(!html.contains("/admin/decks/demo/sessions"));
        assert!(html.contains("formaction=\"/admin/decks/demo/publish\""));
    }

    #[test]
    fn dashboard_directs_new_decks_to_api_uploads() {
        let html = super::DashboardTemplate {
            decks: Vec::new(),
            ended_sessions: Vec::new(),
        }
        .render()
        .unwrap();
        assert!(html.contains("Upload your first presentation"));
        assert!(html.contains("href=\"/admin/settings\""));
        assert!(!html.contains("action=\"/admin/decks\""));
        assert!(!html.contains("create-presentation"));
    }
}
