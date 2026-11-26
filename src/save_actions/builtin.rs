//! Eingebaute Aktionen, die den Text vor dem Schreiben ändern (#383): schnell und im UI-Thread, als ein rückgängig
//! machbarer Schritt. Zeilenenden und Kodierung ändern nur das Format beim Schreiben (das macht der Aufrufer am
//! Dokument).
//!
//! Formatieren nutzt den Formatierer des Editors (`crate::editor::format`, #381) über [`format_text`] – solange er
//! nicht eingebunden ist, meldet die Aktion „nicht verfügbar“ und ändert nichts.

use super::Builtin;
use crate::state::Lang;
use std::path::Path;

/// Größte Datei für *Formatieren* beim Speichern (wie das Formatieren beim Speichern des Editors).
pub const FORMAT_MAX_BYTES: usize = 1024 * 1024;

/// Einrückung für das Formatieren.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    Spaces(u8),
    Tab,
}

/// Ergebnis des Formatierers.
#[allow(dead_code)] // `Text`/`Unsupported`/`Invalid` kommen mit dem Formatierer (#381), siehe [`format_text`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatResult {
    Text(String),
    /// Weder JSON noch XML (keine Änderung, kein Fehler).
    Unsupported,
    /// Ungültiger Inhalt (Meldung mit Zeile/Spalte).
    Invalid(String),
    /// Der Formatierer ist in dieser Version nicht eingebunden.
    Unavailable,
}

/// Formatierer: hier wird `crate::editor::format` (#381) eingebunden, sobald er da ist, etwa so:
///
/// ```ignore
/// use crate::editor::format::{self, Op, Options, Outcome};
/// let Some(kind) = format::detect(path, text) else { return FormatResult::Unsupported };
/// let opts = Options { indent: match indent { Indent::Tab => format::Indent::Tab, Indent::Spaces(n) => format::Indent::Spaces(n) }, final_newline: text.ends_with('\n') };
/// match format::run(kind, Op::Format, text, opts, &AtomicBool::new(false), &|_| {}) {
///     Outcome::Text(t) => FormatResult::Text(t),
///     Outcome::Invalid(e) => FormatResult::Invalid(format!("{}:{} {}", e.line, e.column, e.message)),
///     Outcome::Cancelled => FormatResult::Unsupported,
/// }
/// ```
pub fn format_text(path: Option<&Path>, text: &str, indent: Indent) -> FormatResult {
    let _ = (path, text, indent);
    FormatResult::Unavailable
}

/// Rahmen für die Text-Aktionen.
#[derive(Clone, Copy, Debug)]
pub struct TextCx<'a> {
    pub path: Option<&'a Path>,
    /// Tabbreite des Dokuments (für Tabs↔Leerzeichen ohne eigene Breite).
    pub tab_width: usize,
    /// Einrückung des Dokuments (für das Formatieren).
    pub indent: Indent,
}

/// Ergebnis von [`apply_text`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub text: String,
    /// Aktionen, die etwas geändert haben.
    pub changed: Vec<Builtin>,
    /// Fehler je Aktion (übrige laufen weiter).
    pub errors: Vec<(Builtin, String)>,
    /// Hinweise ohne Fehler (z. B. „kein JSON/XML“).
    pub notes: Vec<(Builtin, String)>,
}

/// Die Text-Aktionen (ohne Zeilenenden und Kodierung) der Reihe nach auf `text` anwenden.
pub fn apply_text(text: &str, list: &[Builtin], cx: TextCx, lang: Lang) -> Applied {
    let mut cur = text.to_string();
    let mut out = Applied::default();
    for b in list {
        let next = match b {
            Builtin::TrimWhitespace => Some(trim_trailing(&cur)),
            Builtin::FinalNewline => Some(final_newline(&cur)),
            Builtin::TabsToSpaces(w) => Some(tabs_to_spaces(&cur, width(*w, cx.tab_width))),
            Builtin::SpacesToTabs(w) => Some(spaces_to_tabs(&cur, width(*w, cx.tab_width))),
            Builtin::Format => {
                if cur.len() > FORMAT_MAX_BYTES {
                    out.notes.push((b.clone(), lang.t("Datei größer als 1 MB – nicht formatiert", "File larger than 1 MB – not formatted").into()));
                    None
                } else {
                    match format_text(cx.path, &cur, cx.indent) {
                        FormatResult::Text(t) => Some(t),
                        FormatResult::Unsupported => {
                            out.notes.push((b.clone(), lang.t("weder JSON noch XML – nicht formatiert", "neither JSON nor XML – not formatted").into()));
                            None
                        }
                        FormatResult::Invalid(e) => {
                            out.errors.push((b.clone(), format!("{} {e}", lang.t("Ungültiger Inhalt, nicht formatiert:", "Invalid content, not formatted:"))));
                            None
                        }
                        FormatResult::Unavailable => {
                            out.notes.push((b.clone(), lang.t("Formatieren ist in dieser Version nicht verfügbar", "Formatting is not available in this version").into()));
                            None
                        }
                    }
                }
            }
            Builtin::LineEndings(_) | Builtin::Encoding(_) => None,
        };
        if let Some(n) = next
            && n != cur
        {
            cur = n;
            out.changed.push(b.clone());
        }
    }
    out.text = cur;
    out
}

