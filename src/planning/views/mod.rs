//! Konfiguration der Ansichten (Datenschicht, keine Oberfläche): Boards, gespeicherte Filter, JQL-Schlüsselwörter und
//! Dashboards samt Export/Import. Reine Logik auf `&Connection` im Stil von `planning`.
//!
//! * `boards` – mehrere Kanban-Boards je Projekt (Spalten mit mehreren Status, Typ-Filter, Quelle, Kartenfelder);
//!   `prune_board_refs` entfernt verwaiste Verweise nach dem Löschen von Status/Typen/Feldern/Filtern.
//! * `filters` – gespeicherte JQL-Abfragen (je Projekt oder global).
//! * `keywords` – JQL-Schlüsselwörter (eingebaute und eigene Prädikate bzw. Sortierschlüssel) und ihre Einspeisung in den
//!   Validator (`crate::query::Schema`).
//! * `dashboards` – Dashboards mit Kachel-Raster (12 Spalten), Layout-Prüfung, Packen und Export/Import als JSON-Dokument.
//! * `transfer` – Export/Import der Boards und gespeicherten Filter eines Projekts (Teil der späteren Projekt-Sicherung).
//! * `schema` – SQL der Migration.
//!
//! Fehler: [`ViewError`] (umhüllt `PlanError`), Meldungen zweisprachig über `message(lang)`. Mehrstufige Änderungen laufen
//! atomar (SAVEPOINT) und lassen sich in einer äußeren Transaktion verschachteln. Ein JSON-Dokument von Export/Import hat
//! immer Kennung und Format-Version; der Import erzeugt nur Neues und ändert nie Bestehendes.

pub mod boards;
pub mod dashboards;
pub mod filters;
pub mod keywords;
pub mod schema;
pub mod transfer;

#[cfg(test)]
mod tests_review;

use super::PlanError;
use crate::state::Lang;
use rusqlite::{params_from_iter, Connection};

/// Was an einer Bedingung eines JQL-Schlüsselworts nicht stimmt.
#[derive(Debug, PartialEq, Clone)]
pub enum ConditionProblem {
    /// UND/ODER ohne Einträge.
    Empty,
    /// Zu viele Einträge oder zu tief verschachtelt.
    TooLarge,
    /// Vergleichsart passt nicht zum Feldtyp.
    Operator,
    /// Der Vergleichswert passt nicht zum Feldtyp.
    Value,
}

/// Was an einem Export-/Import-Dokument nicht stimmt.
#[derive(Debug, PartialEq, Clone)]
pub enum DocProblem {
    /// Kein Dokument des erwarteten Formats.
    Format,
    /// Neuere Format-Version als bekannt.
    Version { found: u32, supported: u32 },
    /// JSON nicht lesbar oder Inhalt ungültig.
    Json(String),
    /// Verweist auf etwas, das es nicht gibt (z. B. unbekannter Projektschlüssel als Ziel).
    Reference(String),
}

#[derive(Debug, PartialEq, Clone)]
pub enum ViewError {
    /// Gemeinsame Fehler der Konfigurations-Schicht (Name leer/vergeben, nicht gefunden, Datenbank …).
    Plan(PlanError),
    // ---- Boards
    TooManyColumns { max: usize },
    /// Eine Spalte ohne Status.
    ColumnEmpty,
    /// Ein Status gehört nicht zum Projekt.
    StatusNotInProject,
    /// Ein Status steht in zwei Spalten (oder zweimal in einer).
    StatusTwice,
    /// Ein Typ des Typ-Filters gehört nicht zum Projekt.
    TypeNotInProject,
    /// Ein Kartenfeld gehört zu keinem Typ des Projekts.
    FieldNotInProject,
    /// Der Quell-Filter existiert nicht oder gehört zu einem anderen Projekt.
    FilterNotAvailable,
    // ---- Texte
    /// Ein Text (Abfrage, Beschreibung, Titel) ist zu lang.
    TextTooLong { max: usize },
    // ---- JQL-Schlüsselwörter
    /// Name: nur Buchstaben, Ziffern und `_`, nicht mit einer Ziffer beginnend.
    KeywordNameInvalid,
    /// Name ist ein reserviertes Wort der Abfragesprache.
    KeywordReserved,
    /// Eingebaute Schlüsselwörter: nur die Beschreibung ist änderbar, löschen geht nicht.
    BuiltinProtected,
    /// Das Feld der Definition gibt es nicht (mehr).
    KeywordFieldMissing,
    Condition(ConditionProblem),
    // ---- Dashboards
    TooManyTiles { max: usize },
    /// Kachel außerhalb des Rasters oder ohne Größe.
    TileGrid { tile: u32 },
    TileOverlap { a: u32, b: u32 },
    TileIdTwice { tile: u32 },
    /// Kachel-Einstellungen ungültig (Abfrage/Titel/Spalten zu lang oder zu viele).
    TileConfig { tile: u32 },
    /// Kachel braucht ein Projekt (globales Dashboard) oder verweist auf ein unbekanntes.
    TileProject { tile: u32 },
    Doc(DocProblem),
}

