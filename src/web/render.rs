use std::collections::HashMap;

use anyhow::Result;
use html_escape::{encode_double_quoted_attribute, encode_text};
use sqlx::SqlitePool;

use crate::{
    markdown::{ChartOrientation, DeckDocument, Interaction, Slide},
    models::{DeckVersion, LiveSession, Theme},
    store,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveView {
    Presenter,
    Audience,
}

#[derive(Debug, Clone, Copy)]
pub struct LiveRequest<'a> {
    pub view: LiveView,
    pub participant_hash: Option<&'a str>,
    pub requested_slide: Option<usize>,
    pub viewers: u64,
}

#[derive(Debug, Default)]
struct LiveData {
    selected: Vec<String>,
    counts: HashMap<String, i64>,
    word_cloud_responses: Vec<store::WordCloudResponse>,
    ordering_values: Vec<String>,
    reactions: HashMap<String, i64>,
    answerers: i64,
    raised_hands: i64,
    hand_raised: bool,
    viewers: u64,
    questions: Vec<store::QuestionRow>,
}

pub fn printable(document: &DeckDocument) -> String {
    document
        .slides
        .iter()
        .enumerate()
        .map(|(index, slide)| {
            let mut body = slide.html.clone();
            if let Some(interaction) = &slide.interaction {
                body.push_str(&preview_interaction(interaction));
            }
            format!(
                "<article class=\"print-slide\" aria-label=\"Slide {}\"><div class=\"slide-content\">{body}</div></article>",
                index + 1
            )
        })
        .collect()
}

pub async fn archived_slides(
    pool: &SqlitePool,
    session: &LiveSession,
    document: &DeckDocument,
) -> Result<String> {
    let mut slides = String::new();
    for (index, slide) in document.slides.iter().enumerate() {
        let data = live_data(pool, session, slide, index, None).await?;
        let interaction = slide
            .interaction
            .as_ref()
            .map(|spec| {
                interaction_results(
                    spec,
                    &data.counts,
                    data.answerers,
                    index,
                    &data.word_cloud_responses,
                    &data.ordering_values,
                    false,
                )
            })
            .unwrap_or_default();
        slides.push_str(&format!(
            "<article class=\"archive-slide\" aria-label=\"Slide {}\"><div class=\"slide-content\">{}{interaction}{}</div></article>",
            index + 1,
            slide.html,
            archived_reactions(&data.reactions),
        ));
    }
    let questions = store::list_visible_questions(pool, session.id, "").await?;
    slides.push_str(&archived_questions(&questions));
    Ok(slides)
}

fn archived_questions(questions: &[store::QuestionRow]) -> String {
    if questions.is_empty() {
        return String::new();
    }
    let items = questions
        .iter()
        .map(|question| {
            let status = if question.answered {
                " · Answered"
            } else {
                ""
            };
            format!(
                "<li><p>{}</p><small>{} upvotes{status}</small></li>",
                encode_text(&question.body),
                question.vote_count,
            )
        })
        .collect::<String>();
    format!(
        "<section class=\"archive-questions\"><h2>Audience questions</h2><ol>{items}</ol></section>"
    )
}

pub fn preview(document: &DeckDocument, theme: &Theme) -> String {
    if document.slides.is_empty() {
        return "<div class=\"empty-state\">Nothing to preview yet.</div>".into();
    }

    let slides = document
        .slides
        .iter()
        .enumerate()
        .map(|(index, slide)| {
            let mut body = slide.html.clone();
            if let Some(interaction) = &slide.interaction {
                body.push_str(&preview_interaction(interaction));
            }
            let active = if index == 0 { " active" } else { "" };
            let current = if index == 0 { "true" } else { "false" };
            format!(
                "<article class=\"slide{active}\" data-preview-slide aria-current=\"{current}\" aria-label=\"Slide {}\"><div class=\"slide-content\">{body}</div></article>",
                index + 1
            )
        })
        .collect::<String>();
    let next_disabled = if document.slides.len() == 1 {
        " disabled"
    } else {
        ""
    };

    format!(
        "<div class=\"editor-preview\" data-preview-deck data-presentation-theme data-slide-index=\"0\" style=\"{}\"><div class=\"slide-stage\">{slides}</div><nav class=\"preview-navigation\" aria-label=\"Preview slide navigation\"><button class=\"secondary\" type=\"button\" data-preview-nav=\"previous\" disabled>{}Previous</button><span class=\"preview-position\" data-preview-position aria-live=\"polite\">Slide 1 of {total}</span><button class=\"secondary\" type=\"button\" data-preview-nav=\"next\"{next_disabled}>Next{}</button></nav></div>",
        encode_double_quoted_attribute(&theme.style()),
        icon("previous"),
        icon("next"),
        total = document.slides.len(),
    )
}

pub async fn live(
    pool: &SqlitePool,
    session: &LiveSession,
    version: &DeckVersion,
    document: &DeckDocument,
    request: LiveRequest<'_>,
) -> Result<String> {
    if session.ended_at.is_some() {
        return Ok("<main id=\"live-view\" class=\"audience-shell\"><section class=\"interaction session-complete-state\"><p class=\"status-pill success\">Session ended</p><h1>Thanks for taking part.</h1><p>The presenter has ended this presentation.</p><a class=\"button secondary\" href=\"/\">Join another presentation</a></section></main>".into());
    }

    let current = (session.current_slide as usize).min(document.slides.len().saturating_sub(1));
    let last = document.slides.len().saturating_sub(1);
    let index = if request.view == LiveView::Audience {
        request
            .requested_slide
            .map(|requested| requested.min(if session.locked { current } else { last }))
            .unwrap_or(current)
    } else {
        current
    };
    let slide = &document.slides[index];
    let mut data = live_data(pool, session, slide, index, request.participant_hash).await?;
    data.viewers = request.viewers;

    Ok(match request.view {
        LiveView::Presenter => {
            let deck_slug = store::deck_slug_for_session(pool, session.id).await?;
            presenter_view(session, version, &deck_slug, document, slide, index, &data)
        }
        LiveView::Audience => audience_view(
            session,
            &version.title,
            slide,
            index,
            document.slides.len(),
            &data,
        ),
    })
}

