//! Bounded, unpublished ZIP bundle extraction.
//!
//! The caller owns a new, empty, private directory and must remove it on error,
//! publish it only on success, and never reuse a generation. HTML is deliberately
//! left unchanged: the serving middleware supplies its sandbox/CSP policy.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::{
    collections::BTreeMap,
    fmt, fs,
    io::{Cursor, Read, Write},
    path::Path,
};
use zip::{CompressionMethod, ZipArchive};

const UPLOAD_LIMIT: usize = 20 * 1024 * 1024;
const EXTRACT_LIMIT: u64 = 100 * 1024 * 1024;
const SOURCE_LIMIT: u64 = 2 * 1024 * 1024;
const HTML_LIMIT: u64 = 4 * 1024 * 1024;
const ENTRY_LIMIT: usize = 512;

#[derive(Debug)]
pub struct Bundle {
    pub title: String,
    /// Resolved, renderable Markdown. The original remains at `slides.md`.
    pub source: String,
    /// Regular files, excluding directory entries.
    pub files: usize,
    /// Actual uncompressed bytes, including the original slides.md.
    pub bytes: u64,
}

#[derive(Debug)]
pub enum BundleError {
    Invalid(String),
    TooLarge(String),
    Internal(anyhow::Error),
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::TooLarge(message) => f.write_str(message),
            Self::Internal(error) => write!(f, "{error:#}"),
        }
    }
}
impl std::error::Error for BundleError {}
impl From<std::io::Error> for BundleError {
    fn from(error: std::io::Error) -> Self {
        Self::Internal(error.into())
    }
}
type Result<T> = std::result::Result<T, BundleError>;
fn invalid(message: impl Into<String>) -> BundleError {
    BundleError::Invalid(message.into())
}
fn large(message: impl Into<String>) -> BundleError {
    BundleError::TooLarge(message.into())
}

