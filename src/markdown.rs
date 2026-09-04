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
    pub notes: Option<String>,
    pub iframe_assets: Vec<String>,
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
        .map(|(index, source)| {
            parse_slide(source, index).with_context(|| format!("slide {}", index + 1))
        })
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
            if let Some(slide) = nonempty_slide(&source[start..offset]) {
                slides.push(slide);
            }
            start = offset + line.len();
        }
    }

    if let Some(slide) = nonempty_slide(&source[start..]) {
        slides.push(slide);
    }
    slides
}

fn nonempty_slide(source: &str) -> Option<&str> {
    let source = source.trim_matches(['\r', '\n']);
    (!source.trim().is_empty()).then_some(source)
}

const IFRAME_MARKER_PREFIX: &str = "\u{e000}slides-iframe-";

#[derive(Debug)]
struct IframeSpec {
    src: String,
    asset_path: String,
    title: String,
}

#[derive(Debug)]
struct ExtractedSlide {
    markdown: String,
    iframes: Vec<IframeSpec>,
    interaction: Option<Interaction>,
    notes: Option<String>,
}

fn parse_slide(source: &str, slide_index: usize) -> Result<Slide> {
    let source = resolve_code_references(source)?;
    if source.contains(IFRAME_MARKER_PREFIX) {
        bail!("slide content contains a reserved iframe marker");
    }
    let extracted = extract_directives(&source)?;
    let mut html = render_markdown(&extracted.markdown);
    for (iframe_index, iframe) in extracted.iframes.iter().enumerate() {
        let marker = format!("<p>{}</p>\n", iframe_marker(iframe_index));
        if !html.contains(&marker) {
            bail!("could not place iframe in rendered slide");
        }
        html = html.replacen(
            &marker,
            &render_iframe(iframe, slide_index, iframe_index),
            1,
        );
    }
    let iframe_assets = extracted
        .iframes
        .into_iter()
        .map(|iframe| iframe.asset_path)
        .collect();
    Ok(Slide {
        html,
        interaction: extracted.interaction,
        notes: extracted.notes.map(|notes| render_markdown(&notes)),
        iframe_assets,
    })
}

fn extract_directives(source: &str) -> Result<ExtractedSlide> {
    let lines: Vec<&str> = source.lines().collect();
    let mut output = Vec::new();
    let mut iframes = Vec::new();
    let mut interaction = None;
    let mut notes = None;
    let mut index = 0;
    let mut fence = None;

    while index < lines.len() {
        if inside_fence(lines[index], &mut fence) {
            output.push(lines[index].to_owned());
            index += 1;
            continue;
        }
        let Some(line) = directive_line(lines[index]) else {
            output.push(lines[index].to_owned());
            index += 1;
            continue;
        };
        let interaction_name = interaction_kind(line);
        let directive = directive_name(line);
        let is_notes = directive == Some("notes");
        let is_iframe = directive == Some("iframe");
        if interaction_name.is_none() && !is_notes && !is_iframe {
            output.push(lines[index].to_owned());
            index += 1;
            continue;
        }

        let inline_header = is_iframe
            .then_some(line)
            .and_then(self_closing_directive_header);
        let header = inline_header.unwrap_or(line).to_owned();
        let mut body = Vec::new();
        index += 1;
        if inline_header.is_none() {
            let mut body_fence = None;
            while index < lines.len()
                && (inside_fence(lines[index], &mut body_fence)
                    || directive_line(lines[index]) != Some(":::"))
            {
                body.push(lines[index]);
                index += 1;
            }
            if index == lines.len() {
                let name = if is_notes {
                    "presenter notes"
                } else if is_iframe {
                    "iframe"
                } else {
                    "interactive"
                };
                bail!("{name} block is missing its closing :::");
            }
            index += 1;
        }

        if is_notes {
            if header != ":::notes" {
                bail!("a presenter notes block does not accept arguments");
            }
            if notes.is_some() {
                bail!("only one presenter notes block is allowed per slide");
            }
            notes = Some(body.join("\n").trim().to_owned());
            continue;
        }

        if is_iframe {
            output.push(String::new());
            output.push(iframe_marker(iframes.len()));
            output.push(String::new());
            iframes.push(parse_iframe(&header, &body)?);
            continue;
        }

        if interaction.is_some() {
            bail!("only one interactive block is allowed per slide");
        }
        interaction = Some(match interaction_name {
            Some("poll") => parse_poll(&header, &body)?,
            Some("wordcloud") => parse_word_cloud(&header, &body)?,
            Some("quiz") => parse_quiz(&header, &body)?,
            Some("ordering") => parse_ordering(&header, &body)?,
            Some(_) | None => unreachable!(),
        });
    }

    Ok(ExtractedSlide {
        markdown: output.join("\n"),
        iframes,
        interaction,
        notes,
    })
}