async fn live_data(
    pool: &SqlitePool,
    session: &LiveSession,
    slide: &Slide,
    index: usize,
    participant_hash: Option<&str>,
) -> Result<LiveData> {
    let selected = if let Some(participant_hash) = participant_hash {
        store::selected_values(pool, session.id, index, participant_hash).await?
    } else {
        Vec::new()
    };
    let counts = store::value_counts(pool, session.id, index)
        .await?
        .into_iter()
        .map(|item| (item.value, item.count))
        .collect();
    let word_cloud_responses = if matches!(&slide.interaction, Some(Interaction::WordCloud { .. }))
    {
        store::word_cloud_responses(pool, session.id, index).await?
    } else {
        Vec::new()
    };
    let ordering_values = if matches!(&slide.interaction, Some(Interaction::Ordering { .. })) {
        store::ordering_values(pool, session.id, index).await?
    } else {
        Vec::new()
    };
    let reactions = store::reaction_counts(pool, session.id, index)
        .await?
        .into_iter()
        .map(|item| (item.kind, item.count))
        .collect();
    let hand_raised = if let Some(participant_hash) = participant_hash {
        store::hand_is_raised(pool, session.id, participant_hash).await?
    } else {
        false
    };
    Ok(LiveData {
        selected,
        counts,
        word_cloud_responses,
        ordering_values,
        reactions,
        answerers: store::answerer_count(pool, session.id, index).await?,
        raised_hands: store::raised_hand_count(pool, session.id).await?,
        hand_raised,
        viewers: 0,
        questions: store::list_visible_questions(
            pool,
            session.id,
            participant_hash.unwrap_or_default(),
        )
        .await?,
    })
}

fn archived_reactions(reactions: &HashMap<String, i64>) -> String {
    let items = [
        ("applause", "👏", "Applause"),
        ("lightbulb", "💡", "Lightbulb"),
        ("question", "❓", "Question"),
    ]
    .into_iter()
    .filter_map(|(key, symbol, label)| {
        let count = reactions.get(key).copied().unwrap_or_default();
        (count > 0).then(|| {
            format!(
                "<span title=\"{label}\"><span aria-hidden=\"true\">{symbol}</span> {count}</span>"
            )
        })
    })
    .collect::<String>();
    if items.is_empty() {
        String::new()
    } else {
        format!("<aside class=\"archive-reactions\"><strong>Reactions</strong>{items}</aside>")
    }
}

// Share a gate outside #live-view so morphs cannot reset in-flight navigation.
const NAVIGATION_REQUEST_ATTRIBUTES: &str = r#"hx-sync="body:drop" hx-config='{"timeout":"8s"}'"#;

fn presenter_view(
    session: &LiveSession,
    version: &DeckVersion,
    deck_slug: &str,
    document: &DeckDocument,
    slide: &Slide,
    index: usize,
    data: &LiveData,
) -> String {
    let first_disabled = if index == 0 { " disabled" } else { "" };
    let previous_disabled = first_disabled;
    let next_disabled = if index + 1 >= document.slides.len() {
        " disabled"
    } else {
        ""
    };
    let (lock_label, lock_icon) = if session.locked {
        ("Unlock navigation", "unlock")
    } else {
        ("Lock navigation", "lock")
    };
    let interaction = slide
        .interaction
        .as_ref()
        .map(|spec| {
            interaction_results(
                spec,
                &data.counts,
                data.answerers,
                index,
                &data.word_cloud_responses,
                &data.ordering_values,
                true,
            )
        })
        .unwrap_or_default();
    let interaction_controls = if slide.interaction.is_some() {
        let action = if session.interaction_open {
            "close"
        } else {
            "open"
        };
        let label = if session.interaction_open {
            "Close voting"
        } else {
            "Open voting"
        };
        format!(
            "<form class=\"inline-form\" hx-post=\"/sessions/{}/interaction\" hx-swap=\"none\" hx-disable=\"find button\"><input type=\"hidden\" name=\"action\" value=\"{}\"><button class=\"secondary small\" type=\"submit\">{}{}</button></form><form class=\"inline-form\" hx-post=\"/sessions/{}/interaction\" hx-swap=\"none\" hx-disable=\"find button\"><input type=\"hidden\" name=\"action\" value=\"reveal\"><button class=\"secondary small\" type=\"submit\">{}Reveal</button></form>",
            session.code,
            action,
            icon("responses"),
            label,
            session.code,
            icon("reveal"),
        )
    } else {
        String::new()
    };
    let hand_signal = presenter_hand_signal(&session.code, data.raised_hands);
    let live_status = live_status(data.viewers);
    let notes = presenter_notes(slide.notes.as_deref());
    let questions = presenter_questions(&session.code, &data.questions);

    format!(
        "<main id=\"live-view\" class=\"presenter-shell\" data-slide-index=\"{index}\"><nav class=\"presenter-toolbar\" aria-label=\"Presentation controls\"><div class=\"presenter-status\"><a class=\"brand\" href=\"/admin\">Slides</a>{live_status}<strong class=\"nav-title\">{title}</strong><span class=\"nav-position\">{position}/{total}</span></div><div class=\"presenter-share\"><span class=\"share-code\"><span>Join code</span><strong>{code}</strong></span><button class=\"secondary small\" type=\"button\" data-share-url=\"/join/{code}\">{share_icon}Copy link</button><span id=\"share-status\" class=\"share-status\" role=\"status\"></span></div><div class=\"presenter-actions\">{color_scheme_toggle}<a class=\"button secondary small\" href=\"/admin/decks/{deck_slug}/edit\" target=\"_blank\" rel=\"noopener\" title=\"Edit presentation in a new tab\">{edit_icon}Edit</a><button class=\"secondary small\" hx-post=\"/sessions/{code}/lock\" hx-swap=\"none\" hx-disable=\"this\">{lock_icon_markup}{lock_label}</button>{interaction_controls}<form class=\"inline-form\" method=\"post\" action=\"/sessions/{code}/end\" data-confirm=\"End this live session?\"><button class=\"danger small\" type=\"submit\">{end_icon}End</button></form></div></nav><div class=\"slide-stage\"><article class=\"slide active\" aria-label=\"Slide {position} of {total}\"><div class=\"slide-content\">{slide_html}{interaction}<div class=\"presenter-reactions\">{reactions}</div></div></article></div>{questions}{notes}<nav class=\"presentation-navigation\" aria-label=\"Slide navigation\"><button class=\"secondary\" data-nav=\"first\" title=\"Jump to first slide\" hx-post=\"/sessions/{code}/first\" hx-swap=\"none\" {navigation_request}{first_disabled}>{first_icon}First</button><button class=\"secondary\" data-nav=\"previous\" hx-post=\"/sessions/{code}/previous\" hx-swap=\"none\" {navigation_request}{previous_disabled}>{previous_icon}Previous</button><button class=\"attention-control\" data-nav=\"current\" hx-post=\"/sessions/{code}/attention\" hx-swap=\"none\" {navigation_request}>{attention_icon}Attention</button><button class=\"secondary\" data-nav=\"next\" hx-post=\"/sessions/{code}/next\" hx-swap=\"none\" {navigation_request}{next_disabled}>Next{next_icon}</button></nav>{hand_signal}</main>",
        title = encode_text(&version.title),
        position = index + 1,
        total = document.slides.len(),
        code = session.code,
        deck_slug = deck_slug,
        share_icon = icon("copy"),
        edit_icon = icon("edit"),
        lock_icon_markup = icon(lock_icon),
        end_icon = icon("end"),
        slide_html = slide.html,
        reactions = reaction_buttons(&session.code, index, &data.reactions, false),
        navigation_request = NAVIGATION_REQUEST_ATTRIBUTES,
        first_icon = icon("first"),
        previous_icon = icon("previous"),
        attention_icon = icon("attention"),
        next_icon = icon("next"),
        color_scheme_toggle = color_scheme_toggle(),
    )
}

