//! Markdown-Modul: Parser, Darstellung und Rich-Text-Feld mit Umschalter „Text | Visuell“.
//!
//! * [`parse`]: Parser für eine Markdown-Teilmenge samt Ticket-Verweisen (`PAN-12`) → AST ([`Doc`]).
//! * [`render`]: Zeichnet ein [`Doc`] in egui (nur Anzeige) und meldet Klicks auf Verweise.
//! * [`format`]: reine Funktionen der Formatierungsleiste (`apply_format`).
//! * [`rich_text_field`]: das Feld selbst (Textmodus mit Leiste, Visuell-Ansicht).
//!
//! Das Modul kennt keine Datenbank und keine Ticket-Logik; es erkennt Verweise nur und meldet Klicks.

// Das Modul ist (noch) in keine Oberfläche eingebunden; ohne Verwender wäre alles „ungenutzt“.
#![allow(dead_code, unused_imports)]

mod field;
mod format;
mod parse;
mod render;

pub use field::{FieldMode, FieldOpts, FieldOutput, rich_text_field, rich_text_field_ex};
pub use format::{Format, Words, apply_format, apply_format_with};
pub use parse::{Align, Block, Doc, Inline, ListItem, inline_text, is_safe_url, parse, parse_plain};
pub use render::{RenderOpts, RenderOutput, render};

/// Beispieltext mit allen unterstützten Elementen (für Handtests).
pub const SAMPLE_MARKDOWN: &str = "\
# Markdown sample

Plain text with **bold**, *italic*, ***both***, `inline code` and a [link](https://example.org).
A soft line break follows here
and a hard one ends this line\u{20}\u{20}
right here. Ticket references like PAN-12 or DEV-7 become clickable; ABC-1 stays text.

## Lists

- First item
- Second item with a very long text that has to wrap at the width of the container, however wide that is
  - Nested item
    - Deeper item
- Third item

1. One
2. Two
   1. Two.one
3. Three

### Tasks

- [ ] Open task
- [x] Done task
  - [ ] Nested open task

> A quote with **bold** text and PAN-3.
> It can span lines.

#### Table

| Name | Count | Note |
|:-----|------:|:----:|
| Alpha | 1 | left, right, center |
| Beta | 22 | `code` and **bold** |
| Gamma | 333 | PAN-9 |

```rust
fn main() {
    println!(\"code block\");
}
```

---

The end.
";

/// Das Beispieldokument, geparst (Kürzel `PAN` und `DEV` gelten als bekannt).
pub fn sample_doc() -> Doc {
    parse(SAMPLE_MARKDOWN, &|p| p == "PAN" || p == "DEV")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_has_every_block_kind() {
        let d = sample_doc();
        let has = |f: &dyn Fn(&Block) -> bool| d.blocks.iter().any(f);
        assert!(has(&|b| matches!(b, Block::Heading { level: 1, .. })));
        assert!(has(&|b| matches!(b, Block::Heading { level: 4, .. })));
        assert!(has(&|b| matches!(b, Block::List { ordered: false, .. })));
        assert!(has(&|b| matches!(b, Block::List { ordered: true, .. })));
        assert!(has(&|b| matches!(b, Block::Quote(_))));
        assert!(has(&|b| matches!(b, Block::Table { .. })));
        assert!(has(&|b| matches!(b, Block::CodeBlock { .. })));
        assert!(has(&|b| matches!(b, Block::Rule)));
        let dump = format!("{d:?}");
        for needle in ["TicketRef(\"PAN-12\")", "TicketRef(\"DEV-7\")", "HardBreak", "SoftBreak", "Link {", "Code(", "Bold(", "Italic(", "task: Some(true)"] {
            assert!(dump.contains(needle), "fehlt: {needle}");
        }
        assert!(!dump.contains("TicketRef(\"ABC-1\")"));
    }
}
