//! Reine Logik der Ergebnislisten (Reiter „Suche“ und Ticket-Liste mit Abfrage): Sortierung über die Abfrage selbst, Seiten,
//! Hervorhebung der Fehlerstelle. Kein egui-Zustand, keine Datenbank – nur Text und Zahlen, damit alles einzeln testbar ist.
//!
//! **Sortieren:** Hat die Abfrage kein `ORDER BY`, hängt ein Klick auf eine Spaltenüberschrift eines an den ausgeführten Text an
//! ([`with_sort`]). So sortiert die Datenbank über **alle** Treffer und nicht nur über die geladene Seite. Hat die Abfrage ein
//! eigenes `ORDER BY`, gilt dessen Reihenfolge und die Spalten sind nicht sortierbar ([`has_order`]).

use super::query_logic::clamp_range;
use crate::planning::search::Problem;
use crate::query::{self, Schema};
use eframe::egui::{text::LayoutJob, Color32, FontId, Stroke, TextFormat};

/// So viele Zeilen hat eine Seite der Ergebnisliste.
pub const PAGE_SIZE: i64 = 100;

/// Eine Spalte der Ergebnistabelle.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Col {
    Type,
    Key,
    Title,
    Status,
    Priority,
    Points,
    Project,
    Marks,
}

impl Col {
    /// Erste Richtung beim ersten Klick: Markierungen und Punkte zuerst absteigend (das Wichtige oben), sonst aufsteigend.
    pub fn first_desc(self) -> bool {
        matches!(self, Col::Marks | Col::Points)
    }
}

/// Sortierung nach einer Spalte.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SortBy {
    pub col: Col,
    pub desc: bool,
}

impl SortBy {
    /// Neueste zuerst (Standard der Ticket-Liste).
    pub const NEWEST: SortBy = SortBy { col: Col::Key, desc: true };

    /// Klick auf eine Überschrift: dieselbe Spalte kehrt die Richtung um, eine neue beginnt mit [`Col::first_desc`].
    pub fn toggled(self, col: Col) -> SortBy {
        if self.col == col { SortBy { col, desc: !self.desc } } else { SortBy { col, desc: col.first_desc() } }
    }

    /// Wie [`SortBy::toggled`], aber mit Rückweg: erste Richtung → Gegenrichtung → ohne eigene Sortierung (`None`).
    pub fn next(cur: Option<SortBy>, col: Col) -> Option<SortBy> {
        match cur {
            Some(s) if s.col == col => {
                if s.desc == col.first_desc() {
                    Some(SortBy { col, desc: !s.desc })
                } else {
                    None
                }
            }
            _ => Some(SortBy { col, desc: col.first_desc() }),
        }
    }
}

/// Hat die Abfrage ein eigenes `ORDER BY` / `SORT BY`? (Eine unlesbare Abfrage hat keines.)
pub fn has_order(jql: &str) -> bool {
    query::parse(jql).map(|q| !q.order.is_empty()).unwrap_or(false)
}

/// Der `ORDER BY`-Text (ohne `ORDER BY`) für die Spalte; `None`, wenn das Schema das Feld nicht kennt (z. B. umbenanntes Prioritätsfeld).
pub fn order_terms(schema: &Schema, s: SortBy) -> Option<String> {
    let dir = if s.desc { "DESC" } else { "ASC" };
    let field = |name: &str| schema.field(name).map(|_| name.to_string());
    let one = |f: String| Some(format!("{f} {dir}"));
    match s.col {
        Col::Type => one(field("type")?),
        Col::Key => one(field("key")?),
        Col::Title => one(field("title")?),
        Col::Status => one(field("status")?),
        Col::Project => one(field("project")?),
        Col::Priority => one(field("priority")?),
        Col::Points => {
            let name = ["Story Points", "Story points"].into_iter().find(|n| schema.field(n).is_some())?;
            one(query::quote_string(name))
        }
        Col::Marks => {
            // „absteigend“ heißt: Markierte zuerst. Die Sortier-Schlüsselwörter zählen aufsteigend „markierte zuerst“, die Felder umgekehrt.
            if schema.sort_keyword("favorites").is_some() && schema.sort_keyword("flagged").is_some() {
                let d = if s.desc { "ASC" } else { "DESC" };
                Some(format!("favorites {d}, flagged {d}"))
            } else if schema.field("starred").is_some() && schema.field("flagged").is_some() {
                Some(format!("starred {dir}, flagged {dir}"))
            } else {
                None
            }
        }
    }
}