fn audience_view(
    session: &LiveSession,
    title: &str,
    slide: &Slide,
    index: usize,
    slide_count: usize,
    data: &LiveData,
) -> String {
    let interaction = slide
        .interaction
        .as_ref()
        .map(|spec| audience_interaction(session, index, spec, data))
        .unwrap_or_default();
    let reactions = reaction_buttons(&session.code, index, &data.reactions, true);
    let navigation = audience_navigation(session, index, slide_count);
    let following_presenter = index == session.current_slide as usize;
    let live_status = live_status(data.viewers);
    let questions = audience_questions(&session.code, &data.questions);
    format!(
        "<main id=\"live-view\" class=\"audience-shell\" data-follow-url=\"/join/{code}\" data-following-presenter=\"{following_presenter}\" data-slide-index=\"{index}\"><nav class=\"audience-toolbar\" aria-label=\"Presentation status\"><div class=\"audience-status\"><a class=\"brand\" href=\"/\">Slides</a><strong class=\"nav-title\">{title}</strong><span class=\"nav-position\">{position}/{slide_count}</span></div><div class=\"audience-toolbar-actions\">{live_status}{color_scheme_toggle}</div></nav><section class=\"interaction audience-slide\" aria-label=\"Slide {position} of {slide_count}\"><div class=\"slide-content audience-slide-content\">{slide_html}</div>{interaction}</section>{questions}<div class=\"audience-actions\">{hand_button}{reactions}</div>{navigation}</main>",
        code = session.code,
        title = encode_text(title),
        position = index + 1,
        slide_html = slide.html,
        hand_button = audience_hand_button(&session.code, data.hand_raised),
        color_scheme_toggle = color_scheme_toggle(),
    )
}

fn live_status(viewers: u64) -> String {
    let noun = if viewers == 1 { "viewer" } else { "viewers" };
    let label = format!("Live · {viewers} {noun}");
    format!(
        "<span class=\"status-pill live\" data-live-status data-live-label=\"{label}\">{label}</span>"
    )
}

fn presenter_notes(notes: Option<&str>) -> String {
    let Some(notes) = notes.filter(|notes| !notes.trim().is_empty()) else {
        return String::new();
    };
    format!(
        "<details class=\"presenter-notes\" data-presenter-notes><summary>Presenter notes</summary><div class=\"presenter-notes-content\">{notes}</div></details>"
    )
}

fn audience_questions(code: &str, questions: &[store::QuestionRow]) -> String {
    let items = question_items(code, questions, false);
    let count = questions.len();
    format!(
        "<details class=\"question-panel audience-questions\" data-question-panel data-question-context=\"audience\"><summary><span class=\"question-summary-title\"><span class=\"question-summary-icon\" aria-hidden=\"true\">?</span><span><strong>Questions</strong><small>Ask the presenter</small></span></span><span class=\"question-summary-count\">{count}</span><span class=\"question-summary-chevron\" aria-hidden=\"true\">›</span></summary><div class=\"question-panel-content\"><ol class=\"question-list\" aria-label=\"Audience questions\">{items}</ol><div class=\"question-error\" data-question-error role=\"alert\"></div><form class=\"question-form\" hx-post=\"/sessions/{code}/questions\" hx-swap=\"none\" hx-disable=\"find button\"><label class=\"visually-hidden\" for=\"question-body\">Ask the presenter</label><div class=\"question-composer\"><textarea id=\"question-body\" name=\"body\" rows=\"2\" maxlength=\"280\" required placeholder=\"Ask a question…\" aria-describedby=\"question-help\"></textarea><button class=\"icon-only\" type=\"submit\" aria-label=\"Send question\">{}</button></div><small id=\"question-help\">Anonymous · up to 280 characters</small></form></div></details>",
        icon("next"),
    )
}

fn presenter_questions(code: &str, questions: &[store::QuestionRow]) -> String {
    let unanswered = questions
        .iter()
        .filter(|question| !question.answered)
        .count();
    let count = questions.len();
    let items = question_items(code, questions, true);
    format!(
        "<details class=\"question-panel presenter-questions\" data-question-panel data-question-context=\"presenter\"><summary><span class=\"question-summary-title\"><span class=\"question-summary-icon\" aria-hidden=\"true\">?</span><span><strong>Questions</strong><small>{unanswered} open</small></span></span><span class=\"question-summary-count\">{count}</span><span class=\"question-summary-chevron\" aria-hidden=\"true\">›</span></summary><div class=\"question-panel-content\"><div class=\"question-error\" data-question-error role=\"alert\"></div><ol class=\"question-list\" aria-label=\"Audience questions\">{items}</ol></div></details>"
    )
}

fn question_items(code: &str, questions: &[store::QuestionRow], presenter: bool) -> String {
    if questions.is_empty() {
        let message = if presenter {
            "Audience questions will appear here."
        } else {
            "No questions yet. Start the conversation."
        };
        return format!("<li class=\"question-empty\">{message}</li>");
    }
    questions
        .iter()
        .map(|question| {
            let answered_class = if question.answered { " answered" } else { "" };
            let answered_label = if question.answered {
                "<span class=\"question-answered\">Answered</span>"
            } else {
                ""
            };
            let actions = if presenter {
                let action = if question.answered { "unanswered" } else { "answered" };
                let label = if question.answered { "Reopen" } else { "Mark answered" };
                format!(
                    "<div class=\"question-moderation\"><form hx-post=\"/sessions/{code}/questions/{}/moderate\" hx-swap=\"none\" hx-disable=\"find button\"><input type=\"hidden\" name=\"action\" value=\"{action}\"><button class=\"secondary small\" type=\"submit\">{label}</button></form><form hx-post=\"/sessions/{code}/questions/{}/moderate\" hx-swap=\"none\" hx-disable=\"find button\"><input type=\"hidden\" name=\"action\" value=\"dismiss\"><button class=\"ghost small\" type=\"submit\">Dismiss</button></form></div>",
                    question.id, question.id,
                )
            } else {
                let own = if question.participant_upvoted { " upvoted" } else { "" };
                format!(
                    "<form hx-post=\"/sessions/{code}/questions/{}/vote\" hx-swap=\"none\" hx-disable=\"find button\"><button class=\"question-vote{own}\" type=\"submit\" aria-pressed=\"{}\" aria-label=\"Upvote question; {} votes\"><span aria-hidden=\"true\">▲</span>{}</button></form>",
                    question.id, question.participant_upvoted, question.vote_count, question.vote_count,
                )
            };
            format!(
                "<li class=\"question-item{answered_class}\"><div class=\"question-message\"><span class=\"question-avatar\" aria-hidden=\"true\">?</span><div class=\"question-copy\"><p>{}</p>{answered_label}</div></div>{actions}</li>",
                encode_text(&question.body),
            )
        })
        .collect()
}