fn width(own: u8, doc: usize) -> usize {
    if own > 0 { own as usize } else { doc.clamp(1, 16) }
}

/// Leerzeichen und Tabs am Ende jeder Zeile entfernen.
pub fn trim_trailing(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line.trim_end_matches([' ', '\t']));
    }
    out
}

/// Fehlt am Ende ein Zeilenumbruch, kommt einer dazu (leere Texte bleiben leer).
pub fn final_newline(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') { text.to_string() } else { format!("{text}\n") }
}

/// Einrückung am Zeilenanfang: Tabs in Leerzeichen (Spalten bleiben gleich).
pub fn tabs_to_spaces(text: &str, width: usize) -> String {
    map_indent(text, |col| " ".repeat(col), width)
}

/// Einrückung am Zeilenanfang: Leerzeichen in Tabs (Rest als Leerzeichen).
pub fn spaces_to_tabs(text: &str, width: usize) -> String {
    map_indent(text, |col| format!("{}{}", "\t".repeat(col / width), " ".repeat(col % width)), width)
}

fn map_indent(text: &str, make: impl Fn(usize) -> String, width: usize) -> String {
    let width = width.max(1);
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let lead: usize = line.len() - line.trim_start_matches([' ', '\t']).len();
        let (head, rest) = line.split_at(lead);
        // Nur Leerraum (oder leer): unverändert lassen, das ist Sache von „Leerraum entfernen“.
        if rest.is_empty() {
            out.push_str(line);
            continue;
        }
        let mut col = 0;
        for c in head.chars() {
            col = if c == '\t' { (col / width + 1) * width } else { col + 1 };
        }
        out.push_str(&make(col));
        out.push_str(rest);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> TextCx<'static> {
        TextCx { path: None, tab_width: 4, indent: Indent::Spaces(2) }
    }

    #[test]
    fn text_actions() {
        assert_eq!(trim_trailing("a  \n\tb\t\n  \nc "), "a\n\tb\n\nc");
        assert_eq!(final_newline("a"), "a\n");
        assert_eq!(final_newline("a\n"), "a\n");
        assert_eq!(final_newline(""), "");
        assert_eq!(tabs_to_spaces("\tx\n \tz\n\t\n", 4), "    x\n    z\n\t\n");
        assert_eq!(spaces_to_tabs("      x\n    y\nz  w", 4), "\t  x\n\ty\nz  w");
    }

    #[test]
    fn apply_runs_in_order_and_reports() {
        let a = apply_text("\tx  \ny", &[Builtin::TrimWhitespace, Builtin::FinalNewline, Builtin::TabsToSpaces(2), Builtin::LineEndings(crate::editor::LineEnding::CrLf)], cx(), Lang::En);
        assert_eq!(a.text, "  x\ny\n");
        assert_eq!(a.changed, [Builtin::TrimWhitespace, Builtin::FinalNewline, Builtin::TabsToSpaces(2)]);
        let f = apply_text("{\"a\":1}", &[Builtin::Format], cx(), Lang::En);
        assert_eq!(f.text, "{\"a\":1}");
        assert!(f.errors.is_empty() && f.notes.len() == 1, "ohne Formatierer: Hinweis, keine Änderung");
        let big = "x".repeat(FORMAT_MAX_BYTES + 1);
        assert_eq!(apply_text(&big, &[Builtin::Format], cx(), Lang::En).notes.len(), 1);
    }
}
