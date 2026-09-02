use std::{
    fs,
    io::Cursor,
    path::{Component, Path},
    sync::OnceLock,
};

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
        question: Option<String>,
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
    Ordering {
        prompt: String,
        options: Vec<String>,
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

fn fence_start(line: &str) -> Option<(Fence, &str)> {
    let indentation = line.len() - line.trim_start().len();
    if indentation > 3 {
        return None;
    }
    let trimmed = line.trim_start();
    let marker @ (b'`' | b'~') = trimmed.bytes().next()? else {
        return None;
    };
    let length = trimmed.bytes().take_while(|byte| *byte == marker).count();
    (length >= 3).then(|| (Fence { marker, length }, trimmed[length..].trim()))
}

fn fence_end(line: &str, fence: Fence) -> bool {
    let trimmed = line.trim_start();
    let run = trimmed
        .bytes()
        .take_while(|byte| *byte == fence.marker)
        .count();
    run >= fence.length && trimmed[run..].trim().is_empty()
}

fn inside_fence(line: &str, fence: &mut Option<Fence>) -> bool {
    if let Some(active) = *fence {
        if fence_end(line, active) {
            *fence = None;
        }
        return true;
    }

    if let Some((opening, _)) = fence_start(line) {
        *fence = Some(opening);
        true
    } else {
        false
    }
}

pub fn resolve_code_references(source: &str) -> Result<String> {
    resolve_code_references_from(source, Path::new("examples"))
}

fn resolve_code_references_from(source: &str, presentation_root: &Path) -> Result<String> {
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut output = String::with_capacity(source.len());
    let mut regular_fence = None;
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if regular_fence.is_some() {
            inside_fence(line, &mut regular_fence);
            output.push_str(line);
            index += 1;
            continue;
        }

        let Some((fence, info)) = fence_start(line) else {
            output.push_str(line);
            index += 1;
            continue;
        };
        let mut tokens = info.split_whitespace();
        let Some(language) = tokens.next() else {
            regular_fence = Some(fence);
            output.push_str(line);
            index += 1;
            continue;
        };
        let Some(reference) = tokens.next().filter(|token| token.starts_with("code/")) else {
            regular_fence = Some(fence);
            output.push_str(line);
            index += 1;
            continue;
        };
        if tokens.next().is_some() {
            bail!("a code reference fence accepts only a language and code path");
        }

        let indentation = line.len() - line.trim_start().len();
        output.push_str(&line[..indentation]);
        output.extend(std::iter::repeat_n(char::from(fence.marker), fence.length));
        output.push_str(language);
        output.push('\n');

        index += 1;
        while index < lines.len() && !fence_end(lines[index], fence) {
            if !lines[index].trim().is_empty() {
                bail!("code reference {reference:?} cannot also contain inline code");
            }
            index += 1;
        }
        if index == lines.len() {
            bail!("code reference {reference:?} is missing its closing fence");
        }

        let code = read_code_reference(presentation_root, reference)?;
        if code.lines().any(|line| fence_end(line, fence)) {
            bail!("code reference {reference:?} contains the closing fence marker");
        }
        output.push_str(&code);
        if !code.is_empty() && !code.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(lines[index]);
        index += 1;
    }

    Ok(output)
}

fn read_code_reference(presentation_root: &Path, reference: &str) -> Result<String> {
    let relative = Path::new(reference);
    let mut components = relative.components();
    let inside_code = matches!(
        components.next(),
        Some(Component::Normal(component)) if component == "code"
    ) && components.all(|component| matches!(component, Component::Normal(_)));
    if !inside_code {
        bail!("code references must stay inside the code/ directory");
    }

    let code_root = fs::canonicalize(presentation_root.join("code"))
        .context("could not resolve the presentation code directory")?;
    let path = fs::canonicalize(presentation_root.join(relative))
        .with_context(|| format!("could not resolve code reference {reference:?}"))?;
    if !path.starts_with(&code_root) {
        bail!("code references must stay inside the code/ directory");
    }
    fs::read_to_string(&path)
        .with_context(|| format!("could not read code reference {reference:?} as UTF-8"))
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
    let source = resolve_code_references(source)?;
    let (markdown, interaction) = extract_interaction(&source)?;
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
        let Some(kind) = interaction_kind(line) else {
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
            "wordcloud" => parse_word_cloud(&header, &body)?,
            "quiz" => parse_quiz(&header, &body)?,
            "ordering" => parse_ordering(&header, &body)?,
            _ => unreachable!(),
        });
    }

    Ok((output.join("\n"), interaction))
}