/// Extract into an existing, empty, non-symlink directory owned exclusively by
/// the caller. A failure can leave partial files; never serve that directory.
pub fn extract(bytes: &[u8], destination: &Path, generation: &str) -> Result<Bundle> {
    if bytes.len() > UPLOAD_LIMIT {
        return Err(large("ZIP upload exceeds 20 MiB"));
    }
    if generation.is_empty()
        || !generation
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(invalid(
            "generation must contain only ASCII letters, digits, - or _",
        ));
    }
    let metadata = fs::symlink_metadata(destination)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || fs::read_dir(destination)?.next().transpose()?.is_some()
    {
        return Err(invalid(
            "destination must be a new empty directory, not a symlink",
        ));
    }
    let entry_count = validate_directory(bytes)?;
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|e| invalid(format!("invalid ZIP: {e}")))?;
    if archive.len() != entry_count {
        return Err(invalid("duplicate or inconsistent ZIP entries"));
    }
    let mut paths = BTreeMap::new();
    for index in 0..archive.len() {
        let file = archive
            .by_index_raw(index)
            .map_err(|e| invalid(format!("invalid ZIP entry: {e}")))?;
        let name =
            std::str::from_utf8(file.name_raw()).map_err(|_| invalid("ZIP paths must be UTF-8"))?;
        let directory = file.is_dir();
        let name = name.strip_suffix('/').unwrap_or(name);
        validate_path(name)?;
        let kind = file.unix_mode().unwrap_or(0) & 0o170000;
        if !matches!(kind, 0 | 0o100000 | 0o040000)
            || (kind == 0o040000 && !directory)
            || (kind == 0o100000 && directory)
        {
            return Err(invalid(format!("special ZIP entry: {name}")));
        }
        if file.encrypted() {
            return Err(invalid("encrypted ZIP entries are not allowed"));
        }
        if !matches!(
            file.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(invalid(
                "only Stored and Deflated ZIP entries are supported",
            ));
        }
        if directory && file.size() != 0 {
            return Err(invalid("directory entries cannot contain data"));
        }
        if !directory {
            file_limit(name)?;
        }
        // Case folding also prevents aliases on the default macOS filesystem.
        let key = name.to_ascii_lowercase();
        if paths.insert(key, directory).is_some() {
            return Err(invalid("duplicate ZIP path"));
        }
    }
    for name in paths.keys() {
        for (offset, _) in name.match_indices('/') {
            if paths.get(&name[..offset]) == Some(&false) {
                return Err(invalid("ZIP file/directory collision"));
            }
        }
    }
    if paths.get("slides.md") != Some(&false) {
        return Err(invalid("bundle requires root slides.md"));
    }
    let mut total = 0;
    let mut files = 0;
    let mut has_source = false;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|e| invalid(format!("cannot decode ZIP entry: {e}")))?;
        let name = file.name().to_owned();
        let target = destination.join(&name);
        if file.is_dir() {
            fs::create_dir_all(target)?;
            continue;
        }
        let limit = file_limit(&name)?.min(EXTRACT_LIMIT - total);
        if file.size() > limit {
            return Err(large(format!("{name} exceeds its extraction limit")));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        let mut actual = 0u64;
        let mut buffer = [0u8; 16 * 1024];
        loop {
            // Read one extra byte at the boundary: never trust ZIP size metadata.
            let capacity = buffer.len().min((limit - actual + 1) as usize);
            let count = file
                .read(&mut buffer[..capacity])
                .map_err(|e| invalid(format!("corrupt ZIP entry {name}: {e}")))?;
            if count == 0 {
                break;
            }
            actual += count as u64;
            if actual > limit {
                return Err(large(format!("{name} exceeds its extraction limit")));
            }
            output.write_all(&buffer[..count])?;
        }
        if actual != file.size() {
            return Err(invalid(format!("incorrect ZIP size for {name}")));
        }
        total += actual;
        files += 1;
        has_source |= name == "slides.md";
    }
    if !has_source {
        return Err(invalid("bundle requires exact root filename slides.md"));
    }
    let original = read_utf8(destination, "slides.md")?;
    let title = deck_title(&original)?;
    let source = resolve_code(&original, destination)?;
    let source = rewrite_iframes(&source, destination, generation)?;
    let source = rewrite_markdown(&source, destination, generation)?;
    crate::markdown::parse_deck(&source).map_err(|e| invalid(format!("invalid slides: {e:#}")))?;
    Ok(Bundle {
        title,
        source,
        files,
        bytes: total,
    })
}

// Walk the central directory before ZipArchive allocates/indexes it. In particular,
// ZIP readers may collapse duplicate names, so archive.len() alone is insufficient.
// Multi-disk and ZIP64 directory layouts are not upload formats we accept.
fn validate_directory(bytes: &[u8]) -> Result<usize> {
    fn u16_at(b: &[u8], n: usize) -> usize {
        u16::from_le_bytes([b[n], b[n + 1]]) as usize
    }
    fn u32_at(b: &[u8], n: usize) -> usize {
        u32::from_le_bytes([b[n], b[n + 1], b[n + 2], b[n + 3]]) as usize
    }
    let end = (bytes.len().saturating_sub(65557)..bytes.len().saturating_sub(21))
        .rev()
        .find(|&p| {
            bytes.get(p..p + 4) == Some(b"PK\x05\x06")
                && p + 22 + u16_at(bytes, p + 20) == bytes.len()
        })
        .ok_or_else(|| invalid("missing ZIP end record"))?;
    let count = u16_at(bytes, end + 10);
    if count > ENTRY_LIMIT {
        return Err(large(
            "ZIP contains more than 512 entries (including directories)",
        ));
    }
    if u16_at(bytes, end + 4) != 0 || u16_at(bytes, end + 6) != 0 || u16_at(bytes, end + 8) != count
    {
        return Err(invalid("multi-disk ZIP is unsupported"));
    }
    let mut p = u32_at(bytes, end + 16);
    let length = u32_at(bytes, end + 12);
    if p.checked_add(length) != Some(end) {
        return Err(invalid("unsupported ZIP directory layout"));
    }
    let mut names = std::collections::BTreeSet::new();
    for _ in 0..count {
        if p + 46 > end || bytes.get(p..p + 4) != Some(b"PK\x01\x02") {
            return Err(invalid("invalid ZIP directory"));
        }
        let name_end = p + 46 + u16_at(bytes, p + 28);
        let next = name_end + u16_at(bytes, p + 30) + u16_at(bytes, p + 32);
        if next > end || !names.insert(&bytes[p + 46..name_end]) {
            return Err(invalid("duplicate or truncated ZIP entry"));
        }
        if u16_at(bytes, p + 8) & (1 | 64) != 0 {
            return Err(invalid("encrypted ZIP entry"));
        }
        if !matches!(u16_at(bytes, p + 10), 0 | 8) {
            return Err(invalid("unsupported ZIP compression"));
        }
        if u16_at(bytes, p + 34) != 0 {
            return Err(invalid("multi-disk ZIP entry"));
        }
        let local = u32_at(bytes, p + 42);
        if local.checked_add(30).is_none_or(|end| end > bytes.len())
            || bytes.get(local..local + 4) != Some(b"PK\x03\x04")
        {
            return Err(invalid("invalid ZIP local header"));
        }
        if u16_at(bytes, local + 6) & (1 | 64) != 0 {
            return Err(invalid("encrypted ZIP local entry"));
        }
        let local_name_end = local + 30 + u16_at(bytes, local + 26);
        if bytes.get(local + 30..local_name_end) != Some(&bytes[p + 46..name_end])
            || u16_at(bytes, local + 8) != u16_at(bytes, p + 10)
        {
            return Err(invalid("inconsistent ZIP local header"));
        }
        p = next;
    }
    if p != end {
        return Err(invalid("inconsistent ZIP directory size"));
    }
    Ok(count)
}

