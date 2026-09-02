use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::markdown;

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Serve,
    Validate(PathBuf),
    Help,
}

pub fn command(args: impl IntoIterator<Item = OsString>) -> Result<Command> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Ok(Command::Serve);
    };
    let command = command
        .into_string()
        .map_err(|_| anyhow::anyhow!("command must be valid UTF-8"))?;

    match command.as_str() {
        "validate" => {
            let path = args
                .next()
                .map(PathBuf::from)
                .context("usage: slides validate <FILE>")?;
            if args.next().is_some() {
                bail!("usage: slides validate <FILE>");
            }
            Ok(Command::Validate(path))
        }
        "help" | "--help" | "-h" => {
            if args.next().is_some() {
                bail!("usage: slides [validate <FILE>]");
            }
            Ok(Command::Help)
        }
        _ => bail!("unknown command {command:?}\n\n{}", help()),
    }
}

pub fn validate(path: &Path) -> Result<()> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("could not read {} as UTF-8", path.display()))?;
    let document = markdown::parse_deck(&source)
        .with_context(|| format!("{} is not valid Slides Markdown", path.display()))?;
    let interactions = document
        .slides
        .iter()
        .filter(|slide| slide.interaction.is_some())
        .count();
    let slide_label = if document.slides.len() == 1 {
        "slide"
    } else {
        "slides"
    };
    let interaction_label = if interactions == 1 {
        "interaction"
    } else {
        "interactions"
    };

    println!(
        "Valid Slides Markdown: {} ({} {slide_label}, {interactions} {interaction_label})",
        path.display(),
        document.slides.len(),
    );
    Ok(())
}

pub fn help() -> &'static str {
    "Usage:\n  slides                  Start the web server\n  slides validate <FILE>  Validate a Slides Markdown file\n  slides --help           Show this help"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_the_server() {
        assert_eq!(command([]).unwrap(), Command::Serve);
    }

    #[test]
    fn parses_validation_command() {
        assert_eq!(
            command(["validate".into(), "deck.md".into()]).unwrap(),
            Command::Validate(PathBuf::from("deck.md"))
        );
    }

    #[test]
    fn validation_command_requires_exactly_one_path() {
        assert!(command(["validate".into()]).is_err());
        assert!(command(["validate".into(), "a.md".into(), "b.md".into()]).is_err());
    }

    #[test]
    fn validates_syntax_and_semantics() {
        let directory = tempfile::tempdir().unwrap();
        let valid = directory.path().join("valid.md");
        let invalid = directory.path().join("invalid.md");
        std::fs::write(&valid, "# Hello\n\n:::poll\n- One\n- Two\n:::").unwrap();
        std::fs::write(&invalid, "# Hello\n\n:::poll\n- Only one\n:::").unwrap();

        assert!(validate(&valid).is_ok());
        let error = validate(&invalid).unwrap_err();
        assert!(error.to_string().contains("not valid Slides Markdown"));
        assert!(format!("{error:#}").contains("a poll needs at least two options"));
    }
}
