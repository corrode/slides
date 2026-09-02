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

#[derive(Debug, Default)]
struct LiveData {
    selected: Vec<String>,
    counts: HashMap<String, i64>,
    ordering_values: Vec<String>,
    reactions: HashMap<String, i64>,
    answerers: i64,
    raised_hands: i64,
    hand_raised: bool,
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
                "<article class=\"slide{active}\" data-preview-slide aria-current=\"{current}\"><div class=\"slide-content\">{body}</div></article>"
            )
        })
        .collect::<String>();
    let next_disabled = if document.slides.len() == 1 {
        " disabled"
    } else {
        ""
    };

    format!(
        "<div class=\"editor-preview\" data-preview-deck data-slide-index=\"0\" style=\"{}\"><div class=\"slide-stage\">{slides}</div><nav class=\"preview-navigation\" aria-label=\"Preview slide navigation\"><button class=\"secondary\" type=\"button\" data-preview-nav=\"previous\" disabled>{}Previous</button><span class=\"preview-position\" data-preview-position aria-live=\"polite\">Slide 1 of {total}</span><button class=\"secondary\" type=\"button\" data-preview-nav=\"next\"{next_disabled}>Next{}</button></nav></div>",
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
    view: LiveView,
    participant_hash: Option<&str>,
    requested_slide: Option<usize>,
) -> Result<String> {
    if session.ended_at.is_some() {
        return Ok("<main id=\"live-view\" class=\"audience-shell\"><section class=\"interaction\" style=\"text-align:center\"><p class=\"status-pill\">Session ended</p><h1>Thanks for taking part.</h1><p>The presenter has ended this presentation.</p></section></main>".into());
    }

    let current = (session.current_slide as usize).min(document.slides.len().saturating_sub(1));
    let last = document.slides.len().saturating_sub(1);
    let index = if view == LiveView::Audience {
        requested_slide
            .map(|requested| requested.min(if session.locked { current } else { last }))
            .unwrap_or(current)
    } else {
        current
    };
    let slide = &document.slides[index];
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
    let data = LiveData {
        selected,
        counts,
        ordering_values,
        reactions,
        answerers: store::answerer_count(pool, session.id, index).await?,
        raised_hands: store::raised_hand_count(pool, session.id).await?,
        hand_raised,
    };

    Ok(match view {
        LiveView::Presenter => presenter_view(session, version, document, slide, index, &data),
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

fn presenter_view(
    session: &LiveSession,
    version: &DeckVersion,
    document: &DeckDocument,
    slide: &Slide,
    index: usize,
    data: &LiveData,
) -> String {
    let previous_disabled = if index == 0 { " disabled" } else { "" };
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
                &data.ordering_values,
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
            "<form class=\"inline-form\" hx-post=\"/sessions/{}/interaction\" hx-swap=\"none\"><input type=\"hidden\" name=\"action\" value=\"{}\"><button class=\"secondary small\" type=\"submit\">{}{}</button></form><form class=\"inline-form\" hx-post=\"/sessions/{}/interaction\" hx-swap=\"none\"><input type=\"hidden\" name=\"action\" value=\"reveal\"><button class=\"secondary small\" type=\"submit\">{}Reveal</button></form>",
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

    format!(
        "<main id=\"live-view\" class=\"presenter-shell\" data-slide-index=\"{index}\"><div id=\"live-error\"></div><nav class=\"presenter-toolbar\" aria-label=\"Presentation controls\"><div class=\"presenter-status\"><a class=\"brand\" href=\"/admin\">Slides</a><span class=\"status-pill live\">Live</span><strong class=\"nav-title\">{title}</strong><span class=\"nav-position\">{position}/{total}</span></div><div class=\"presenter-share\"><span class=\"share-code\"><span>Join code</span><strong>{code}</strong></span><button class=\"secondary small\" type=\"button\" data-share-url=\"/join/{code}\">{share_icon}Copy link</button><span id=\"share-status\" class=\"share-status\" role=\"status\"></span></div><div class=\"presenter-actions\"><button class=\"secondary small\" hx-post=\"/sessions/{code}/lock\" hx-swap=\"none\">{lock_icon_markup}{lock_label}</button>{interaction_controls}<form class=\"inline-form\" method=\"post\" action=\"/sessions/{code}/end\" data-confirm=\"End this live session?\"><button class=\"danger small\" type=\"submit\">{end_icon}End</button></form></div></nav><div class=\"slide-stage\"><article class=\"slide active\"><div class=\"slide-content\">{slide_html}{interaction}<div class=\"presenter-reactions\">{reactions}</div></div></article></div><nav class=\"presentation-navigation\" aria-label=\"Slide navigation\"><button class=\"secondary\" data-nav=\"previous\" hx-post=\"/sessions/{code}/previous\" hx-swap=\"none\"{previous_disabled}>{previous_icon}Previous</button><button class=\"attention-control\" data-nav=\"current\" hx-post=\"/sessions/{code}/attention\" hx-swap=\"none\">{attention_icon}Attention</button><button class=\"secondary\" data-nav=\"next\" hx-post=\"/sessions/{code}/next\" hx-swap=\"none\"{next_disabled}>Next{next_icon}</button></nav>{hand_signal}</main>",
        title = encode_text(&version.title),
        position = index + 1,
        total = document.slides.len(),
        code = session.code,
        share_icon = icon("copy"),
        lock_icon_markup = icon(lock_icon),
        end_icon = icon("end"),
        slide_html = slide.html,
        reactions = reaction_buttons(&session.code, index, &data.reactions, false),
        previous_icon = icon("previous"),
        attention_icon = icon("attention"),
        next_icon = icon("next"),
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
    format!(
        "<main id=\"live-view\" class=\"audience-shell\" data-follow-url=\"/join/{code}\" data-following-presenter=\"{following_presenter}\" data-slide-index=\"{index}\"><div id=\"live-error\"></div><nav class=\"audience-toolbar\" aria-label=\"Presentation status\"><div class=\"audience-status\"><a class=\"brand\" href=\"/\">Slides</a><strong class=\"nav-title\">{title}</strong><span class=\"nav-position\">{position}/{slide_count}</span></div><span class=\"status-pill live\">Live</span></nav><section class=\"interaction audience-slide\"><div class=\"slide-content audience-slide-content\">{slide_html}</div>{interaction}</section><div class=\"audience-actions\">{hand_button}{reactions}</div>{navigation}</main>",
        code = session.code,
        title = encode_text(title),
        position = index + 1,
        slide_html = slide.html,
        hand_button = audience_hand_button(&session.code, data.hand_raised),
    )
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
            &data.ordering_values,
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
            );
            format!(
                "<div class=\"interaction-body\"><h2>{}</h2><p>{}</p><div id=\"interaction-error\" role=\"alert\"></div><div class=\"choices\">{}</div></div>",
                encode_text(question),
                if *multiple {
                    "Select all that apply."
                } else {
                    "Choose one answer."
                },
                choices,
            )
        }
        Interaction::WordCloud { prompt, max_length } => format!(
            "<div class=\"interaction-body\"><h2>{}</h2><div id=\"interaction-error\" role=\"alert\"></div><form hx-post=\"/sessions/{}/answer\" hx-target=\"#interaction-error\" hx-swap=\"innerHTML\"><input type=\"hidden\" name=\"slide\" value=\"{}\"><label>Your response<input id=\"word-cloud-response\" name=\"value\" maxlength=\"{}\" required value=\"{}\"></label><button type=\"submit\">Send response</button></form></div>",
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
            );
            format!(
                "<div class=\"interaction-body\"><h2>{}</h2><p>Choose the correct answer.</p><div id=\"interaction-error\" role=\"alert\"></div><div class=\"choices\">{}</div></div>",
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
) -> String {
    labels
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let value = index.to_string();
            let selected_class = if selected.contains(&value) { " selected" } else { "" };
            format!(
                "<form style=\"display:contents\"><input type=\"hidden\" name=\"slide\" value=\"{}\"><input type=\"hidden\" name=\"value\" value=\"{}\"><button type=\"submit\" class=\"choice{}\" hx-post=\"/sessions/{}/answer\" hx-include=\"closest form\" hx-target=\"#interaction-error\" aria-pressed=\"{}\">{}</button></form>",
                slide_index,
                index,
                selected_class,
                code,
                selected.contains(&value),
                encode_text(label),
            )
        })
        .collect()
}

fn interaction_results(
    spec: &Interaction,
    counts: &HashMap<String, i64>,
    answerers: i64,
    slide_index: usize,
    ordering_values: &[String],
) -> String {
    match spec {
        Interaction::Poll {
            question,
            options,
            orientation,
            ..
        } => format!(
            "<section class=\"interaction-body\"><div style=\"display:flex;justify-content:space-between;gap:1rem\"><h2>{}</h2><span>{}</span></div>{}</section>",
            encode_text(question),
            answer_count_label(answerers),
            chart(options, counts, answerers, *orientation, slide_index),
        ),
        Interaction::WordCloud { prompt, .. } => {
            let mut words: Vec<_> = counts.iter().collect();
            words.sort_by_key(|(word, count)| (std::cmp::Reverse(**count), word.as_str()));
            let max = words.first().map(|(_, count)| **count).unwrap_or(1).max(1);
            let words = words
                .into_iter()
                .map(|(word, count)| {
                    let weight = 1 + ((*count * 5) / max);
                    format!(
                        "<span style=\"--weight:{}\">{}</span>",
                        weight,
                        encode_text(word)
                    )
                })
                .collect::<String>();
            format!(
                "<section class=\"interaction-body\"><div style=\"display:flex;justify-content:space-between;gap:1rem\"><h2>{}</h2><span>{}</span></div><div class=\"word-cloud\">{}</div></section>",
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
                "<section class=\"interaction-body\"><div style=\"display:flex;justify-content:space-between;gap:1rem\"><h2>{}</h2><span>{}</span></div>{}</section>",
                encode_text(question),
                answer_count_label(answerers),
                chart(
                    &labels,
                    counts,
                    answerers,
                    ChartOrientation::Horizontal,
                    slide_index,
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
        "<section class=\"interaction-body ordering-response\"><h2>{}</h2><p>Drag the cards into your preferred order. Changes are saved automatically.</p><div id=\"interaction-error\" role=\"alert\"></div><form class=\"ordering-form\" hx-post=\"/sessions/{}/answer\" hx-target=\"#interaction-error\" hx-swap=\"innerHTML\"><input type=\"hidden\" name=\"slide\" value=\"{}\"><input type=\"hidden\" name=\"value\" value=\"{}\" data-order-value><p class=\"visually-hidden\" data-order-status role=\"status\"></p><ol class=\"ordering-cards\" data-ordering-list>{}</ol><div class=\"ordering-submit\"><button class=\"secondary\" type=\"submit\">{}Save order</button></div></form></section>",
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
                    format!("<div class=\"result-row\"><span>{}</span><div class=\"bar-track\"><div class=\"bar-fill live\" style=\"--value:0%\" data-live-bar=\"slide-{}-option-{}\" data-bar-value=\"{}\"></div></div><span>{} · {}%</span></div>", encode_text(option), slide_index, index, percentage, count, percentage)
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
                    format!("<div class=\"vertical-bar\" role=\"img\" aria-label=\"{}: {} responses, {} percent\" style=\"--value:0%\" data-live-bar=\"slide-{}-option-{}\" data-bar-value=\"{}\" data-label=\"{}\"></div>", encode_double_quoted_attribute(option), count, percentage, slide_index, index, percentage, encode_double_quoted_attribute(option))
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
    const REACTIONS: &[(&str, &str, &str)] = &[
        ("applause", "👏", "Clap"),
        ("lightbulb", "💡", "Lightbulb"),
        ("question", "?", "Question mark"),
    ];
    let buttons = REACTIONS
        .iter()
        .map(|(kind, symbol, label)| {
            let count = counts.get(*kind).copied().unwrap_or(0);
            let key = format!("{slide_index}-{kind}");
            if interactive {
                format!("<form style=\"display:contents\"><input type=\"hidden\" name=\"slide\" value=\"{}\"><button type=\"submit\" class=\"reaction\" aria-label=\"{}\" hx-post=\"/sessions/{}/react/{}\" hx-include=\"closest form\" hx-swap=\"none\" data-reaction-key=\"{}\" data-reaction-count=\"{}\" data-reaction-symbol=\"{}\"><span aria-hidden=\"true\">{}</span><span class=\"count\">{}</span></button></form>", slide_index, label, code, kind, key, count, symbol, symbol, count)
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
        "<form class=\"question-signal\" hx-post=\"/sessions/{code}/hands/reset\" hx-swap=\"none\"><button class=\"danger\" type=\"submit\"><span aria-hidden=\"true\">✋</span><span>{label} · Reset</span></button></form>"
    )
}

fn audience_hand_button(code: &str, raised: bool) -> String {
    format!(
        "<button class=\"secondary hand-button{}\" type=\"button\" aria-pressed=\"{}\" hx-post=\"/sessions/{}/hand\" hx-swap=\"none\"><span aria-hidden=\"true\">✋</span>{}</button>",
        if raised { " raised" } else { "" },
        raised,
        code,
        if raised { "Lower hand" } else { "Raise hand" },
    )
}

fn icon(name: &str) -> &'static str {
    match name {
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
            "<h2>{}</h2><div class=\"choices\">{}</div>",
            encode_text(question),
            options
                .iter()
                .map(|option| format!(
                    "<span class=\"choice static\">{}</span>",
                    encode_text(option)
                ))
                .collect::<String>()
        ),
        Interaction::WordCloud { prompt, .. } => format!(
            "<h2>{}</h2><div class=\"word-cloud\"><span style=\"--weight:5\">Rust</span><span style=\"--weight:3\">Fast</span><span style=\"--weight:2\">Safe</span></div>",
            encode_text(prompt)
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
            DEFAULT_THEME_ACCENT, DEFAULT_THEME_BACKGROUND, DEFAULT_THEME_TEXT, DeckVersion,
            LiveSession, Theme,
        },
    };

    use super::{
        LiveData, audience_view, ordering_response, ordering_results, presenter_view, preview,
    };

    fn options() -> Vec<String> {
        ["First", "Second", "Third"].map(str::to_owned).to_vec()
    }

    #[test]
    fn preview_contains_every_slide_and_navigation() {
        let document = parse_deck("# First\n\n---\n\n# Second").unwrap();
        let theme = Theme {
            font: "system".into(),
            background: DEFAULT_THEME_BACKGROUND.into(),
            text: DEFAULT_THEME_TEXT.into(),
            accent: DEFAULT_THEME_ACCENT.into(),
        };

        let html = preview(&document, &theme);

        assert_eq!(html.matches("data-preview-slide").count(), 2);
        assert!(html.contains("data-preview-nav=\"previous\""));
        assert!(html.contains("data-preview-nav=\"next\""));
        assert!(html.contains("Slide 1 of 2"));
    }

    #[test]
    fn live_toolbars_keep_essential_context_and_actions() {
        let document = parse_deck("# First\n\n---\n\n# Second").unwrap();
        let version = DeckVersion {
            id: 1,
            title: "A useful deck".into(),
            source: String::new(),
            theme_font: "system".into(),
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
        let data = LiveData::default();

        let presenter =
            presenter_view(&session, &version, &document, &document.slides[0], 0, &data);
        assert!(presenter.contains("class=\"presenter-toolbar\""));
        assert!(presenter.contains("Join code</span><strong>553675"));
        assert!(presenter.contains("Copy link"));
        assert!(!presenter.contains("Future slides locked"));

        let audience = audience_view(
            &session,
            &version.title,
            &document.slides[0],
            0,
            document.slides.len(),
            &data,
        );
        assert!(audience.contains("class=\"audience-toolbar\""));
        assert!(audience.contains("class=\"nav-title\">A useful deck"));
        assert!(audience.contains("class=\"nav-position\">1/2"));
        assert!(!audience.contains("Join code"));
        assert!(audience.contains("</div><span class=\"status-pill live\">Live</span></nav>"));
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
