use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd, html};
use syntect::{
    highlighting::{Theme, ThemeSet},
    html::highlighted_html_for_string,
    parsing::SyntaxSet,
};

#[derive(Debug, Clone)]
pub struct DeckDocument {
    pub slides: Vec<Slide>,
}

#[derive(Debug, Clone)]
pub struct Slide {
    pub html: String,
    pub interaction: Option<Interaction>,
}

#[derive(Debug, Clone)]
pub enum Interaction {
    Poll {
        question: String,
        options: Vec<String>,
        multiple: bool,
        orientation: ChartOrientation,
    },
    WordCloud {
        prompt: String,
        max_length: usize,
    },
    Quiz {
        question: String,
        options: Vec<QuizOption>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartOrientation {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone)]
pub struct QuizOption {
    pub label: String,
    pub correct: bool,
}

pub fn parse_deck(source: &str) -> Result<DeckDocument> {
    let raw_slides = split_slides(source);
    if raw_slides.is_empty() {
        bail!("A deck needs at least one slide");
    }

    let slides = raw_slides
        .into_iter()
        .enumerate()
        .map(|(index, source)| parse_slide(source).with_context(|| format!("slide {}", index + 1)))
        .collect::<Result<Vec<_>>>()?;

    Ok(DeckDocument { slides })
}

#[derive(Clone, Copy)]
struct Fence {
    marker: u8,
    length: usize,
}

fn inside_fence(line: &str, fence: &mut Option<Fence>) -> bool {
    if let Some(active) = *fence {
        let trimmed = line.trim_start();
        let run = trimmed
            .bytes()
            .take_while(|byte| *byte == active.marker)
            .count();
        if run >= active.length && trimmed[run..].trim().is_empty() {
            *fence = None;
        }
        return true;
    }

    let indentation = line.len() - line.trim_start().len();
    if indentation > 3 {
        return false;
    }
    let trimmed = line.trim_start();
    let Some(marker @ (b'`' | b'~')) = trimmed.bytes().next() else {
        return false;
    };
    let length = trimmed.bytes().take_while(|byte| *byte == marker).count();
    if length >= 3 {
        *fence = Some(Fence { marker, length });
        true
    } else {
        false
    }
}

fn split_slides(source: &str) -> Vec<&str> {
    let mut slides = Vec::new();
    let mut start = 0;
    let mut fence = None;

    for (offset, line) in source.split_inclusive('\n').scan(0, |offset, line| {
        let current = *offset;
        *offset += line.len();
        Some((current, line))
    }) {
        if !inside_fence(line, &mut fence) && line.trim() == "---" {
            let slide = source[start..offset].trim();
            if !slide.is_empty() {
                slides.push(slide);
            }
            start = offset + line.len();
        }
    }

    let final_slide = source[start..].trim();
    if !final_slide.is_empty() {
        slides.push(final_slide);
    }
    slides
}

fn parse_slide(source: &str) -> Result<Slide> {
    let (markdown, interaction) = extract_interaction(source)?;
    Ok(Slide {
        html: render_markdown(&markdown),
        interaction,
    })
}

fn extract_interaction(source: &str) -> Result<(String, Option<Interaction>)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut output = Vec::new();
    let mut interaction = None;
    let mut index = 0;
    let mut fence = None;

    while index < lines.len() {
        if inside_fence(lines[index], &mut fence) {
            output.push(lines[index]);
            index += 1;
            continue;
        }
        let line = lines[index].trim();
        let kind = if line.starts_with(":::poll") {
            Some("poll")
        } else if line.starts_with(":::wordcloud") {
            Some("wordcloud")
        } else if line.starts_with(":::quiz") {
            Some("quiz")
        } else {
            None
        };

        let Some(kind) = kind else {
            output.push(lines[index]);
            index += 1;
            continue;
        };

        if interaction.is_some() {
            bail!("only one interactive block is allowed per slide");
        }

        let header = line.to_owned();
        let mut body = Vec::new();
        let mut body_fence = None;
        index += 1;
        while index < lines.len()
            && (inside_fence(lines[index], &mut body_fence) || lines[index].trim() != ":::")
        {
            body.push(lines[index]);
            index += 1;
        }
        if index == lines.len() {
            bail!("interactive block is missing its closing :::");
        }
        index += 1;

        interaction = Some(match kind {
            "poll" => parse_poll(&header, &body)?,
            "wordcloud" => parse_word_cloud(&header)?,
            "quiz" => parse_quiz(&header, &body)?,
            _ => unreachable!(),
        });
    }

    Ok((output.join("\n"), interaction))
}

fn parse_poll(header: &str, body: &[&str]) -> Result<Interaction> {
    let question = attribute(header, "question").unwrap_or_else(|| "Choose an option".into());
    let options = list_items(body);
    if options.len() < 2 {
        bail!("a poll needs at least two options");
    }
    let orientation = match attribute(header, "orientation").as_deref() {
        Some("vertical") => ChartOrientation::Vertical,
        Some("horizontal") | None => ChartOrientation::Horizontal,
        Some(value) => bail!("unsupported chart orientation {value:?}"),
    };
    Ok(Interaction::Poll {
        question,
        options,
        multiple: flag(header, "multiple"),
        orientation,
    })
}

