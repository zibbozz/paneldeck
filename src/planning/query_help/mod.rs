//! Inhalt der Abfrage-Hilfe (Komplettanleitung zur JQL-artigen Abfragesprache) als Daten – ohne Oberfläche.
//!
//! Die Hilfeseite (später, hinter dem Fragezeichen neben dem Suchfeld) zeichnet dieses Dokument nur noch. Hier liegen:
//!
//! * die Datenstruktur: [`HelpDoc`] → [`Section`] → [`Block`] (Absatz, Code, [`Example`], Tabelle, Hinweis), alle Texte zweisprachig
//!   ([`Text`], Englisch ist der Standard);
//! * der Inhalt ([`HelpDoc::builtin`]): Einstieg, einfache bis komplexe Beispiele, Fehlersuche, Referenz;
//! * die Beispieldaten je Beispiel ([`DemoSpec`], `demo.rs`, `dataset.rs`) samt Referenz-Auswertung (`eval.rs`), die belegt, welche
//!   Tickets die Abfrage trifft ([`Example::expected_keys`]);
//! * [`to_markdown`] (Export und Tests) und [`search`] (Volltextsuche für die Hilfeseite).
//!
//! **Screenshots:** Zu jedem Beispiel gehört ein Bild des Ergebnisses. Die Bilder sind NICHT Teil dieses Moduls; sie werden später
//! aus der gebauten Software erzeugt (frische Datenbank aus [`Example::demo`] befüllen, [`Example::jql`] in der Suche ausführen,
//! Screenshot machen; nur englische Screenshots, der Text ist zweisprachig). Dateiname: [`Example::screenshot_file`].
//!
//! **Kleines Markup in den Texten:** `` `code` `` (Feldnamen, Operatoren, Abfragen im Fließtext) und `**fett**`; Absätze sind einzelne
//! Zeilen, Listen gibt es nur als Tabelle.
//!
//! Die Prüfungen (`tests.rs`) stellen sicher, dass jedes Beispiel parst, gegen das Schema seiner Beispieldaten validiert, stabil
//! ausgegeben wird, Treffer hat (außer ausdrücklich als „keine Treffer“ markiert) und beide Sprachen vorhanden sind.

// Die Hilfeseite und die Screenshot-Erzeugung (nächste Schritte) nutzen das Modul erst nach und nach; die `pub use` sind seine künftige API.
#![allow(dead_code, unused_imports)]

mod content;
mod dataset;
mod demo;
mod errors;
mod eval;
mod reference;

#[cfg(test)]
mod tests;

pub use dataset::shop;
pub use demo::{DemoField, DemoSpec, DemoStatus, DemoSprint, DemoTicket, DemoValue, DemoVersion, SprintState, resolve_day};
pub use errors::{ErrorCase, error_cases};
pub use eval::REFERENCE_TODAY;

use crate::state::Lang;
use std::sync::OnceLock;

/// Ein zweisprachiger Text. Englisch ist der Standard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Text {
    pub en: &'static str,
    pub de: &'static str,
}

impl Text {
    pub const fn new(en: &'static str, de: &'static str) -> Self {
        Self { en, de }
    }

    /// Text, der in beiden Sprachen gleich ist (Code, Namen, Operatoren).
    pub const fn same(s: &'static str) -> Self {
        Self { en: s, de: s }
    }

    /// Aus zur Laufzeit gebauten Texten (z. B. echte Fehlermeldungen). Der Speicher wird absichtlich nie freigegeben: das Dokument
    /// wird nur einmal gebaut ([`HelpDoc::builtin`]), die Menge ist klein und fest.
    pub(crate) fn leak(en: String, de: String) -> Self {
        Self { en: Box::leak(en.into_boxed_str()), de: Box::leak(de.into_boxed_str()) }
    }

    pub fn get(&self, lang: Lang) -> &'static str {
        match lang {
            Lang::En => self.en,
            Lang::De => self.de,
        }
    }
}

/// Schwierigkeit eines Beispiels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Beginner,
    Intermediate,
    Advanced,
}

impl Level {
    pub fn label(self) -> Text {
        match self {
            Level::Beginner => Text::new("Beginner", "Einsteiger"),
            Level::Intermediate => Text::new("Intermediate", "Fortgeschritten"),
            Level::Advanced => Text::new("Advanced", "Profi"),
        }
    }
}