fn audience_navigation(session: &LiveSession, index: usize, slide_count: usize) -> String {
    let current = session.current_slide as usize;
    let last_available = if session.locked {
        current
    } else {
        slide_count.saturating_sub(1)
    };
    let previous = if index > 0 {
        format!(
            "<a class=\"button secondary\" data-nav=\"previous\" href=\"/join/{}?slide={}\">{}Previous</a>",
            session.code,
            index - 1,
            icon("previous"),
        )
    } else {
        format!(
            "<span class=\"button secondary nav-placeholder\" aria-disabled=\"true\">{}Previous</span>",
            icon("previous")
        )
    };
    let current_control = if index != current {
        format!(
            "<a class=\"button\" data-nav=\"current\" href=\"/join/{}\">{}Current</a>",
            session.code,
            icon("attention"),
        )
    } else {
        format!(
            "<span class=\"button nav-placeholder\" aria-disabled=\"true\">{}Current</span>",
            icon("attention")
        )
    };
    let next = if index < last_available {
        format!(
            "<a class=\"button secondary\" data-nav=\"next\" href=\"/join/{}?slide={}\">Next{}</a>",
            session.code,
            index + 1,
            icon("next"),
        )
    } else {
        format!(
            "<span class=\"button secondary nav-placeholder\" aria-disabled=\"true\">Next{}</span>",
            icon("next")
        )
    };
    format!(
        "<nav class=\"presentation-navigation\" aria-label=\"Slide navigation\">{previous}{current_control}{next}</nav>"
    )
}

fn optional_heading(question: Option<&str>) -> String {
    question
        .map(|question| format!("<h2>{}</h2>", encode_text(question)))
        .unwrap_or_default()
}

fn audience_interaction(
    session: &LiveSession,
    slide_index: usize,
    spec: &Interaction,
    data: &LiveData,
) -> String {
    if let Interaction::Ordering { prompt, options } = spec {
        let results = ordering_results(prompt, options, &data.ordering_values);
        if session.results_revealed || !session.interaction_open {
            return results;
        }
        return format!(
            "{}{results}",
            ordering_response(
                &session.code,
                slide_index,
                prompt,
                options,
                data.selected.first().map(String::as_str),
            )
        );
    }
    if session.results_revealed {
        return interaction_results(
            spec,
            &data.counts,
            data.answerers,
            slide_index,
            &data.word_cloud_responses,
            &data.ordering_values,
            true,
        );
    }
    if !session.interaction_open {
        return "<div class=\"notice\">Responses are closed. The presenter may reveal the results shortly.</div>".into();
    }

    match spec {
        Interaction::Poll {
            question,
            options,
            multiple,
            ..
        } => {
            let choices = choice_buttons(
                &session.code,
                slide_index,
                options.iter().map(String::as_str),
                &data.selected,
                !multiple,
            );
            format!(
                "<div class=\"interaction-body\">{}<p>{}</p><div id=\"interaction-error\" role=\"alert\"></div>{}</div>",
                optional_heading(question.as_deref()),
                if *multiple {
                    "Select all that apply."
                } else {
                    "Choose one answer."
                },
                choices,
            )
        }
        Interaction::WordCloud { prompt, max_length } => format!(
            "<div class=\"interaction-body\"><h2>{}</h2><div id=\"interaction-error\" role=\"alert\"></div><form hx-post=\"/sessions/{}/answer\" hx-target=\"#interaction-error\" hx-swap=\"innerHTML\" hx-disable=\"find button\"><input type=\"hidden\" name=\"slide\" value=\"{}\"><label>Your response<input id=\"word-cloud-response\" name=\"value\" maxlength=\"{}\" required value=\"{}\"></label><button type=\"submit\">Send response</button></form></div>",
            encode_text(prompt),
            session.code,
            slide_index,
            max_length,
            data.selected
                .first()
                .map(|value| encode_double_quoted_attribute(value).to_string())
                .unwrap_or_default(),
        ),
        Interaction::Quiz { question, options } => {
            let choices = choice_buttons(
                &session.code,
                slide_index,
                options.iter().map(|option| option.label.as_str()),
                &data.selected,
                true,
            );
            format!(
                "<div class=\"interaction-body\"><h2>{}</h2><p>Choose the correct answer.</p><div id=\"interaction-error\" role=\"alert\"></div>{}</div>",
                encode_text(question),
                choices
            )
        }
        Interaction::Ordering { .. } => unreachable!("ordering handled above"),
    }
}

fn choice_buttons<'a>(
    code: &str,
    slide_index: usize,
    labels: impl IntoIterator<Item = &'a str>,
    selected: &[String],
    exclusive: bool,
) -> String {
    let labels = labels.into_iter().collect::<Vec<_>>();
    if exclusive {
        let choices = labels
            .into_iter()
            .enumerate()
            .map(|(index, label)| {
                let value = index.to_string();
                let checked = if selected.contains(&value) { " checked" } else { "" };
                format!(
                    "<label class=\"choice\"><input type=\"radio\" name=\"value\" value=\"{index}\"{checked}><span>{}</span></label>",
                    encode_text(label),
                )
            })
            .collect::<String>();
        return format!(
            "<form class=\"choice-form\" hx-post=\"/sessions/{code}/answer\" hx-trigger=\"change\" hx-target=\"#interaction-error\" hx-sync=\"this:replace\" hx-disable=\"findAll input\"><input type=\"hidden\" name=\"slide\" value=\"{slide_index}\"><fieldset><legend class=\"visually-hidden\">Choose one answer</legend><div class=\"choices\">{choices}</div></fieldset></form>"
        );
    }

    let choices = labels
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let value = index.to_string();
            let selected_class = if selected.contains(&value) { " selected" } else { "" };
            format!(
                "<form class=\"choice-toggle-form\"><input type=\"hidden\" name=\"slide\" value=\"{slide_index}\"><input type=\"hidden\" name=\"value\" value=\"{index}\"><button type=\"submit\" class=\"choice{selected_class}\" hx-post=\"/sessions/{code}/answer\" hx-include=\"closest form\" hx-target=\"#interaction-error\" hx-disable=\"this\" aria-pressed=\"{}\">{}</button></form>",
                selected.contains(&value),
                encode_text(label),
            )
        })
        .collect::<String>();
    format!("<div class=\"choices\">{choices}</div>")
}

const WORD_CLOUD_COLORS: [&str; 13] = [
    "#f5e0dc", "#f2cdcd", "#f5c2e7", "#cba6f7", "#f38ba8", "#fab387", "#f9e2af", "#a6e3a1",
    "#94e2d5", "#89dceb", "#74c7ec", "#89b4fa", "#b4befe",
];

fn participant_color(participant_hash: &str) -> &'static str {
    let hash = participant_hash
        .bytes()
        .fold(2_166_136_261usize, |hash, byte| {
            (hash ^ usize::from(byte)).wrapping_mul(16_777_619)
        });
    WORD_CLOUD_COLORS[hash % WORD_CLOUD_COLORS.len()]
}

