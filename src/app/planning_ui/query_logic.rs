//! Reine Logik der Abfrage-Leiste (ohne egui, gut testbar): Vorschlag übernehmen, Prüfergebnis mit Positionen und
//! Korrekturvorschlägen („meinten Sie …“), Markierungsbereiche für die Wellenlinie und der Verlauf der letzten Abfragen.
//!
//! Alle Positionen sind Byte-Offsets in den Abfragetext (wie in `crate::query`); egui rechnet in Zeichen – die
//! Umrechnung ([`char_of`], [`byte_of`]) steht hier, damit Umlaute und Anführungszeichen nicht in der Oberfläche schiefgehen.

use crate::query::{self, ErrorCode, Found, Schema, Span, Suggestion, SuggestionKind, ValidationIssue, name_literal};
use crate::state::Lang;
use std::ops::Range;

// ------------------------------------------------------------------ Byte- und Zeichenpositionen

/// Byte-Offset → Zeichenindex (für egui). Ein Offset hinter dem Ende zählt als Ende.
pub fn char_of(text: &str, byte: usize) -> usize {
    let b = clamp_boundary(text, byte);
    text[..b].chars().count()
}

/// Zeichenindex → Byte-Offset. Ein Index hinter dem Ende ergibt das Ende.
pub fn byte_of(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map(|(i, _)| i).unwrap_or(text.len())
}

/// Offset auf die nächste Zeichengrenze davor (und höchstens bis zum Ende) zurückführen.
pub fn clamp_boundary(text: &str, mut b: usize) -> usize {
    b = b.min(text.len());
    while !text.is_char_boundary(b) {
        b -= 1;
    }
    b
}

/// Bereich auf gültige Zeichengrenzen bringen (nie hinter dem Ende, `start <= end`).
pub fn clamp_range(text: &str, r: Range<usize>) -> Range<usize> {
    let start = clamp_boundary(text, r.start);
    let mut end = clamp_boundary(text, r.end);
    if end < start {
        end = start;
    }
    start..end
}

fn span_range(text: &str, s: Span) -> Range<usize> {
    clamp_range(text, s.start..s.start.saturating_add(s.len))
}

// ------------------------------------------------------------------ Vorschlag übernehmen

/// Soll nach dem Übernehmen ein Leerzeichen folgen, damit gleich der nächste Vorschlag kommt (nach Feld, Operator, Wort)?
/// Nein bei Klammern/Komma, bei Werten (danach kann ein Komma stehen) und bei Funktionen.
fn wants_trailing_space(s: &Suggestion) -> bool {
    match s.kind {
        SuggestionKind::Value | SuggestionKind::Function => false,
        SuggestionKind::Keyword => !matches!(s.insert_text.trim(), "(" | ")" | ","),
        SuggestionKind::Field | SuggestionKind::Operator | SuggestionKind::Predicate | SuggestionKind::SortKey => true,
    }
}

/// Übernimmt einen Vorschlag: ersetzt `replace_range` durch `insert_text` (oder fügt am Cursor ein, wenn der Bereich leer ist).
/// Am Textende folgt ein Leerzeichen, wenn der nächste Schritt ein neues Wort ist. Ergebnis: neuer Text und die neue
/// Cursor-Position (Byte-Offset, hinter dem Eingefügten).
pub fn apply_suggestion(text: &str, s: &Suggestion) -> (String, usize) {
    let r = clamp_range(text, s.replace_range.clone());
    let mut ins = s.insert_text.clone();
    if r.end == text.len() && wants_trailing_space(s) && !ins.ends_with(' ') {
        ins.push(' ');
    }
    let mut out = String::with_capacity(text.len() + ins.len());
    out.push_str(&text[..r.start]);
    out.push_str(&ins);
    let cursor = out.len();
    out.push_str(&text[r.end..]);
    (out, cursor)
}

// ------------------------------------------------------------------ Feldliste

/// Ein Eintrag der Feldliste („Felder ▾“): Name und Typ zum Anzeigen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldEntry {
    pub name: String,
    /// Typ in der Sprache der Oberfläche (bei Mehrfachwerten mit Zusatz).
    pub kind: String,
}