/// Ein Beispiel: Abfrage, Erklärung und die Daten, mit denen der Screenshot entsteht.
#[derive(Clone, Debug, PartialEq)]
pub struct Example {
    /// Eindeutig, nur Kleinbuchstaben/Ziffern/`-` (auch Teil des Screenshot-Dateinamens)
    pub id: &'static str,
    pub title: Text,
    pub jql: String,
    pub explanation: Text,
    pub level: Level,
    pub demo: DemoSpec,
    /// Schlüssel der Tickets, die die Abfrage trifft, in Ergebnis-Reihenfolge (von Hand festgelegt; die Prüfung vergleicht sie mit
    /// der Referenz-Auswertung).
    pub expect: Vec<String>,
    /// Ausdrücklich ein „keine Treffer“-Beispiel (die Hilfe zeigt dann ein leeres Ergebnis)
    pub expect_none: bool,
}

impl Example {
    /// Die Tickets, die die Abfrage trifft, berechnet von der Referenz-Auswertung (Reihenfolge wie im Ergebnis).
    /// Fehler (nicht parsbar/nicht gültig) liefern eine leere Liste; die Tests prüfen, dass das nie vorkommt.
    pub fn expected_keys(&self) -> Vec<String> {
        self.demo.expected_keys(&self.jql).unwrap_or_default()
    }

    /// Dateiname des Screenshots (nur Englisch), z. B. `query-help-epics-release.png`.
    pub fn screenshot_file(&self) -> String {
        format!("query-help-{}.png", self.id)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Text),
    /// Mehrzeiliger Code (Abfragen, Grammatik); in beiden Sprachen gleich
    Code(String),
    Example(Example),
    Table { header: Vec<Text>, rows: Vec<Vec<Text>> },
    /// Hinweis/Warnung, optisch abgesetzt
    Note(Text),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    /// Eindeutig; Sprungmarke der Hilfeseite
    pub id: &'static str,
    pub title: Text,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HelpDoc {
    pub sections: Vec<Section>,
}

/// Titel der Hilfeseite.
pub const HELP_TITLE: Text = Text::new("Query language – complete guide", "Abfragesprache – Komplettanleitung");

impl HelpDoc {
    /// Das eingebaute Dokument (einmal gebaut, danach nur gelesen).
    pub fn builtin() -> &'static HelpDoc {
        static DOC: OnceLock<HelpDoc> = OnceLock::new();
        DOC.get_or_init(content::build)
    }

    /// Alle Beispiele in Dokument-Reihenfolge.
    pub fn examples(&self) -> Vec<&Example> {
        self.sections
            .iter()
            .flat_map(|s| s.blocks.iter())
            .filter_map(|b| if let Block::Example(e) = b { Some(e) } else { None })
            .collect()
    }

    pub fn example(&self, id: &str) -> Option<&Example> {
        self.examples().into_iter().find(|e| e.id == id)
    }

    pub fn section(&self, id: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.id == id)
    }
}

// ---------------------------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------------------------

