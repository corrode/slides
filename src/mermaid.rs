//! Best-effort Mermaid syntax checks at the bundle admission boundary.
//!
//! Merman targets our vendored Mermaid baseline, but is not the browser renderer.
//! Keep rendering fallbacks and preview checks: parser acceptance is not SVG/layout validation.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use merman_core::{Engine, OperationControl, ParseOptions};

use crate::markdown::DeckDocument;

const DIAGRAM_LIMIT: usize = 100;
// JavaScript's maxTextSize counts UTF-16 code units, not UTF-8 bytes.
const TEXT_LIMIT: usize = 50_000;
const PARSE_BUDGET: Duration = Duration::from_secs(10);

pub fn validate(deck: &DeckDocument) -> Result<()> {
    validate_with_control(deck, &OperationControl::new().with_deadline(PARSE_BUDGET))
}

fn validate_with_control(deck: &DeckDocument, control: &OperationControl) -> Result<()> {
    let count: usize = deck.slides.iter().map(|s| s.mermaid_diagrams.len()).sum();
    if count > DIAGRAM_LIMIT {
        bail!("bundle contains more than {DIAGRAM_LIMIT} Mermaid diagrams");
    }
    if count == 0 {
        return Ok(());
    }

    // Check all input sizes before spending time in the parser.
    for (slide, content) in deck.slides.iter().enumerate() {
        for (diagram, source) in content.mermaid_diagrams.iter().enumerate() {
            if source.encode_utf16().count() > TEXT_LIMIT {
                bail!(
                    "slide {}, Mermaid diagram {}: source exceeds {TEXT_LIMIT} UTF-16 code units",
                    slide + 1,
                    diagram + 1
                );
            }
        }
    }

    let engine = Engine::new();
    for (slide, content) in deck.slides.iter().enumerate() {
        for (diagram, source) in content.mermaid_diagrams.iter().enumerate() {
            validate_diagram(&engine, source, control)
                .with_context(|| format!("slide {}, Mermaid diagram {}", slide + 1, diagram + 1))?;
        }
    }
    Ok(())
}