fn interaction_results(
    spec: &Interaction,
    counts: &HashMap<String, i64>,
    answerers: i64,
    slide_index: usize,
    word_cloud_responses: &[store::WordCloudResponse],
    ordering_values: &[String],
    animate_charts: bool,
) -> String {
    match spec {
        Interaction::Poll {
            question,
            options,
            orientation,
            ..
        } => format!(
            "<section class=\"interaction-body\"><div class=\"interaction-heading\">{}<span>{}</span></div>{}</section>",
            optional_heading(question.as_deref()),
            answer_count_label(answerers),
            chart(
                options,
                counts,
                answerers,
                *orientation,
                slide_index,
                animate_charts,
            ),
        ),
        Interaction::WordCloud { prompt, .. } => {
            let max = counts.values().copied().max().unwrap_or(1).max(1);
            let mut responses: Vec<_> = word_cloud_responses.iter().collect();
            responses.sort_by(|left, right| {
                let left_count = *counts.get(&left.value).unwrap_or(&1);
                let right_count = *counts.get(&right.value).unwrap_or(&1);
                right_count
                    .cmp(&left_count)
                    .then_with(|| left.value.cmp(&right.value))
                    .then_with(|| left.participant_hash.cmp(&right.participant_hash))
            });
            let words = responses
                .into_iter()
                .map(|response| {
                    let count = *counts.get(&response.value).unwrap_or(&1);
                    let weight = 1 + ((count * 5) / max);
                    format!(
                        "<span style=\"--weight:{weight};--word-color:{}\">{}</span>",
                        participant_color(&response.participant_hash),
                        encode_text(&response.value)
                    )
                })
                .collect::<String>();
            format!(
                "<section class=\"interaction-body\"><div class=\"interaction-heading\"><h2>{}</h2><span>{}</span></div><div class=\"word-cloud\">{}</div></section>",
                encode_text(prompt),
                answer_count_label(answerers),
                words
            )
        }
        Interaction::Quiz { question, options } => {
            let labels: Vec<String> = options
                .iter()
                .map(|option| {
                    if option.correct {
                        format!("✓ {}", option.label)
                    } else {
                        option.label.clone()
                    }
                })
                .collect();
            format!(
                "<section class=\"interaction-body\"><div class=\"interaction-heading\"><h2>{}</h2><span>{}</span></div>{}</section>",
                encode_text(question),
                answer_count_label(answerers),
                chart(
                    &labels,
                    counts,
                    answerers,
                    ChartOrientation::Horizontal,
                    slide_index,
                    animate_charts,
                )
            )
        }
        Interaction::Ordering { prompt, options } => {
            ordering_results(prompt, options, ordering_values)
        }
    }
}

fn ordering_response(
    code: &str,
    slide_index: usize,
    prompt: &str,
    options: &[String],
    selected: Option<&str>,
) -> String {
    let order = selected
        .and_then(|value| parse_ordering_value(value, options.len()))
        .unwrap_or_else(|| (0..options.len()).collect());
    let value = order
        .iter()
        .map(|index| index.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let cards = order
        .iter()
        .enumerate()
        .map(|(position, option_index)| {
            let previous_disabled = if position == 0 { " disabled" } else { "" };
            let next_disabled = if position + 1 == order.len() {
                " disabled"
            } else {
                ""
            };
            format!(
                "<li class=\"ordering-card\" draggable=\"true\" data-order-index=\"{option_index}\" data-order-label=\"{label_attribute}\"><span class=\"drag-handle\" aria-hidden=\"true\">{drag_icon}</span><span>{label}</span><span class=\"ordering-card-controls\"><button class=\"ghost small icon-only\" type=\"button\" data-order-move=\"up\" aria-label=\"Move {label_attribute} up\"{previous_disabled}>{up_icon}</button><button class=\"ghost small icon-only\" type=\"button\" data-order-move=\"down\" aria-label=\"Move {label_attribute} down\"{next_disabled}>{down_icon}</button></span></li>",
                label = encode_text(&options[*option_index]),
                label_attribute = encode_double_quoted_attribute(&options[*option_index]),
                drag_icon = icon("drag"),
                up_icon = icon("up"),
                down_icon = icon("down"),
            )
        })
        .collect::<String>();
    format!(
        "<section class=\"interaction-body ordering-response\"><h2>{}</h2><p>Drag the cards into your preferred order. Changes are saved automatically.</p><div id=\"interaction-error\" role=\"alert\"></div><form class=\"ordering-form\" hx-post=\"/sessions/{}/answer\" hx-target=\"#interaction-error\" hx-swap=\"innerHTML\" hx-disable=\"find button\"><input type=\"hidden\" name=\"slide\" value=\"{}\"><input type=\"hidden\" name=\"value\" value=\"{}\" data-order-value><p class=\"visually-hidden\" data-order-status role=\"status\"></p><ol class=\"ordering-cards\" data-ordering-list>{}</ol><div class=\"ordering-submit\"><button class=\"secondary\" type=\"submit\">{}Save order</button></div></form></section>",
        encode_text(prompt),
        code,
        slide_index,
        value,
        cards,
        icon("save"),
    )
}

fn parse_ordering_value(value: &str, option_count: usize) -> Option<Vec<usize>> {
    let order = value
        .split(',')
        .map(str::parse::<usize>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if order.len() != option_count {
        return None;
    }
    let mut seen = vec![false; option_count];
    for index in &order {
        if *index >= option_count || seen[*index] {
            return None;
        }
        seen[*index] = true;
    }
    Some(order)
}

fn ordering_results(prompt: &str, options: &[String], values: &[String]) -> String {
    let orders = values
        .iter()
        .filter_map(|value| parse_ordering_value(value, options.len()))
        .collect::<Vec<_>>();
    if orders.is_empty() {
        return format!(
            "<section class=\"interaction-body group-ordering\"><div class=\"interaction-heading\"><h2>{}</h2><span>0 answers</span></div><div class=\"notice\">The group order will appear after the first response.</div></section>",
            encode_text(prompt)
        );
    }

    let mut scores = vec![0usize; options.len()];
    for order in &orders {
        for (position, option_index) in order.iter().enumerate() {
            scores[*option_index] += position;
        }
    }
    let mut ranked = (0..options.len()).collect::<Vec<_>>();
    ranked.sort_by_key(|index| (scores[*index], *index));
    let cards = ranked
        .iter()
        .map(|index| {
            format!(
                "<li class=\"ordering-card static\">{}</li>",
                encode_text(&options[*index])
            )
        })
        .collect::<String>();
    format!(
        "<section class=\"interaction-body group-ordering\"><div class=\"interaction-heading\"><h2>{}</h2><span>{}</span></div><p>Live group order</p><ol class=\"ordering-cards group-order\">{}</ol></section>",
        encode_text(prompt),
        answer_count_label(orders.len() as i64),
        cards,
    )
}

fn answer_count_label(count: i64) -> String {
    format!("{count} {}", if count == 1 { "answer" } else { "answers" })
}

fn chart(
    options: &[String],
    counts: &HashMap<String, i64>,
    answerers: i64,
    orientation: ChartOrientation,
    slide_index: usize,
    animate: bool,
) -> String {
    let denominator = answerers.max(1) as f64;
    match orientation {
        ChartOrientation::Horizontal => {
            let rows = options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    let count = *counts.get(&index.to_string()).unwrap_or(&0);
                    let percentage = (count as f64 / denominator * 100.0).round() as i64;
                    let initial = if animate { 0 } else { percentage };
                    format!("<div class=\"result-row\"><span>{}</span><div class=\"bar-track\"><div class=\"bar-fill live\" style=\"--value:{initial}%\" data-live-bar=\"slide-{}-option-{}\" data-bar-value=\"{}\"></div></div><span>{} · {}%</span></div>", encode_text(option), slide_index, index, percentage, count, percentage)
                })
                .collect::<String>();
            format!("<div class=\"results\">{rows}</div>")
        }
        ChartOrientation::Vertical => {
            let bars = options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    let count = *counts.get(&index.to_string()).unwrap_or(&0);
                    let percentage = (count as f64 / denominator * 100.0).round() as i64;
                    let initial = if animate { 0 } else { percentage };
                    format!("<div class=\"vertical-bar\" role=\"img\" aria-label=\"{}: {} responses, {} percent\" style=\"--value:{initial}%\" data-live-bar=\"slide-{}-option-{}\" data-bar-value=\"{}\" data-label=\"{}\"></div>", encode_double_quoted_attribute(option), count, percentage, slide_index, index, percentage, encode_double_quoted_attribute(option))
                })
                .collect::<String>();
            format!("<div class=\"vertical-chart\">{bars}</div>")
        }
    }
}