/// Kann nach der Spalte sortiert werden (das Schema kennt das Feld)?
pub fn sortable(schema: &Schema, col: Col) -> bool {
    order_terms(schema, SortBy { col, desc: false }).is_some()
}

/// Der auszuführende Text: die Abfrage, mit angehängtem `ORDER BY`, wenn sie keines hat und eine Sortierung gewählt ist.
pub fn with_sort(jql: &str, schema: &Schema, sort: Option<SortBy>) -> String {
    let Some(sort) = sort else { return jql.to_string() };
    if has_order(jql) {
        return jql.to_string();
    }
    let Some(terms) = order_terms(schema, sort) else { return jql.to_string() };
    let base = jql.trim_end();
    if base.is_empty() { format!("ORDER BY {terms}") } else { format!("{base} ORDER BY {terms}") }
}

// ------------------------------------------------------------------ Seiten

/// Anzahl der Seiten (mindestens 1).
pub fn page_count(total: i64) -> i64 {
    ((total.max(0) + PAGE_SIZE - 1) / PAGE_SIZE).max(1)
}

/// Die Seite in den gültigen Bereich bringen.
pub fn clamp_page(total: i64, page: i64) -> i64 {
    page.clamp(0, page_count(total) - 1)
}

/// Erste und letzte angezeigte Zeile (1-basiert) einer Seite; `(0, 0)` ohne Treffer.
pub fn page_span(total: i64, page: i64) -> (i64, i64) {
    if total <= 0 {
        return (0, 0);
    }
    let p = clamp_page(total, page);
    (p * PAGE_SIZE + 1, ((p + 1) * PAGE_SIZE).min(total))
}

// ------------------------------------------------------------------ Fehlerstelle