fn validate_path(path: &str) -> Result<()> {
    // A deliberately portable URL-safe filename subset avoids percent decoding,
    // Unicode normalization, platform aliases and directive/Markdown injection.
    if path.is_empty()
        || path.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with('.')
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        })
    {
        return Err(invalid(format!("unsafe bundle path: {path:?}")));
    }
    Ok(())
}
fn file_limit(name: &str) -> Result<u64> {
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(
        extension.as_str(),
        "rs" | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "py"
            | "go"
            | "java"
            | "ts"
            | "tsx"
            | "jsx"
            | "sh"
            | "toml"
            | "json"
            | "yaml"
            | "yml"
            | "txt"
            | "css"
            | "js"
            | "mjs"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "svg"
            | "ico"
            | "avif"
            | "woff"
            | "woff2"
            | "ttf"
            | "otf"
            | "md"
            | "html"
            | "htm"
    ) {
        return Err(invalid(format!("unsupported asset extension: {name}")));
    }
    Ok(if name == "slides.md" {
        SOURCE_LIMIT
    } else if matches!(extension.as_str(), "html" | "htm") {
        HTML_LIMIT
    } else {
        EXTRACT_LIMIT
    })
}
fn read_utf8(root: &Path, name: &str) -> Result<String> {
    let bytes = fs::read(root.join(name))?;
    String::from_utf8(bytes).map_err(|_| invalid(format!("{name} must be UTF-8")))
}
fn deck_title(source: &str) -> Result<String> {
    let mut in_title = false;
    let mut title = String::new();
    for event in Parser::new(source) {
        match event {
            Event::Start(Tag::Heading {
                level: HeadingLevel::H1,
                ..
            }) => in_title = true,
            Event::End(TagEnd::Heading(HeadingLevel::H1)) if in_title => {
                let title = title.trim();
                return if title.is_empty() || title.chars().count() > 120 {
                    Err(invalid("first H1 title must contain 1–120 characters"))
                } else {
                    Ok(title.to_owned())
                };
            }
            Event::Text(text) | Event::Code(text) if in_title => title.push_str(&text),
            Event::SoftBreak | Event::HardBreak if in_title => title.push(' '),
            _ => {}
        }
    }
    Err(invalid("slides.md requires a nonempty H1 title"))
}

