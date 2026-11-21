//! Indentation settings and detection.

use ropey::Rope;
use serde::{Deserialize, Serialize};

/// How one indentation level is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "style", content = "size")]
pub enum IndentStyle {
    Tabs,
    /// That many spaces per level (1..=16).
    Spaces(u8),
}

impl Default for IndentStyle {
    fn default() -> Self {
        IndentStyle::Spaces(4)
    }
}

impl IndentStyle {
    /// Text of one level.
    pub fn unit(self) -> String {
        match self {
            IndentStyle::Tabs => "\t".into(),
            IndentStyle::Spaces(n) => " ".repeat(n.max(1) as usize),
        }
    }

    /// Width of one level in columns (`tab_width` for tabs).
    pub fn width(self, tab_width: usize) -> usize {
        match self {
            IndentStyle::Tabs => tab_width.max(1),
            IndentStyle::Spaces(n) => n.max(1) as usize,
        }
    }
}

/// Where the indentation setting came from (for the status bar).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndentSource {
    /// Default from the editor settings.
    #[default]
    Default,
    /// Detected from the file content.
    Detected,
    /// From `.editorconfig` (wins over detection).
    EditorConfig,
    /// Chosen by the user for this document.
    User,
}

/// Indentation of a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndentSettings {
    pub style: IndentStyle,
    /// Display width of a tab character.
    pub tab_width: usize,
    pub source: IndentSource,
}

impl Default for IndentSettings {
    fn default() -> Self {
        Self { style: IndentStyle::default(), tab_width: 4, source: IndentSource::Default }
    }
}

impl IndentSettings {
    /// Width of one level in columns.
    pub fn unit_width(&self) -> usize {
        self.style.width(self.tab_width)
    }
}

/// Lines looked at for detection.
const DETECT_LINES: usize = 10_000;

/// Guess the indentation style from the first lines. `None` if the text has (almost) no indented lines.
///
/// Counts lines indented with tabs vs. spaces; for spaces, the most common increase of indentation between
/// consecutive non-blank lines decides the size (2, 3, 4 or 8; 1 only if nothing else shows up).
pub fn detect_indent(rope: &Rope) -> Option<IndentStyle> {
    let mut tabs = 0usize;
    let mut spaces = 0usize;
    let mut deltas = [0usize; 9];
    let mut prev_spaces: Option<usize> = None;
    for line in rope.lines().take(DETECT_LINES) {
        let mut chars = line.chars();
        let first = match chars.next() {
            Some(c) => c,
            None => continue,
        };
        if first == '\n' {
            continue;
        }
        if first == '\t' {
            tabs += 1;
            prev_spaces = None;
            continue;
        }
        let mut n = if first == ' ' { 1 } else { 0 };
        let mut rest_blank = true;
        if first == ' ' {
            for c in chars {
                if c == ' ' {
                    n += 1;
                } else {
                    rest_blank = c == '\n' || c == '\r';
                    break;
                }
            }
            if rest_blank {
                continue; // whitespace-only line
            }
        }
        if n > 0 {
            spaces += 1;
        }
        // A single space before `*` is the usual block comment continuation, not indentation.
        let comment_cont = n % 2 == 1 && line.chars().nth(n) == Some('*');
        if !comment_cont {
            if let Some(p) = prev_spaces {
                if n > p {
                    let d = n - p;
                    if d < deltas.len() {
                        deltas[d] += 1;
                    }
                }
            }
            prev_spaces = Some(n);
        }
    }
    if tabs == 0 && spaces == 0 {
        return None;
    }
    if tabs > spaces {
        return Some(IndentStyle::Tabs);
    }
    let best = [2usize, 4, 8, 3]
        .into_iter()
        .max_by_key(|d| (deltas[*d], std::cmp::Reverse(*d == 3)))
        .filter(|d| deltas[*d] > 0);
    match best {
        Some(d) => Some(IndentStyle::Spaces(d as u8)),
        None if deltas[1] > 0 => Some(IndentStyle::Spaces(1)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_styles() {
        assert_eq!(detect_indent(&Rope::from_str("fn a() {\n\tx;\n\tif y {\n\t\tz;\n\t}\n}\n")), Some(IndentStyle::Tabs));
        assert_eq!(
            detect_indent(&Rope::from_str("a:\n  b:\n    c: 1\n  d: 2\ne:\n  f: 3\n")),
            Some(IndentStyle::Spaces(2))
        );
        assert_eq!(
            detect_indent(&Rope::from_str("def f():\n    if x:\n        y()\n    return 1\n")),
            Some(IndentStyle::Spaces(4))
        );
        assert_eq!(detect_indent(&Rope::from_str("no\nindent\nhere\n")), None);
        // Block comment continuation lines do not make it 1 or 3.
        assert_eq!(
            detect_indent(&Rope::from_str("/**\n * doc\n */\nfn x() {\n    a();\n    if b {\n        c();\n    }\n}\n")),
            Some(IndentStyle::Spaces(4))
        );
    }

    #[test]
    fn unit_text() {
        assert_eq!(IndentStyle::Spaces(2).unit(), "  ");
        assert_eq!(IndentStyle::Tabs.unit(), "\t");
        assert_eq!(IndentStyle::Tabs.width(8), 8);
    }
}