fn reaction_buttons(
    code: &str,
    slide_index: usize,
    counts: &HashMap<String, i64>,
    interactive: bool,
) -> String {
    const REACTIONS: &[(&str, &str, &str, &str)] = &[
        ("applause", "👏", "Clap", "Alt+1"),
        ("lightbulb", "💡", "Lightbulb", "Alt+2"),
        ("question", "?", "Question mark", "Alt+3"),
    ];
    let buttons = REACTIONS
        .iter()
        .map(|(kind, symbol, label, shortcut)| {
            let count = counts.get(*kind).copied().unwrap_or(0);
            let key = format!("{slide_index}-{kind}");
            if interactive {
                format!("<form class=\"reaction-form\"><input type=\"hidden\" name=\"slide\" value=\"{}\"><button type=\"submit\" class=\"reaction\" aria-label=\"{}\" aria-keyshortcuts=\"{}\" title=\"{} ({})\" hx-post=\"/sessions/{}/react/{}\" hx-include=\"closest form\" hx-swap=\"none\" hx-disable=\"this\" data-audience-shortcut=\"{}\" data-reaction-key=\"{}\" data-reaction-count=\"{}\" data-reaction-symbol=\"{}\"><span aria-hidden=\"true\">{}</span><span class=\"count\">{}</span></button></form>", slide_index, label, shortcut, label, shortcut, code, kind, kind, key, count, symbol, symbol, count)
            } else {
                format!("<span class=\"reaction static\" aria-label=\"{}\" data-reaction-key=\"{}\" data-reaction-count=\"{}\" data-reaction-symbol=\"{}\"><span aria-hidden=\"true\">{}</span><span class=\"count\">{}</span></span>", label, key, count, symbol, symbol, count)
            }
        })
        .collect::<String>();
    format!("<div class=\"reactions\">{buttons}</div>")
}

fn presenter_hand_signal(code: &str, count: i64) -> String {
    if count == 0 {
        return String::new();
    }
    let label = if count == 1 {
        "1 person has a question".into()
    } else {
        format!("{count} people have questions")
    };
    format!(
        "<form class=\"question-signal\" hx-post=\"/sessions/{code}/hands/reset\" hx-swap=\"none\" hx-disable=\"find button\"><button class=\"danger\" type=\"submit\"><span aria-hidden=\"true\">✋</span><span>{label} · Reset</span></button></form>"
    )
}

fn audience_hand_button(code: &str, raised: bool) -> String {
    let label = if raised { "Lower hand" } else { "Raise hand" };
    format!(
        "<button class=\"secondary hand-button{}\" type=\"button\" aria-pressed=\"{}\" aria-keyshortcuts=\"Alt+H\" title=\"{} (Alt+H)\" hx-post=\"/sessions/{}/hand\" hx-swap=\"none\" hx-disable=\"this\" data-audience-shortcut=\"hand\"><span aria-hidden=\"true\">✋</span>{}</button>",
        if raised { " raised" } else { "" },
        raised,
        label,
        code,
        label,
    )
}

fn color_scheme_toggle() -> &'static str {
    "<button class=\"ghost small color-scheme-toggle\" type=\"button\" data-color-scheme-toggle aria-label=\"Change color mode\"><span data-color-scheme-icon aria-hidden=\"true\">☀</span><span data-color-scheme-label>Light mode</span></button>"
}

fn icon(name: &str) -> &'static str {
    match name {
        "first" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"M6 5v14M18 6l-6 6 6 6\"/></svg>"
        }
        "previous" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"m15 18-6-6 6-6\"/></svg>"
        }
        "next" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"m9 18 6-6-6-6\"/></svg>"
        }
        "up" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"m6 15 6-6 6 6\"/></svg>"
        }
        "down" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"m6 9 6 6 6-6\"/></svg>"
        }
        "attention" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9M10 21h4\"/></svg>"
        }
        "copy" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><rect x=\"8\" y=\"8\" width=\"11\" height=\"11\" rx=\"2\"/><path d=\"M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2\"/></svg>"
        }
        "edit" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"M12 20h9\"/><path d=\"M16.5 3.5a2.1 2.1 0 0 1 3 3L8 18l-4 1 1-4Z\"/></svg>"
        }
        "lock" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><rect x=\"5\" y=\"10\" width=\"14\" height=\"11\" rx=\"2\"/><path d=\"M8 10V7a4 4 0 0 1 8 0v3\"/></svg>"
        }
        "unlock" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><rect x=\"5\" y=\"10\" width=\"14\" height=\"11\" rx=\"2\"/><path d=\"M8 10V7a4 4 0 0 1 7.5-2\"/></svg>"
        }
        "responses" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"M4 6h16M4 12h10M4 18h7\"/></svg>"
        }
        "reveal" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6S2 12 2 12Z\"/><circle cx=\"12\" cy=\"12\" r=\"2\"/></svg>"
        }
        "end" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"m7 7 10 10M17 7 7 17\"/></svg>"
        }
        "drag" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"M8 6h8M8 12h8M8 18h8\"/></svg>"
        }
        "save" => {
            "<svg class=\"button-icon\" aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"M5 4h12l2 2v14H5zM8 4v6h8V4M8 20v-6h8v6\"/></svg>"
        }
        _ => "",
    }
}