#[derive(Clone, Copy)]
struct Fence {
    marker: u8,
    length: usize,
}
fn opening(line: &str) -> Option<(Fence, &str)> {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker = trimmed.bytes().next()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let length = trimmed.bytes().take_while(|&b| b == marker).count();
    (length >= 3).then(|| (Fence { marker, length }, trimmed[length..].trim()))
}
fn closing(line: &str, fence: Fence) -> bool {
    let trimmed = line.trim_start();
    let run = trimmed.bytes().take_while(|&b| b == fence.marker).count();
    run >= fence.length && trimmed[run..].trim().is_empty()
}
fn resolve_code(source: &str, root: &Path) -> Result<String> {
    let lines: Vec<_> = source.split_inclusive('\n').collect();
    let mut result = String::new();
    let mut regular = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(fence) = regular {
            if closing(line, fence) {
                regular = None;
            }
        } else if let Some((fence, info)) = opening(line) {
            let tokens: Vec<_> = info.split_whitespace().collect();
            if tokens.get(1).is_some_and(|path| path.starts_with("code/")) {
                if tokens.len() != 2 {
                    return Err(invalid("code fence accepts only language and code/path"));
                }
                let path = tokens[1];
                validate_path(path)?;
                if !root.join(path).is_file() {
                    return Err(invalid(format!("missing code reference: {path}")));
                }
                let code = read_utf8(root, path)?;
                if code.lines().any(|line| closing(line, fence)) {
                    return Err(invalid("code reference contains its closing fence"));
                }
                i += 1;
                while i < lines.len() && !closing(lines[i], fence) {
                    if !lines[i].trim().is_empty() {
                        return Err(invalid("code reference cannot contain inline code"));
                    }
                    i += 1;
                }
                if i == lines.len() {
                    return Err(invalid("code reference missing closing fence"));
                }
                let indentation = line.len() - line.trim_start().len();
                result.push_str(&line[..indentation]);
                result.extend(std::iter::repeat_n(fence.marker as char, fence.length));
                result.push_str(tokens[0]);
                result.push('\n');
                result.push_str(&code);
                if !code.ends_with('\n') {
                    result.push('\n');
                }
                result.push_str(lines[i]);
                if result.len() as u64 > EXTRACT_LIMIT {
                    return Err(large("resolved Markdown exceeds 100 MiB"));
                }
                i += 1;
                continue;
            }
            regular = Some(fence);
        }
        result.push_str(line);
        i += 1;
    }
    Ok(result)
}

fn asset_url(value: &str, root: &Path, generation: &str, embed: bool) -> Result<String> {
    let value = value.trim();
    if !embed
        && (value.starts_with('#')
            || ["https://", "http://", "mailto:"].iter().any(|prefix| {
                value
                    .get(..prefix.len())
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case(prefix))
            }))
    {
        return Ok(value.to_owned());
    }
    let end = value.find(['?', '#']).unwrap_or(value.len());
    let (path, suffix) = value.split_at(end);
    let path = path.strip_prefix("./").unwrap_or(path);
    validate_path(path)?;
    if path == "slides.md" {
        return Err(invalid(
            "slides.md is private presentation source, not a downloadable asset",
        ));
    }
    if suffix
        .chars()
        .any(|c| c.is_control() || matches!(c, '\\' | '%' | ':' | '"' | '<' | '>' | ' '))
    {
        return Err(invalid("unsafe asset URL suffix"));
    }
    if !root.join(path).is_file() {
        return Err(invalid(format!("missing local asset: {path}")));
    }
    Ok(format!("/assets/embeds/{generation}/{path}{suffix}"))
}