fn iframe_marker(index: usize) -> String {
    format!("{IFRAME_MARKER_PREFIX}{index}\u{e001}")
}

fn directive_line(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let indentation = &line[..line.len() - trimmed.len()];
    (!indentation.contains('\t') && indentation.len() <= 3).then(|| trimmed.trim_end())
}

fn directive_name(line: &str) -> Option<&str> {
    line.strip_prefix(":::")?.split_whitespace().next()
}

fn self_closing_directive_header(line: &str) -> Option<&str> {
    line.strip_suffix(" :::")
}

fn interaction_kind(line: &str) -> Option<&'static str> {
    match directive_name(line)? {
        "poll" => Some("poll"),
        "wordcloud" => Some("wordcloud"),
        "quiz" => Some("quiz"),
        "ordering" => Some("ordering"),
        _ => None,
    }
}

fn parse_iframe(header: &str, body: &[&str]) -> Result<IframeSpec> {
    let arguments = parse_arguments(
        &std::iter::once(header)
            .chain(body.iter().copied())
            .collect::<Vec<_>>()
            .join(" "),
        &["src", "title"],
        &[],
    )?;

    let src = arguments
        .attribute("src")
        .context("an iframe requires a src attribute")?
        .trim();
    let title = arguments
        .attribute("title")
        .context("an iframe requires a title attribute")?
        .trim();
    if title.is_empty() {
        bail!("an iframe title cannot be empty");
    }
    if title.chars().count() > 200 || title.chars().any(char::is_control) {
        bail!("an iframe title must be at most 200 characters and contain no control characters");
    }
    if src.chars().any(char::is_control) || src.contains(['\\', '%', ':']) {
        bail!("an iframe src contains unsupported characters");
    }

    let path_end = src.find(['?', '#']).unwrap_or(src.len());
    let path = &src[..path_end];
    let relative = path
        .strip_prefix("/assets/")
        .context("an iframe src must start with /assets/")?;
    let components = relative.split('/').collect::<Vec<_>>();
    if components.len() < 3
        || components[0] != "embeds"
        || components
            .iter()
            .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        bail!("an iframe src must point inside /assets/embeds/<bundle>/");
    }
    let extension = Path::new(relative)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !extension.eq_ignore_ascii_case("html") && !extension.eq_ignore_ascii_case("htm") {
        bail!("an iframe src must point to an HTML file");
    }

    Ok(IframeSpec {
        src: src.to_owned(),
        asset_path: relative.to_owned(),
        title: title.to_owned(),
    })
}