/// Alle Felder des Schemas für die Feldliste, alphabetisch (ohne Beachtung der Groß-/Kleinschreibung).
pub fn field_list(schema: &Schema, lang: Lang) -> Vec<FieldEntry> {
    let mut out: Vec<FieldEntry> = schema
        .fields
        .iter()
        .map(|f| {
            let k = f.ty.kind().label(lang);
            let kind = if f.multi { format!("{k}, {}", lang.t("mehrfach", "multiple")) } else { k.to_string() };
            FieldEntry { name: f.name.clone(), kind }
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Setzt einen Feldnamen am Cursor ein (bei Bedarf in Anführungszeichen), mit Leerzeichen davor, wenn direkt davor ein
/// Wort steht, und dahinter, damit gleich der Operator folgen kann. Ergebnis: neuer Text und Cursor hinter dem Leerzeichen.
pub fn insert_name(text: &str, cursor: usize, name: &str) -> (String, usize) {
    let c = clamp_boundary(text, cursor);
    let (before, after) = text.split_at(c);
    let mut ins = String::new();
    if before.chars().next_back().is_some_and(|ch| !ch.is_whitespace() && ch != '(') {
        ins.push(' ');
    }
    ins.push_str(&name_literal(name));
    ins.push(' ');
    let mut out = String::with_capacity(text.len() + ins.len());
    out.push_str(before);
    out.push_str(&ins);
    let cursor = out.len();
    out.push_str(after.strip_prefix(' ').unwrap_or(after));
    (out, cursor)
}

// ------------------------------------------------------------------ Prüfung

/// Wie ernst eine Meldung ist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Die Abfrage ist falsch.
    Error,
    /// Die Abfrage ist nur noch nicht fertig (Eingabe endet zu früh) – wird gedämpft gezeigt.
    Incomplete,
}

/// Ein Korrekturvorschlag: `range` durch `text` ersetzen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub range: Range<usize>,
    pub text: String,
    /// Der Name, wie er dem Nutzer angeboten wird (ohne Anführungszeichen).
    pub shown: String,
}

/// Eine Meldung zur Abfrage, verankert an einer Stelle des Textes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    pub range: Range<usize>,
    pub message: String,
    pub severity: Severity,
    pub fix: Option<Fix>,
}

fn fix_of(text: &str, issue: &ValidationIssue) -> Option<Fix> {
    let shown = issue.suggestion()?;
    let replacement = match &issue.code {
        // Bei Funktionen steht nur der Name im Bereich (die Klammern bleiben), also ohne Anführungszeichen.
        query::IssueCode::UnknownFunction { .. } => shown.to_string(),
        _ => name_literal(shown),
    };
    Some(Fix { range: span_range(text, issue.span), text: replacement, shown: shown.to_string() })
}

/// Prüft eine Abfrage: erst die Syntax, dann (nur wenn sie stimmt) gegen das Schema. Leere Abfragen sind in Ordnung.
pub fn diagnose(text: &str, schema: &Schema, lang: Lang) -> Vec<Diag> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    match query::parse(text) {
        Err(errors) => errors
            .iter()
            .map(|e| {
                let incomplete = matches!(e.code, ErrorCode::Expected { found: Found::End, .. });
                Diag { range: span_range(text, e.span), message: e.message(lang), severity: if incomplete { Severity::Incomplete } else { Severity::Error }, fix: None }
            })
            .collect(),
        Ok(q) => query::validate(&q, schema)
            .iter()
            .map(|i| Diag { range: span_range(text, i.span), message: i.message(lang), severity: Severity::Error, fix: fix_of(text, i) })
            .collect(),
    }
}

/// Darf die Abfrage abgeschickt werden (leer oder ohne jede Meldung)?
pub fn is_submittable(diags: &[Diag]) -> bool {
    diags.is_empty()
}

/// Wendet einen Korrekturvorschlag an: neuer Text und Cursor-Position hinter dem Ersetzten.
pub fn apply_fix(text: &str, fix: &Fix) -> (String, usize) {
    let r = clamp_range(text, fix.range.clone());
    let mut out = String::with_capacity(text.len() + fix.text.len());
    out.push_str(&text[..r.start]);
    out.push_str(&fix.text);
    let cursor = out.len();
    out.push_str(&text[r.end..]);
    (out, cursor)
}