fn rewrite_iframes(source: &str, root: &Path, generation: &str) -> Result<String> {
    let lines: Vec<_> = source.split_inclusive('\n').collect();
    let mut result = String::new();
    let mut fence = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(active) = fence {
            if closing(line, active) {
                fence = None;
            }
        } else if let Some((active, _)) = opening(line) {
            fence = Some(active);
        } else {
            let trimmed = line.trim_start();
            if line.len() - trimmed.len() <= 3
                && trimmed
                    .strip_prefix(":::")
                    .and_then(|s| s.split_whitespace().next())
                    == Some("iframe")
            {
                let mut block = line.to_owned();
                if !line.trim_end().ends_with(" :::") {
                    i += 1;
                    while i < lines.len() && lines[i].trim() != ":::" {
                        block.push_str(lines[i]);
                        i += 1;
                    }
                    if i == lines.len() {
                        return Err(invalid("iframe missing closing :::"));
                    }
                    block.push_str(lines[i]);
                }
                // Tokenize quoted arguments so a title containing `src=` is not rewritten.
                let mut pos = block.find("iframe").unwrap_or(3) + 6;
                let mut edits = Vec::new();
                while pos < block.len() {
                    while pos < block.len() && block.as_bytes()[pos].is_ascii_whitespace() {
                        pos += 1;
                    }
                    let start = pos;
                    while pos < block.len()
                        && !matches!(block.as_bytes()[pos], b'=' | b' ' | b'\n' | b'\r' | b'\t')
                    {
                        pos += 1;
                    }
                    if block.get(pos..pos + 2) == Some("=\"") {
                        let key = &block[start..pos];
                        pos += 2;
                        let end = block[pos..]
                            .find('"')
                            .map(|n| pos + n)
                            .ok_or_else(|| invalid("unterminated iframe attribute"))?;
                        if key == "src" {
                            edits.push((
                                pos..end,
                                asset_url(&block[pos..end], root, generation, true)?,
                            ));
                        }
                        pos = end + 1;
                    } else if pos == start {
                        pos += 1;
                    }
                }
                for (range, value) in edits.into_iter().rev() {
                    block.replace_range(range, &value);
                }
                result.push_str(&block);
                i += 1;
                continue;
            }
        }
        result.push_str(line);
        i += 1;
    }
    Ok(result)
}

fn rewrite_markdown(source: &str, root: &Path, generation: &str) -> Result<String> {
    let mut links = Vec::new();
    for (event, range) in crate::markdown::split_slides(source)
        .into_iter()
        .flat_map(|slide| {
            // The splitter returns source slices; retain deck-relative edit offsets.
            let offset = slide.as_ptr() as usize - source.as_ptr() as usize;
            Parser::new_ext(
                slide,
                Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
            )
            .into_offset_iter()
            .map(move |(event, range)| (event, range.start + offset..range.end + offset))
        })
    {
        let (url, title, image) = match event {
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => (dest_url, title, true),
            Event::Start(Tag::Link {
                dest_url, title, ..
            }) => (dest_url, title, false),
            _ => continue,
        };
        let resolved = asset_url(&url, root, generation, image)?;
        let raw = &source[range.clone()];
        // Autolinks have no local destination and need no source rewrite.
        if raw.starts_with('<') {
            continue;
        }
        links.push((range, resolved, title.into_string(), image));
    }
    // Process nested images before their enclosing links. Retain the original
    // parser context so reference-style images still see their slide's definitions.
    let mut edits: BTreeMap<usize, (usize, String)> = BTreeMap::new();
    for (range, resolved, title, image) in links.into_iter().rev() {
        let raw = &source[range.clone()];
        let start = if image { 2 } else { 1 };
        let end = label_end(raw, start)?;
        let mut label = raw[start..end].to_owned();
        let nested: Vec<_> = edits
            .range(range.start..range.end)
            .map(|(&start, _)| start)
            .collect();
        for offset in nested.into_iter().rev() {
            let (nested_end, replacement) = edits.remove(&offset).expect("collected edit exists");
            let base = range.start + start;
            if offset < base || nested_end > range.start + end {
                return Err(invalid("unsupported nested Markdown link syntax"));
            }
            label.replace_range(offset - base..nested_end - base, &replacement);
        }
        let title = if title.is_empty() {
            String::new()
        } else {
            format!(" \"{}\"", title.replace('\\', "\\\\").replace('"', "\\\""))
        };
        let replacement = format!(
            "{}[{label}](<{}>{title})",
            if image { "!" } else { "" },
            resolved.replace('<', "%3C").replace('>', "%3E")
        );
        edits.insert(range.start, (range.end, replacement));
    }
    let mut result = source.to_owned();
    for (start, (end, value)) in edits.into_iter().rev() {
        result.replace_range(start..end, &value);
    }
    if result.len() as u64 > EXTRACT_LIMIT {
        return Err(large("resolved Markdown exceeds 100 MiB"));
    }
    Ok(result)
}