fn render_iframe(iframe: &IframeSpec, slide_index: usize, iframe_index: usize) -> String {
    format!(
        "<figure class=\"iframe-embed\"><iframe id=\"slide-iframe-{}-{}\" class=\"slide-iframe\" src=\"{}\" title=\"{}\" sandbox=\"allow-scripts\" referrerpolicy=\"no-referrer\"></iframe></figure>",
        slide_index + 1,
        iframe_index + 1,
        html_escape::encode_double_quoted_attribute(&iframe.src),
        html_escape::encode_double_quoted_attribute(&iframe.title),
    )
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
        .context("directive must start with :::")?;
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
            bail!("malformed directive argument near {input:?}");
        }
        let key = &input[..key_end];
        input = &input[key_end..];

        if let Some(value_input) = input.strip_prefix("=\"") {
            if !allowed_attributes.contains(&key) {
                bail!("unsupported directive attribute {key:?}");
            }
            if arguments
                .attributes
                .iter()
                .any(|(existing, _)| existing == key)
            {
                bail!("duplicate directive attribute {key:?}");
            }
            let value_end = value_input.find('"').with_context(|| {
                format!("directive attribute {key:?} is missing a closing quote")
            })?;
            let after = &value_input[value_end + 1..];
            if after
                .chars()
                .next()
                .is_some_and(|character| !character.is_whitespace())
            {
                bail!("directive attribute {key:?} must be followed by whitespace");
            }
            arguments
                .attributes
                .push((key.to_owned(), value_input[..value_end].to_owned()));
            input = after;
        } else if input.starts_with('=') {
            bail!("directive attribute {key:?} must use a quoted value");
        } else {
            if !allowed_flags.contains(&key) {
                bail!("unsupported directive flag {key:?}");
            }
            if arguments.flags.iter().any(|existing| existing == key) {
                bail!("duplicate directive flag {key:?}");
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
                        render_code_block(&language, &code).into_boxed_str(),
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
                dest_url, title, ..
            }) => {
                let destination = safe_destination(dest_url);
                let title = if title.is_empty() {
                    String::new()
                } else {
                    format!(
                        " title=\"{}\"",
                        html_escape::encode_double_quoted_attribute(&title)
                    )
                };
                let external = if is_external_destination(&destination) {
                    " target=\"_blank\" rel=\"noopener noreferrer\""
                } else {
                    ""
                };
                rendered_events.push(Event::Html(CowStr::Boxed(
                    format!(
                        "<a href=\"{}\"{title}{external}>",
                        html_escape::encode_double_quoted_attribute(&destination)
                    )
                    .into_boxed_str(),
                )));
            }
            Event::End(TagEnd::Link) => {
                rendered_events.push(Event::Html(CowStr::Borrowed("</a>")));
            }
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