/// Die Bereiche, die mit einer Wellenlinie markiert werden: der Bereich der Meldung; ein leerer Bereich mitten im Text
/// markiert das nächste Zeichen, am Textende das letzte. „Unfertig“-Meldungen (Eingabe endet zu früh) bekommen keine Linie.
pub fn wave_ranges(text: &str, diags: &[Diag]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for d in diags.iter().filter(|d| d.severity == Severity::Error) {
        let mut r = clamp_range(text, d.range.clone());
        if r.is_empty() {
            if let Some(c) = text[r.end..].chars().next() {
                r = r.start..r.start + c.len_utf8();
            } else if let Some(c) = text[..r.start].chars().next_back() {
                r = r.start - c.len_utf8()..r.start;
            } else {
                continue;
            }
        }
        out.push(r);
    }
    out
}

// ------------------------------------------------------------------ Verlauf

/// Die letzten Abfragen (neueste zuletzt) samt Blättern mit Pfeil hoch/runter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    items: Vec<String>,
    /// Gerade angezeigter Eintrag beim Blättern; `None` = der Entwurf.
    pos: Option<usize>,
    /// Was vor dem Blättern im Feld stand.
    draft: String,
}

impl History {
    /// So viele Abfragen merkt sich der Verlauf.
    pub const MAX: usize = 30;

    /// Abfrage merken (ohne Ränder; leer wird ignoriert; eine ältere gleiche wandert nach hinten). Beendet das Blättern.
    pub fn push(&mut self, q: &str) {
        self.pos = None;
        self.draft.clear();
        let q = q.trim();
        if q.is_empty() {
            return;
        }
        self.items.retain(|i| i != q);
        self.items.push(q.to_string());
        if self.items.len() > Self::MAX {
            let extra = self.items.len() - Self::MAX;
            self.items.drain(..extra);
        }
    }

    /// Ein Eintrag zurück (älter). Beim ersten Mal wird `current` als Entwurf gemerkt. `None`, wenn es nichts Älteres gibt.
    pub fn prev(&mut self, current: &str) -> Option<String> {
        if self.items.is_empty() {
            return None;
        }
        // Wurde der angezeigte Eintrag von Hand geändert, beginnt das Blättern von vorn.
        if self.pos.is_some_and(|p| self.items.get(p).map(String::as_str) != Some(current)) {
            self.pos = None;
        }
        let next = match self.pos {
            None => {
                self.draft = current.to_string();
                self.items.len() - 1
            }
            Some(0) => return None,
            Some(p) => p - 1,
        };
        self.pos = Some(next);
        Some(self.items[next].clone())
    }

    /// Ein Eintrag vor (neuer); hinter dem neuesten kommt der Entwurf zurück. `None`, wenn nicht geblättert wird.
    pub fn next(&mut self, current: &str) -> Option<String> {
        let p = self.pos?;
        if self.items.get(p).map(String::as_str) != Some(current) {
            self.pos = None;
            return None;
        }
        if p + 1 < self.items.len() {
            self.pos = Some(p + 1);
            Some(self.items[p + 1].clone())
        } else {
            self.pos = None;
            Some(std::mem::take(&mut self.draft))
        }
    }

    /// Blättern beenden (der Nutzer hat selbst getippt).
    pub fn reset_nav(&mut self) {
        self.pos = None;
    }

    /// Die Einträge, neueste zuerst (für das Dropdown).
    pub fn newest_first(&self) -> impl Iterator<Item = &str> {
        self.items.iter().rev().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Zum Speichern (JSON-Liste, neueste zuletzt).
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.items).unwrap_or_else(|_| "[]".to_string())
    }

    /// Aus gespeichertem JSON; Unlesbares ergibt einen leeren Verlauf.
    pub fn from_json(s: &str) -> History {
        let mut h = History::default();
        if let Ok(list) = serde_json::from_str::<Vec<String>>(s) {
            for q in list {
                h.push(&q);
            }
        }
        h
    }
}