fn label_end(raw: &str, start: usize) -> Result<usize> {
    let bytes = raw.as_bytes();
    let mut depth = 1;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'`' => {
                let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
                let mut next = i + run;
                let mut closing = None;
                while next < bytes.len() {
                    if bytes[next] == b'`' {
                        let length = bytes[next..].iter().take_while(|&&b| b == b'`').count();
                        if length == run {
                            closing = Some(next + length);
                            break;
                        }
                        next += length;
                    } else {
                        next += 1;
                    }
                }
                i = closing.unwrap_or(i + run);
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err(invalid("unsupported Markdown link syntax"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::{ZipWriter, write::SimpleFileOptions};
    #[test]
    fn title_limits_and_private_source_links() {
        assert!(deck_title(&format!("# {}", "a".repeat(120))).is_ok());
        assert!(deck_title(&format!("# {}", "a".repeat(121))).is_err());
        assert!(run(&[("slides.md", b"# Title\n\n[Source](slides.md)")]).is_err());
    }

    fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            if name.ends_with('/') {
                writer
                    .add_directory(*name, SimpleFileOptions::default())
                    .unwrap();
            } else {
                writer
                    .start_file(
                        *name,
                        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
                    )
                    .unwrap();
                writer.write_all(data).unwrap();
            }
        }
        writer.finish().unwrap().into_inner()
    }
    fn run(entries: &[(&str, &[u8])]) -> Result<Bundle> {
        let directory = tempfile::tempdir().unwrap();
        extract(&zip(entries), directory.path(), "generation-1")
    }
    #[test]
    fn resolves_bundle_and_preserves_original() {
        let source = b"# A **deck**\n\n![plot][p]\n\n[p]: img.png\n\n[code](code/main.rs)\n\n```rust code/main.rs\n```\n\n:::iframe\nsrc=\"demo/index.html\"\ntitle=\"Example\"\n:::\n";
        let directory = tempfile::tempdir().unwrap();
        let bundle = extract(
            &zip(&[
                ("slides.md", source),
                ("img.png", b"image"),
                ("code/main.rs", b"fn main() {}"),
                ("demo/index.html", b"<h1>Demo</h1>"),
            ]),
            directory.path(),
            "g1",
        )
        .unwrap();
        assert_eq!(bundle.title, "A deck");
        assert_eq!(bundle.files, 4);
        assert_eq!(
            bundle.bytes,
            (source.len() + b"image".len() + b"fn main() {}".len() + b"<h1>Demo</h1>".len()) as u64
        );
        assert!(bundle.source.contains("/assets/embeds/g1/img.png"));
        assert!(
            bundle
                .source
                .contains("src=\"/assets/embeds/g1/demo/index.html\"")
        );
        assert!(bundle.source.contains("```rust\nfn main() {}\n```"));
        assert_eq!(
            fs::read(directory.path().join("slides.md")).unwrap(),
            source
        );
        assert_eq!(
            fs::read(directory.path().join("demo/index.html")).unwrap(),
            b"<h1>Demo</h1>"
        );
    }
    #[test]
    fn rejects_unsafe_paths_and_collisions() {
        for name in [
            "../bad.rs",
            "/bad.rs",
            "code\\bad.rs",
            "C:/bad.rs",
            "x.zip",
            "a/../b.rs",
            "a//b.rs",
            "a%2fb.rs",
        ] {
            assert!(
                run(&[("slides.md", b"# Deck"), (name, b"")]).is_err(),
                "{name}"
            );
        }
        assert!(run(&[("slides.md", b"# Deck"), ("a.rs", b""), ("a.rs/b.rs", b"")]).is_err());
        assert!(run(&[("slides.md", b"# Deck"), ("A.rs", b""), ("a.rs", b"")]).is_err());
    }
    #[test]
    fn rejects_missing_remote_and_invalid_slides() {
        for source in [
            "# ",
            "not a title",
            "# Deck\n![x](https://example.com/x.png)",
            "# Deck\n![x](//example.com/x.png)",
            "# Deck\n[x](missing.md)",
            "# Deck\n:::iframe src=\"https://example.com/a.html\" title=\"X\" :::",
            "# Deck\n```rust code/missing.rs\n```",
            "# Deck\n:::poll\n:::",
        ] {
            assert!(
                run(&[("slides.md", source.as_bytes())]).is_err(),
                "{source}"
            );
        }
        assert!(run(&[("slides.md", &[0xff])]).is_err());
        assert!(run(&[("other.md", b"# Deck")]).is_err());
    }
    #[test]
    fn allows_remote_navigation_and_ignores_code_literals() {
        let bundle = run(&[("slides.md",b"# Deck\n[site](https://example.com)\n\n```md\n![x](missing.png)\n:::iframe src=\"missing.html\" title=\"X\" :::\n```\n")]).unwrap();
        assert!(bundle.source.contains("![x](missing.png)"));
    }
    #[test]
    fn reference_images_are_scoped_to_slides() {
        for separator in ["\n---\n", "\r\n  ---  \r\n"] {
            let source = format!(
                "# First\n\n![first][pic]\n\n```md\n---\n```\n\n[pic]: first.png\n\n:::notes\n~~~~md\n---\n~~~~\n::: {separator}# Second\n\n[![second][pic]](https://example.com)\n\n[pic]: second.png\n"
            );
            let bundle = run(&[
                ("slides.md", source.as_bytes()),
                ("first.png", b"first"),
                ("second.png", b"second"),
            ])
            .unwrap();
            assert!(bundle.source.contains(separator));
            let deck = crate::markdown::parse_deck(&bundle.source).unwrap();
            assert_eq!(deck.slides.len(), 2);
            assert!(
                deck.slides[0]
                    .html
                    .contains("/assets/embeds/generation-1/first.png")
            );
            assert!(!deck.slides[0].html.contains("second.png"));
            assert!(
                deck.slides[1]
                    .html
                    .contains("/assets/embeds/generation-1/second.png")
            );
            assert!(!deck.slides[1].html.contains("first.png"));
            assert!(deck.slides[0].notes.as_ref().unwrap().contains("---"));
        }
    }

    #[test]
    fn external_navigation_schemes_are_case_insensitive_but_images_stay_local() {
        for destination in [
            "HTTP://example.com/Path",
            "hTtP://example.com/Path",
            "HTTPS://example.com/Path",
            "hTtPs://example.com/Path",
            "MAILTO:Somebody@example.com",
            "mAiLtO:Somebody@example.com",
        ] {
            let source = format!("# Deck\n[site]({destination})");
            let bundle = run(&[("slides.md", source.as_bytes())]).unwrap();
            assert!(bundle.source.contains(&format!("[site](<{destination}>)")));
            let source = format!("# Deck\n![image]({destination})");
            assert!(
                run(&[("slides.md", source.as_bytes())]).is_err(),
                "{destination}"
            );
        }
    }

    #[test]
    fn nested_reference_images_and_code_labels() {
        assert!(
            run(&[(
                "slides.md",
                b"# Deck\n[![x][pic]](https://example.com)\n\n[pic]: https://example.com/x.png"
            )])
            .is_err()
        );
        let bundle = run(&[
            (
                "slides.md",
                b"# Deck\n[![x][pic]](https://example.com)\n\n[`]`](img.png)\n\n[pic]: img.png",
            ),
            ("img.png", b"x"),
        ])
        .unwrap();
        assert_eq!(
            bundle
                .source
                .matches("/assets/embeds/generation-1/img.png")
                .count(),
            2
        );
        assert!(bundle.source.contains("[`]`]"));
    }
    #[test]
    fn rejects_duplicates_corruption_and_nonempty_destination() {
        let mut bytes = zip(&[("slides.md", b"# Deck"), ("otherx.md", b"# Other")]);
        for index in 0..bytes.len() - 8 {
            if &bytes[index..index + 9] == b"otherx.md" {
                bytes[index..index + 9].copy_from_slice(b"slides.md");
            }
        }
        let directory = tempfile::tempdir().unwrap();
        assert!(extract(&bytes, directory.path(), "g").is_err());
        let mut bytes = zip(&[("slides.md", b"# Deck")]);
        let content = bytes.windows(6).position(|b| b == b"# Deck").unwrap();
        bytes[content] = b'!';
        assert!(extract(&bytes, directory.path(), "g").is_err());
        fs::write(directory.path().join("sentinel"), b"keep").unwrap();
        assert!(extract(&zip(&[("slides.md", b"# Deck")]), directory.path(), "g").is_err());
        assert_eq!(
            fs::read(directory.path().join("sentinel")).unwrap(),
            b"keep"
        );
    }
    #[test]
    fn deflated_actual_byte_limit() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        writer.start_file("slides.md", options).unwrap();
        writer.write_all(b"# Deck").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(extract(&bytes, directory.path(), "g").unwrap().bytes, 6);
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.start_file("slides.md", options).unwrap();
        writer.write_all(b"# Deck").unwrap();
        writer.start_file("big.txt", options).unwrap();
        let chunk = vec![b'x'; 1024 * 1024];
        for _ in 0..100 {
            writer.write_all(&chunk).unwrap();
        }
        let mut bytes = writer.finish().unwrap().into_inner();
        // Lie about the uncompressed size so the streaming bound, rather than
        // the metadata precheck, must stop decompression.
        let central = bytes
            .windows(4)
            .enumerate()
            .rfind(|(_, b)| *b == b"PK\x01\x02")
            .unwrap()
            .0;
        bytes[central + 24..central + 28].copy_from_slice(&1u32.to_le_bytes());
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            extract(&bytes, directory.path(), "g"),
            Err(BundleError::TooLarge(_))
        ));
        assert!(
            fs::metadata(directory.path().join("big.txt"))
                .unwrap()
                .len()
                <= EXTRACT_LIMIT - 6
        );
    }
    #[test]
    fn enforces_limits() {
        assert!(matches!(
            run(&[("slides.md", &vec![b'a'; SOURCE_LIMIT as usize + 1])]),
            Err(BundleError::TooLarge(_))
        ));
        assert!(matches!(
            run(&[
                ("slides.md", b"# Deck"),
                ("x.html", &vec![0; HTML_LIMIT as usize + 1])
            ]),
            Err(BundleError::TooLarge(_))
        ));
        let names: Vec<_> = (0..512).map(|i| format!("d{i}/")).collect();
        let mut entries = vec![("slides.md", b"# Deck".as_slice())];
        entries.extend(names.iter().map(|name| (name.as_str(), b"".as_slice())));
        assert!(matches!(run(&entries), Err(BundleError::TooLarge(_))));
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            extract(&vec![0; UPLOAD_LIMIT + 1], directory.path(), "g"),
            Err(BundleError::TooLarge(_))
        ));
    }
    #[test]
    fn rejects_encryption_special_files_and_unsupported_compression() {
        let original = zip(&[("slides.md", b"# Deck")]);
        let central = original
            .windows(4)
            .position(|b| b == b"PK\x01\x02")
            .unwrap();
        let mut encrypted_local = original.clone();
        encrypted_local[6] |= 1;
        let directory = tempfile::tempdir().unwrap();
        assert!(extract(&encrypted_local, directory.path(), "g").is_err());
        for (offset, value) in [(8, 1), (10, 12)] {
            let mut bytes = original.clone();
            bytes[central + offset] = value;
            let directory = tempfile::tempdir().unwrap();
            assert!(extract(&bytes, directory.path(), "g").is_err());
        }
        for kind in [0o120777u32, 0o020600, 0o010600, 0o140600] {
            let mut bytes = original.clone();
            bytes[central + 5] = 3;
            bytes[central + 38..central + 42].copy_from_slice(&(kind << 16).to_le_bytes());
            let directory = tempfile::tempdir().unwrap();
            assert!(extract(&bytes, directory.path(), "g").is_err());
        }
    }
}