fn parse_word_cloud(header: &str) -> Result<Interaction> {
    let prompt = attribute(header, "prompt").unwrap_or_else(|| "What comes to mind?".into());
    let max_length = attribute(header, "max")
        .map(|value| value.parse::<usize>())
        .transpose()
        .context("word-cloud max must be a number")?
        .unwrap_or(80)
        .clamp(1, 240);
    Ok(Interaction::WordCloud { prompt, max_length })
}

fn parse_quiz(header: &str, body: &[&str]) -> Result<Interaction> {
    let question =
        attribute(header, "question").unwrap_or_else(|| "Choose the correct answer".into());
    let mut options = Vec::new();
    for line in body {
        let trimmed = line.trim();
        let (correct, label) = if let Some(label) = trimmed.strip_prefix("- [x]") {
            (true, label.trim())
        } else if let Some(label) = trimmed.strip_prefix("- [X]") {
            (true, label.trim())
        } else if let Some(label) = trimmed.strip_prefix("- [ ]") {
            (false, label.trim())
        } else {
            continue;
        };
        if !label.is_empty() {
            options.push(QuizOption {
                label: label.to_owned(),
                correct,
            });
        }
    }
    if options.len() < 2 {
        bail!("a quiz needs at least two checkbox-style options");
    }
    if !options.iter().any(|option| option.correct) {
        bail!("a quiz needs at least one correct option marked with [x]");
    }
    Ok(Interaction::Quiz { question, options })
}

fn list_items(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| line.trim().strip_prefix("- "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn attribute(header: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = header.find(&needle)? + needle.len();
    let rest = &header[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

fn flag(header: &str, name: &str) -> bool {
    header
        .split_whitespace()
        .any(|part| part.trim_end_matches(":::") == name)
}

fn render_markdown(source: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(source, options);
    let mut rendered_events = Vec::new();
    let mut code_block: Option<(String, String)> = None;

    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    CodeBlockKind::Fenced(value) => {
                        value.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                code_block = Some((language, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((language, code)) = code_block.take() {
                    rendered_events.push(Event::Html(CowStr::Boxed(
                        highlight_code(&language, &code).into_boxed_str(),
                    )));
                }
            }
            Event::Text(text) if code_block.is_some() => {
                if let Some((_, code)) = code_block.as_mut() {
                    code.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak if code_block.is_some() => {
                if let Some((_, code)) = code_block.as_mut() {
                    code.push('\n');
                }
            }
            Event::Html(raw) | Event::InlineHtml(raw) => {
                rendered_events.push(Event::Text(raw));
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => rendered_events.push(Event::Start(Tag::Link {
                link_type,
                dest_url: safe_destination(dest_url),
                title,
                id,
            })),
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => rendered_events.push(Event::Start(Tag::Image {
                link_type,
                dest_url: safe_destination(dest_url),
                title,
                id,
            })),
            other if code_block.is_none() => rendered_events.push(other),
            _ => {}
        }
    }

    let mut output = String::new();
    html::push_html(&mut output, rendered_events.into_iter());
    output
}

fn safe_destination<'a>(destination: CowStr<'a>) -> CowStr<'a> {
    let value = destination.trim().to_ascii_lowercase();
    let relative = value.starts_with('/')
        || value.starts_with('#')
        || value.starts_with("./")
        || value.starts_with("../")
        || !value.contains(':');
    if relative
        || value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("mailto:")
    {
        destination
    } else {
        CowStr::Borrowed("#")
    }
}

fn highlight_code(language: &str, code: &str) -> String {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    static THEME: OnceLock<Theme> = OnceLock::new();

    let syntaxes = SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines);
    let theme = THEME.get_or_init(|| {
        ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .expect("built-in syntect theme")
    });
    let syntax = syntaxes
        .find_syntax_by_token(language)
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());

    highlighted_html_for_string(code, syntaxes, syntax, theme)
        .unwrap_or_else(|_| format!("<pre><code>{}</code></pre>", html_escape::encode_text(code)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_slides_and_poll() {
        let deck = parse_deck(
            "# Hello\n\n---\n\n# Vote\n\n:::poll question=\"Language?\" multiple orientation=\"vertical\"\n- Rust\n- Go\n:::",
        )
        .unwrap();
        assert_eq!(deck.slides.len(), 2);
        assert!(matches!(
            deck.slides[1].interaction,
            Some(Interaction::Poll {
                multiple: true,
                orientation: ChartOrientation::Vertical,
                ..
            })
        ));
    }

    #[test]
    fn strips_raw_html() {
        let deck = parse_deck("<script>alert(1)</script>").unwrap();
        assert!(!deck.slides[0].html.contains("<script>"));
    }

    #[test]
    fn ignores_slide_and_interaction_markers_inside_code_fences() {
        let deck =
            parse_deck("```text\n---\n:::poll\n- one\n- two\n:::\n```\n\n---\n\n# Second").unwrap();
        assert_eq!(deck.slides.len(), 2);
        assert!(deck.slides[0].interaction.is_none());
        assert!(deck.slides[0].html.contains(":::poll"));
    }

    #[test]
    fn blocks_unsafe_link_schemes() {
        let deck = parse_deck("[unsafe](javascript:alert(1)) [safe](https://example.com)").unwrap();
        assert!(!deck.slides[0].html.contains("javascript:"));
        assert!(deck.slides[0].html.contains("https://example.com"));
    }
}
