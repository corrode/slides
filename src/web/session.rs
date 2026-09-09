use std::{convert::Infallible, sync::Arc, time::Duration};

use askama::Template;
use async_stream::stream;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{
        Html, IntoResponse, Redirect, Response, Sse,
        sse::{Event, KeepAlive},
    },
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;

use crate::{
    archive,
    error::{AppError, AppResult},
    live::{LiveUpdate, SessionDeck, SessionRuntime},
    markdown::{DeckDocument, Interaction, parse_deck},
    models::{LiveSession, Theme},
    store,
    web::{AppState, is_admin, participant_hash, require_admin, template},
};

use super::render::{self, LiveRequest, LiveView};

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
    has_mermaid: bool,
}

#[derive(Template)]
#[template(path = "session-ended.html")]
struct SessionEndedTemplate {
    title: String,
    slug: String,
    code: String,
    share_token: Option<String>,
}

#[derive(Template)]
#[template(path = "audience.html")]
struct AudienceTemplate {
    title: String,
    events_url: String,
    theme_style: String,
    initial_live: String,
    has_mermaid: bool,
}

#[derive(Debug, Deserialize)]
pub struct JoinForm {
    code: String,
}

#[derive(Debug, Deserialize)]
pub struct EventQuery {
    view: Option<String>,
    slide: Option<usize>,
    presenter_slide: Option<usize>,
    presenter_revision: Option<i64>,
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

#[derive(Debug, Deserialize)]
pub struct QuestionForm {
    body: String,
}

#[derive(Debug, Deserialize)]
pub struct QuestionActionForm {
    action: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionMarker {
    slide: i64,
    follow_revision: i64,
    ended_at: Option<i64>,
}

impl From<&LiveSession> for SessionMarker {
    fn from(session: &LiveSession) -> Self {
        Self {
            slide: session.current_slide,
            follow_revision: session.follow_revision,
            ended_at: session.ended_at,
        }
    }
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
    if let Some(session) = store::active_session(&state.pool).await?
        && session.deck_id == deck.id
    {
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
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let SessionDeck { version, document } =
        runtime.deck(&state.pool, session.deck_version_id).await?;

    let (jar, participant) = ensure_participant(jar, &state);
    let viewers = runtime.viewer_count();
    let initial_live = render::live(
        &state.pool,
        &session,
        version,
        document,
        LiveRequest {
            view: LiveView::Audience,
            participant_hash: Some(&participant),
            requested_slide: query.slide,
            viewers,
        },
    )
    .await?;
    let events_url = match query.slide {
        Some(slide) => format!(
            "/sessions/{}/events?view=audience&slide={slide}&presenter_slide={}&presenter_revision={}",
            session.code, session.current_slide, session.follow_revision
        ),
        None => format!("/sessions/{}/events?view=audience", session.code),
    };
    let page = AudienceTemplate {
        title: version.title.clone(),
        events_url,
        theme_style: Theme::from(version).style(),
        initial_live,
        has_mermaid: document
            .slides
            .iter()
            .any(|slide| slide.html.contains("data-mermaid-diagram")),
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
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let SessionDeck { version, document } =
        runtime.deck(&state.pool, session.deck_version_id).await?;
    let viewers = runtime.viewer_count();
    let initial_live = render::live(
        &state.pool,
        &session,
        version,
        document,
        LiveRequest {
            view: LiveView::Presenter,
            participant_hash: None,
            requested_slide: None,
            viewers,
        },
    )
    .await?;
    template(PresenterTemplate {
        title: version.title.clone(),
        code: session.code.clone(),
        theme_style: Theme::from(version).style(),
        initial_live,
        has_mermaid: document
            .slides
            .iter()
            .any(|slide| slide.html.contains("data-mermaid-diagram")),
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
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let audience_connection = (view == LiveView::Audience).then(|| runtime.track_audience());
    let mut updates = runtime.subscribe();
    let session = store::get_session(&state.pool, session.id).await?;
    let mut requested_slide = historical_slide(
        query.slide,
        query.presenter_slide,
        query.presenter_revision,
        session.current_slide as usize,
        session.follow_revision,
    );
    let stream_state = state.clone();
    let stream_code = code.clone();
    let mut last_marker = SessionMarker::from(&session);

    let events = stream! {
        let _audience_connection = audience_connection;
        let mut reconcile = tokio::time::interval(Duration::from_secs(1));
        reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        reconcile.tick().await;

        'updates: loop {
            let rendered_revision = runtime.revision();
            let (fragment, render_failed) = match snapshot(
                &stream_state,
                &runtime,
                &stream_code,
                view,
                participant.as_deref(),
                requested_slide,
                last_marker,
            ).await {
                Ok((fragment, marker, reconciled_slide)) => {
                    last_marker = marker;
                    requested_slide = reconciled_slide;
                    (fragment, false)
                }
                Err(error) => {
                    tracing::warn!(error = ?error, code = %stream_code, "could not render live update");
                    ("<div id=\"live-error\" class=\"notice error\" role=\"alert\" aria-live=\"assertive\" hx-swap-oob=\"outerHTML\">Could not apply the latest live update. The next update will retry automatically.</div>".into(), true)
                }
            };
            yield Ok::<Event, Infallible>(Event::default().data(fragment));

            loop {
                tokio::select! {
                    update = updates.recv() => match update {
                        Ok(LiveUpdate::Content) => break,
                        Ok(LiveUpdate::SlideChanged | LiveUpdate::Attention)
                        | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            requested_slide = None;
                            break;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break 'updates,
                    },
                    _ = reconcile.tick() => {
                        if render_failed || runtime.revision() != rendered_revision {
                            break;
                        }
                        match required_session(&stream_state, &stream_code).await {
                            Ok(fresh) => {
                                let marker = SessionMarker::from(&fresh);
                                if marker != last_marker {
                                    if marker.slide != last_marker.slide
                                        || marker.follow_revision != last_marker.follow_revision
                                    {
                                        requested_slide = None;
                                    }
                                    break;
                                }
                            }
                            Err(error) => {
                                tracing::warn!(error = ?error, code = %stream_code, "could not reconcile live session");
                                break;
                            }
                        }
                    },
                }
            }
        }
    };

    let mut response = Sse::new(events)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .event(Event::default().event("heartbeat").data("alive")),
        )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

pub async fn first(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    mutate_position(&state, &code, |_, _| 0).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
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

pub async fn focus_audience(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let _guard = runtime.mutation.lock().await;
    store::focus_audience(&state.pool, session.id).await?;
    state.hub.notify(session.id, LiveUpdate::Attention).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn toggle_hand(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    let participant = participant_hash(&jar).ok_or_else(|| {
        AppError::bad_request("Reload the presentation before raising your hand.")
    })?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let _guard = runtime.mutation.lock().await;
    let session = store::get_session(&state.pool, session.id).await?;
    if session.ended_at.is_some() {
        return Err(AppError::bad_request("This presentation has ended."));
    }
    store::toggle_hand(&state.pool, session.id, &participant).await?;
    state.hub.notify(session.id, LiveUpdate::Content).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn reset_hands(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let _guard = runtime.mutation.lock().await;
    store::reset_hands(&state.pool, session.id).await?;
    state.hub.notify(session.id, LiveUpdate::Content).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn toggle_lock(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let _guard = runtime.mutation.lock().await;
    let fresh = store::get_session(&state.pool, session.id).await?;
    store::set_lock(&state.pool, fresh.id, !fresh.locked).await?;
    state.hub.notify(session.id, LiveUpdate::Content).await;
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
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let _guard = runtime.mutation.lock().await;
    store::set_interaction_state(&state.pool, session.id, open, revealed).await?;
    state.hub.notify(session.id, LiveUpdate::Content).await;
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
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
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
        Interaction::Ordering { options, .. } => {
            let value = valid_ordering(&form.value, options.len())?;
            store::replace_answer(
                &state.pool,
                session.id,
                slide_index,
                &participant,
                "ordering",
                &value,
            )
            .await?;
        }
    }
    state.hub.notify(session.id, LiveUpdate::Content).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn ask_question(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
    Form(form): Form<QuestionForm>,
) -> AppResult<Response> {
    const MAX_QUESTIONS_PER_PARTICIPANT: i64 = 5;
    const MAX_QUESTION_LENGTH: usize = 280;

    let participant = participant_hash(&jar).ok_or_else(|| {
        AppError::bad_request("Reload the presentation before asking a question.")
    })?;
    let body = form.body.split_whitespace().collect::<Vec<_>>().join(" ");
    if body.is_empty() || body.chars().count() > MAX_QUESTION_LENGTH {
        return Err(AppError::bad_request(format!(
            "Questions must contain between 1 and {MAX_QUESTION_LENGTH} characters."
        )));
    }

    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let _guard = runtime.mutation.lock().await;
    let session = store::get_session(&state.pool, session.id).await?;
    if session.ended_at.is_some() {
        return Err(AppError::bad_request("This presentation has ended."));
    }
    if store::participant_question_count(&state.pool, session.id, &participant).await?
        >= MAX_QUESTIONS_PER_PARTICIPANT
    {
        return Err(AppError::bad_request(
            "You have reached the five-question limit for this presentation.",
        ));
    }

    store::create_question(&state.pool, session.id, &participant, &body).await?;
    state.hub.notify(session.id, LiveUpdate::Content).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn toggle_question_upvote(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((code, question_id)): Path<(String, i64)>,
) -> AppResult<Response> {
    let participant = participant_hash(&jar)
        .ok_or_else(|| AppError::bad_request("Reload the presentation before voting."))?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let _guard = runtime.mutation.lock().await;
    let session = store::get_session(&state.pool, session.id).await?;
    if session.ended_at.is_some() {
        return Err(AppError::bad_request("This presentation has ended."));
    }

    store::toggle_question_upvote(&state.pool, session.id, question_id, &participant).await?;
    state.hub.notify(session.id, LiveUpdate::Content).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn moderate_question(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((code, question_id)): Path<(String, i64)>,
    Form(form): Form<QuestionActionForm>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let _guard = runtime.mutation.lock().await;
    let changed = match form.action.as_str() {
        "answered" => {
            store::set_question_answered(&state.pool, session.id, question_id, true).await?
        }
        "unanswered" => {
            store::set_question_answered(&state.pool, session.id, question_id, false).await?
        }
        "dismiss" => store::dismiss_question(&state.pool, session.id, question_id).await?,
        _ => return Err(AppError::bad_request("Unknown question action.")),
    };
    if !changed {
        return Err(AppError::bad_request("Question not found."));
    }
    state.hub.notify(session.id, LiveUpdate::Content).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn react(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((code, kind)): Path<(String, String)>,
    Form(form): Form<ReactionForm>,
) -> AppResult<Response> {
    const ALLOWED: &[&str] = &["applause", "lightbulb", "question"];
    if !ALLOWED.contains(&kind.as_str()) {
        return Err(AppError::bad_request("Unknown reaction."));
    }
    let participant = participant_hash(&jar)
        .ok_or_else(|| AppError::bad_request("Reload the presentation before reacting."))?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let _guard = runtime.mutation.lock().await;
    let session = store::get_session(&state.pool, session.id).await?;
    if session.ended_at.is_some() {
        return Err(AppError::bad_request("This presentation has ended."));
    }
    let _ = available_document(&state, &session, form.slide).await?;
    let inserted =
        store::add_reaction(&state.pool, session.id, form.slide, &participant, &kind).await?;
    if inserted {
        state.hub.notify(session.id, LiveUpdate::Content).await;
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
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let _guard = runtime.mutation.lock().await;
    let mut session = store::get_session(&state.pool, session.id).await?;
    let ended_at = session.ended_at.unwrap_or_else(store::now_millis);
    store::end_session(&state.pool, session.id, ended_at).await?;
    session.ended_at = Some(ended_at);
    state.hub.finish(session.id).await;
    if let Err(error) = ensure_session_artifact(&state, &session).await {
        tracing::error!(
            session_id = session.id,
            ?error,
            "session ended without an archive"
        );
    }
    Ok(Redirect::to(&format!("/admin/sessions/{code}/ended")).into_response())
}

pub async fn ended(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let session = required_session(&state, &code).await?;
    if session.ended_at.is_none() {
        return Err(AppError::bad_request("This presentation is still live."));
    }
    let version = store::get_version(&state.pool, session.deck_version_id).await?;
    let share_token = store::artifact_for_session(&state.pool, session.id)
        .await?
        .map(|artifact| artifact.share_token);
    template(SessionEndedTemplate {
        title: version.title,
        slug: store::deck_slug_for_session(&state.pool, session.id).await?,
        code: session.code,
        share_token,
    })
}

pub async fn create_artifact(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let _guard = runtime.mutation.lock().await;
    let session = store::get_session(&state.pool, session.id).await?;
    if session.ended_at.is_none() {
        return Err(AppError::bad_request(
            "End the presentation before creating its archive.",
        ));
    }
    let token = ensure_session_artifact(&state, &session).await?;
    Ok(Redirect::to(&format!("/shared/{token}/")).into_response())
}

async fn ensure_session_artifact(
    state: &AppState,
    session: &LiveSession,
) -> anyhow::Result<String> {
    if let Some(artifact) = store::artifact_for_session(&state.pool, session.id).await? {
        return Ok(artifact.share_token);
    }

    let version = store::get_version(&state.pool, session.deck_version_id).await?;
    let document = parse_deck(&version.source)?;
    let started_at = store::session_started_at(&state.pool, session.id).await?;
    let ended_at = session.ended_at.unwrap_or_else(store::now_millis);
    let archive = archive::build(
        &state.pool,
        session,
        &version,
        &document,
        started_at,
        ended_at,
        &state.embed_dir,
    )
    .await?;
    let token = super::random_token();
    store::create_session_artifact(&state.pool, session.id, &token, &archive).await
}

async fn mutate_position(
    state: &AppState,
    code: &str,
    update: impl FnOnce(usize, usize) -> usize,
) -> AppResult<()> {
    let session = required_session(state, code).await?;
    if session.ended_at.is_some() {
        return Err(AppError::bad_request("This presentation has ended."));
    }
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let deck = runtime.deck(&state.pool, session.deck_version_id).await?;
    let _guard = runtime.mutation.lock().await;
    let session = store::get_session(&state.pool, session.id).await?;
    // Ending the session may have won the mutation lock while the deck loaded.
    if session.ended_at.is_some() {
        return Err(AppError::bad_request("This presentation has ended."));
    }
    let current = session.current_slide as usize;
    let target = update(current, deck.document.slides.len());
    if target == current {
        return Ok(());
    }
    store::move_to_slide(&state.pool, session.id, target).await?;
    state.hub.notify(session.id, LiveUpdate::SlideChanged).await;
    Ok(())
}

async fn snapshot(
    state: &AppState,
    runtime: &SessionRuntime,
    code: &str,
    view: LiveView,
    participant: Option<&str>,
    requested_slide: Option<usize>,
    expected_marker: SessionMarker,
) -> AppResult<(String, SessionMarker, Option<usize>)> {
    let session = required_session(state, code).await?;
    let requested_slide = historical_slide(
        requested_slide,
        Some(expected_marker.slide as usize),
        Some(expected_marker.follow_revision),
        session.current_slide as usize,
        session.follow_revision,
    );
    let SessionDeck { version, document } =
        runtime.deck(&state.pool, session.deck_version_id).await?;
    let marker = SessionMarker::from(&session);
    let viewers = runtime.viewer_count();
    let fragment = render::live(
        &state.pool,
        &session,
        version,
        document,
        LiveRequest {
            view,
            participant_hash: participant,
            requested_slide,
            viewers,
        },
    )
    .await?;
    Ok((fragment, marker, requested_slide))
}

async fn available_document(
    state: &AppState,
    session: &LiveSession,
    requested_slide: usize,
) -> AppResult<Arc<DeckDocument>> {
    let runtime = state.hub.runtime(&state.pool, session.id).await?;
    let document = &runtime
        .deck(&state.pool, session.deck_version_id)
        .await?
        .document;
    let current_slide = session.current_slide as usize;
    let last_slide = document.slides.len().saturating_sub(1);
    if requested_slide > last_slide || (session.locked && requested_slide > current_slide) {
        return Err(AppError::bad_request("That slide is not available yet."));
    }
    Ok(Arc::clone(document))
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

fn valid_ordering(value: &str, option_count: usize) -> AppResult<String> {
    let indices = value
        .split(',')
        .map(str::parse::<usize>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AppError::bad_request("Invalid ordering."))?;
    if indices.len() != option_count {
        return Err(AppError::bad_request("Invalid ordering."));
    }

    let mut seen = vec![false; option_count];
    for index in &indices {
        if *index >= option_count || seen[*index] {
            return Err(AppError::bad_request("Invalid ordering."));
        }
        seen[*index] = true;
    }
    Ok(indices
        .into_iter()
        .map(|index| index.to_string())
        .collect::<Vec<_>>()
        .join(","))
}

fn historical_slide(
    requested_slide: Option<usize>,
    observed_presenter_slide: Option<usize>,
    observed_follow_revision: Option<i64>,
    current_presenter_slide: usize,
    current_follow_revision: i64,
) -> Option<usize> {
    (observed_presenter_slide == Some(current_presenter_slide)
        && observed_follow_revision == Some(current_follow_revision))
    .then_some(requested_slide)
    .flatten()
}

#[cfg(test)]
mod tests {
    use askama::Template;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use futures_util::StreamExt;
    use tower::ServiceExt;

    use crate::live::LiveHub;

    use super::*;

    async fn test_session(source: &str) -> (tempfile::TempDir, AppState, LiveSession) {
        let directory = tempfile::tempdir().unwrap();
        let pool = store::connect(&format!(
            "sqlite://{}",
            directory.path().join("slides.db").display()
        ))
        .await
        .unwrap();
        let deck = store::create_deck(&pool, "cache", "Original title")
            .await
            .unwrap();
        let version = store::save_and_publish_deck(
            &pool,
            deck.id,
            "Original title",
            source,
            source,
            &Theme::default(),
        )
        .await
        .unwrap();
        let session = store::start_session(&pool, deck.id, version).await.unwrap();
        let state = AppState {
            pool,
            hub: Arc::new(LiveHub::default()),
            admin_password_hash: super::super::hash("password"),
            admin_cookie: super::super::hash("cookie"),
            secure_cookies: false,
            embed_dir: directory.path().join("embeds"),
        };
        (directory, state, session)
    }

    #[tokio::test]
    async fn ended_page_requests_do_not_resurrect_a_retained_cache() {
        let (_directory, state, session) =
            test_session("# Ended\n\n```rust\nfn main() {}\n```").await;
        let runtime = state.hub.runtime(&state.pool, session.id).await.unwrap();
        let cached = runtime
            .deck(&state.pool, session.deck_version_id)
            .await
            .unwrap();
        let document = Arc::downgrade(&cached.document);
        let weak_runtime = Arc::downgrade(&runtime);
        store::end_session(&state.pool, session.id, store::now_millis())
            .await
            .unwrap();
        state.hub.finish(session.id).await;
        drop(runtime);
        assert!(document.upgrade().is_none());
        assert!(weak_runtime.upgrade().is_none());

        let app = super::super::router(state.clone());
        for page in ["join", "present"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/{page}/{}", session.code))
                        .header(
                            header::COOKIE,
                            format!("slides_admin={}", state.admin_cookie),
                        )
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert!(
                std::str::from_utf8(&body)
                    .unwrap()
                    .contains("Session ended")
            );
            drop(body);
            assert_eq!(state.hub.retained_session_count().await, 0);
        }

        // A late mutation's notification must not recreate the hub entry either.
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/sessions/{}/attention", session.code))
                    .header(
                        header::COOKIE,
                        format!("slides_admin={}", state.admin_cookie),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        drop(response);
        assert_eq!(state.hub.retained_session_count().await, 0);
    }

    #[tokio::test]
    async fn ended_event_request_owns_its_runtime_until_disconnect() {
        let (_directory, state, session) = test_session("# Ended").await;
        store::end_session(&state.pool, session.id, store::now_millis())
            .await
            .unwrap();
        state.hub.finish(session.id).await;
        let response = events(
            State(state.clone()),
            CookieJar::new(),
            Path(session.code),
            Query(EventQuery {
                view: None,
                slide: None,
                presenter_slide: None,
                presenter_revision: None,
            }),
        )
        .await
        .unwrap();
        let mut frames = response.into_body().into_data_stream();
        let frame = tokio::time::timeout(Duration::from_secs(2), frames.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            std::str::from_utf8(&frame)
                .unwrap()
                .contains("Session ended")
        );
        assert_eq!(state.hub.retained_session_count().await, 0);
        drop(frames);
        assert_eq!(state.hub.retained_session_count().await, 0);
    }

    #[tokio::test]
    async fn events_send_data_bearing_heartbeats_and_proxy_headers() {
        let (_directory, state, session) = test_session("# Heartbeat").await;
        let runtime = state.hub.runtime(&state.pool, session.id).await.unwrap();
        let response = events(
            State(state),
            CookieJar::new(),
            Path(session.code),
            Query(EventQuery {
                view: None,
                slide: None,
                presenter_slide: None,
                presenter_revision: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "no-cache, no-transform"
        );
        assert_eq!(response.headers()["x-accel-buffering"], "no");
        let mut frames = response.into_body().into_data_stream();
        let first = tokio::time::timeout(Duration::from_secs(2), frames.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let first = std::str::from_utf8(&first).unwrap();
        assert!(first.starts_with("data: "));
        assert!(first.contains("Heartbeat"));
        let revision = runtime.revision();
        let heartbeat = tokio::time::timeout(Duration::from_secs(16), frames.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            std::str::from_utf8(&heartbeat).unwrap(),
            "event: heartbeat\ndata: alive\n\n"
        );
        assert_eq!(runtime.revision(), revision);
        assert_eq!(runtime.viewer_count(), 1);
        drop(frames);
        assert_eq!(runtime.viewer_count(), 0);
    }

    #[tokio::test]
    async fn pages_navigation_and_historical_snapshots_reuse_the_cached_deck() {
        let (_directory, state, session) =
            test_session("# Historical content\n\n---\n\n# Current content").await;
        let runtime = state.hub.runtime(&state.pool, session.id).await.unwrap();
        let cached = runtime
            .deck(&state.pool, session.deck_version_id)
            .await
            .unwrap();
        let audience_page = audience(
            State(state.clone()),
            CookieJar::new(),
            Path(session.code.clone()),
            Query(AudienceQuery::default()),
        )
        .await
        .unwrap();
        assert_eq!(audience_page.status(), StatusCode::OK);
        let admin_jar =
            CookieJar::new().add(Cookie::new("slides_admin", state.admin_cookie.clone()));
        let presenter_page = presenter(State(state.clone()), admin_jar, Path(session.code.clone()))
            .await
            .unwrap();
        assert_eq!(presenter_page.status(), StatusCode::OK);
        let body = to_bytes(presenter_page.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            std::str::from_utf8(&body)
                .unwrap()
                .contains("Historical content")
        );

        mutate_position(&state, &session.code, |_, len| len - 1)
            .await
            .unwrap();
        let current = store::get_session(&state.pool, session.id).await.unwrap();
        assert_eq!(current.current_slide, 1);
        let document = available_document(&state, &current, 0).await.unwrap();
        assert!(Arc::ptr_eq(&cached.document, &document));
        assert!(available_document(&state, &current, 2).await.is_err());

        let (historical, marker, requested) = snapshot(
            &state,
            &runtime,
            &session.code,
            LiveView::Audience,
            None,
            Some(0),
            SessionMarker::from(&current),
        )
        .await
        .unwrap();
        assert!(historical.contains("Historical content"));
        assert_eq!(requested, Some(0));
        let (following, _, _) = snapshot(
            &state,
            &runtime,
            &session.code,
            LiveView::Audience,
            None,
            None,
            marker,
        )
        .await
        .unwrap();
        assert!(following.contains("Current content"));

        // Even a return to the same slide expires history via follow_revision.
        mutate_position(&state, &session.code, |_, _| 0)
            .await
            .unwrap();
        mutate_position(&state, &session.code, |_, _| 1)
            .await
            .unwrap();
        let (following, _, requested) = snapshot(
            &state,
            &runtime,
            &session.code,
            LiveView::Audience,
            None,
            Some(0),
            marker,
        )
        .await
        .unwrap();
        assert!(following.contains("Current content"));
        assert_eq!(requested, None);
        assert!(std::ptr::eq(
            cached,
            runtime
                .deck(&state.pool, session.deck_version_id)
                .await
                .unwrap()
        ));
    }

    #[tokio::test]
    async fn publishing_does_not_change_a_sessions_cached_or_cold_version() {
        let (_directory, state, session) = test_session("# Original slide").await;
        let runtime = state.hub.runtime(&state.pool, session.id).await.unwrap();
        let cached = runtime
            .deck(&state.pool, session.deck_version_id)
            .await
            .unwrap();
        let deck = store::deck_by_slug(&state.pool, "cache")
            .await
            .unwrap()
            .unwrap();
        let theme = Theme {
            accent: "#123456".into(),
            ..Theme::default()
        };
        let new_version = store::save_and_publish_deck(
            &state.pool,
            deck.id,
            "New title",
            "# New draft",
            "# New publication",
            &theme,
        )
        .await
        .unwrap();
        assert_ne!(session.deck_version_id, new_version);
        assert!(std::ptr::eq(
            cached,
            runtime
                .deck(&state.pool, session.deck_version_id)
                .await
                .unwrap()
        ));

        // A fresh hub (e.g. after a restart) still loads the session's pinned version.
        let cold_hub = LiveHub::default();
        let cold_runtime = cold_hub.runtime(&state.pool, session.id).await.unwrap();
        let cold = cold_runtime
            .deck(&state.pool, session.deck_version_id)
            .await
            .unwrap();
        for deck in [cached, cold] {
            assert_eq!(deck.version.title, "Original title");
            assert_eq!(deck.version.theme_accent, Theme::default().accent);
            assert!(deck.document.slides[0].html.contains("Original slide"));
            assert!(!deck.document.slides[0].html.contains("New publication"));
        }
        store::end_session(&state.pool, session.id, store::now_millis())
            .await
            .unwrap();
        state.hub.finish(session.id).await;
        let new_session = store::start_session(&state.pool, deck.id, new_version)
            .await
            .unwrap();
        let new_runtime = state
            .hub
            .runtime(&state.pool, new_session.id)
            .await
            .unwrap();
        let new_deck = new_runtime
            .deck(&state.pool, new_session.deck_version_id)
            .await
            .unwrap();
        assert_eq!(new_deck.version.title, "New title");
        assert!(new_deck.document.slides[0].html.contains("New publication"));
        assert!(!Arc::ptr_eq(&cached.document, &new_deck.document));
    }

    #[tokio::test]
    async fn navigation_rechecks_ended_state_after_waiting_for_the_mutation_lock() {
        let (_directory, state, session) = test_session("# First\n\n---\n\n# Second").await;
        let runtime = state.hub.runtime(&state.pool, session.id).await.unwrap();
        runtime
            .deck(&state.pool, session.deck_version_id)
            .await
            .unwrap();
        let guard = runtime.mutation.lock().await;
        let mut navigation = std::pin::pin!(mutate_position(&state, &session.code, |_, _| {
            panic!("ended navigation must not call the update closure")
        }));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut navigation)
                .await
                .is_err()
        );
        store::end_session(&state.pool, session.id, store::now_millis())
            .await
            .unwrap();
        state.hub.finish(session.id).await;
        let revision = runtime.revision();
        drop(guard);
        assert_eq!(
            navigation.await.unwrap_err().into_response().status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(runtime.revision(), revision);
        assert!(
            mutate_position(&state, &session.code, |_, _| 1)
                .await
                .is_err()
        );
        let ended = store::get_session(&state.pool, session.id).await.unwrap();
        assert_eq!(ended.current_slide, 0);
        assert_eq!(ended.follow_revision, session.follow_revision);
    }

    #[tokio::test]
    #[ignore = "manual navigation timing with a larger highlighted deck"]
    async fn benchmark_cached_navigation() {
        let code =
            "fn example(value: usize) -> usize { (0..value).map(|x| x * 2).sum() }\n".repeat(40);
        let source = (0..40)
            .map(|slide| format!("# Slide {slide}\n\n```rust\n{code}```"))
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");
        let (_directory, state, session) = test_session(&source).await;
        let runtime = state.hub.runtime(&state.pool, session.id).await.unwrap();
        let start = std::time::Instant::now();
        let cached = runtime
            .deck(&state.pool, session.deck_version_id)
            .await
            .unwrap();
        let cold = start.elapsed();
        let start = std::time::Instant::now();
        for _ in 0..100 {
            mutate_position(&state, &session.code, |current, len| (current + 1) % len)
                .await
                .unwrap();
        }
        let navigation = start.elapsed();
        assert!(std::ptr::eq(
            cached,
            runtime
                .deck(&state.pool, session.deck_version_id)
                .await
                .unwrap()
        ));
        eprintln!(
            "40 slides / 1600 Rust lines: cold load {cold:?}; 100 cached navigations {navigation:?}"
        );
    }

    #[test]
    fn live_pages_load_mermaid_eagerly_only_when_the_deck_uses_it() {
        let presenter = PresenterTemplate {
            title: "Deck".into(),
            code: "123456".into(),
            theme_style: String::new(),
            initial_live: String::new(),
            has_mermaid: true,
        }
        .render()
        .unwrap();
        let audience = AudienceTemplate {
            title: "Deck".into(),
            events_url: "/sessions/123456/events?view=audience".into(),
            theme_style: String::new(),
            initial_live: String::new(),
            has_mermaid: true,
        }
        .render()
        .unwrap();
        let audience_without_mermaid = AudienceTemplate {
            title: "Deck".into(),
            events_url: "/sessions/123456/events?view=audience".into(),
            theme_style: String::new(),
            initial_live: String::new(),
            has_mermaid: false,
        }
        .render()
        .unwrap();

        assert!(presenter.contains("/assets/vendor/mermaid/mermaid.min.js"));
        assert!(audience.contains("/assets/vendor/mermaid/mermaid.min.js"));
        assert!(!audience_without_mermaid.contains("/assets/vendor/mermaid/mermaid.min.js"));
    }

    #[test]
    fn historical_slide_expires_when_the_presenter_moves_or_requests_attention() {
        assert_eq!(historical_slide(Some(1), Some(2), Some(4), 2, 4), Some(1));
        assert_eq!(historical_slide(Some(1), Some(2), Some(4), 3, 4), None);
        assert_eq!(historical_slide(Some(1), Some(2), Some(4), 2, 5), None);
        assert_eq!(historical_slide(Some(1), None, None, 2, 4), None);
    }

    #[test]
    fn ended_session_links_to_editor_overview_and_archive() {
        let html = SessionEndedTemplate {
            title: "Intro to Rust".into(),
            slug: "intro-to-rust".into(),
            code: "123456".into(),
            share_token: Some("a".repeat(64)),
        }
        .render()
        .unwrap();

        assert!(html.contains("/admin/decks/intro-to-rust/edit"));
        assert!(html.contains("href=\"/admin\""));
        assert!(html.contains(&format!("/shared/{}/", "a".repeat(64))));
    }

    #[test]
    fn ended_session_offers_archive_retry_when_automatic_creation_fails() {
        let html = SessionEndedTemplate {
            title: "Intro to Rust".into(),
            slug: "intro-to-rust".into(),
            code: "123456".into(),
            share_token: None,
        }
        .render()
        .unwrap();

        assert!(html.contains("no longer live"));
        assert!(html.contains("action=\"/admin/sessions/123456/artifact\""));
    }

    #[test]
    fn ordering_must_be_a_complete_permutation() {
        assert_eq!(valid_ordering("2,0,1", 3).unwrap(), "2,0,1");
        assert!(valid_ordering("0,0,1", 3).is_err());
        assert!(valid_ordering("0,1", 3).is_err());
        assert!(valid_ordering("0,1,3", 3).is_err());
    }
}