/// Die Abfrage als Text mit hervorgehobenen Fehlerstellen (rot, unterstrichen). Eine Stelle ohne Länge (Ende der Eingabe) wird mit
/// einem roten Dreieck markiert, das nicht zum Text gehört.
pub fn highlight_job(text: &str, problems: &[Problem], normal: Color32, bad: Color32, font: FontId) -> LayoutJob {
    let mut ranges: Vec<(usize, usize)> = problems
        .iter()
        .map(|p| {
            let r = clamp_range(text, p.start..p.start.saturating_add(p.len));
            (r.start, r.end)
        })
        .collect();
    ranges.sort_unstable();
    let mut job = LayoutJob::default();
    let fmt = |color: Color32, underline: bool| TextFormat { font_id: font.clone(), color, underline: if underline { Stroke::new(1.4_f32, bad) } else { Stroke::NONE }, ..Default::default() };
    let mut pos = 0usize;
    for (s, e) in ranges {
        if s < pos {
            continue; // überlappende Stelle: schon abgedeckt
        }
        job.append(&text[pos..s], 0.0, fmt(normal, false));
        if e > s {
            job.append(&text[s..e], 0.0, fmt(bad, true));
        } else {
            job.append("▲", 0.0, fmt(bad, false));
        }
        pos = e;
    }
    job.append(&text[pos..], 0.0, fmt(normal, false));
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::FieldType;

    fn schema() -> Schema {
        Schema::new()
            .with_field("key", FieldType::Text)
            .with_field("type", FieldType::Option(vec!["Epic".into()]))
            .with_field("status", FieldType::Option(vec!["Done".into()]))
            .with_field("title", FieldType::Text)
            .with_field("project", FieldType::Option(vec!["AA".into()]))
            .with_field("Priority", FieldType::Option(vec!["High".into()]))
            .with_field("Story points", FieldType::Number)
            .with_field("starred", FieldType::Bool)
            .with_field("flagged", FieldType::Bool)
            .with_sort_keyword("favorites")
            .with_sort_keyword("flagged")
    }

    #[test]
    fn first_clicks_follow_the_column_and_third_click_resets() {
        let s = SortBy::next(None, Col::Title);
        assert_eq!(s, Some(SortBy { col: Col::Title, desc: false }));
        let s = SortBy::next(s, Col::Title);
        assert_eq!(s, Some(SortBy { col: Col::Title, desc: true }));
        assert_eq!(SortBy::next(s, Col::Title), None);
        // Markierungen und Punkte beginnen absteigend
        assert_eq!(SortBy::next(None, Col::Marks), Some(SortBy { col: Col::Marks, desc: true }));
        let m = SortBy::next(None, Col::Points);
        assert_eq!(SortBy::next(m, Col::Points), Some(SortBy { col: Col::Points, desc: false }));
        // eine andere Spalte beginnt neu
        assert_eq!(SortBy::next(s, Col::Status), Some(SortBy { col: Col::Status, desc: false }));
    }

    #[test]
    fn toggled_flips_the_direction_and_never_resets() {
        let s = SortBy::NEWEST;
        assert_eq!(s.toggled(Col::Key), SortBy { col: Col::Key, desc: false });
        assert_eq!(s.toggled(Col::Title), SortBy { col: Col::Title, desc: false });
        assert_eq!(s.toggled(Col::Marks), SortBy { col: Col::Marks, desc: true });
    }

    #[test]
    fn order_clause_per_column_uses_the_schema_names() {
        let sc = schema();
        assert_eq!(order_terms(&sc, SortBy { col: Col::Status, desc: true }).as_deref(), Some("status DESC"));
        assert_eq!(order_terms(&sc, SortBy { col: Col::Priority, desc: false }).as_deref(), Some("priority ASC"));
        assert_eq!(order_terms(&sc, SortBy { col: Col::Points, desc: true }).as_deref(), Some("\"Story Points\" DESC"), "Namen mit Leerzeichen stehen in Anführungszeichen");
        // Markierte zuerst = absteigend; die Schlüsselwörter zählen umgekehrt
        assert_eq!(order_terms(&sc, SortBy { col: Col::Marks, desc: true }).as_deref(), Some("favorites ASC, flagged ASC"));
        assert_eq!(order_terms(&sc, SortBy { col: Col::Marks, desc: false }).as_deref(), Some("favorites DESC, flagged DESC"));
        // ohne Schlüsselwörter gelten die Felder
        let no_kw = Schema::new().with_field("starred", FieldType::Bool).with_field("flagged", FieldType::Bool);
        assert_eq!(order_terms(&no_kw, SortBy { col: Col::Marks, desc: true }).as_deref(), Some("starred DESC, flagged DESC"));
        // unbekannte Felder: nicht sortierbar
        let bare = Schema::new().with_field("title", FieldType::Text);
        assert!(order_terms(&bare, SortBy { col: Col::Priority, desc: false }).is_none());
        assert!(!sortable(&bare, Col::Points));
        assert!(sortable(&bare, Col::Title));
    }

    #[test]
    fn with_sort_appends_only_when_the_query_has_no_order() {
        let sc = schema();
        let by_status = Some(SortBy { col: Col::Status, desc: true });
        assert_eq!(with_sort("type = Epic", &sc, by_status), "type = Epic ORDER BY status DESC");
        assert_eq!(with_sort("type = Epic  \n", &sc, by_status), "type = Epic ORDER BY status DESC", "Leerraum am Ende stört nicht");
        assert_eq!(with_sort("", &sc, by_status), "ORDER BY status DESC", "eine leere Abfrage wird nur sortiert");
        assert_eq!(with_sort("type = Epic ORDER BY title", &sc, by_status), "type = Epic ORDER BY title", "ein eigenes ORDER BY gewinnt");
        assert_eq!(with_sort("type = Epic SORT BY favorites", &sc, by_status), "type = Epic SORT BY favorites");
        assert_eq!(with_sort("type = Epic", &sc, None), "type = Epic");
        // das Ergebnis ist selbst eine gültige Abfrage
        for s in [Col::Type, Col::Key, Col::Title, Col::Status, Col::Priority, Col::Points, Col::Project, Col::Marks] {
            for desc in [false, true] {
                let q = with_sort("status = Done", &sc, Some(SortBy { col: s, desc }));
                assert!(query::parse(&q).is_ok(), "{q}");
                assert!(query::validate(&query::parse(&q).unwrap(), &sc).is_empty(), "{q}");
            }
        }
    }

    #[test]
    fn has_order_reads_both_spellings_and_ignores_garbage() {
        assert!(has_order("a = 1 ORDER BY title"));
        assert!(has_order("sort by title"));
        assert!(!has_order("type = Epic"));
        assert!(!has_order("type = ("));
        assert!(!has_order(""));
    }

    #[test]
    fn pages_count_and_clamp() {
        assert_eq!(page_count(0), 1);
        assert_eq!(page_count(1), 1);
        assert_eq!(page_count(100), 1);
        assert_eq!(page_count(101), 2);
        assert_eq!(page_count(1000), 10);
        assert_eq!(clamp_page(250, 9), 2);
        assert_eq!(clamp_page(250, -3), 0);
        assert_eq!(clamp_page(0, 4), 0);
        assert_eq!(page_span(0, 0), (0, 0));
        assert_eq!(page_span(250, 0), (1, 100));
        assert_eq!(page_span(250, 2), (201, 250));
        assert_eq!(page_span(250, 99), (201, 250), "zu große Seite zeigt die letzte");
    }

    fn problem(start: usize, len: usize) -> Problem {
        Problem { start, len, column: 1, message: String::new() }
    }

    #[test]
    fn highlight_marks_the_problem_range_and_keeps_all_text() {
        let text = "stauts = Done AND x";
        let job = highlight_job(text, &[problem(0, 6)], Color32::WHITE, Color32::RED, FontId::monospace(12.0));
        assert_eq!(job.text, text);
        let bad: Vec<&str> = job.sections.iter().filter(|s| s.format.color == Color32::RED).map(|s| &job.text[s.byte_range.clone()]).collect();
        assert_eq!(bad, vec!["stauts"]);
        assert!(job.sections.iter().any(|s| s.format.underline != Stroke::NONE));
    }

    #[test]
    fn highlight_handles_the_end_overlaps_unicode_and_bad_offsets() {
        // Stelle am Ende ohne Länge: ein Dreieck wird angehängt
        let job = highlight_job("a =", &[problem(3, 0)], Color32::WHITE, Color32::RED, FontId::monospace(12.0));
        assert_eq!(job.text, "a =▲");
        // Überlappung und Stelle mitten in einem Umlaut (wird auf die Zeichengrenze gelegt), Länge über das Ende hinaus
        let text = "Größe = 1";
        let job = highlight_job(text, &[problem(0, 3), problem(2, 4), problem(7, 99)], Color32::WHITE, Color32::RED, FontId::monospace(12.0));
        assert_eq!(job.text, text);
        // keine Probleme: ein Abschnitt
        let job = highlight_job(text, &[], Color32::WHITE, Color32::RED, FontId::monospace(12.0));
        assert_eq!(job.text, text);
        assert!(job.sections.iter().all(|s| s.format.color == Color32::WHITE));
    }
}
