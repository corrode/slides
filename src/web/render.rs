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

pub fn preview(document: &DeckDocument, theme: &Theme, show_join_code: bool) -> String {
    let Some(slide) = document.slides.first() else {
        return "<div class=\"empty-state\">Nothing to preview yet.</div>".into();
    };
    let mut body = slide.html.clone();
    if let Some(interaction) = &slide.interaction {
        body.push_str(&preview_interaction(interaction));
    }
    let code = if show_join_code {
        "<div class=\"join-code\" style=\"position:absolute;right:2rem;top:2rem;font-size:1rem\">PREVIEW</div>"
    } else {
        ""
    };
    format!(
        "<div style=\"{};width:100%\"><div class=\"slide-stage\"><article class=\"slide active\"><div class=\"slide-content\">{}{}</div></article></div><p style=\"text-align:center;color:var(--muted);margin:.75rem 0 0\">Slide 1 of {}</p></div>",
        encode_double_quoted_attribute(&theme.style()),
        code,
        body,
        document.slides.len(),
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
        return Ok("<main id=\"live-view\" class=\"audience-shell\" hx-swap-oob=\"outerMorph\"><section class=\"interaction\" style=\"text-align:center\"><p class=\"status-pill\">Session ended</p><h1>Thanks for taking part.</h1><p>The presenter has ended this presentation.</p></section></main>".into());
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
    let counts = store::value_counts(pool, session.id, index).await?;
    let answerers = store::answerer_count(pool, session.id, index).await?;
    let reactions = store::reaction_counts(pool, session.id, index).await?;
    let counts: HashMap<String, i64> = counts
        .into_iter()
        .map(|item| (item.value, item.count))
        .collect();
    let reactions: HashMap<String, i64> = reactions
        .into_iter()
        .map(|item| (item.kind, item.count))
        .collect();

    Ok(match view {
        LiveView::Presenter => presenter_view(
            session, version, document, slide, index, &counts, answerers, &reactions,
        ),
        LiveView::Audience => audience_view(
            session,
            slide,
            index,
            document.slides.len(),
            &counts,
            answerers,
            &reactions,
            &selected,
        ),
    })
}

#[allow(clippy::too_many_arguments)]
fn presenter_view(
    session: &LiveSession,
    version: &DeckVersion,
    document: &DeckDocument,
    slide: &Slide,
    index: usize,
    counts: &HashMap<String, i64>,
    answerers: i64,
    reactions: &HashMap<String, i64>,
) -> String {
    let previous_disabled = if index == 0 { " disabled" } else { "" };
    let next_disabled = if index + 1 >= document.slides.len() {
        " disabled"
    } else {
        ""
    };
    let lock_label = if session.locked {
        "Allow free navigation"
    } else {
        "Prevent future slides"
    };
    let status = if session.locked {
        "Future slides locked"
    } else {
        "Free navigation"
    };
    let mut interaction = String::new();
    if let Some(spec) = &slide.interaction {
        interaction.push_str(&interaction_results(spec, counts, answerers));
    }
    let code_on_slide = if version.show_join_code {
        format!(
            "<div class=\"join-code\" style=\"position:absolute;right:2rem;top:2rem;font-size:1rem\">{}</div>",
            session.code
        )
    } else {
        String::new()
    };
    let reaction_summary = reaction_buttons(&session.code, index, reactions, false);
    let interaction_controls = if slide.interaction.is_some() {
        format!(
            "<button class=\"secondary small\" hx-post=\"/sessions/{}/interaction\" hx-vals='{{\"action\":\"{}\"}}' hx-swap=\"none\">{}</button><button class=\"secondary small\" hx-post=\"/sessions/{}/interaction\" hx-vals='{{\"action\":\"reveal\"}}' hx-swap=\"none\">Reveal results</button>",
            session.code,
            if session.interaction_open {
                "close"
            } else {
                "open"
            },
            if session.interaction_open {
                "Close responses"
            } else {
                "Open responses"
            },
            session.code,
        )
    } else {
        String::new()
    };

    format!(
        "<main id=\"live-view\" class=\"presenter-shell\" hx-swap-oob=\"outerMorph\"><div id=\"live-error\"></div><nav class=\"presenter-toolbar\"><div><span class=\"status-pill live\">Live</span><strong>{}</strong><span>{}/{}</span><span class=\"status-pill\">{}</span></div><div><button class=\"secondary small\" hx-post=\"/sessions/{}/previous\" hx-swap=\"none\"{}>Previous</button><button class=\"secondary small\" hx-post=\"/sessions/{}/next\" hx-swap=\"none\"{}>Next</button><button class=\"secondary small\" hx-post=\"/sessions/{}/lock\" hx-swap=\"none\">{}</button>{}<form method=\"post\" action=\"/sessions/{}/end\" style=\"display:inline\" data-confirm=\"End this live session?\"><button class=\"danger small\" type=\"submit\">End</button></form></div></nav><div class=\"slide-stage\"><article class=\"slide active\"><div class=\"slide-content\">{}{}{}<div style=\"margin-top:auto;padding-top:1rem\">{}</div></div></article></div></main>",
        encode_text(&version.title),
        index + 1,
        document.slides.len(),
        status,
        session.code,
        previous_disabled,
        session.code,
        next_disabled,
        session.code,
        lock_label,
        interaction_controls,
        session.code,
        code_on_slide,
        slide.html,
        interaction,
        reaction_summary,
    )
}

#[allow(clippy::too_many_arguments)]
fn audience_view(
    session: &LiveSession,
    slide: &Slide,
    index: usize,
    slide_count: usize,
    counts: &HashMap<String, i64>,
    answerers: i64,
    reactions: &HashMap<String, i64>,
    selected: &[String],
) -> String {
    let mut interaction = String::new();
    if let Some(spec) = &slide.interaction {
        interaction = audience_interaction(session, index, spec, counts, answerers, selected);
    }
    let reactions = reaction_buttons(&session.code, index, reactions, true);
    let navigation = audience_navigation(session, index, slide_count);
    format!(
        "<main id=\"live-view\" class=\"audience-shell\" hx-swap-oob=\"outerMorph\"><div id=\"live-error\"></div><div style=\"display:flex;justify-content:space-between;align-items:center\"><span class=\"status-pill live\">Live · {}</span><span class=\"status-pill\">Slide {}</span></div><section class=\"interaction\"><div class=\"slide-content\" style=\"padding:0;font-size:1rem\">{}</div>{}</section>{}{}</main>",
        session.code,
        index + 1,
        slide.html,
        interaction,
        navigation,
        reactions,
    )
}

fn audience_navigation(session: &LiveSession, index: usize, slide_count: usize) -> String {
    let current = session.current_slide as usize;
    let last_available = if session.locked {
        current
    } else {
        slide_count.saturating_sub(1)
    };
    let previous = (index > 0).then(|| {
        format!(
            "<a class=\"button secondary small\" href=\"/join/{}?slide={}\">Previous</a>",
            session.code,
            index - 1,
        )
    });
    let next = (index < last_available).then(|| {
        format!(
            "<a class=\"button secondary small\" href=\"/join/{}?slide={}\">Next</a>",
            session.code,
            index + 1,
        )
    });
    let follow = (index != current).then(|| {
        format!(
            "<a class=\"button small\" href=\"/join/{}\">Return to current slide</a>",
            session.code,
        )
    });
    let controls = [previous, next, follow]
        .into_iter()
        .flatten()
        .collect::<String>();
    if controls.is_empty() {
        String::new()
    } else {
        format!(
            "<nav style=\"display:flex;justify-content:center;gap:.6rem;flex-wrap:wrap\">{controls}</nav>"
        )
    }
}

fn audience_interaction(
    session: &LiveSession,
    slide_index: usize,
    spec: &Interaction,
    counts: &HashMap<String, i64>,
    answerers: i64,
    selected: &[String],
) -> String {
    if session.results_revealed {
        return interaction_results(spec, counts, answerers);
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
                selected,
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
            selected
                .first()
                .map(|value| encode_double_quoted_attribute(value).to_string())
                .unwrap_or_default(),
        ),
        Interaction::Quiz { question, options } => {
            let choices = choice_buttons(
                &session.code,
                slide_index,
                options.iter().map(|option| option.label.as_str()),
                selected,
            );
            format!(
                "<div class=\"interaction-body\"><h2>{}</h2><p>Choose the correct answer.</p><div id=\"interaction-error\" role=\"alert\"></div><div class=\"choices\">{}</div></div>",
                encode_text(question),
                choices
            )
        }
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
            chart(options, counts, answerers, *orientation),
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
                chart(&labels, counts, answerers, ChartOrientation::Horizontal)
            )
        }
    }
}