fn is_external_destination(destination: &str) -> bool {
    let destination = destination.trim().to_ascii_lowercase();
    destination.starts_with("http://") || destination.starts_with("https://")
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

fn render_code_block(language: &str, code: &str) -> String {
    if language.trim().eq_ignore_ascii_case("mermaid") {
        return format!(
            "<figure class=\"mermaid-diagram\" data-mermaid-diagram><pre class=\"mermaid-source\" data-mermaid-source><code>{}</code></pre><div class=\"mermaid-output\" data-mermaid-output hidden></div><p class=\"mermaid-error\" data-mermaid-error role=\"status\" hidden>Could not render this diagram. Check the Mermaid syntax.</p></figure>",
            html_escape::encode_text(code)
        );
    }

    highlight_code(language, code)
}

/// Render a syntax-highlighted HTML code block using the bundled theme.
pub fn highlight_code(language: &str, code: &str) -> String {
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

    let highlighted = highlighted_html_for_string(code, syntaxes, syntax, theme)
        .unwrap_or_else(|_| format!("<pre><code>{}</code></pre>", html_escape::encode_text(code)));
    if matches!(language.trim().to_ascii_lowercase().as_str(), "rust" | "rs") {
        format!("<div class=\"rust-code\" data-rust-code>{highlighted}</div>")
    } else {
        highlighted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_mermaid_blocks_as_safe_client_side_diagrams() {
        let deck = parse_deck(
            "```mermaid\nflowchart LR\n    A[<script>alert('no')</script>] --> B[Safe]\n```",
        )
        .unwrap();
        let html = &deck.slides[0].html;

        assert!(html.contains("data-mermaid-diagram"));
        assert!(html.contains("data-mermaid-source"));
        assert!(html.contains("data-mermaid-output"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn highlights_code_with_catppuccin_mocha() {
        let html = highlight_code(
            "rust",
            "async fn main() { let string = \"mocha\"; let number = 42; }\n",
        );

        assert!(html.contains("#cba6f7"));
        assert!(html.contains("#a6e3a1"));
        assert!(html.contains("#fab387"));
        assert!(html.contains("data-rust-code"));
        assert!(!highlight_code("python", "print('hello')\n").contains("data-rust-code"));
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

        assert_eq!(deck.slides.len(), 16);
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
        assert!(
            deck.slides
                .iter()
                .any(|slide| slide.html.contains("data-mermaid-diagram"))
        );
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
    fn parses_sanitized_presenter_notes_separately_from_slide_content() {
        let deck = parse_deck(
            "# Ownership\n\n:::notes\nRemind everyone about **moves**.\n\n<script>alert('no')</script>\n:::",
        )
        .unwrap();
        let slide = &deck.slides[0];

        assert!(slide.html.contains("<h1>Ownership</h1>"));
        assert!(!slide.html.contains("Remind everyone"));
        let notes = slide.notes.as_deref().unwrap();
        assert!(notes.contains("<strong>moves</strong>"));
        assert!(!notes.contains("<script>"));
    }

    #[test]
    fn parses_sandboxed_local_iframes_in_content_order() {
        let deck = parse_deck(
            "# Before\n\n:::iframe src=\"/assets/embeds/demo/index.html?step=1#example\" title=\"Interactive & accessible\"\n:::\n\nAfter the demo.\n\n:::poll\n- Yes\n- No\n:::"
        )
        .unwrap();
        let slide = &deck.slides[0];

        let heading = slide.html.find("<h1>Before</h1>").unwrap();
        let iframe = slide.html.find("<iframe").unwrap();
        let after = slide.html.find("After the demo.").unwrap();
        assert!(heading < iframe && iframe < after);
        assert!(slide.html.contains("id=\"slide-iframe-1-1\""));
        assert!(
            slide
                .html
                .contains("src=\"/assets/embeds/demo/index.html?step=1#example\"")
        );
        assert!(
            slide
                .html
                .contains("title=\"Interactive &amp; accessible\"")
        );
        assert!(slide.html.contains("sandbox=\"allow-scripts\""));
        assert!(!slide.html.contains("allow-same-origin"));
        assert_eq!(slide.iframe_assets, ["embeds/demo/index.html"]);
        assert!(slide.interaction.is_some());
    }

    #[test]
    fn accepts_wrapped_and_self_closing_iframe_directives() {
        for source in [
            "# Architecture under review\n\n:::iframe src=\"/assets/embeds/kgdb-architecture/index.html\"\ntitle=\"Interactive KGDB application architecture\"\n:::",
            "# Architecture under review\n\n:::iframe src=\"/assets/embeds/kgdb-architecture/index.html\" title=\"Interactive KGDB application architecture\" :::",
        ] {
            let deck = parse_deck(source).unwrap();
            let slide = &deck.slides[0];

            assert!(slide.html.contains("<iframe"));
            assert!(!slide.html.contains(":::iframe"));
            assert_eq!(slide.iframe_assets, ["embeds/kgdb-architecture/index.html"]);
        }
    }

    #[test]
    fn iframe_directives_preserve_whole_slide_markdown_semantics() {
        let deck = parse_deck(
            "[Link defined later][reference]\n\n:::iframe src=\"/assets/embeds/demo/index.html\" title=\"Demo\"\n:::\n\n[reference]: https://example.com",
        )
        .unwrap();

        assert!(deck.slides[0].html.contains("href=\"https://example.com\""));
        assert!(deck.slides[0].html.contains("<iframe"));
    }

    #[test]
    fn rejects_unsafe_or_inaccessible_iframe_directives() {
        for source in [
            ":::iframe title=\"Missing source\"\n:::",
            ":::iframe src=\"/assets/embeds/demo/index.html\"\n:::",
            ":::iframe src=\"/assets/embeds/demo/index.html\" title=\"\"\n:::",
            ":::iframe src=\"https://example.com/demo.html\" title=\"External\"\n:::",
            ":::iframe src=\"//example.com/demo.html\" title=\"External\"\n:::",
            ":::iframe src=\"/assets/demo.html\" title=\"Outside embeds\"\n:::",
            ":::iframe src=\"/assets/embeds/../secret.html\" title=\"Traversal\"\n:::",
            ":::iframe src=\"/assets/embeds/demo/%2e%2e/secret.html\" title=\"Encoded traversal\"\n:::",
            ":::iframe src=\"/assets/embeds/demo:name/index.html\" title=\"Unsafe filename\"\n:::",
            ":::iframe src=\"/assets/embeds/demo/image.png\" title=\"Not HTML\"\n:::",
            ":::iframe src=\"/assets/embeds/demo/index.html\" title=\"Body\"\nnot allowed\n:::",
        ] {
            let error = parse_deck(source).unwrap_err();
            assert!(error.to_string().contains("slide 1"), "{error:#}");
        }
    }

    #[test]
    fn ignores_iframe_markers_inside_code_blocks() {
        for source in [
            "```markdown\n:::iframe src=\"/assets/embeds/demo/index.html\" title=\"Example\"\n:::\n```",
            "    :::iframe src=\"/assets/embeds/demo/index.html\" title=\"Example\"\n    :::",
        ] {
            let deck = parse_deck(source).unwrap();

            assert!(deck.slides[0].iframe_assets.is_empty());
            assert!(deck.slides[0].html.contains(":::iframe"));
            assert!(!deck.slides[0].html.contains("<iframe"));
        }
    }

    #[test]
    fn presenter_notes_can_coexist_with_an_interaction() {
        let deck = parse_deck(
            ":::poll\n- Rust\n- Go\n:::\n\n:::notes\nAsk for a show of hands first.\n:::",
        )
        .unwrap();

        assert!(deck.slides[0].interaction.is_some());
        assert!(deck.slides[0].notes.is_some());
    }

    #[test]
    fn validates_presenter_notes_blocks() {
        for source in [
            ":::notes speaker=\"me\"\nNo arguments.\n:::",
            ":::notes\nFirst\n:::\n:::notes\nSecond\n:::",
            ":::notes\nMissing close",
        ] {
            let error = parse_deck(source).unwrap_err();
            assert!(error.to_string().contains("slide 1"));
        }
    }

    #[test]
    fn ignores_presenter_notes_markers_inside_code_fences() {
        let deck = parse_deck("```markdown\n:::notes\nNot notes\n:::\n```").unwrap();

        assert!(deck.slides[0].notes.is_none());
        assert!(deck.slides[0].html.contains(":::notes"));
    }

    #[test]
    fn strips_raw_html() {
        let deck = parse_deck(
            "<script>alert(1)</script><iframe src=\"/assets/embeds/demo/index.html\"></iframe>",
        )
        .unwrap();
        assert!(!deck.slides[0].html.contains("<script>"));
        assert!(!deck.slides[0].html.contains("<iframe"));
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
    fn secures_links_and_opens_external_destinations_in_a_new_tab() {
        let deck = parse_deck(
            "[unsafe](javascript:alert(1)) [external](https://example.com) [local](/join)",
        )
        .unwrap();
        let html = &deck.slides[0].html;

        assert!(!html.contains("javascript:"));
        assert!(html.contains(
            "href=\"https://example.com\" target=\"_blank\" rel=\"noopener noreferrer\""
        ));
        assert!(html.contains("href=\"/join\">local</a>"));
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