fn preview_interaction(spec: &Interaction) -> String {
    match spec {
        Interaction::Poll {
            question, options, ..
        } => format!(
            "{}<div class=\"choices\">{}</div>",
            optional_heading(question.as_deref()),
            options
                .iter()
                .map(|option| format!(
                    "<span class=\"choice static\">{}</span>",
                    encode_text(option)
                ))
                .collect::<String>()
        ),
        Interaction::WordCloud { prompt, .. } => format!(
            "<h2>{}</h2><div class=\"word-cloud\"><span style=\"--weight:5;--word-color:{}\">Rust</span><span style=\"--weight:3;--word-color:{}\">Fast</span><span style=\"--weight:2;--word-color:{}\">Safe</span></div>",
            encode_text(prompt),
            participant_color("preview-rust"),
            participant_color("preview-fast"),
            participant_color("preview-safe"),
        ),
        Interaction::Quiz { question, options } => format!(
            "<h2>{}</h2><div class=\"choices\">{}</div>",
            encode_text(question),
            options
                .iter()
                .map(|option| format!(
                    "<span class=\"choice static\">{}</span>",
                    encode_text(&option.label)
                ))
                .collect::<String>()
        ),
        Interaction::Ordering { prompt, options } => format!(
            "<h2>{}</h2><ol class=\"ordering-cards group-order\">{}</ol>",
            encode_text(prompt),
            options
                .iter()
                .map(|option| format!(
                    "<li class=\"ordering-card static\">{}</li>",
                    encode_text(option)
                ))
                .collect::<String>()
        ),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        markdown::parse_deck,
        models::{
            DEFAULT_CODE_FONT, DEFAULT_HEADLINE_FONT, DEFAULT_TEXT_FONT, DEFAULT_THEME_ACCENT,
            DEFAULT_THEME_BACKGROUND, DEFAULT_THEME_TEXT, DeckVersion, LiveSession, Theme,
        },
    };

    use super::{
        LiveData, archived_questions, archived_slides, audience_interaction, audience_questions,
        audience_view, interaction_results, live_status, ordering_response, ordering_results,
        presenter_questions, presenter_view, preview, printable, question_items,
    };

    fn options() -> Vec<String> {
        ["First", "Second", "Third"].map(str::to_owned).to_vec()
    }

    #[test]
    fn printable_contains_every_slide_without_navigation_or_notes() {
        let document = parse_deck(
            "# First\n\n:::notes\nDo not print this private note.\n:::\n\n---\n\n# Second",
        )
        .unwrap();

        let html = printable(&document);

        assert_eq!(html.matches("class=\"print-slide\"").count(), 2);
        assert!(!html.contains("data-preview-nav"));
        assert!(!html.contains("private note"));
        assert!(html.contains("<h1>First</h1>"));
        assert!(html.contains("<h1>Second</h1>"));
    }

    #[test]
    fn preview_contains_every_slide_and_navigation_but_not_notes() {
        let document = parse_deck(
            "# First\n\n:::notes\nDo not preview this private note.\n:::\n\n---\n\n# Second",
        )
        .unwrap();
        let theme = Theme::default();

        let html = preview(&document, &theme);

        assert_eq!(html.matches("data-preview-slide").count(), 2);
        assert!(html.contains("data-preview-nav=\"previous\""));
        assert!(html.contains("data-preview-nav=\"next\""));
        assert!(html.contains("Slide 1 of 2"));
        assert!(!html.contains("private note"));
    }

    #[tokio::test]
    async fn archive_excludes_presenter_notes() {
        let pool = crate::store::connect("sqlite::memory:").await.unwrap();
        let document =
            parse_deck("# Public slide\n\n:::notes\nDo not archive this private note.\n:::")
                .unwrap();
        let session = LiveSession {
            id: 1,
            deck_version_id: 1,
            code: "553675".into(),
            current_slide: 0,
            locked: true,
            interaction_open: false,
            results_revealed: false,
            follow_revision: 0,
            ended_at: Some(1),
        };

        let html = archived_slides(&pool, &session, &document).await.unwrap();

        assert!(html.contains("Public slide"));
        assert!(!html.contains("private note"));
    }

    #[test]
    fn questionless_polls_omit_interaction_headings() {
        let document = parse_deck("# Coffee or beer?\n\n:::poll\n- Coffee\n- Beer\n:::").unwrap();
        let theme = Theme::default();
        let session = LiveSession {
            id: 1,
            deck_version_id: 1,
            code: "553675".into(),
            current_slide: 0,
            locked: true,
            interaction_open: true,
            results_revealed: false,
            follow_revision: 0,
            ended_at: None,
        };
        let interaction = document.slides[0].interaction.as_ref().unwrap();
        let data = LiveData::default();

        let preview = preview(&document, &theme);
        let audience = audience_interaction(&session, 0, interaction, &data);
        let results = interaction_results(interaction, &data.counts, 0, 0, &[], &[], true);

        assert!(!preview.contains("<h2>"));
        assert!(!audience.contains("<h2>"));
        assert!(!results.contains("<h2>"));
        assert!(preview.contains("Coffee"));
        assert!(audience.contains("Choose one answer."));
        assert!(audience.contains("type=\"radio\" name=\"value\""));
        assert!(audience.contains("hx-trigger=\"change\""));
        assert!(results.contains("0 answers"));
    }

    #[test]
    fn archived_charts_render_their_final_values_without_javascript() {
        let document = parse_deck(":::poll\n- Alpha\n- Beta\n:::").unwrap();
        let interaction = document.slides[0].interaction.as_ref().unwrap();
        let counts = [("0".to_owned(), 2)].into_iter().collect();

        let html = interaction_results(interaction, &counts, 2, 0, &[], &[], false);

        assert!(html.contains("style=\"--value:100%\""));
        assert!(html.contains("style=\"--value:0%\""));
    }

    #[test]
    fn word_cloud_uses_stable_participant_colors() {
        let document = parse_deck(":::wordcloud prompt=\"What do you enjoy?\"\n:::").unwrap();
        let interaction = document.slides[0].interaction.as_ref().unwrap();
        let counts = [("Hiking".to_owned(), 2), ("Music".to_owned(), 1)]
            .into_iter()
            .collect();
        let responses = vec![
            crate::store::WordCloudResponse {
                value: "Hiking".into(),
                participant_hash: "participant-a".into(),
            },
            crate::store::WordCloudResponse {
                value: "Hiking".into(),
                participant_hash: "participant-b".into(),
            },
            crate::store::WordCloudResponse {
                value: "Music".into(),
                participant_hash: "participant-c".into(),
            },
        ];

        let html = interaction_results(interaction, &counts, 3, 0, &responses, &[], true);
        let participant_color = super::participant_color("participant-a");

        assert_eq!(html.matches("--word-color:").count(), 3);
        assert!(html.contains(&format!("--word-color:{participant_color}")));
        assert_eq!(html.matches("--weight:6").count(), 2);
        assert_eq!(html.matches("Hiking").count(), 2);
        assert!(html.contains("3 answers"));
    }

    #[test]
    fn live_notice_targets_share_a_stack_outside_morphed_content() {
        for template in [
            include_str!("../../templates/presenter.html"),
            include_str!("../../templates/audience.html"),
        ] {
            let (shell, _) = template.split_once("{{ initial_live|safe }}").unwrap();
            let (_, stack) = shell.split_once("<div class=\"live-notices\">").unwrap();
            let (stack, _) = stack.split_once("\n  </div>").unwrap();
            for id in ["live-error", "live-transport-error"] {
                assert_eq!(template.matches(&format!("id=\"{id}\"")).count(), 1);
                assert!(stack.contains(&format!(
                    "<div id=\"{id}\" role=\"alert\" aria-live=\"assertive\"></div>"
                )));
            }
        }
    }

    #[test]
    fn live_toolbars_keep_essential_context_and_actions() {
        let document =
            parse_deck("# First\n\n:::notes\nMention **ownership** here.\n:::\n\n---\n\n# Second")
                .unwrap();
        let version = DeckVersion {
            title: "A useful deck".into(),
            source: String::new(),
            theme_headline_font: DEFAULT_HEADLINE_FONT.into(),
            theme_text_font: DEFAULT_TEXT_FONT.into(),
            theme_code_font: DEFAULT_CODE_FONT.into(),
            theme_background: DEFAULT_THEME_BACKGROUND.into(),
            theme_text: DEFAULT_THEME_TEXT.into(),
            theme_accent: DEFAULT_THEME_ACCENT.into(),
        };
        let session = LiveSession {
            id: 1,
            deck_version_id: 1,
            code: "553675".into(),
            current_slide: 0,
            locked: true,
            interaction_open: true,
            results_revealed: false,
            follow_revision: 0,
            ended_at: None,
        };
        let data = LiveData {
            viewers: 3,
            ..LiveData::default()
        };

        let presenter = presenter_view(
            &session,
            &version,
            "a-useful-deck",
            &document,
            &document.slides[0],
            0,
            &data,
        );
        assert!(!presenter.contains("id=\"live-error\""));
        assert!(!presenter.contains("id=\"live-transport-error\""));
        assert!(presenter.contains("class=\"presenter-toolbar\""));
        assert!(presenter.contains("Join code</span><strong>553675"));
        assert!(presenter.contains("Copy link"));
        assert!(presenter.contains("Live · 3 viewers"));
        assert!(presenter.contains("data-color-scheme-toggle"));
        assert!(presenter.contains(
            "href=\"/admin/decks/a-useful-deck/edit\" target=\"_blank\" rel=\"noopener\""
        ));
        let assert_navigation = |html: &str, disabled: &[&str]| {
            for action in ["first", "previous", "current", "next"] {
                let control = html
                    .split(&format!("data-nav=\"{action}\""))
                    .nth(1)
                    .unwrap()
                    .split('>')
                    .next()
                    .unwrap();
                assert!(control.contains(super::NAVIGATION_REQUEST_ATTRIBUTES));
                assert!(!control.contains("hx-disable"));
                assert_eq!(control.ends_with(" disabled"), disabled.contains(&action));
            }
        };
        assert_navigation(&presenter, &["first", "previous"]);
        // Other mutations retain their existing request-driven disabling.
        assert!(
            presenter
                .contains("hx-post=\"/sessions/553675/lock\" hx-swap=\"none\" hx-disable=\"this\"")
        );
        assert!(presenter.contains("data-presenter-notes"));
        assert!(presenter.contains("Mention <strong>ownership</strong> here."));
        assert!(!presenter.contains("Future slides locked"));

        let presenter_on_second = presenter_view(
            &session,
            &version,
            "a-useful-deck",
            &document,
            &document.slides[1],
            1,
            &data,
        );
        assert!(presenter_on_second.contains("data-nav=\"first\""));
        assert_navigation(&presenter_on_second, &["next"]);

        let audience = audience_view(
            &session,
            &version.title,
            &document.slides[0],
            0,
            document.slides.len(),
            &data,
        );
        assert!(!audience.contains("id=\"live-error\""));
        assert!(!audience.contains("id=\"live-transport-error\""));
        assert!(audience.contains("class=\"audience-toolbar\""));
        assert!(audience.contains("class=\"nav-title\">A useful deck"));
        assert!(audience.contains("class=\"nav-position\">1/2"));
        assert!(!audience.contains("Join code"));
        assert!(audience.contains("Live · 3 viewers"));
        assert!(audience.contains("data-color-scheme-toggle"));
        assert!(audience.contains("aria-keyshortcuts=\"Alt+H\""));
        assert!(audience.contains("aria-keyshortcuts=\"Alt+1\""));
        assert!(!audience.contains("presenter-notes"));
        assert!(!audience.contains("Mention"));
        assert_eq!(
            live_status(1),
            "<span class=\"status-pill live\" data-live-status data-live-label=\"Live · 1 viewer\">Live · 1 viewer</span>"
        );
    }

    #[test]
    fn questions_are_escaped_and_have_view_specific_controls() {
        let questions = vec![crate::store::QuestionRow {
            id: 7,
            body: "Does <script>alert(1)</script> compile?".into(),
            answered: false,
            vote_count: 3,
            participant_upvoted: true,
        }];

        let audience = question_items("553675", &questions, false);
        let presenter = question_items("553675", &questions, true);
        let audience_panel = audience_questions("553675", &questions);
        let presenter_panel = presenter_questions("553675", &questions);
        let archive = archived_questions(&questions);

        assert!(!audience.contains("<script>"));
        assert!(audience.contains("&lt;script&gt;"));
        assert!(audience.contains("aria-pressed=\"true\""));
        assert!(audience.contains("3 votes"));
        assert!(presenter.contains("Mark answered"));
        assert!(presenter.contains("Dismiss"));
        assert!(audience_panel.contains("data-question-context=\"audience\""));
        assert!(audience_panel.contains("class=\"question-composer\""));
        assert!(presenter_panel.contains("data-question-context=\"presenter\""));
        assert!(!presenter_panel.contains("class=\"question-form\""));
        assert!(!archive.contains("<script>"));
        assert!(archive.contains("3 upvotes"));
    }

    #[test]
    fn source_order_can_be_submitted_unchanged() {
        let html = ordering_response("123456", 2, "Rank them", &options(), None);

        assert!(html.contains("name=\"value\" value=\"0,1,2\""));
        assert!(html.contains("Save order"));
    }

    #[test]
    fn group_order_uses_average_position_and_source_order_for_ties() {
        let html = ordering_results(
            "Rank them",
            &options(),
            &["2,0,1".into(), "0,2,1".into(), "invalid".into()],
        );
        let first = html.find("First").unwrap();
        let third = html.find("Third").unwrap();
        let second = html.find("Second").unwrap();

        assert!(first < third && third < second);
        assert!(html.contains("2 answers"));
    }
}