fn answer_count_label(count: i64) -> String {
    format!("{count} {}", if count == 1 { "answer" } else { "answers" })
}

fn chart(
    options: &[String],
    counts: &HashMap<String, i64>,
    answerers: i64,
    orientation: ChartOrientation,
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
                    format!("<div class=\"result-row\"><span>{}</span><div class=\"bar-track\"><div class=\"bar-fill live\" style=\"--value:0%\" data-live-bar=\"option-{}\" data-bar-value=\"{}\"></div></div><span>{} · {}%</span></div>", encode_text(option), index, percentage, count, percentage)
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
                    format!("<div class=\"vertical-bar\" role=\"img\" aria-label=\"{}: {} responses, {} percent\" style=\"--value:0%\" data-live-bar=\"option-{}\" data-bar-value=\"{}\" data-label=\"{}\"></div>", encode_double_quoted_attribute(option), count, percentage, index, percentage, encode_double_quoted_attribute(option))
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
    const REACTIONS: &[(&str, &str)] = &[
        ("heart", "♥"),
        ("thumbs-up", "👍"),
        ("applause", "👏"),
        ("laugh", "😄"),
        ("question", "?"),
    ];
    let buttons = REACTIONS
        .iter()
        .map(|(kind, symbol)| {
            let count = counts.get(*kind).copied().unwrap_or(0);
            if interactive {
                format!("<form style=\"display:contents\"><input type=\"hidden\" name=\"slide\" value=\"{}\"><button type=\"submit\" class=\"reaction\" aria-label=\"React with {}\" hx-post=\"/sessions/{}/react/{}\" hx-include=\"closest form\" hx-swap=\"none\">{} <span class=\"count\">{}</span></button></form>", slide_index, kind, code, kind, symbol, count)
            } else {
                format!("<span class=\"reaction static\">{} <span class=\"count\">{}</span></span>", symbol, count)
            }
        })
        .collect::<String>();
    format!("<div class=\"reactions\">{buttons}</div>")
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
    }
}