fn interaction_kind(line: &str) -> Option<&'static str> {
    match line.strip_prefix(":::")?.split_whitespace().next()? {
        "poll" => Some("poll"),
        "wordcloud" => Some("wordcloud"),
        "quiz" => Some("quiz"),
        "ordering" => Some("ordering"),
        _ => None,
    }
}

fn parse_poll(header: &str, body: &[&str]) -> Result<Interaction> {
    let arguments = parse_arguments(header, &["question", "orientation"], &["multiple"])?;
    let question = arguments
        .attribute("question")
        .filter(|question| !question.trim().is_empty())
        .map(str::to_owned);
    let options = list_items(body);
    if options.len() < 2 {
        bail!("a poll needs at least two options");
    }
    let orientation = match arguments.attribute("orientation") {
        Some("vertical") => ChartOrientation::Vertical,
        Some("horizontal") | None => ChartOrientation::Horizontal,
        Some(value) => bail!("unsupported chart orientation {value:?}"),
    };
    Ok(Interaction::Poll {
        question,
        options,
        multiple: arguments.flag("multiple"),
        orientation,
    })
}

fn parse_word_cloud(header: &str, body: &[&str]) -> Result<Interaction> {
    let arguments = parse_arguments(header, &["prompt", "max"], &[])?;
    if body.iter().any(|line| !line.trim().is_empty()) {
        bail!("a word-cloud block cannot contain body content");
    }
    let prompt = arguments
        .attribute("prompt")
        .unwrap_or("What comes to mind?")
        .to_owned();
    let max_length = arguments
        .attribute("max")
        .map(str::parse::<usize>)
        .transpose()
        .context("word-cloud max must be a number")?
        .unwrap_or(80)
        .clamp(1, 240);
    Ok(Interaction::WordCloud { prompt, max_length })
}

