use std::{convert::Infallible, time::Duration};

use askama::Template;
use async_stream::stream;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        Html, IntoResponse, Redirect, Response, Sse,
        sse::{Event, KeepAlive},
    },
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;

use crate::{
    error::{AppError, AppResult},
    markdown::{DeckDocument, Interaction, parse_deck},
    models::{LiveSession, Theme},
    store,
    web::{AppState, is_admin, participant_hash, require_admin, template},
};

use super::render::{self, LiveView};

#[derive(Template)]
#[template(path = "landing.html")]
struct LandingTemplate;

#[derive(Template)]
#[template(path = "waiting.html")]
struct WaitingTemplate {
    title: String,
    slug: String,
    theme_style: String,
}

#[derive(Template)]
#[template(path = "presenter.html")]
struct PresenterTemplate {
    title: String,
    code: String,
    theme_style: String,
    initial_live: String,
}

#[derive(Template)]
#[template(path = "audience.html")]
struct AudienceTemplate {
    title: String,
    events_url: String,
    theme_style: String,
    initial_live: String,
}

#[derive(Debug, Deserialize)]
pub struct JoinForm {
    code: String,
}

#[derive(Debug, Deserialize)]
pub struct EventQuery {
    view: Option<String>,
    slide: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AudienceQuery {
    slide: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct InteractionForm {
    action: String,
}

#[derive(Debug, Deserialize)]
pub struct AnswerForm {
    slide: usize,
    value: String,
}

#[derive(Debug, Deserialize)]
pub struct ReactionForm {
    slide: usize,
}

pub async fn landing() -> AppResult<Response> {
    template(LandingTemplate)
}

pub async fn join_code(Form(form): Form<JoinForm>) -> AppResult<Response> {
    let code: String = form.code.chars().filter(char::is_ascii_digit).collect();
    if code.len() != 6 {
        return Err(AppError::bad_request(
            "Enter a six-digit presentation code.",
        ));
    }
    Ok(Redirect::to(&format!("/join/{code}")).into_response())
}

pub async fn named_shortlink(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> AppResult<Response> {
    let deck = store::deck_by_slug(&state.pool, &slug)
        .await?
        .ok_or_else(|| AppError::not_found("Presentation not found."))?;
    if let Some(session) = store::active_session_for_slug(&state.pool, &slug).await? {
        return Ok(Redirect::to(&format!("/join/{}", session.code)).into_response());
    }
    template(WaitingTemplate {
        title: deck.title.clone(),
        slug: deck.slug.clone(),
        theme_style: Theme::from(&deck).style(),
    })
}

pub async fn audience(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
    Query(query): Query<AudienceQuery>,
) -> AppResult<Response> {
    let session = required_session(&state, &code).await?;
    let version = store::get_version(&state.pool, session.deck_version_id).await?;
    let document = parse_deck(&version.source)?;

    let (jar, participant) = ensure_participant(jar, &state);
    let initial_live = render::live(
        &state.pool,
        &session,
        &version,
        &document,
        LiveView::Audience,
        Some(&participant),
        query.slide,
    )
    .await?;
    let events_url = match query.slide {
        Some(slide) => format!(
            "/sessions/{}/events?view=audience&slide={slide}",
            session.code
        ),
        None => format!("/sessions/{}/events?view=audience", session.code),
    };
    let page = AudienceTemplate {
        title: version.title.clone(),
        events_url,
        theme_style: Theme::from(&version).style(),
        initial_live,
    };
    Ok((jar, Html(page.render()?)).into_response())
}

pub async fn presenter(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    if !is_admin(&jar, &state) {
        return Ok(Redirect::to("/admin/login").into_response());
    }
    let session = required_session(&state, &code).await?;
    let version = store::get_version(&state.pool, session.deck_version_id).await?;
    let document = parse_deck(&version.source)?;
    let initial_live = render::live(
        &state.pool,
        &session,
        &version,
        &document,
        LiveView::Presenter,
        None,
        None,
    )
    .await?;
    template(PresenterTemplate {
        title: version.title.clone(),
        code: session.code.clone(),
        theme_style: Theme::from(&version).style(),
        initial_live,
    })
}

pub async fn events(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
    Query(query): Query<EventQuery>,
) -> AppResult<Response> {
    let session = required_session(&state, &code).await?;
    let view = if query.view.as_deref() == Some("presenter") {
        if !is_admin(&jar, &state) {
            return Err(AppError::bad_request(
                "Presenter authentication is required.",
            ));
        }
        LiveView::Presenter
    } else {
        LiveView::Audience
    };
    let participant = participant_hash(&jar);
    let requested_slide = query.slide;
    let mut updates = state.hub.subscribe(session.id).await;
    let stream_state = state.clone();
    let stream_code = code.clone();

    let events = stream! {
        loop {
            let fragment = match snapshot(
                &stream_state,
                &stream_code,
                view,
                participant.as_deref(),
                requested_slide,
            ).await {
                Ok(fragment) => fragment,
                Err(error) => {
                    tracing::warn!(error = ?error, code = %stream_code, "could not render live update");
                    "<div id=\"live-error\" class=\"notice error\" hx-swap-oob=\"outerHTML\">Could not apply the latest live update. The next update will retry automatically.</div>".into()
                }
            };
            yield Ok::<Event, Infallible>(Event::default().data(fragment));

            match updates.recv().await {
                Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(events)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keepalive"),
        )
        .into_response())
}

pub async fn previous(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    mutate_position(&state, &code, |current, _| current.saturating_sub(1)).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn next(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    mutate_position(&state, &code, |current, len| {
        (current + 1).min(len.saturating_sub(1))
    })
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn toggle_lock(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(session.id).await;
    let _guard = runtime.mutation.lock().await;
    let fresh = store::get_session(&state.pool, session.id).await?;
    store::set_lock(&state.pool, fresh.id, !fresh.locked).await?;
    state.hub.notify(session.id).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn interaction_state(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
    Form(form): Form<InteractionForm>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let (open, revealed) = match form.action.as_str() {
        "open" => (true, false),
        "close" => (false, false),
        "reveal" => (false, true),
        _ => return Err(AppError::bad_request("Unknown interaction action.")),
    };
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(session.id).await;
    let _guard = runtime.mutation.lock().await;
    store::set_interaction_state(&state.pool, session.id, open, revealed).await?;
    state.hub.notify(session.id).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn answer(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
    Form(form): Form<AnswerForm>,
) -> AppResult<Response> {
    let participant = participant_hash(&jar)
        .ok_or_else(|| AppError::bad_request("Reload the presentation before answering."))?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(session.id).await;
    let _guard = runtime.mutation.lock().await;
    let session = store::get_session(&state.pool, session.id).await?;
    if !session.interaction_open || session.ended_at.is_some() {
        return Err(AppError::bad_request("Responses are closed."));
    }
    let document = available_document(&state, &session, form.slide).await?;
    let slide_index = form.slide;
    let slide = document
        .slides
        .get(slide_index)
        .ok_or_else(|| AppError::bad_request("The current slide is unavailable."))?;
    let interaction = slide
        .interaction
        .as_ref()
        .ok_or_else(|| AppError::bad_request("This slide does not accept responses."))?;

    match interaction {
        Interaction::Poll {
            options, multiple, ..
        } => {
            let value = valid_option(&form.value, options.len())?;
            if *multiple {
                store::toggle_answer(
                    &state.pool,
                    session.id,
                    slide_index,
                    &participant,
                    "poll",
                    value,
                )
                .await?;
            } else {
                store::replace_answer(
                    &state.pool,
                    session.id,
                    slide_index,
                    &participant,
                    "poll",
                    value,
                )
                .await?;
            }
        }
        Interaction::WordCloud { max_length, .. } => {
            let value = normalize_words(&form.value);
            if value.is_empty() || value.chars().count() > *max_length {
                return Err(AppError::bad_request(format!(
                    "Responses must contain between 1 and {max_length} characters."
                )));
            }
            store::replace_answer(
                &state.pool,
                session.id,
                slide_index,
                &participant,
                "wordcloud",
                &value,
            )
            .await?;
        }
        Interaction::Quiz { options, .. } => {
            let value = valid_option(&form.value, options.len())?;
            store::replace_answer(
                &state.pool,
                session.id,
                slide_index,
                &participant,
                "quiz",
                value,
            )
            .await?;
        }
    }
    state.hub.notify(session.id).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn react(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((code, kind)): Path<(String, String)>,
    Form(form): Form<ReactionForm>,
) -> AppResult<Response> {
    const ALLOWED: &[&str] = &["heart", "thumbs-up", "applause", "laugh", "question"];
    if !ALLOWED.contains(&kind.as_str()) {
        return Err(AppError::bad_request("Unknown reaction."));
    }
    let participant = participant_hash(&jar)
        .ok_or_else(|| AppError::bad_request("Reload the presentation before reacting."))?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(session.id).await;
    let _guard = runtime.mutation.lock().await;
    let session = store::get_session(&state.pool, session.id).await?;
    if session.ended_at.is_some() {
        return Err(AppError::bad_request("This presentation has ended."));
    }
    let _ = available_document(&state, &session, form.slide).await?;
    let inserted =
        store::add_reaction(&state.pool, session.id, form.slide, &participant, &kind).await?;
    if inserted {
        state.hub.notify(session.id).await;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn end(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(session.id).await;
    let _guard = runtime.mutation.lock().await;
    store::end_session(&state.pool, session.id).await?;
    state.hub.finish(session.id).await;
    Ok(Redirect::to("/admin").into_response())
}

async fn mutate_position(
    state: &AppState,
    code: &str,
    update: impl FnOnce(usize, usize) -> usize,
) -> AppResult<()> {
    let session = required_session(state, code).await?;
    let runtime = state.hub.runtime(session.id).await;
    let _guard = runtime.mutation.lock().await;
    let session = store::get_session(&state.pool, session.id).await?;
    let version = store::get_version(&state.pool, session.deck_version_id).await?;
    let document = parse_deck(&version.source)?;
    let target = update(session.current_slide as usize, document.slides.len());
    store::move_to_slide(&state.pool, session.id, target).await?;
    state.hub.notify(session.id).await;
    Ok(())
}

async fn snapshot(
    state: &AppState,
    code: &str,
    view: LiveView,
    participant: Option<&str>,
    requested_slide: Option<usize>,
) -> AppResult<String> {
    let session = required_session(state, code).await?;
    let version = store::get_version(&state.pool, session.deck_version_id).await?;
    let document = parse_deck(&version.source)?;
    Ok(render::live(
        &state.pool,
        &session,
        &version,
        &document,
        view,
        participant,
        requested_slide,
    )
    .await?)
}

async fn available_document(
    state: &AppState,
    session: &LiveSession,
    requested_slide: usize,
) -> AppResult<DeckDocument> {
    let version = store::get_version(&state.pool, session.deck_version_id).await?;
    let document = parse_deck(&version.source)?;
    let current_slide = session.current_slide as usize;
    let last_slide = document.slides.len().saturating_sub(1);
    if requested_slide > last_slide || (session.locked && requested_slide > current_slide) {
        return Err(AppError::bad_request("That slide is not available yet."));
    }
    Ok(document)
}

async fn required_session(state: &AppState, code: &str) -> AppResult<LiveSession> {
    store::session_by_code(&state.pool, code)
        .await?
        .ok_or_else(|| AppError::not_found("Live presentation not found."))
}

fn ensure_participant(mut jar: CookieJar, state: &AppState) -> (CookieJar, String) {
    if let Some(hash) = participant_hash(&jar) {
        return (jar, hash);
    }
    let token = super::random_token();
    let participant = super::hash(&token);
    let cookie = Cookie::build(("slides_participant", token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(state.secure_cookies)
        .build();
    jar = jar.add(cookie);
    (jar, participant)
}

fn valid_option(value: &str, option_count: usize) -> AppResult<&str> {
    let index = value
        .parse::<usize>()
        .map_err(|_| AppError::bad_request("Invalid answer."))?;
    if index >= option_count {
        return Err(AppError::bad_request("Invalid answer."));
    }
    Ok(value)
}

fn normalize_words(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