/// Kürzt eine Abfrage für Listen auf eine Zeile (Umbrüche zu Leerzeichen, mit „…“ am Ende).
pub fn one_line(q: &str, max_chars: usize) -> String {
    let flat: String = q.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let cut: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{FieldType, suggest};

    fn schema() -> Schema {
        Schema::new()
            .with_field("status", FieldType::Option(vec!["To Do".into(), "Done".into(), "In Progress".into()]))
            .with_field("type", FieldType::Option(vec!["Epic".into(), "Defect".into()]))
            .with_field("Release version", FieldType::Option(vec!["2.0.0".into(), "1.1.0".into()]))
            .with_field("Priorität", FieldType::Option(vec!["Hoch".into(), "Niedrig".into()]))
            .with_field("Größe", FieldType::Number)
            .with_field("summary", FieldType::Text)
            .with_field("due", FieldType::Date)
            .with_predicate("favorite")
            .with_sort_keyword("favorites")
    }

    /// Alle Vorschläge am Ende des Textes, Anzeigetexte.
    fn labels(text: &str) -> Vec<String> {
        suggest(text, text.len(), &schema()).into_iter().map(|s| s.label).collect()
    }

    fn pick(text: &str, cursor: usize, label: &str) -> (String, usize) {
        let s = suggest(text, cursor, &schema()).into_iter().find(|s| s.label == label).unwrap_or_else(|| panic!("kein Vorschlag {label} bei {text:?}"));
        apply_suggestion(text, &s)
    }

    #[test]
    fn positions_convert_between_bytes_and_chars() {
        let t = "Größe = 5 äöü";
        for (i, _) in t.char_indices() {
            assert_eq!(byte_of(t, char_of(t, i)), i);
        }
        assert_eq!(char_of(t, t.len()), t.chars().count());
        assert_eq!(byte_of(t, 99), t.len());
        // mitten in einem Zeichen: auf die Grenze davor
        assert_eq!(char_of("ä", 1), 0);
        assert_eq!(clamp_range("äb", 1..99), 0..3);
        assert_eq!(clamp_range("abc", 5..2), 3..3);
    }

    #[test]
    fn replaces_the_partial_word_under_the_cursor() {
        let (t, c) = pick("sta", 3, "status");
        assert_eq!(t, "status ", "Feld wird ersetzt, am Ende folgt ein Leerzeichen");
        assert_eq!(c, t.len());
        // Wort in der Mitte des Textes: kein zusätzliches Leerzeichen, der Rest bleibt
        let (t, c) = pick("sta = Done", 3, "status");
        assert_eq!(t, "status = Done");
        assert_eq!(c, "status".len());
    }

    #[test]
    fn inserts_at_the_cursor_when_nothing_is_typed() {
        let (t, c) = pick("status ", 7, "=");
        assert_eq!(t, "status = ");
        assert_eq!(c, t.len());
        let (t, _) = pick("status = Done ", 14, "AND");
        assert_eq!(t, "status = Done AND ");
    }

    #[test]
    fn names_with_spaces_and_quotes_survive() {
        // Feldname mit Leerzeichen kommt in Anführungszeichen
        let (t, c) = pick("Rel", 3, "Release version");
        assert_eq!(t, "\"Release version\" ");
        assert_eq!(c, t.len());
        // Wert mit Leerzeichen
        let (t, _) = pick("status = In", 11, "In Progress");
        assert_eq!(t, "status = \"In Progress\"");
        // ein angefangener Name in Anführungszeichen wird komplett ersetzt, auch das schließende Zeichen
        let (t, _) = pick("\"Rel\" = ", 4, "Release version");
        assert_eq!(t, "\"Release version\" = ", "ohne doppeltes Leerzeichen");
    }

    #[test]
    fn umlauts_are_handled_by_bytes() {
        let (t, c) = pick("Prio", 4, "Priorität");
        assert_eq!(t, "Priorität ");
        assert_eq!(c, t.len());
        // Cursor hinter einem Umlaut mitten im Text
        let text = "Größe > 3 AND Pri";
        let (t, c) = pick(text, text.len(), "Priorität");
        assert_eq!(t, "Größe > 3 AND Priorität ");
        assert_eq!(char_of(&t, c), t.chars().count());
        let (t, _) = pick("Gr", 2, "Größe");
        assert_eq!(t, "Größe ");
        // Wert mit Umlaut
        let (t, _) = pick("Priorität = Ho", 14, "Hoch");
        assert_eq!(t, "Priorität = Hoch");
    }

    #[test]
    fn brackets_and_values_get_no_trailing_space() {
        let (t, _) = pick("status IN ", 10, "(");
        assert_eq!(t, "status IN (");
        let (t, _) = pick("due < ", 6, "currentDate()");
        assert_eq!(t, "due < currentDate()");
        let (t, _) = pick("status IN (Done ", 16, ",");
        assert_eq!(t, "status IN (Done ,", "nach dem Komma folgt kein Leerzeichen");
    }

    #[test]
    fn suggestions_cover_all_kinds() {
        let kinds = |text: &str| -> Vec<SuggestionKind> { suggest(text, text.len(), &schema()).iter().map(|s| s.kind).collect() };
        assert!(kinds("").contains(&SuggestionKind::Field));
        assert!(kinds("").contains(&SuggestionKind::Predicate));
        assert!(kinds("status ").contains(&SuggestionKind::Operator));
        assert!(kinds("status = ").contains(&SuggestionKind::Value));
        assert!(kinds("due = ").contains(&SuggestionKind::Function));
        assert!(kinds("status = Done ").contains(&SuggestionKind::Keyword));
        assert!(kinds("ORDER BY ").contains(&SuggestionKind::SortKey));
        assert!(labels("status = ").contains(&"Done".to_string()));
    }

    #[test]
    fn empty_and_complete_queries_have_no_diagnostics() {
        assert!(diagnose("", &schema(), Lang::En).is_empty());
        assert!(diagnose("   \n ", &schema(), Lang::En).is_empty());
        assert!(diagnose("status = Done AND type = Epic ORDER BY due DESC", &schema(), Lang::En).is_empty());
        assert!(is_submittable(&[]));
    }

    #[test]
    fn syntax_errors_carry_position_and_both_languages() {
        let text = "status = Done AND (type = Epic";
        let en = diagnose(text, &schema(), Lang::En);
        let de = diagnose(text, &schema(), Lang::De);
        assert_eq!(en.len(), de.len());
        assert!(!en.is_empty());
        assert!(!is_submittable(&en));
        assert_ne!(en[0].message, de[0].message);
        assert!(en[0].range.end <= text.len());
        // „)“ fehlt: die Meldung zeigt auf die öffnende Klammer
        assert!(en.iter().any(|d| d.range.start == text.find('(').unwrap()), "{en:?}");
    }

    #[test]
    fn unexpected_end_is_only_incomplete() {
        let d = diagnose("status = ", &schema(), Lang::En);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Severity::Incomplete);
        assert!(!is_submittable(&d), "auch eine unfertige Abfrage wird nicht abgeschickt");
        assert!(wave_ranges("status = ", &d).is_empty(), "keine Wellenlinie für Unfertiges");
        let d = diagnose("status = Done AND", &schema(), Lang::De);
        assert_eq!(d[0].severity, Severity::Incomplete);
        // ein echter Fehler mitten im Text bleibt ein Fehler
        let d = diagnose("status = = Done", &schema(), Lang::En);
        assert_eq!(d[0].severity, Severity::Error);
    }

    #[test]
    fn unknown_field_offers_a_fix_that_applies_cleanly() {
        let text = "stauts = Done";
        let d = diagnose(text, &schema(), Lang::En);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].range, 0..6);
        assert!(d[0].message.contains("did you mean"), "{}", d[0].message);
        let fix = d[0].fix.clone().expect("Vorschlag");
        assert_eq!(fix.shown, "status");
        let (t, c) = apply_fix(text, &fix);
        assert_eq!(t, "status = Done");
        assert_eq!(c, 6);
        assert!(diagnose(&t, &schema(), Lang::En).is_empty(), "nach der Korrektur ist alles gut");
        // deutsch
        assert!(diagnose(text, &schema(), Lang::De)[0].message.contains("meinten Sie"));
    }

    #[test]
    fn fixes_quote_names_and_values_when_needed() {
        // unbekanntes Feld mit Leerzeichen im Vorschlag → Anführungszeichen
        let text = "\"Release verion\" = 2.0.0";
        let d = diagnose(text, &schema(), Lang::En);
        let fix = d[0].fix.clone().expect("Vorschlag");
        let (t, _) = apply_fix(text, &fix);
        assert_eq!(t, "\"Release version\" = 2.0.0");
        // Wert mit Leerzeichen
        let text = "status = \"In Progres\"";
        let d = diagnose(text, &schema(), Lang::En);
        let (t, _) = apply_fix(text, &d[0].fix.clone().unwrap());
        assert_eq!(t, "status = \"In Progress\"");
        // Wert ohne Leerzeichen bleibt ohne Anführungszeichen
        let text = "type = Epik";
        let (t, _) = apply_fix(text, &diagnose(text, &schema(), Lang::En)[0].fix.clone().unwrap());
        assert_eq!(t, "type = Epic");
        // Umlaut-Feld
        let text = "Prioritat = Hoch";
        let d = diagnose(text, &schema(), Lang::De);
        let (t, c) = apply_fix(text, &d[0].fix.clone().unwrap());
        assert_eq!(t, "Priorität = Hoch");
        assert_eq!(c, "Priorität".len());
    }

    #[test]
    fn fixes_cover_predicates_functions_and_sort_keys() {
        let apply = |text: &str| -> String {
            let d = diagnose(text, &schema(), Lang::En);
            assert_eq!(d.len(), 1, "{text}: {d:?}");
            apply_fix(text, &d[0].fix.clone().unwrap_or_else(|| panic!("kein Vorschlag für {text}"))).0
        };
        assert_eq!(apply("IS favorit"), "IS favorite");
        assert_eq!(apply("due < currentDat()"), "due < currentDate()");
        assert_eq!(apply("ORDER BY favorits"), "ORDER BY favorites");
    }

    #[test]
    fn issues_without_a_suggestion_have_no_fix() {
        let d = diagnose("nonexistentfieldxyz = 1", &schema(), Lang::En);
        assert_eq!(d.len(), 1);
        assert!(d[0].fix.is_none());
        let d = diagnose("Größe = abc", &schema(), Lang::En);
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn wave_marks_the_error_position() {
        let text = "stauts = Done AND type = Epic";
        let d = diagnose(text, &schema(), Lang::En);
        assert_eq!(wave_ranges(text, &d), vec![0..6]);
        // leerer Bereich mitten im Text → nächstes Zeichen; am Ende → letztes Zeichen
        let d = vec![Diag { range: 3..3, message: String::new(), severity: Severity::Error, fix: None }];
        assert_eq!(wave_ranges("abcdef", &d), vec![3..4]);
        let d = vec![Diag { range: 6..6, message: String::new(), severity: Severity::Error, fix: None }];
        assert_eq!(wave_ranges("abcdef", &d), vec![5..6]);
        assert_eq!(wave_ranges("äöü", &[Diag { range: 6..6, ..d[0].clone() }]), vec![4..6], "Umlaut am Ende");
        assert!(wave_ranges("", &d).is_empty());
        // ein zu großer Bereich wird gekürzt
        let d = vec![Diag { range: 2..99, message: String::new(), severity: Severity::Error, fix: None }];
        assert_eq!(wave_ranges("abcd", &d), vec![2..4]);
    }

    #[test]
    fn history_remembers_recent_unique_queries() {
        let mut h = History::default();
        assert!(h.is_empty());
        h.push("  a = 1 ");
        h.push("b = 2");
        h.push("");
        h.push("   ");
        h.push("a = 1");
        assert_eq!(h.newest_first().collect::<Vec<_>>(), vec!["a = 1", "b = 2"], "ältere gleiche wandert nach hinten");
        for i in 0..40 {
            h.push(&format!("x = {i}"));
        }
        assert_eq!(h.len(), History::MAX);
        assert_eq!(h.newest_first().next(), Some("x = 39"));
        assert_eq!(h.newest_first().last(), Some("x = 10"));
    }

    #[test]
    fn history_navigation_restores_the_draft() {
        let mut h = History::default();
        assert_eq!(h.prev("x"), None, "leer");
        h.push("one");
        h.push("two");
        h.push("three");
        assert_eq!(h.prev("draft"), Some("three".into()));
        assert_eq!(h.prev("three"), Some("two".into()));
        assert_eq!(h.prev("two"), Some("one".into()));
        assert_eq!(h.prev("one"), None, "am Anfang");
        assert_eq!(h.next("one"), Some("two".into()));
        assert_eq!(h.next("two"), Some("three".into()));
        assert_eq!(h.next("three"), Some("draft".into()), "hinter dem neuesten kommt der Entwurf zurück");
        assert_eq!(h.next("draft"), None, "nicht mehr beim Blättern");
        // von Hand geänderter Eintrag: neu anfangen
        assert_eq!(h.prev("mine"), Some("three".into()));
        assert_eq!(h.prev("three edited"), Some("three".into()), "Blättern beginnt von vorn");
        h.reset_nav();
        assert_eq!(h.next("x"), None);
        // Senden beendet das Blättern
        h.prev("");
        h.push("four");
        assert_eq!(h.next("four"), None);
        assert_eq!(h.prev(""), Some("four".into()));
    }

    #[test]
    fn history_survives_json() {
        let mut h = History::default();
        h.push("status = Done");
        h.push("summary ~ \"ä ö\"");
        let back = History::from_json(&h.to_json());
        assert_eq!(back.newest_first().collect::<Vec<_>>(), h.newest_first().collect::<Vec<_>>());
        assert!(History::from_json("kaputt").is_empty());
        assert!(History::from_json("").is_empty());
        assert!(History::from_json("[1,2]").is_empty());
    }

    #[test]
    fn field_list_is_sorted_and_names_the_types_in_both_languages() {
        let sc = schema().with_multi_field("Komponente", FieldType::Labels);
        let en = field_list(&sc, Lang::En);
        let names: Vec<&str> = en.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["due", "Größe", "Komponente", "Priorität", "Release version", "status", "summary", "type"]);
        let size = en.iter().find(|f| f.name == "Größe").unwrap();
        assert_eq!(size.kind, "number");
        let de = field_list(&sc, Lang::De);
        assert_eq!(de.iter().find(|f| f.name == "Größe").unwrap().kind, "Zahl");
        assert!(en.iter().find(|f| f.name == "Komponente").unwrap().kind.contains("multiple"));
        assert!(de.iter().find(|f| f.name == "Komponente").unwrap().kind.contains("mehrfach"));
        assert!(field_list(&Schema::new(), Lang::En).is_empty());
    }

    #[test]
    fn insert_name_quotes_and_spaces_as_needed() {
        assert_eq!(insert_name("", 0, "status"), ("status ".to_string(), 7));
        let (t, c) = insert_name("type = Epic AND", 15, "Release version");
        assert_eq!(t, "type = Epic AND \"Release version\" ");
        assert_eq!(c, t.len());
        // mitten im Text: kein doppeltes Leerzeichen, der Rest bleibt
        let (t, c) = insert_name("( = 1", 1, "Größe");
        assert_eq!(t, "(Größe = 1");
        assert_eq!(&t[..c], "(Größe ");
        // Cursor hinter einem Umlaut, Offset mitten im Zeichen wird abgerundet
        let (t, _) = insert_name("ä", 1, "x");
        assert_eq!(t, "x ä");
        assert!(diagnose(&insert_name("", 0, "Priorität").0, &schema(), Lang::En).iter().all(|d| d.severity == Severity::Incomplete));
    }

    #[test]
    fn one_line_flattens_and_cuts() {
        assert_eq!(one_line("a  =\n 1", 20), "a = 1");
        assert_eq!(one_line("abcdefghij", 5), "abcd…");
        assert_eq!(one_line("äöüäöüäöü", 4), "äöü…");
    }
}
