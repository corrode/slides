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
    archive,
    error::{AppError, AppResult},
    live::LiveUpdate,
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
}

#[derive(Template)]
#[template(path = "session-ended.html")]
struct SessionEndedTemplate {
    title: String,
    slug: String,
    share_token: String,
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
    let viewers = state.hub.runtime(session.id).await.viewer_count();
    let initial_live = render::live(
        &state.pool,
        &session,
        &version,
        &document,
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
    let viewers = state.hub.runtime(session.id).await.viewer_count();
    let initial_live = render::live(
        &state.pool,
        &session,
        &version,
        &document,
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
    let runtime = state.hub.runtime(session.id).await;
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
                    ("<div id=\"live-error\" class=\"notice error\" hx-swap-oob=\"outerHTML\">Could not apply the latest live update. The next update will retry automatically.</div>".into(), true)
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

pub async fn focus_audience(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(session.id).await;
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
    let runtime = state.hub.runtime(session.id).await;
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
    let runtime = state.hub.runtime(session.id).await;
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
    let runtime = state.hub.runtime(session.id).await;
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
    let runtime = state.hub.runtime(session.id).await;
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
    let runtime = state.hub.runtime(session.id).await;
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
    let runtime = state.hub.runtime(session.id).await;
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
    let runtime = state.hub.runtime(session.id).await;
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
    let runtime = state.hub.runtime(session.id).await;
    let _guard = runtime.mutation.lock().await;
    let session = store::get_session(&state.pool, session.id).await?;
    ensure_session_artifact(&state, &session).await?;
    state.hub.finish(session.id).await;
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
    let artifact = store::artifact_for_session(&state.pool, session.id)
        .await?
        .ok_or_else(|| AppError::not_found("Session archive not found."))?;
    template(SessionEndedTemplate {
        title: artifact.title,
        slug: store::deck_slug_for_session(&state.pool, session.id).await?,
        share_token: artifact.share_token,
    })
}

pub async fn create_artifact(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let session = required_session(&state, &code).await?;
    let runtime = state.hub.runtime(session.id).await;
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

async fn ensure_session_artifact(state: &AppState, session: &LiveSession) -> AppResult<String> {
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
    )
    .await?;
    let token = super::random_token();
    Ok(
        store::finish_session_with_artifact(&state.pool, session.id, ended_at, &token, &archive)
            .await?,
    )
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
    let current = session.current_slide as usize;
    let target = update(current, document.slides.len());
    if target == current {
        return Ok(());
    }
    store::move_to_slide(&state.pool, session.id, target).await?;
    state.hub.notify(session.id, LiveUpdate::SlideChanged).await;
    Ok(())
}

async fn snapshot(
    state: &AppState,
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
    let version = store::get_version(&state.pool, session.deck_version_id).await?;
    let document = parse_deck(&version.source)?;
    let marker = SessionMarker::from(&session);
    let viewers = state.hub.runtime(session.id).await.viewer_count();
    let fragment = render::live(
        &state.pool,
        &session,
        &version,
        &document,
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

    use super::{SessionEndedTemplate, historical_slide, valid_ordering};

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
            share_token: "a".repeat(64),
        }
        .render()
        .unwrap();

        assert!(html.contains("/admin/decks/intro-to-rust/edit"));
        assert!(html.contains("href=\"/admin\""));
        assert!(html.contains(&format!("/shared/{}/", "a".repeat(64))));
    }

    #[test]
    fn ordering_must_be_a_complete_permutation() {
        assert_eq!(valid_ordering("2,0,1", 3).unwrap(), "2,0,1");
        assert!(valid_ordering("0,0,1", 3).is_err());
        assert!(valid_ordering("0,1", 3).is_err());
        assert!(valid_ordering("0,1,3", 3).is_err());
    }
}