fn parse_quiz(header: &str, body: &[&str]) -> Result<Interaction> {
    let arguments = parse_arguments(header, &["question"], &[])?;
    let question = arguments
        .attribute("question")
        .unwrap_or("Choose the correct answer")
        .to_owned();
    let mut options = Vec::new();
    for line in body {
        let (correct, label) = if let Some(label) = line.strip_prefix("- [x]") {
            (true, label.trim())
        } else if let Some(label) = line.strip_prefix("- [X]") {
            (true, label.trim())
        } else if let Some(label) = line.strip_prefix("- [ ]") {
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

fn parse_ordering(header: &str, body: &[&str]) -> Result<Interaction> {
    let arguments = parse_arguments(header, &["prompt"], &[])?;
    let prompt = arguments
        .attribute("prompt")
        .unwrap_or("Put these items in order")
        .to_owned();
    let options = list_items(body);
    if options.len() < 2 {
        bail!("an ordering interaction needs at least two items");
    }
    Ok(Interaction::Ordering { prompt, options })
}

fn list_items(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| line.strip_prefix("- "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Default)]
struct DirectiveArguments {
    attributes: Vec<(String, String)>,
    flags: Vec<String>,
}

impl DirectiveArguments {
    fn attribute(&self, key: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find_map(|(name, value)| (name == key).then_some(value.as_str()))
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }
}

fn parse_arguments(
    header: &str,
    allowed_attributes: &[&str],
    allowed_flags: &[&str],
) -> Result<DirectiveArguments> {
    let directive = header
        .strip_prefix(":::")
        .context("interaction directive must start with :::")?;
    let name_end = directive
        .find(char::is_whitespace)
        .unwrap_or(directive.len());
    let mut input = &directive[name_end..];
    let mut arguments = DirectiveArguments::default();

    while !input.trim_start().is_empty() {
        input = input.trim_start();
        let key_end = input
            .find(|character: char| character.is_whitespace() || character == '=')
            .unwrap_or(input.len());
        if key_end == 0 {
            bail!("malformed interaction argument near {input:?}");
        }
        let key = &input[..key_end];
        input = &input[key_end..];

        if let Some(value_input) = input.strip_prefix("=\"") {
            if !allowed_attributes.contains(&key) {
                bail!("unsupported interaction attribute {key:?}");
            }
            if arguments
                .attributes
                .iter()
                .any(|(existing, _)| existing == key)
            {
                bail!("duplicate interaction attribute {key:?}");
            }
            let value_end = value_input.find('"').with_context(|| {
                format!("interaction attribute {key:?} is missing a closing quote")
            })?;
            let after = &value_input[value_end + 1..];
            if after
                .chars()
                .next()
                .is_some_and(|character| !character.is_whitespace())
            {
                bail!("interaction attribute {key:?} must be followed by whitespace");
            }
            arguments
                .attributes
                .push((key.to_owned(), value_input[..value_end].to_owned()));
            input = after;
        } else if input.starts_with('=') {
            bail!("interaction attribute {key:?} must use a quoted value");
        } else {
            if !allowed_flags.contains(&key) {
                bail!("unsupported interaction flag {key:?}");
            }
            if arguments.flags.iter().any(|existing| existing == key) {
                bail!("duplicate interaction flag {key:?}");
            }
            arguments.flags.push(key.to_owned());
        }
    }

    Ok(arguments)
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
        let mut reader = Cursor::new(include_bytes!("../assets/catppuccin-mocha.tmTheme"));
        ThemeSet::load_from_reader(&mut reader).expect("embedded Catppuccin Mocha theme")
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
    fn highlights_code_with_catppuccin_mocha() {
        let html = highlight_code(
            "rust",
            "async fn main() { let string = \"mocha\"; let number = 42; }\n",
        );

        assert!(html.contains("#cba6f7"));
        assert!(html.contains("#a6e3a1"));
        assert!(html.contains("#fab387"));
    }

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
    fn kitchen_sink_example_covers_every_interaction() {
        let deck = parse_deck(include_str!("../examples/kitchen-sink.md")).unwrap();

        assert_eq!(deck.slides.len(), 15);
        assert_eq!(
            deck.slides
                .iter()
                .filter(|slide| matches!(slide.interaction, Some(Interaction::Poll { .. })))
                .count(),
            3
        );
        assert_eq!(
            deck.slides
                .iter()
                .filter(|slide| matches!(slide.interaction, Some(Interaction::WordCloud { .. })))
                .count(),
            1
        );
        assert_eq!(
            deck.slides
                .iter()
                .filter(|slide| matches!(slide.interaction, Some(Interaction::Quiz { .. })))
                .count(),
            1
        );
        assert_eq!(
            deck.slides
                .iter()
                .filter(|slide| matches!(slide.interaction, Some(Interaction::Ordering { .. })))
                .count(),
            1
        );
        assert!(matches!(
            deck.slides[7].interaction,
            Some(Interaction::Poll {
                orientation: ChartOrientation::Vertical,
                ..
            })
        ));
        assert!(matches!(
            deck.slides[8].interaction,
            Some(Interaction::Poll { multiple: true, .. })
        ));
    }

    #[test]
    fn intro_to_rust_example_parses() {
        let deck = parse_deck(include_str!("../examples/intro-to-rust.md")).unwrap();

        assert!(!deck.slides.is_empty());
        assert!(
            deck.slides
                .iter()
                .any(|slide| matches!(slide.interaction, Some(Interaction::Poll { .. })))
        );
        assert!(
            deck.slides
                .iter()
                .any(|slide| matches!(slide.interaction, Some(Interaction::WordCloud { .. })))
        );
    }

    #[test]
    fn parses_ordering_cards() {
        let deck = parse_deck(
            ":::ordering prompt=\"Put the release steps in order\"\n- Build\n- Test\n- Deploy\n:::",
        )
        .unwrap();

        assert!(matches!(
            &deck.slides[0].interaction,
            Some(Interaction::Ordering { prompt, options })
                if prompt == "Put the release steps in order" && options.len() == 3
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
    fn resolves_code_files_inside_the_presentation_code_directory() {
        let source = "```python code/word-count/python/step_01.py\n```";

        let resolved = resolve_code_references(source).unwrap();
        let deck = parse_deck(source).unwrap();

        assert!(resolved.starts_with("```python\n"));
        assert!(resolved.contains("def count_words(filename):"));
        assert!(!resolved.contains("code/word-count"));
        assert!(deck.slides[0].html.contains("count_words"));
    }

    #[test]
    fn code_references_reject_inline_content_and_path_traversal() {
        let inline = "```python code/word-count/python/step_01.py\nprint('ambiguous')\n```";
        let traversal = "```text code/../intro-to-rust.md\n```";

        assert!(
            resolve_code_references(inline)
                .unwrap_err()
                .to_string()
                .contains("cannot also contain inline code")
        );
        assert!(
            resolve_code_references(traversal)
                .unwrap_err()
                .to_string()
                .contains("must stay inside the code/ directory")
        );
    }

    #[test]
    fn code_reference_errors_identify_the_slide() {
        let source = "# First\n\n---\n\n```rust code/word-count/rust/does-not-exist.rs\n```";

        let error = parse_deck(source).unwrap_err();

        assert!(error.to_string().contains("slide 2"));
    }

    #[test]
    fn blocks_unsafe_link_schemes() {
        let deck = parse_deck("[unsafe](javascript:alert(1)) [safe](https://example.com)").unwrap();
        assert!(!deck.slides[0].html.contains("javascript:"));
        assert!(deck.slides[0].html.contains("https://example.com"));
    }

    #[test]
    fn requires_exact_interaction_names() {
        let deck = parse_deck(":::poll-results\n- Rust\n- Go\n:::").unwrap();
        assert!(deck.slides[0].interaction.is_none());
        assert!(deck.slides[0].html.contains("poll-results"));
    }

    #[test]
    fn rejects_word_cloud_body_content() {
        let error = parse_deck(":::wordcloud\nunexpected\n:::").unwrap_err();
        assert!(error.to_string().contains("slide 1"));
    }

    #[test]
    fn poll_questions_are_optional_and_horizontal_by_default() {
        for source in [
            ":::poll\n- Coffee\n- Beer\n:::",
            ":::poll question=\"\"\n- Coffee\n- Beer\n:::",
        ] {
            let deck = parse_deck(source).unwrap();
            assert!(matches!(
                &deck.slides[0].interaction,
                Some(Interaction::Poll {
                    question: None,
                    orientation: ChartOrientation::Horizontal,
                    ..
                })
            ));
        }
    }

    #[test]
    fn interaction_arguments_follow_the_declared_grammar() {
        for source in [
            ":::poll notquestion=\"Wrong\"\n- Rust\n- Go\n:::",
            ":::poll orientation=vertical\n- Rust\n- Go\n:::",
            ":::wordcloud max=abc\n:::",
        ] {
            let error = parse_deck(source).unwrap_err();
            assert!(error.to_string().contains("slide 1"));
        }

        let deck =
            parse_deck(":::poll question=\"Choose multiple values\"\n- Rust\n- Go\n:::").unwrap();
        assert!(matches!(
            deck.slides[0].interaction,
            Some(Interaction::Poll {
                multiple: false,
                ..
            })
        ));
    }

    #[test]
    fn nested_poll_options_do_not_count() {
        let error = parse_deck(":::poll\n- Rust\n  - nested\n:::").unwrap_err();
        assert!(error.to_string().contains("slide 1"));
    }
}