impl From<PlanError> for ViewError {
    fn from(e: PlanError) -> Self {
        ViewError::Plan(e)
    }
}

impl From<rusqlite::Error> for ViewError {
    fn from(e: rusqlite::Error) -> Self {
        ViewError::Plan(PlanError::from(e))
    }
}

pub type Res<T> = Result<T, ViewError>;

impl ViewError {
    pub fn not_found() -> ViewError {
        ViewError::Plan(PlanError::NotFound)
    }

    pub fn message(&self, lang: Lang) -> String {
        let s = |de: &'static str, en: &'static str| lang.t(de, en).to_string();
        match self {
            ViewError::Plan(e) => e.message(lang),
            ViewError::TooManyColumns { max } => match lang {
                Lang::De => format!("Ein Board hat höchstens {max} Spalten."),
                Lang::En => format!("A board has at most {max} columns."),
            },
            ViewError::ColumnEmpty => s("Jede Spalte braucht mindestens einen Status.", "Every column needs at least one status."),
            ViewError::StatusNotInProject => s("Ein Status gehört nicht zu diesem Projekt.", "A status does not belong to this project."),
            ViewError::StatusTwice => s("Ein Status darf nur in einer Spalte vorkommen.", "A status may appear in one column only."),
            ViewError::TypeNotInProject => s("Ein Ticket-Typ des Filters gehört nicht zu diesem Projekt.", "A ticket type of the filter does not belong to this project."),
            ViewError::FieldNotInProject => s("Ein Kartenfeld gehört zu keinem Typ dieses Projekts.", "A card field does not belong to any type of this project."),
            ViewError::FilterNotAvailable => s("Der gewählte Filter ist für dieses Projekt nicht verfügbar.", "The chosen filter is not available for this project."),
            ViewError::TextTooLong { max } => match lang {
                Lang::De => format!("Der Text ist zu lang (höchstens {max} Zeichen)."),
                Lang::En => format!("The text is too long (at most {max} characters)."),
            },
            ViewError::KeywordNameInvalid => s(
                "Der Name darf nur Buchstaben, Ziffern und _ enthalten und nicht mit einer Ziffer beginnen.",
                "The name may contain only letters, digits and _ and must not start with a digit.",
            ),
            ViewError::KeywordReserved => s("Dieses Wort ist in der Abfragesprache reserviert.", "This word is reserved in the query language."),
            ViewError::BuiltinProtected => s(
                "Eingebaute Schlüsselwörter lassen sich nicht löschen; nur die Beschreibung ist änderbar.",
                "Built-in keywords cannot be deleted; only the description can be changed.",
            ),
            ViewError::KeywordFieldMissing => s("Das Feld der Definition gibt es nicht.", "The field of the definition does not exist."),
            ViewError::Condition(p) => match p {
                ConditionProblem::Empty => s("Eine UND-/ODER-Bedingung braucht mindestens einen Eintrag.", "An AND/OR condition needs at least one entry."),
                ConditionProblem::TooLarge => s("Die Bedingung ist zu groß oder zu tief verschachtelt.", "The condition is too large or nested too deeply."),
                ConditionProblem::Operator => s("Diese Vergleichsart passt nicht zum Feld.", "This comparison does not fit the field."),
                ConditionProblem::Value => s("Der Wert passt nicht zum Feld.", "The value does not fit the field."),
            },
            ViewError::TooManyTiles { max } => match lang {
                Lang::De => format!("Ein Dashboard hat höchstens {max} Kacheln."),
                Lang::En => format!("A dashboard has at most {max} tiles."),
            },
            ViewError::TileGrid { tile } => match lang {
                Lang::De => format!("Kachel {tile} liegt nicht vollständig im Raster."),
                Lang::En => format!("Tile {tile} does not fit inside the grid."),
            },
            ViewError::TileOverlap { a, b } => match lang {
                Lang::De => format!("Die Kacheln {a} und {b} überlappen sich."),
                Lang::En => format!("Tiles {a} and {b} overlap."),
            },
            ViewError::TileIdTwice { tile } => match lang {
                Lang::De => format!("Die Kachel-Nummer {tile} kommt doppelt vor."),
                Lang::En => format!("Tile number {tile} appears twice."),
            },
            ViewError::TileConfig { tile } => match lang {
                Lang::De => format!("Die Einstellungen von Kachel {tile} sind ungültig."),
                Lang::En => format!("The settings of tile {tile} are invalid."),
            },
            ViewError::TileProject { tile } => match lang {
                Lang::De => format!("Kachel {tile} braucht ein vorhandenes Projekt."),
                Lang::En => format!("Tile {tile} needs an existing project."),
            },
            ViewError::Doc(d) => match d {
                DocProblem::Format => s("Das ist kein passendes Paneldeck-Dokument.", "This is not a matching Paneldeck document."),
                DocProblem::Version { found, supported } => format!(
                    "{} {found}, {} {supported}.",
                    lang.t("Das Dokument hat die Format-Version", "The document has format version"),
                    lang.t("bekannt ist nur bis", "but only up to")
                ),
                DocProblem::Json(r) => format!("{}: {r}", lang.t("Das Dokument ist nicht lesbar", "The document cannot be read")),
                DocProblem::Reference(r) => format!("{}: {r}", lang.t("Das Dokument verweist auf etwas Unbekanntes", "The document refers to something unknown")),
            },
        }
    }
}

