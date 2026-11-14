//! Fehler der Abfrage-Ausführung (zweisprachig): Parse-Fehler und Prüfprobleme samt Positionen, noch nicht verfügbare Funktionen
//! (Sprints, Versionen), SQL-Grenzen (Länge, Tiefe, Größe, Zeitlimit) und Datenbankfehler.

use crate::query::{ParseError, ValidationIssue};
use crate::state::Lang;

/// Funktionen, die es noch nicht gibt (die Tabellen kommen in einem anderen Teil). Siehe `pending.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Sprints,
    Versions,
}

impl Feature {
    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            Feature::Sprints => lang.t("Sprints", "Sprints"),
            Feature::Versions => lang.t("Versionen und Meilensteine", "Versions and milestones"),
        }
    }
}

/// Ein Problem der Abfrage an einer Stelle des Eingabetexts (für Unterstreichung und Meldung).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Byte-Offset und Länge in Bytes (wie in `query::Span`).
    pub start: usize,
    pub len: usize,
    /// Spalte (1-basiert, in Zeichen) – für Meldungen wie „Position 12“.
    pub column: usize,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SearchError {
    /// Die Abfrage ist nicht lesbar (alle gefundenen Fehler, nach Position sortiert).
    Parse { input: String, errors: Vec<ParseError> },
    /// Die Abfrage ist lesbar, passt aber nicht zu den Feldern (unbekanntes Feld, falscher Wert, falscher Operator …).
    Invalid { input: String, issues: Vec<ValidationIssue> },
    /// Sprints bzw. Versionen gibt es noch nicht.
    NotAvailableYet(Feature),
    /// Der Abfragetext ist länger als erlaubt (Zeichen).
    TooLong { len: usize, max: usize },
    /// Zu tief verschachtelt (Klammern, `NOT`).
    TooDeep { max: usize },
    /// Zu viele Bedingungen oder zu großes SQL.
    TooComplex { max: usize },
    /// Die Abfrage hat das Zeitlimit überschritten und wurde abgebrochen.
    Timeout,
    /// Gespeicherter Filter, Board oder Projekt gibt es nicht (mehr).
    NotFound,
    /// Datenbankfehler (Text von SQLite).
    Db(String),
    /// Nicht vorgesehener Zustand (sollte nie vorkommen; statt eines Absturzes als Fehler gemeldet).
    Internal(String),
}

impl From<rusqlite::Error> for SearchError {
    fn from(e: rusqlite::Error) -> Self {
        match &e {
            rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::OperationInterrupted => SearchError::Timeout,
            // „Expression tree is too large“, „LIKE or GLOB pattern too complex“, „too many SQL variables“ …
            rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::TooBig => SearchError::TooComplex { max: 0 },
            _ => SearchError::Db(e.to_string()),
        }
    }
}

impl From<crate::planning::PlanError> for SearchError {
    fn from(e: crate::planning::PlanError) -> Self {
        match e {
            crate::planning::PlanError::NotFound => SearchError::NotFound,
            other => SearchError::Db(other.message(Lang::En)),
        }
    }
}

impl From<crate::planning::views::ViewError> for SearchError {
    fn from(e: crate::planning::views::ViewError) -> Self {
        match e {
            crate::planning::views::ViewError::Plan(p) => p.into(),
            other => SearchError::Db(other.message(Lang::En)),
        }
    }
}

impl From<crate::planning::ticket_error::TicketError> for SearchError {
    fn from(e: crate::planning::ticket_error::TicketError) -> Self {
        match e {
            crate::planning::ticket_error::TicketError::Plan(p) => p.into(),
            other => SearchError::Db(other.message(Lang::En)),
        }
    }
}

/// Spalte (1-basiert, in Zeichen) eines Byte-Offsets.
fn column(input: &str, byte: usize) -> usize {
    let mut b = byte.min(input.len());
    while b > 0 && !input.is_char_boundary(b) {
        b -= 1;
    }
    input[..b].chars().count() + 1
}

impl SearchError {
    /// Die einzelnen Probleme mit Positionen (leer bei Fehlern ohne Stelle im Text).
    pub fn problems(&self, lang: Lang) -> Vec<Problem> {
        match self {
            SearchError::Parse { input, errors } => errors
                .iter()
                .map(|e| Problem { start: e.span.start, len: e.span.len, column: column(input, e.span.start), message: e.message(lang) })
                .collect(),
            SearchError::Invalid { input, issues } => issues
                .iter()
                .map(|i| Problem { start: i.span.start, len: i.span.len, column: column(input, i.span.start), message: i.message(lang) })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn message(&self, lang: Lang) -> String {
        let list = |ps: Vec<Problem>| -> String {
            ps.iter().map(|p| format!("{} {}: {}", lang.t("Position", "Position"), p.column, p.message)).collect::<Vec<_>>().join("; ")
        };
        match self {
            SearchError::Parse { .. } | SearchError::Invalid { .. } => list(self.problems(lang)),
            SearchError::NotAvailableYet(f) => format!(
                "{} {}",
                f.label(lang),
                lang.t("sind noch nicht verfügbar – die Abfrage kann sie noch nicht auswerten.", "are not available yet – the query cannot evaluate them yet.")
            ),
            SearchError::TooLong { len, max } => match lang {
                Lang::De => format!("Die Abfrage ist zu lang ({len} Zeichen, höchstens {max})."),
                Lang::En => format!("The query is too long ({len} characters, at most {max})."),
            },
            SearchError::TooDeep { max } => match lang {
                Lang::De => format!("Die Abfrage ist zu tief verschachtelt (höchstens {max} Ebenen)."),
                Lang::En => format!("The query is nested too deeply (at most {max} levels)."),
            },
            SearchError::TooComplex { max } if *max > 0 => match lang {
                Lang::De => format!("Die Abfrage ist zu umfangreich (höchstens {max} Bedingungen bzw. Teile)."),
                Lang::En => format!("The query is too extensive (at most {max} conditions or parts)."),
            },
            SearchError::TooComplex { .. } => lang.t("Die Abfrage ist zu umfangreich.", "The query is too extensive.").to_string(),
            SearchError::Timeout => lang
                .t("Die Abfrage hat zu lange gedauert und wurde abgebrochen. Grenze sie weiter ein.", "The query took too long and was cancelled. Narrow it down.")
                .to_string(),
            SearchError::NotFound => lang.t("Der Eintrag existiert nicht mehr.", "The entry no longer exists.").to_string(),
            SearchError::Db(e) => format!("{}: {e}", lang.t("Datenbankfehler", "Database error")),
            SearchError::Internal(e) => format!("{}: {e}", lang.t("Interner Fehler der Suche", "Internal search error")),
        }
    }
}

pub type SRes<T> = Result<T, SearchError>;