fn validate_diagram(engine: &Engine, source: &str, control: &OperationControl) -> Result<()> {
    // This builds only a semantic model, not SVG. Cooperative deadlines are checked
    // inside the parser; timing out an async join alone would leave its CPU work running.
    let diagram = engine
        .parse_diagram_for_render_model_controlled_sync(source, ParseOptions::strict(), control)
        .context("Mermaid validation stopped; simplify the diagrams and retry")?
        .map_err(|error| {
            // Parser diagnostics may quote user input. Keep the API response bounded.
            anyhow::anyhow!(
                "{}",
                error.to_string().chars().take(1024).collect::<String>()
            )
        })?
        .context("empty Mermaid diagram")?;
    if diagram.metadata().diagram_type == "zenuml" {
        bail!("ZenUML is not included in the app's Mermaid renderer");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parse_deck;

    fn check(source: &str) -> Result<()> {
        validate(&parse_deck(&format!(
            "# Test\n\n```mermaid\n{source}\n```"
        ))?)
    }

    #[test]
    fn rejects_bad_sequence_syntax_not_just_unbalanced_blocks() {
        for source in [
            "sequenceDiagram\nparticipant C\nNote over C: Write fails; skip commit",
            "sequenceDiagram\nparticipant C\nthis is not a statement",
            "sequenceDiagram\nparticipant C\nNote sideways C: hello",
            "sequenceDiagram\nA->>B",
            "sequenceDiagram\nA->>: hello",
            "sequenceDiagram\nalt retry\nA->>B: hi",
        ] {
            let error = format!("{:#}", check(source).unwrap_err());
            assert!(error.contains("slide 1, Mermaid diagram 1"), "{error}");
        }
    }

    #[test]
    fn accepts_fixed_notes_and_valid_semicolons() {
        for source in [
            "sequenceDiagram\nparticipant C\nNote over C: Write fails, no commit",
            "sequenceDiagram\nparticipant C\nNote over C: Write fails#59; skip commit",
            "sequenceDiagram\nparticipant C\nNote over C: Write fails; C->>C: skip commit",
            "sequenceDiagram; participant A; participant B; A->>B: hi",
            "sequenceDiagram\nparticipant A\nA->>B: implicit participant",
            "sequenceDiagram\nparticipant A\nparticipant B\nA->>+B: hello\nB-->>-A: done",
            "flowchart LR\nA[\"Write fails; skip commit\"] --> B",
            "flowchart LR\nsubgraph S; A --> B; end",
        ] {
            check(source).unwrap_or_else(|error| panic!("{source}: {error:#}"));
        }
    }

    #[test]
    fn accepts_common_diagram_families_and_accessibility_fields() {
        for source in [
            "flowchart LR\naccTitle: Workflow\naccDescr: Start to finish\nA --> B",
            "sequenceDiagram\naccTitle: A message\naccDescr: Alice greets Bob\nAlice->>Bob: Hello",
            "classDiagram\nAnimal <|-- Duck",
            "stateDiagram-v2\n[*] --> Ready\nReady --> [*]",
            "erDiagram\nCUSTOMER ||--o{ ORDER : places",
            "pie\n\"A\" : 60\n\"B\" : 40",
            "mindmap\n  root((Root))\n    A\n    B",
            "journey\n  title My day\n  section Work\n    Task: 5: Me",
            "timeline\n  title History\n  2024 : Start\n  2025 : Done",
            "gantt\n  dateFormat YYYY-MM-DD\n  section Work\n  Task :a1, 2024-01-01, 1d",
            "requirementDiagram\nrequirement r1 {\nid: 1\ntext: \"test\"\nrisk: low\nverifymethod: test\n}",
            "gitGraph\ncommit\nbranch develop\ncheckout develop\ncommit",
            "C4Context\nPerson(user, \"User\", \"A user\")\nSystem(sys, \"System\", \"Our system\")\nRel(user, sys, \"Uses\")",
            "sankey-beta\nA,B,10\nB,C,5",
            "quadrantChart\n  title Priorities\n  x-axis Low --> High\n  y-axis Low --> High\n  A: [0.3, 0.6]",
            "block-beta\ncolumns 2\na[\"A\"] b[\"B\"]\na --> b",
            "packet-beta\n0-7: \"Header\"\n8-15: \"Body\"",
            "kanban\n  todo[Todo]\n    task[Task]",
            "architecture-beta\nservice db(database)[Database]\nservice api(server)[API]\napi:R -- L:db",
            "radar-beta\naxis a[\"A\"], b[\"B\"], c[\"C\"]\ncurve s[\"Series\"]{1,2,3}",
            "treemap-beta\n\"Root\"\n    \"A\": 10\n    \"B\": 20",
            "xychart-beta\nx-axis [jan, feb, mar]\ny-axis \"Value\" 0 --> 10\nbar [1, 4, 8]",
            "info",
        ] {
            check(source).unwrap_or_else(|error| panic!("{source}: {error:#}"));
        }
    }

    #[test]
    fn rejects_empty_unknown_and_malformed_diagrams() {
        for source in [
            "",
            "notADiagram\nA --> B",
            "zenuml\nA->B: hello",
            "flowchart LR\nA[unfinished --> B",
            "flowchart LR\nA -->",
            "classDiagram\nclass A {\n+String name",
            "stateDiagram-v2\nstate Group {\n[*] --> Ready",
            "erDiagram\nA ||--o{ : owns",
            "pie\n\"A\" : nope",
        ] {
            assert!(check(source).is_err(), "accepted {source:?}");
        }
    }

    #[test]
    fn identifies_diagrams_in_notes_and_multiple_blocks() {
        let deck = parse_deck(
            "# Intro\n\n---\n\n# Diagrams\n\n```mermaid\nflowchart LR\nA --> B\n```\n\n:::notes\n```mermaid\nsequenceDiagram\nA->>B\n```\n:::"
        ).unwrap();
        let error = format!("{:#}", validate(&deck).unwrap_err());
        assert!(error.contains("slide 2, Mermaid diagram 2"), "{error}");
    }

    #[test]
    fn enforces_count_and_utf16_size_limits() {
        let mut deck = parse_deck("# Test\n\n```mermaid\nflowchart LR\nA --> B\n```").unwrap();
        let diagram = deck.slides[0].mermaid_diagrams[0].clone();
        deck.slides[0].mermaid_diagrams = vec![diagram.clone(); DIAGRAM_LIMIT];
        validate(&deck).unwrap();
        deck.slides[0].mermaid_diagrams.push(diagram);
        assert!(validate(&deck).unwrap_err().to_string().contains("100"));

        let prefix = "flowchart LR\n%% ";
        let source = format!("{prefix}{}", "x".repeat(TEXT_LIMIT - prefix.len()));
        deck.slides[0].mermaid_diagrams = vec![source];
        validate(&deck).unwrap();
        deck.slides[0].mermaid_diagrams[0].push('x');
        assert!(validate(&deck).unwrap_err().to_string().contains("50000"));
        // Same byte limit would incorrectly reject these BMP characters.
        deck.slides[0].mermaid_diagrams[0] = format!("{prefix}{}", "é".repeat(30_000));
        validate(&deck).unwrap();
        deck.slides[0].mermaid_diagrams[0] = format!("{prefix}{}", "😀".repeat(25_000));
        assert!(validate(&deck).unwrap_err().to_string().contains("50000"));
    }

    #[test]
    fn observes_parser_cancellation() {
        let deck = parse_deck("# Test\n\n```mermaid\nflowchart LR\nA --> B\n```").unwrap();
        let control = OperationControl::new().with_deadline(Duration::ZERO);
        let error = format!("{:#}", validate_with_control(&deck, &control).unwrap_err());
        assert!(error.contains("slide 1, Mermaid diagram 1"), "{error}");
        assert!(error.contains("deadline_exceeded"), "{error}");
    }

    #[test]
    fn existing_examples_validate() {
        for source in [
            include_str!("../examples/intro-to-rust.md"),
            include_str!("../examples/kitchen-sink.md"),
            "# No diagrams\n\nPlain Markdown is unaffected.",
        ] {
            validate(&parse_deck(source).unwrap()).unwrap();
        }
    }
}