/// Führt `f` atomar aus (SAVEPOINT): bei einem Fehler wird alles zurückgenommen. Lässt sich verschachteln und in einer
/// äußeren Transaktion benutzen.
pub(crate) fn atomic<T>(conn: &Connection, f: impl FnOnce() -> Res<T>) -> Res<T> {
    conn.execute_batch("SAVEPOINT view_op")?;
    match f() {
        Ok(v) => {
            conn.execute_batch("RELEASE view_op")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO view_op; RELEASE view_op");
            Err(e)
        }
    }
}

/// Längster Name (Board, Filter, Dashboard, Kachel-Titel) in Zeichen.
pub const MAX_NAME_CHARS: usize = super::common::MAX_NAME_CHARS;
/// Längste Beschreibung in Zeichen.
pub const MAX_DESCRIPTION_CHARS: usize = 500;
/// Längste JQL-Abfrage in Zeichen.
pub const MAX_JQL_CHARS: usize = 4000;

/// Name bereinigen (Leerraum zusammenfassen; leer/zu lang → Fehler).
pub(crate) fn clean_name(s: &str) -> Res<String> {
    Ok(super::common::clean_name(s)?)
}

/// Freitext bereinigen (Ränder abschneiden, Länge prüfen).
pub(crate) fn clean_text(s: &str, max: usize) -> Res<String> {
    let t = s.trim().to_string();
    if t.chars().count() > max { Err(ViewError::TextTooLong { max }) } else { Ok(t) }
}

/// Gleicher Name (Unicode, ohne Groß-/Kleinschreibung)?
pub(crate) fn same_name(a: &str, b: &str) -> bool {
    a == b || a.to_lowercase() == b.to_lowercase()
}

/// Stellt sicher, dass kein anderer Eintrag des Bereichs (`id <> exclude`) den Namen trägt. Tabellen mit Spalten `id, name`.
pub(crate) fn ensure_name_free(conn: &Connection, table: &str, where_sql: &str, params: &[i64], name: &str, exclude: Option<i64>) -> Res<()> {
    let mut st = conn.prepare(&format!("SELECT id, name FROM {table} WHERE {where_sql}"))?;
    let rows = st.query_map(params_from_iter(params.iter()), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (id, n) = row?;
        if Some(id) != exclude && same_name(&n, name) {
            return Err(PlanError::NameTaken.into());
        }
    }
    Ok(())
}