/// Das ganze Dokument als Markdown (für Tests und Export).
pub fn to_markdown(doc: &HelpDoc, lang: Lang) -> String {
    let mut out = format!("# {}\n\n", HELP_TITLE.get(lang));
    for s in &doc.sections {
        out.push_str(&format!("## {}\n\n", s.title.get(lang)));
        for b in &s.blocks {
            match b {
                Block::Paragraph(t) => out.push_str(&format!("{}\n\n", t.get(lang))),
                Block::Code(c) => out.push_str(&format!("```text\n{c}\n```\n\n")),
                Block::Note(t) => out.push_str(&format!("> **{}** {}\n\n", lang.t("Hinweis:", "Note:"), t.get(lang))),
                Block::Table { header, rows } => {
                    let cell = |t: &Text| t.get(lang).replace('|', "\\|");
                    out.push_str(&format!("| {} |\n", header.iter().map(cell).collect::<Vec<_>>().join(" | ")));
                    out.push_str(&format!("|{}\n", " --- |".repeat(header.len())));
                    for r in rows {
                        out.push_str(&format!("| {} |\n", r.iter().map(cell).collect::<Vec<_>>().join(" | ")));
                    }
                    out.push('\n');
                }
                Block::Example(e) => {
                    out.push_str(&format!("### {} ({})\n\n", e.title.get(lang), e.level.label().get(lang)));
                    out.push_str(&format!("```text\n{}\n```\n\n{}\n\n", e.jql, e.explanation.get(lang)));
                    let keys = e.expected_keys();
                    let result = if keys.is_empty() { lang.t("keine Treffer", "no matches").to_string() } else { keys.join(", ") };
                    out.push_str(&format!("{} {result}\n\n", lang.t("Ergebnis mit den Beispieldaten:", "Result with the example data:")));
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Suche
// ---------------------------------------------------------------------------------------------

/// Ein Treffer der Volltextsuche.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub section_id: &'static str,
    pub section_title: &'static str,
    /// `None` = der Titel des Abschnitts selbst, sonst Index in `Section::blocks`
    pub block: Option<usize>,
    /// Beispiel-ID, wenn der Treffer ein Beispiel ist
    pub example_id: Option<&'static str>,
    /// Kurzer Ausschnitt um die erste Fundstelle (ohne Markup)
    pub snippet: String,
    /// Größer = besser (Titel vor Abfrage vor Fließtext)
    pub score: u32,
}

/// Text ohne das kleine Markup (`` ` `` und `**`).
pub fn plain(s: &str) -> String {
    s.replace("**", "").replace('`', "")
}

fn block_text(b: &Block, lang: Lang) -> String {
    match b {
        Block::Paragraph(t) | Block::Note(t) => plain(t.get(lang)),
        Block::Code(c) => c.clone(),
        Block::Example(e) => format!("{}\n{}\n{}", plain(e.title.get(lang)), e.jql, plain(e.explanation.get(lang))),
        Block::Table { header, rows } => header
            .iter()
            .chain(rows.iter().flatten())
            .map(|t| plain(t.get(lang)))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn snippet(text: &str, needle: &str) -> String {
    let lower = text.to_lowercase();
    // `to_lowercase` kann die Länge ändern (z. B. „İ“); dann ohne Ausschnitt-Mitte beginnen.
    let pos = if lower.len() == text.len() { lower.find(needle).unwrap_or(0) } else { 0 };
    let mut start = pos.saturating_sub(40);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (pos + needle.len() + 80).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    let body = text[start..end].replace('\n', " ");
    format!("{}{}{}", if start > 0 { "…" } else { "" }, body.trim(), if end < text.len() { "…" } else { "" })
}

/// Einfache Volltextsuche: alle Wörter der Anfrage müssen im Block vorkommen (ohne Beachtung der Schreibweise). Sortiert nach
/// Treffergüte, bei Gleichstand in Dokument-Reihenfolge. Eine leere Anfrage liefert nichts.
pub fn search(doc: &HelpDoc, lang: Lang, query: &str) -> Vec<Hit> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let all = |hay: &str| {
        let h = hay.to_lowercase();
        words.iter().all(|w| h.contains(w.as_str()))
    };
    let count = |hay: &str| {
        let h = hay.to_lowercase();
        words.iter().map(|w| h.matches(w.as_str()).count() as u32).sum::<u32>()
    };
    let mut hits = Vec::new();
    for s in &doc.sections {
        let title = s.title.get(lang);
        if all(title) {
            hits.push(Hit { section_id: s.id, section_title: title, block: None, example_id: None, snippet: title.to_string(), score: 1000 });
        }
        for (i, b) in s.blocks.iter().enumerate() {
            let text = block_text(b, lang);
            if !all(&text) {
                continue;
            }
            let mut score = 10 + count(&text).min(50);
            let example_id = match b {
                Block::Example(e) => {
                    if all(e.title.get(lang)) {
                        score += 400;
                    }
                    if all(&e.jql) {
                        score += 300;
                    }
                    Some(e.id)
                }
                _ => None,
            };
            if all(title) {
                score += 5;
            }
            hits.push(Hit { section_id: s.id, section_title: title, block: Some(i), example_id, snippet: snippet(&text, &words[0]), score });
        }
    }
    hits.sort_by(|a, b| b.score.cmp(&a.score)); // stabil: gleiche Güte bleibt in Dokument-Reihenfolge
    hits
}