/// Alle Namen des Bereichs (für die Suche nach einem freien Namen).
pub(crate) fn names_in(conn: &Connection, table: &str, where_sql: &str, params: &[i64]) -> Res<Vec<String>> {
    let mut st = conn.prepare(&format!("SELECT name FROM {table} WHERE {where_sql}"))?;
    Ok(st.query_map(params_from_iter(params.iter()), |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?)
}

fn fit(base: &str, prefix: &str, suffix: &str) -> String {
    let room = MAX_NAME_CHARS.saturating_sub(prefix.chars().count() + suffix.chars().count());
    let b: String = base.chars().take(room).collect();
    format!("{prefix}{b}{suffix}")
}

/// Name für eine Kopie: „Copy of X“, dann „Copy 2 of X“ … (Deutsch „Kopie von X“, „Kopie 2 von X“); höchstens 80 Zeichen.
pub(crate) fn copy_name(lang: Lang, base: &str, taken: &[String]) -> String {
    let is_free = |c: &str| !taken.iter().any(|t| same_name(t, c));
    let mut n = 1;
    loop {
        let pre = match (lang, n) {
            (Lang::De, 1) => "Kopie von ".to_string(),
            (Lang::De, n) => format!("Kopie {n} von "),
            (Lang::En, 1) => "Copy of ".to_string(),
            (Lang::En, n) => format!("Copy {n} of "),
        };
        let cand = fit(base, &pre, "");
        if is_free(&cand) {
            return cand;
        }
        n += 1;
    }
}

/// Freier Name durch Anhängen von „ (2)“, „ (3)“ … (für den Import); der Name selbst, wenn er frei ist.
pub(crate) fn numbered_name(base: &str, taken: &[String]) -> String {
    let is_free = |c: &str| !taken.iter().any(|t| same_name(t, c));
    if is_free(base) {
        return base.to_string();
    }
    let mut n = 2;
    loop {
        let cand = fit(base, "", &format!(" ({n})"));
        if is_free(&cand) {
            return cand;
        }
        n += 1;
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::super::projects::{create_project, NewProject};
    use rusqlite::Connection;

    /// Frische Datenbank im Speicher mit allen Migrationen.
    pub fn db() -> Connection {
        crate::db::open_memory()
    }

    /// Neues Projekt; mit `defaults` samt Status, den fünf Typen und Feldern.
    pub fn project(c: &Connection, key: &str, defaults: bool) -> i64 {
        let mut p = NewProject::new(key, key, "#336699");
        p.with_defaults = defaults;
        create_project(c, &p).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_has_both_languages() {
        let all = vec![
            ViewError::Plan(PlanError::NotFound),
            ViewError::TooManyColumns { max: 30 },
            ViewError::ColumnEmpty,
            ViewError::StatusNotInProject,
            ViewError::StatusTwice,
            ViewError::TypeNotInProject,
            ViewError::FieldNotInProject,
            ViewError::FilterNotAvailable,
            ViewError::TextTooLong { max: 500 },
            ViewError::KeywordNameInvalid,
            ViewError::KeywordReserved,
            ViewError::BuiltinProtected,
            ViewError::KeywordFieldMissing,
            ViewError::Condition(ConditionProblem::Empty),
            ViewError::Condition(ConditionProblem::TooLarge),
            ViewError::Condition(ConditionProblem::Operator),
            ViewError::Condition(ConditionProblem::Value),
            ViewError::TooManyTiles { max: 40 },
            ViewError::TileGrid { tile: 3 },
            ViewError::TileOverlap { a: 1, b: 2 },
            ViewError::TileIdTwice { tile: 4 },
            ViewError::TileConfig { tile: 5 },
            ViewError::TileProject { tile: 6 },
            ViewError::Doc(DocProblem::Format),
            ViewError::Doc(DocProblem::Version { found: 2, supported: 1 }),
            ViewError::Doc(DocProblem::Json("x".into())),
            ViewError::Doc(DocProblem::Reference("x".into())),
        ];
        for e in all {
            let (de, en) = (e.message(Lang::De), e.message(Lang::En));
            assert!(!de.is_empty() && !en.is_empty(), "{e:?}");
            assert_ne!(de, en, "{e:?}: deutsch und englisch sollten sich unterscheiden");
        }
        assert!(ViewError::TooManyColumns { max: 30 }.message(Lang::En).contains("30"));
        assert!(ViewError::TileOverlap { a: 7, b: 9 }.message(Lang::De).contains('7'));
        assert!(ViewError::TileOverlap { a: 7, b: 9 }.message(Lang::De).contains('9'));
    }

    #[test]
    fn plan_errors_are_wrapped_and_translated() {
        let e: ViewError = PlanError::NameTaken.into();
        assert_eq!(e.message(Lang::En), PlanError::NameTaken.message(Lang::En));
        assert_eq!(e.message(Lang::De), PlanError::NameTaken.message(Lang::De));
        let d: ViewError = rusqlite::Error::QueryReturnedNoRows.into();
        assert!(matches!(d, ViewError::Plan(PlanError::Db(_))));
    }

    #[test]
    fn copy_and_numbered_names() {
        let taken: Vec<String> = vec!["A".into(), "Copy of A".into(), "copy 2 of a".into()];
        assert_eq!(copy_name(Lang::En, "A", &taken), "Copy 3 of A");
        assert_eq!(copy_name(Lang::De, "A", &taken), "Kopie von A");
        assert_eq!(numbered_name("B", &taken), "B");
        assert_eq!(numbered_name("a", &taken), "a (2)");
        assert_eq!(numbered_name("a", &["a".into(), "A (2)".into()]), "a (3)");
        // lange Namen werden gekürzt, nicht abgelehnt
        let long = "x".repeat(80);
        assert_eq!(copy_name(Lang::En, &long, &[]).chars().count(), 80);
        assert_eq!(numbered_name(&long, &[long.clone()]).chars().count(), 80);
    }

    #[test]
    fn text_cleaning() {
        assert_eq!(clean_text("  a b  ", 10).unwrap(), "a b");
        assert_eq!(clean_text("abcdef", 3), Err(ViewError::TextTooLong { max: 3 }));
        assert_eq!(clean_name(" "), Err(ViewError::Plan(PlanError::NameEmpty)));
        assert!(same_name("Ärger", "äRGER"));
    }
}
